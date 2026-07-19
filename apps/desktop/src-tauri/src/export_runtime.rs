use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use narracut_core::{
    AcknowledgeCancellationOptions, ClaimJobOptions, CompleteJobOptions, EnqueueExportOptions,
    ExportErrorCode, ExportService, ExportServiceError, ExportTransferAbort,
    ExportTransferObserver, FailJobOptions, GetJobOptions, GetStageJobRequestOptions,
    JobFailureData, JobService, JobSnapshotData, JobStatusData, RecoverJobsOptions,
    RenewJobLeaseOptions, ReportJobProgressOptions, StorageService,
};
use narracut_renderer::{RendererAdapter, RendererIdentity};
use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;

const WORKER_ID: &str = "narracut_export_worker_v1";
const LEASE_MS: u64 = 30_000;
const POLL_MS: u64 = 250;

#[derive(Clone)]
pub struct ExportRuntime {
    service: ExportService,
    jobs: JobService,
    storage: StorageService,
    adapter: Arc<dyn RendererAdapter>,
    active_jobs: Arc<Mutex<HashSet<String>>>,
    worker_slots: Arc<Semaphore>,
}

impl ExportRuntime {
    pub fn new(
        service: ExportService,
        jobs: JobService,
        storage: StorageService,
        adapter: Arc<dyn RendererAdapter>,
    ) -> Self {
        Self {
            service,
            jobs,
            storage,
            adapter,
            active_jobs: Arc::new(Mutex::new(HashSet::new())),
            worker_slots: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn adapter(&self) -> Arc<dyn RendererAdapter> {
        self.adapter.clone()
    }

    pub fn supports_export_job(&self, snapshot: &JobSnapshotData) -> bool {
        snapshot.job.get("stageId").and_then(Value::as_str) == Some("export")
            && snapshot
                .job
                .pointer("/executor/providerId")
                .and_then(Value::as_str)
                == Some("narracut_export")
    }

    pub fn schedule_supported_job(
        &self,
        project_path: String,
        project_id: String,
        snapshot: &JobSnapshotData,
    ) -> bool {
        if !self.supports_export_job(snapshot) || snapshot.status.is_terminal() {
            return false;
        }
        let Some(job_id) = snapshot.job.get("jobId").and_then(Value::as_str) else {
            return false;
        };
        self.schedule(project_path, project_id, job_id.to_owned())
    }

    pub fn resume_recent_projects(&self) -> usize {
        let Ok(recent) = self.storage.list_recent_projects(100, false) else {
            return 0;
        };
        let mut resumed = 0;
        for project in recent
            .projects
            .into_iter()
            .filter(|project| project.path_available)
        {
            let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
            });
            if let Ok(jobs) = self.jobs.list_jobs(narracut_core::ListJobsOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                statuses: vec![
                    JobStatusData::Queued,
                    JobStatusData::Running,
                    JobStatusData::Retrying,
                ],
                limit: 100,
            }) {
                for snapshot in jobs.jobs {
                    if self.schedule_supported_job(
                        project.project_path.clone(),
                        project.project_id.clone(),
                        &snapshot,
                    ) {
                        resumed += 1;
                    }
                }
            }
        }
        resumed
    }

    fn schedule(&self, project_path: String, project_id: String, job_id: String) -> bool {
        let key = format!("{project_id}:{job_id}");
        let should_start = self
            .active_jobs
            .lock()
            .map(|mut jobs| jobs.insert(key.clone()))
            .unwrap_or(false);
        if !should_start {
            return false;
        }
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            runtime
                .run_until_terminal(&project_path, &project_id, &job_id)
                .await;
            if let Ok(mut jobs) = runtime.active_jobs.lock() {
                jobs.remove(&key);
            }
        });
        true
    }

    async fn run_until_terminal(&self, project_path: &str, project_id: &str, job_id: &str) {
        loop {
            let Ok(snapshot) = self.jobs.get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            }) else {
                return;
            };
            if snapshot.status.is_terminal() || !self.supports_export_job(&snapshot) {
                return;
            }
            if matches!(
                snapshot.status,
                JobStatusData::Queued | JobStatusData::Retrying
            ) {
                let Ok(permit) = self.worker_slots.clone().try_acquire_owned() else {
                    tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
                    continue;
                };
                if let Ok(Some(claimed)) = self.jobs.claim_job(ClaimJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.to_owned(),
                    worker_id: WORKER_ID.to_owned(),
                    lease_duration_ms: LEASE_MS,
                }) {
                    self.run_claimed(project_path, project_id, claimed).await;
                }
                drop(permit);
            } else {
                let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                });
            }
            tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
        }
    }

    async fn run_claimed(&self, project_path: &str, project_id: &str, claimed: JobSnapshotData) {
        let Some(lease_id) = claimed.lease.as_ref().map(|lease| lease.lease_id.clone()) else {
            return;
        };
        let Some(job_id) = claimed
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        if claimed.cancellation_requested {
            self.acknowledge(project_path, project_id, &job_id, &lease_id);
            return;
        }
        let request = match self
            .jobs
            .get_stage_job_request(GetStageJobRequestOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.clone(),
            })
            .ok()
            .and_then(|value| serde_json::from_value::<EnqueueExportOptions>(value.request).ok())
        {
            Some(request) => request,
            None => {
                self.fail(
                    (project_path, project_id, &job_id, &lease_id),
                    ExportWorkerFailure::new("invalid_request", "导出请求 receipt 无效。", false),
                );
                return;
            }
        };
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.05,
            message: Some("正在复验 Renderer identity 与导出 QA".to_owned()),
        });
        let probe = self.adapter.probe().await;
        let Some(identity) = probe
            .identity
            .filter(|_| probe.available && probe.supported)
        else {
            self.fail(
                (project_path, project_id, &job_id, &lease_id),
                ExportWorkerFailure::new(
                    "renderer_identity_changed",
                    "FFmpeg/FFprobe 不可用或能力不受支持。",
                    false,
                ),
            );
            return;
        };
        let public_identity = public_identity(&identity);
        let prepared = match self.service.prepare_export(request, Some(&public_identity)) {
            Ok(value) => value,
            Err(error) => {
                self.fail_service(project_path, project_id, &job_id, &lease_id, error);
                return;
            }
        };
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.15,
            message: Some("QA 已通过，正在原子导出媒体与 Manifest".to_owned()),
        });
        let observer = Arc::new(JobExportObserver {
            jobs: self.jobs.clone(),
            project_path: project_path.to_owned(),
            project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
        });
        let service = self.service.clone();
        let commit_job_id = job_id.clone();
        let commit = tauri::async_runtime::spawn_blocking(move || {
            service.commit_export(&commit_job_id, prepared, observer.as_ref())
        })
        .await;
        let commit = match commit {
            Ok(Ok(value)) => value,
            Ok(Err(error)) if error.code == ExportErrorCode::Canceled => {
                self.acknowledge(project_path, project_id, &job_id, &lease_id);
                return;
            }
            Ok(Err(error)) => {
                self.fail_service(project_path, project_id, &job_id, &lease_id, error);
                return;
            }
            Err(_) => {
                self.fail(
                    (project_path, project_id, &job_id, &lease_id),
                    ExportWorkerFailure::new("io_error", "导出 blocking worker 异常结束。", true),
                );
                return;
            }
        };
        if let Ok(snapshot) = self.jobs.get_job(GetJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
        }) {
            if snapshot.cancellation_requested {
                self.acknowledge(project_path, project_id, &job_id, &lease_id);
                return;
            }
        }
        match self.jobs.complete_job(CompleteJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            artifact_ids: commit.artifact_ids,
            log_summary: commit.log_summary,
        }) {
            Ok(_) => {
                let _ = self.storage.complete_artifact_commit_journal(
                    project_path,
                    project_id,
                    &job_id,
                );
            }
            Err(_) => self.fail(
                (project_path, project_id, &job_id, &lease_id),
                ExportWorkerFailure::new(
                    "commit_state_failed",
                    "导出文件已原子提交，但 Job 完成状态写入失败；恢复扫描将继续处理。",
                    true,
                ),
            ),
        }
    }

    fn acknowledge(&self, project_path: &str, project_id: &str, job_id: &str, lease_id: &str) {
        let _ = self
            .jobs
            .acknowledge_cancellation(AcknowledgeCancellationOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
                lease_id: lease_id.to_owned(),
            });
    }
    fn fail_service(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        error: ExportServiceError,
    ) {
        self.fail(
            (project_path, project_id, job_id, lease_id),
            ExportWorkerFailure::new(error.code.as_str(), &error.message, error.retryable),
        );
    }
    fn fail(&self, job: (&str, &str, &str, &str), failure: ExportWorkerFailure<'_>) {
        let (project_path, project_id, job_id, lease_id) = job;
        let ExportWorkerFailure {
            code,
            message,
            retryable,
        } = failure;
        let _ = self.jobs.fail_job(FailJobOptions { project_path: project_path.to_owned(), expected_project_id: project_id.to_owned(), job_id: job_id.to_owned(), lease_id: lease_id.to_owned(), error: JobFailureData { code: code.to_owned(), message: redact(message), retryable, details: Map::new() }, log_summary: json!({ "message": "导出失败；历史与诊断已保留。", "warnings": [], "errors": [code] }) });
    }
}

struct ExportWorkerFailure<'a> {
    code: &'a str,
    message: &'a str,
    retryable: bool,
}
impl<'a> ExportWorkerFailure<'a> {
    fn new(code: &'a str, message: &'a str, retryable: bool) -> Self {
        Self {
            code,
            message,
            retryable,
        }
    }
}

struct JobExportObserver {
    jobs: JobService,
    project_path: String,
    project_id: String,
    job_id: String,
    lease_id: String,
}
impl ExportTransferObserver for JobExportObserver {
    fn checkpoint(
        &self,
        phase: &str,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), ExportTransferAbort> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: self.project_path.clone(),
                expected_project_id: self.project_id.clone(),
                job_id: self.job_id.clone(),
            })
            .map_err(|_| ExportTransferAbort::LeaseLost)?;
        if snapshot.cancellation_requested {
            return Err(ExportTransferAbort::Canceled);
        }
        if snapshot.status.is_terminal()
            || snapshot.lease.as_ref().map(|lease| lease.lease_id.as_str())
                != Some(self.lease_id.as_str())
        {
            return Err(ExportTransferAbort::LeaseLost);
        }
        self.jobs
            .renew_job_lease(RenewJobLeaseOptions {
                project_path: self.project_path.clone(),
                expected_project_id: self.project_id.clone(),
                job_id: self.job_id.clone(),
                lease_id: self.lease_id.clone(),
                lease_duration_ms: LEASE_MS,
            })
            .map_err(|_| ExportTransferAbort::LeaseLost)?;
        let ratio = if total_bytes == 0 {
            0.0
        } else {
            completed_bytes as f64 / total_bytes as f64
        };
        self.jobs
            .report_job_progress(ReportJobProgressOptions {
                project_path: self.project_path.clone(),
                expected_project_id: self.project_id.clone(),
                job_id: self.job_id.clone(),
                lease_id: self.lease_id.clone(),
                progress: (0.15 + ratio * 0.75).min(0.90),
                message: Some(format!(
                    "正在导出 {phase}：{completed_bytes}/{total_bytes} bytes"
                )),
            })
            .map_err(|_| ExportTransferAbort::LeaseLost)?;
        Ok(())
    }
}

fn public_identity(identity: &RendererIdentity) -> Value {
    json!({ "adapterId": identity.adapter_id, "adapterVersion": identity.adapter_version, "executableFileName": identity.executable_file_name, "executableHash": identity.executable_hash, "ffmpegVersion": identity.ffmpeg_version, "ffprobeFileName": identity.ffprobe_file_name, "ffprobeHash": identity.ffprobe_hash, "ffprobeVersion": identity.ffprobe_version, "capabilityHash": identity.capability_hash })
}
fn redact(message: &str) -> String {
    message
        .replace(['\\', '/'], " ")
        .split_whitespace()
        .take(80)
        .collect::<Vec<_>>()
        .join(" ")
}
