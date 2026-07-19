use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use narracut_core::{
    AcknowledgeCancellationOptions, ClaimJobOptions, EnqueueExportOptions, ExportErrorCode,
    ExportService, ExportServiceError, ExportTransferAbort, ExportTransferObserver, FailJobOptions,
    GetJobOptions, GetStageJobRequestOptions, JobFailureData, JobFinalizationModeData, JobService,
    JobSnapshotData, JobStatusData, RecoverJobsOptions, RenewJobLeaseOptions,
    ReportJobProgressOptions, StorageService,
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
        let _ = self.reconcile_succeeded_exports(&project_path, &project_id);
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
            resumed += self.reconcile_succeeded_exports(&project.project_path, &project.project_id);
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

    pub fn reconcile_succeeded_exports(&self, project_path: &str, project_id: &str) -> usize {
        self.service
            .reconcile_succeeded_export_journals(project_path, project_id)
            .unwrap_or(0)
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
            if snapshot.finalization_pending
                && snapshot.finalization_mode == Some(JobFinalizationModeData::ExternalCommit)
            {
                let Ok(permit) = self.worker_slots.clone().try_acquire_owned() else {
                    tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
                    continue;
                };
                let service = self.service.clone();
                let recovery_project_path = project_path.to_owned();
                let recovery_project_id = project_id.to_owned();
                let recovery_job_id = job_id.to_owned();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    service.resume_external_commit(
                        &recovery_project_path,
                        &recovery_project_id,
                        &recovery_job_id,
                    )
                })
                .await;
                drop(permit);
                tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
                continue;
            }
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
        let commit_lease_id = lease_id.clone();
        let commit = tauri::async_runtime::spawn_blocking(move || {
            service.commit_export_for_job(
                &commit_job_id,
                &commit_lease_id,
                prepared,
                observer.as_ref(),
            )
        })
        .await;
        let _commit = match commit {
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
        match self
            .jobs
            .finalize_external_completion(project_path, project_id, &job_id)
        {
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

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, sync::Arc, time::Duration};

    use narracut_contracts::{validate_export_message, ArtifactDraft};
    use narracut_core::{
        ArtifactCommitJournalStatusData, ArtifactCommitPlanEntryData, BeginJobCompletionOptions,
        ClaimJobOptions, ClaimStageJobRequestOptions, CreateProjectOptions, EnqueueStageJobOptions,
        ExportService, GetJobOptions, InitializeWorkflowOptions, JobFinalizationModeData,
        JobService, JobStatusData, PrepareStageRunOptions, ProjectService, RecordStageRunOptions,
        RetryPolicyData, ReviewDecisionData, ReviewStageRunOptions, ReviewerReferenceData,
        StorageService, StoreArtifactFileOptions, TerminalRunStatusData,
    };
    use narracut_renderer::FfmpegRenderer;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    use super::ExportRuntime;

    #[tokio::test]
    async fn resume_recent_projects_discovers_and_runs_a_persisted_export_job() {
        let temp = tempfile::tempdir().expect("export runtime restart parent");
        let projects = ProjectService::default();
        let project = projects
            .create_project(CreateProjectOptions {
                parent_path: temp.path().to_string_lossy().into_owned(),
                directory_name: "export-runtime-restart".to_owned(),
                name: "Export runtime restart fixture".to_owned(),
                workflow_definition_id: "workflow_standard_v1".to_owned(),
                default_locale: Some("zh-CN".to_owned()),
            })
            .expect("create export runtime restart project");
        let index_path = temp.path().join("narracut-index.sqlite3");
        let storage = StorageService::new(index_path.clone(), projects.clone());
        let workflow = narracut_core::WorkflowService::new(projects.clone(), storage.clone());
        workflow
            .initialize_project_workflow(InitializeWorkflowOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
            })
            .expect("initialize export runtime restart workflow");

        let mut approved = std::collections::BTreeMap::new();
        for (stage_id, kind, dependencies) in [
            ("brief", "brief", vec![]),
            ("research", "claim_set", vec![("brief", "brief")]),
            ("script", "script", vec![("research", "claim_set")]),
            ("audio", "voice_audio", vec![("script", "script")]),
            (
                "captions",
                "captions",
                vec![("script", "script"), ("audio", "voice_audio")],
            ),
            (
                "scene_plan",
                "scene_plan",
                vec![
                    ("research", "claim_set"),
                    ("script", "script"),
                    ("captions", "captions"),
                ],
            ),
            (
                "timeline",
                "timeline",
                vec![
                    ("audio", "voice_audio"),
                    ("captions", "captions"),
                    ("scene_plan", "scene_plan"),
                ],
            ),
            ("render", "rendered_video", vec![("timeline", "timeline")]),
        ] {
            let run_id = format!("run_{stage_id}_export_runtime_restart");
            let review_id = format!("review_{stage_id}_export_runtime_restart");
            let input_refs = dependencies
                .iter()
                .map(|(dependency, dependency_kind)| {
                    let input: &Value = approved
                        .get(*dependency)
                        .unwrap_or_else(|| panic!("missing approved dependency {dependency}"));
                    json!({
                        "refId": format!("restart_{}_{}", dependency, input["artifactId"].as_str().expect("dependency artifact id")),
                        "referenceType": "artifact",
                        "kind": dependency_kind,
                        "contentHash": input["contentHash"],
                        "artifactId": input["artifactId"],
                        "sourceRunId": input["runId"],
                        "reviewRecordId": input["reviewId"],
                        "claimIds": ["claim_export_runtime_restart"],
                        "evidenceRefs": ["evidence_export_runtime_restart"]
                    })
                })
                .collect::<Vec<_>>();
            workflow
                .prepare_stage_run(PrepareStageRunOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.clone(),
                    job_id: format!("job_{stage_id}_export_runtime_restart"),
                    input_refs,
                    executor: json!({
                        "providerId": "restart_fixture",
                        "providerVersion": "1.0.0",
                        "executionMode": "local"
                    }),
                })
                .unwrap_or_else(|error| panic!("prepare {stage_id}: {error}"));
            let payload = temp.path().join(format!("{stage_id}-restart-payload.bin"));
            fs::write(&payload, format!("restart fixture {stage_id}"))
                .expect("write restart fixture artifact");
            let artifact: ArtifactDraft = serde_json::from_value(json!({
                "stageId": stage_id,
                "runId": run_id,
                "kind": kind,
                "mediaType": if kind == "rendered_video" { "video/mp4" } else { "application/json" },
                "evidenceRole": "non_evidence",
                "source": {"origin":"generated","providerId":"restart_fixture","model":"fixture"},
                "provenance": [{
                    "claimId":"claim_export_runtime_restart",
                    "evidenceRef":"evidence_export_runtime_restart"
                }]
            }))
            .expect("build restart fixture Artifact draft");
            let stored = storage
                .import_artifact_file(StoreArtifactFileOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    source_path: payload.to_string_lossy().into_owned(),
                    artifact,
                })
                .unwrap_or_else(|error| panic!("store {stage_id}: {error}"));
            let artifact_id = stored.artifact["artifactId"]
                .as_str()
                .expect("stored artifact id")
                .to_owned();
            workflow
                .record_stage_run(RecordStageRunOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.clone(),
                    status: TerminalRunStatusData::Succeeded,
                    job_id: format!("job_{stage_id}_export_runtime_restart"),
                    artifact_ids: vec![artifact_id.clone()],
                    log_summary: json!({"message":"fixture complete","warnings":[],"errors":[]}),
                })
                .unwrap_or_else(|error| panic!("record {stage_id}: {error}"));
            workflow
                .review_stage_run(ReviewStageRunOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.clone(),
                    review_id: review_id.clone(),
                    decision: ReviewDecisionData::Approved,
                    reviewer: ReviewerReferenceData {
                        kind: "human".to_owned(),
                        reviewer_id: "restart_fixture_reviewer".to_owned(),
                        display_name: "Restart Fixture Reviewer".to_owned(),
                    },
                    comments: "approved restart fixture dependency".to_owned(),
                    artifact_ids: vec![artifact_id.clone()],
                })
                .unwrap_or_else(|error| panic!("approve {stage_id}: {error}"));
            approved.insert(
                stage_id.to_owned(),
                json!({
                    "artifactId": artifact_id,
                    "contentHash": stored.artifact["contentHash"],
                    "runId": run_id,
                    "reviewId": review_id
                }),
            );
        }

        let render = approved.get("render").expect("approved Render dependency");
        let input_ref = json!({
            "refId": "restart_export_render_input",
            "referenceType": "artifact",
            "kind": "rendered_video",
            "contentHash": render["contentHash"],
            "artifactId": render["artifactId"],
            "sourceRunId": render["runId"],
            "reviewRecordId": render["reviewId"],
            "claimIds": ["claim_export_runtime_restart"],
            "evidenceRefs": ["evidence_export_runtime_restart"]
        });
        let jobs = JobService::new(projects.clone(), storage.clone(), workflow.clone());
        let invalid_but_persisted_request = json!({"requestVersion":"corrupt_restart_fixture"});
        let claim = jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                idempotency_key: "export-runtime-startup-recovery".to_owned(),
                request: invalid_but_persisted_request.clone(),
            })
            .expect("claim persisted Export request");
        let queued = jobs
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    stage_id: "export".to_owned(),
                    run_id: "run_export_runtime_startup_recovery".to_owned(),
                    input_refs: vec![input_ref],
                    executor: json!({
                        "providerId": "narracut_export",
                        "providerVersion": "1.0.0",
                        "executionMode": "local"
                    }),
                    idempotency_key: "export-runtime-startup-recovery".to_owned(),
                    retry_policy: RetryPolicyData {
                        max_attempts: 1,
                        initial_backoff_ms: 1,
                        backoff_multiplier: 1,
                        max_backoff_ms: 1,
                    },
                },
                invalid_but_persisted_request,
            )
            .expect("persist queued Export job");
        assert_eq!(queued.status, JobStatusData::Queued);
        assert_eq!(queued.job["jobId"], claim.job_id);

        drop(workflow);
        drop(storage);
        drop(jobs);
        drop(projects);

        let restarted_projects = ProjectService::default();
        let restarted_storage = StorageService::new(index_path.clone(), restarted_projects.clone());
        let restarted_workflow = narracut_core::WorkflowService::new(
            restarted_projects.clone(),
            restarted_storage.clone(),
        );
        let restarted_jobs = JobService::new(
            restarted_projects.clone(),
            restarted_storage.clone(),
            restarted_workflow.clone(),
        );
        let restarted_export = ExportService::new(
            restarted_projects,
            restarted_storage.clone(),
            restarted_workflow,
            restarted_jobs.clone(),
        );
        let runtime = ExportRuntime::new(
            restarted_export,
            restarted_jobs.clone(),
            restarted_storage.clone(),
            Arc::new(FfmpegRenderer),
        );
        assert_eq!(runtime.resume_recent_projects(), 1);

        let terminal = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = restarted_jobs
                    .get_job(GetJobOptions {
                        project_path: project.project_path.clone(),
                        expected_project_id: project.project_id.clone(),
                        job_id: claim.job_id.clone(),
                    })
                    .expect("poll restarted Export job");
                if snapshot.status.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("startup-resumed Export job reaches terminal state");
        assert_eq!(terminal.status, JobStatusData::Failed);
        assert_eq!(
            terminal
                .last_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("invalid_request")
        );
        assert!(Path::new(&project.project_path)
            .join("requests/jobs")
            .join(format!("{}.json", claim.job_id))
            .is_file());

        let destination = temp.path().join("external-commit-destination");
        fs::create_dir(&destination).expect("create external commit destination");
        let marker_run_id = "run_export_runtime_external_marker";
        let marker_request = json!({
            "projectPath": project.project_path,
            "expectedProjectId": project.project_id,
            "runId": marker_run_id,
            "renderInput": {
                "stageId": "render",
                "runId": render["runId"],
                "artifactId": render["artifactId"],
                "resultArtifactId": render["artifactId"],
                "contentHash": render["contentHash"],
                "reviewRecordId": render["reviewId"],
                "claimIds": ["claim_export_runtime_restart"],
                "evidenceRefs": ["evidence_export_runtime_restart"]
            },
            "qaHash": format!("sha256:{}", "a".repeat(64)),
            "destinationDirectory": destination,
            "exportName": "runtime-marker-recovered",
            "idempotencyKey": "export-runtime-external-marker",
            "maxTemporaryBytes": 1048576
        });
        let marker_claim = restarted_jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                idempotency_key: "export-runtime-external-marker".to_owned(),
                request: marker_request.clone(),
            })
            .expect("claim marker Export request");
        let marker_job = restarted_jobs
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    stage_id: "export".to_owned(),
                    run_id: marker_run_id.to_owned(),
                    input_refs: vec![json!({
                        "refId": "runtime_marker_render_input",
                        "referenceType": "artifact",
                        "kind": "rendered_video",
                        "contentHash": render["contentHash"],
                        "artifactId": render["artifactId"],
                        "sourceRunId": render["runId"],
                        "reviewRecordId": render["reviewId"],
                        "claimIds": ["claim_export_runtime_restart"],
                        "evidenceRefs": ["evidence_export_runtime_restart"]
                    })],
                    executor: json!({
                        "providerId": "narracut_export",
                        "providerVersion": "1.0.0",
                        "executionMode": "local"
                    }),
                    idempotency_key: "export-runtime-external-marker".to_owned(),
                    retry_policy: RetryPolicyData {
                        max_attempts: 1,
                        initial_backoff_ms: 1,
                        backoff_multiplier: 1,
                        max_backoff_ms: 1,
                    },
                },
                marker_request,
            )
            .expect("persist marker Export job");
        assert_eq!(marker_job.job["jobId"], marker_claim.job_id);
        let claimed_marker = restarted_jobs
            .claim_job(ClaimJobOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                job_id: marker_claim.job_id.clone(),
                worker_id: "export_worker_before_crash".to_owned(),
                lease_duration_ms: 60_000,
            })
            .expect("claim marker Export job")
            .expect("marker Export claim");
        let marker_lease = claimed_marker.lease.expect("marker lease").lease_id;
        let stable_id = |kind: &str| {
            format!(
                "artifact_{}",
                Sha256::digest(format!("{}:{kind}", marker_claim.job_id).as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        };
        let export_id = format!(
            "export_{}",
            Sha256::digest(marker_claim.job_id.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let video_artifact_id = stable_id("video");
        let manifest_artifact_id = stable_id("manifest");

        let import_source = |name: &str, kind: &str, bytes: &[u8]| {
            let path = temp.path().join(format!("marker-{name}"));
            fs::write(&path, bytes).expect("write marker recovery source");
            let draft: ArtifactDraft = serde_json::from_value(json!({
                "stageId": "export",
                "runId": marker_run_id,
                "kind": kind,
                "mediaType": if kind == "audio_source" { "audio/wav" } else { "application/json" },
                "evidenceRole": "non_evidence",
                "source": {"origin":"generated","providerId":"marker_fixture","model":"fixture"},
                "provenance": [{
                    "claimId":"claim_export_runtime_restart",
                    "evidenceRef":"evidence_export_runtime_restart"
                }]
            }))
            .expect("build marker recovery source draft");
            restarted_storage
                .import_artifact_file(StoreArtifactFileOptions {
                    project_path: project.project_path.clone(),
                    expected_project_id: project.project_id.clone(),
                    source_path: path.to_string_lossy().into_owned(),
                    artifact: draft,
                })
                .expect("import marker recovery source")
        };
        let audio_bytes = b"fixture-audio-reference";
        let json_value = json!({"fixture":true});
        let json_source_bytes = serde_json::to_vec(&json_value).expect("serialize JSON source");
        let mut json_export_bytes =
            serde_json::to_vec_pretty(&json_value).expect("serialize pretty JSON export");
        json_export_bytes.push(b'\n');
        let audio_source = import_source("audio.wav", "audio_source", audio_bytes);
        let captions_source = import_source("captions.json", "captions_source", &json_source_bytes);
        let timeline_source = import_source("timeline.json", "timeline", &json_source_bytes);
        let video_bytes = b"fixture-final-video";
        let hash = |bytes: &[u8]| {
            format!(
                "sha256:{}",
                Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        };
        let manifest = json!({
            "manifestVersion":"1.0.0",
            "documentType":"export_manifest",
            "projectId":project.project_id,
            "projectFormatVersion":1,
            "exportId":export_id,
            "createdAt":"2026-07-19T00:00:00Z",
            "exportRunId":marker_run_id,
            "renderRunId":render["runId"],
            "renderReviewRecordId":render["reviewId"],
            "adoptedArtifacts":[
                {"stageId":"audio","runId":"run_audio_export_runtime_restart","artifactId":audio_source.artifact["artifactId"],"kind":"audio_source","uri":audio_source.artifact["uri"],"contentHash":audio_source.artifact["contentHash"],"reviewRecordId":"review_audio_export_runtime_restart"},
                {"stageId":"captions","runId":"run_captions_export_runtime_restart","artifactId":captions_source.artifact["artifactId"],"kind":"captions_source","uri":captions_source.artifact["uri"],"contentHash":captions_source.artifact["contentHash"],"reviewRecordId":"review_captions_export_runtime_restart"},
                {"stageId":"timeline","runId":"run_timeline_export_runtime_restart","artifactId":timeline_source.artifact["artifactId"],"kind":"timeline","uri":timeline_source.artifact["uri"],"contentHash":timeline_source.artifact["contentHash"],"reviewRecordId":"review_timeline_export_runtime_restart"}
            ],
            "rendererIdentity":{"adapterId":"narracut.ffmpeg","adapterVersion":"1.0.0","executableFileName":"ffmpeg.exe","executableHash":format!("sha256:{}","b".repeat(64)),"ffmpegVersion":"fixture","ffprobeFileName":"ffprobe.exe","ffprobeHash":format!("sha256:{}","c".repeat(64)),"ffprobeVersion":"fixture","capabilityHash":format!("sha256:{}","d".repeat(64))},
            "media":{"width":320,"height":180,"durationMs":100,"frameRateNumerator":30,"frameRateDenominator":1,"videoCodec":"libx264","audioCodec":"aac","pixelFormat":"yuv420p","hasAudio":true},
            "files":[
                {"role":"video","relativePath":"video.mp4","sourceUri":format!("artifacts/objects/sha256/aa/{}","a".repeat(64)),"contentHash":hash(video_bytes),"byteLength":video_bytes.len(),"mediaType":"video/mp4"},
                {"role":"audio_reference","relativePath":"audio.wav","sourceUri":audio_source.artifact["uri"],"contentHash":hash(audio_bytes),"byteLength":audio_bytes.len(),"mediaType":"audio/wav"},
                {"role":"captions","relativePath":"captions.json","sourceUri":captions_source.artifact["uri"],"contentHash":hash(&json_export_bytes),"byteLength":json_export_bytes.len(),"mediaType":"application/json"},
                {"role":"timeline","relativePath":"timeline.json","sourceUri":timeline_source.artifact["uri"],"contentHash":hash(&json_export_bytes),"byteLength":json_export_bytes.len(),"mediaType":"application/json"}
            ],
            "provenance":[{"claimId":"claim_export_runtime_restart","evidenceRef":"evidence_export_runtime_restart"}],
            "claimIds":["claim_export_runtime_restart"],
            "evidenceRefs":["evidence_export_runtime_restart"],
            "licenses":[{"artifactId":audio_source.artifact["artifactId"],"mediaDocumentArtifactId":audio_source.artifact["artifactId"],"sourceUri":audio_source.artifact["uri"],"contentHash":audio_source.artifact["contentHash"],"sourceFileName":"fixture.wav","author":"Fixture Author","licenseId":"fixture-license","rightsStatement":"Fixture use authorized.","attributionText":"","authorizationRecordIds":["authorization_marker_fixture"]}],
            "qa":{"status":"passed","passed":true,"warningCount":0,"blockingCount":0,"checks":[{"checkId":"qa_marker_fixture","category":"rights","status":"passed","message":"fixture QA passed","sceneIds":[],"artifactIds":[]}],"diagnostics":[],"checkedAt":"2026-07-19T00:00:00Z","qaHash":format!("sha256:{}","e".repeat(64))},
            "integrity":"complete"
        });
        validate_export_message(&manifest).expect("marker recovery manifest follows Export v1");
        let manifest_bytes = serde_json::to_vec(&manifest).expect("serialize marker manifest");
        for (artifact_id, kind, media_type, bytes) in [
            (
                video_artifact_id.as_str(),
                "final_video",
                "video/mp4",
                video_bytes.as_slice(),
            ),
            (
                manifest_artifact_id.as_str(),
                "render_manifest",
                "application/vnd.narracut.export-manifest+json",
                manifest_bytes.as_slice(),
            ),
        ] {
            let source = temp.path().join(format!("{artifact_id}.payload"));
            fs::write(&source, bytes).expect("write stable marker artifact source");
            let draft: ArtifactDraft = serde_json::from_value(json!({
                "stageId":"export","runId":marker_run_id,"kind":kind,"mediaType":media_type,
                "evidenceRole":"non_evidence",
                "source":{"origin":"derived","sourceArtifactIds":[render["artifactId"]]},
                "provenance":[{"claimId":"claim_export_runtime_restart","evidenceRef":"evidence_export_runtime_restart"}]
            }))
            .expect("build stable marker Artifact draft");
            restarted_storage
                .import_artifact_file_with_identity_for_test(
                    StoreArtifactFileOptions {
                        project_path: project.project_path.clone(),
                        expected_project_id: project.project_id.clone(),
                        source_path: source.to_string_lossy().into_owned(),
                        artifact: draft,
                    },
                    artifact_id,
                    "2026-07-19T00:00:00Z",
                    1024 * 1024,
                )
                .expect("import stable marker Artifact");
        }
        let entries = vec![
            ArtifactCommitPlanEntryData {
                artifact_id: video_artifact_id.clone(),
                kind: "final_video".to_owned(),
            },
            ArtifactCommitPlanEntryData {
                artifact_id: manifest_artifact_id.clone(),
                kind: "render_manifest".to_owned(),
            },
        ];
        restarted_storage
            .begin_artifact_commit_journal_for_test(
                &project.project_path,
                &project.project_id,
                &marker_claim.job_id,
                marker_run_id,
                "2026-07-19T00:00:00Z",
                &entries,
            )
            .expect("persist pending external commit journal");
        let marker = restarted_jobs
            .begin_job_completion(BeginJobCompletionOptions {
                project_path: project.project_path.clone(),
                expected_project_id: project.project_id.clone(),
                job_id: marker_claim.job_id.clone(),
                lease_id: marker_lease,
                artifact_ids: vec![video_artifact_id, manifest_artifact_id],
                log_summary: json!({"message":"external marker persisted","warnings":[],"errors":[]}),
                finalization_mode: JobFinalizationModeData::ExternalCommit,
            })
            .expect("persist running external commit marker");
        assert_eq!(marker.status, JobStatusData::Running);
        assert!(marker.finalization_pending);
        let metadata_before_recovery =
            fs::read_dir(Path::new(&project.project_path).join("artifacts/metadata"))
                .expect("count metadata before runtime restart")
                .count();

        drop(runtime);
        drop(restarted_jobs);
        drop(restarted_storage);
        let final_projects = ProjectService::default();
        let final_storage = StorageService::new(index_path, final_projects.clone());
        let final_workflow =
            narracut_core::WorkflowService::new(final_projects.clone(), final_storage.clone());
        let final_jobs = JobService::new(
            final_projects.clone(),
            final_storage.clone(),
            final_workflow.clone(),
        );
        let final_export = ExportService::new(
            final_projects,
            final_storage.clone(),
            final_workflow,
            final_jobs.clone(),
        );
        let final_runtime = ExportRuntime::new(
            final_export.clone(),
            final_jobs.clone(),
            final_storage.clone(),
            Arc::new(FfmpegRenderer),
        );
        assert_eq!(final_runtime.resume_recent_projects(), 1);
        let recovered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = final_jobs
                    .get_job(GetJobOptions {
                        project_path: project.project_path.clone(),
                        expected_project_id: project.project_id.clone(),
                        job_id: marker_claim.job_id.clone(),
                    })
                    .expect("poll external marker recovery");
                if snapshot.status.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("runtime external marker recovery reaches terminal state");
        assert_eq!(recovered.status, JobStatusData::Succeeded);
        assert!(!recovered.finalization_pending);
        let result = final_export
            .get_result(
                &project.project_path,
                &project.project_id,
                &marker_claim.job_id,
            )
            .expect("read recovered unique ExportResult");
        assert_eq!(result["status"], "succeeded");
        assert!(destination.join("runtime-marker-recovered").is_dir());
        assert!(!destination
            .join(format!(".narracut-{export_id}.partial"))
            .exists());
        assert_eq!(
            final_storage
                .complete_artifact_commit_journal(
                    &project.project_path,
                    &project.project_id,
                    &marker_claim.job_id,
                )
                .expect("journal is idempotently completed")
                .status,
            ArtifactCommitJournalStatusData::Completed
        );
        assert_eq!(
            fs::read_dir(Path::new(&project.project_path).join("artifacts/metadata"))
                .expect("count metadata after runtime recovery")
                .count(),
            metadata_before_recovery
        );
    }
}
