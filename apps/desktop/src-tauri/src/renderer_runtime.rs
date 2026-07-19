use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use narracut_core::{
    AcknowledgeCancellationOptions, ClaimJobOptions, CommitRenderOptions, CompleteJobOptions,
    EnqueueRenderOptions, FailJobOptions, GetJobOptions, GetStageJobRequestOptions, JobFailureData,
    JobService, JobServiceError, JobSnapshotData, JobStatusData, ListJobsOptions,
    RecordJobArtifactOptions, RecoverJobsOptions, RendererService, RendererServiceError,
    RenewJobLeaseOptions, ReportJobProgressOptions, StorageService,
};
use narracut_renderer::{
    FfmpegRenderer, RenderCancellation, RenderSpec, RendererAdapter, RendererError,
    RendererErrorCode,
};
use serde_json::{json, Map, Value};

const WORKER_ID: &str = "renderer_runtime_worker_v1";
const LEASE_MS: u64 = 180_000;
const HEARTBEAT_MS: u64 = 60_000;
const POLL_MS: u64 = 200;
const JOB_SCAN_LIMIT: u32 = 200;
const RECENT_PROJECT_LIMIT: u32 = 25;

#[derive(Clone)]
pub struct RendererRuntime {
    renderer: RendererService,
    storage: StorageService,
    jobs: JobService,
    adapter: Arc<dyn RendererAdapter>,
    active_jobs: Arc<Mutex<HashSet<String>>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
}

impl RendererRuntime {
    pub fn new(renderer: RendererService, storage: StorageService, jobs: JobService) -> Self {
        Self {
            renderer,
            storage,
            jobs,
            adapter: Arc::new(FfmpegRenderer),
            active_jobs: Arc::new(Mutex::new(HashSet::new())),
            worker_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub fn adapter(&self) -> Arc<dyn RendererAdapter> {
        self.adapter.clone()
    }

    pub fn supports_renderer_job(&self, snapshot: &JobSnapshotData) -> bool {
        !snapshot.historical
            && snapshot.job.get("stageId").and_then(Value::as_str) == Some("render")
            && snapshot
                .job
                .get("executor")
                .and_then(|value| value.get("providerId"))
                .and_then(Value::as_str)
                == Some("narracut_renderer")
            && snapshot
                .job
                .get("executor")
                .and_then(|value| value.get("providerVersion"))
                .and_then(Value::as_str)
                == Some("1.0.0")
            && snapshot
                .job
                .get("executor")
                .and_then(|value| value.get("executionMode"))
                .and_then(Value::as_str)
                == Some("local")
    }

    pub fn schedule_supported_job(
        &self,
        project_path: String,
        project_id: String,
        job_id: String,
    ) -> Result<bool, RendererRuntimeError> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.clone(),
                expected_project_id: project_id.clone(),
                job_id: job_id.clone(),
            })
            .map_err(RendererRuntimeError::Job)?;
        if snapshot.status.is_terminal() || !self.supports_renderer_job(&snapshot) {
            return Ok(false);
        }
        Ok(self.schedule(project_path, project_id, job_id))
    }

    pub fn schedule_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, RendererRuntimeError> {
        let jobs = self
            .jobs
            .list_jobs(ListJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                statuses: vec![
                    JobStatusData::Queued,
                    JobStatusData::Running,
                    JobStatusData::Retrying,
                ],
                limit: JOB_SCAN_LIMIT,
            })
            .map_err(RendererRuntimeError::Job)?;
        let mut scheduled = 0;
        for snapshot in jobs.jobs {
            if !self.supports_renderer_job(&snapshot) {
                continue;
            }
            let job_id = snapshot
                .job
                .get("jobId")
                .and_then(Value::as_str)
                .ok_or(RendererRuntimeError::InvalidReceipt)?
                .to_owned();
            if self.schedule(project_path.to_owned(), project_id.to_owned(), job_id) {
                scheduled += 1;
            }
        }
        Ok(scheduled)
    }

    pub fn resume_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, RendererRuntimeError> {
        self.jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
            })
            .map_err(RendererRuntimeError::Job)?;
        self.schedule_project_jobs(project_path, project_id)
    }

    pub fn resume_recent_projects(&self) -> usize {
        let Ok(recent) = self
            .storage
            .list_recent_projects(RECENT_PROJECT_LIMIT, false)
        else {
            return 0;
        };
        recent
            .projects
            .into_iter()
            .filter(|project| {
                self.resume_project_jobs(&project.project_path, &project.project_id)
                    .is_ok()
            })
            .count()
    }

    pub fn retry_render_job(
        &self,
        project_path: String,
        project_id: String,
        source_job_id: String,
        new_run_id: String,
        idempotency_key: String,
    ) -> Result<JobSnapshotData, RendererRuntimeError> {
        let source = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.clone(),
                expected_project_id: project_id.clone(),
                job_id: source_job_id.clone(),
            })
            .map_err(RendererRuntimeError::Job)?;
        if !self.supports_renderer_job(&source)
            || !matches!(
                source.status,
                JobStatusData::Failed | JobStatusData::Canceled
            )
        {
            return Err(RendererRuntimeError::InvalidReceipt);
        }
        let mut request = self.load_request(&project_path, &project_id, &source)?;
        request.run_id = new_run_id;
        request.idempotency_key = idempotency_key;
        let accepted = self
            .renderer
            .enqueue_render(request)
            .map_err(RendererRuntimeError::Service)?;
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.clone(),
                expected_project_id: project_id.clone(),
                job_id: accepted.job_id.clone(),
            })
            .map_err(RendererRuntimeError::Job)?;
        let _ = self.schedule_supported_job(project_path, project_id, accepted.job_id);
        Ok(snapshot)
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
        let mut next_recovery = tokio::time::Instant::now();
        loop {
            let Ok(snapshot) = self.jobs.get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            }) else {
                return;
            };
            if snapshot.status.is_terminal() || !self.supports_renderer_job(&snapshot) {
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
                match self.jobs.claim_job(ClaimJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.to_owned(),
                    worker_id: WORKER_ID.to_owned(),
                    lease_duration_ms: LEASE_MS,
                }) {
                    Ok(Some(claimed)) => self.run_claimed(project_path, project_id, claimed).await,
                    Ok(None) => {}
                    Err(_) => return,
                }
                drop(permit);
            } else if tokio::time::Instant::now() >= next_recovery {
                let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                });
                next_recovery = tokio::time::Instant::now() + Duration::from_secs(5);
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
        if self
            .acknowledge_cancellation(project_path, project_id, &job_id, &lease_id)
            .unwrap_or(false)
        {
            return;
        }
        let request = match self.load_request(project_path, project_id, &claimed) {
            Ok(value) => value,
            Err(error) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_runtime(error),
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
            message: Some("正在复核已审核 Timeline 与 Renderer 身份".to_owned()),
        });
        let probe = self.adapter.probe().await;
        let Some(identity) = probe
            .identity
            .filter(|_| probe.available && probe.supported)
        else {
            self.fail(
                project_path,
                project_id,
                &job_id,
                &lease_id,
                RendererWorkerFailure {
                    code: "renderer_unavailable".to_owned(),
                    message: probe.diagnostics.join("；"),
                    retryable: true,
                },
            );
            return;
        };
        let Some(frozen_identity) = request.renderer_identity.clone() else {
            self.fail(
                project_path,
                project_id,
                &job_id,
                &lease_id,
                RendererWorkerFailure {
                    code: "invalid_renderer_request_receipt".to_owned(),
                    message: "The render request has no frozen Renderer identity.".to_owned(),
                    retryable: false,
                },
            );
            return;
        };
        if frozen_identity != identity {
            self.fail(project_path, project_id, &job_id, &lease_id, RendererWorkerFailure { code: "renderer_identity_changed".to_owned(), message: "The FFmpeg executable or capability identity changed after the Job was accepted.".to_owned(), retryable: false });
            return;
        }
        let prepared = match self.renderer.prepare_render(request, false) {
            Ok(value) => value,
            Err(error) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_service(error),
                );
                return;
            }
        };
        let temp_root = match controlled_temp_root(project_path) {
            Ok(path) => path,
            Err(error) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_runtime(error),
                );
                return;
            }
        };
        let temp = match tempfile::Builder::new()
            .prefix("render-job-")
            .tempdir_in(&temp_root)
        {
            Ok(temp) => temp,
            Err(_) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::io("无法创建受控渲染临时目录。"),
                );
                return;
            }
        };
        let audio_path = temp.path().join("audio.wav");
        let output_path = temp.path().join("output.partial.mp4");
        if fs::File::create(&audio_path)
            .and_then(|mut file| {
                file.write_all(&prepared.audio_bytes)
                    .and_then(|_| file.sync_all())
            })
            .is_err()
        {
            self.fail(
                project_path,
                project_id,
                &job_id,
                &lease_id,
                RendererWorkerFailure::io("无法冻结渲染音频副本。"),
            );
            return;
        }
        let cancellation = RenderCancellation::default();
        let progress_jobs = self.jobs.clone();
        let progress_path = project_path.to_owned();
        let progress_project = project_id.to_owned();
        let progress_job = job_id.clone();
        let progress_lease = lease_id.clone();
        let progress_cancellation = cancellation.clone();
        let last_heartbeat = Arc::new(Mutex::new(tokio::time::Instant::now()));
        let progress_heartbeat = last_heartbeat.clone();
        let progress = Arc::new(move |value: f64, message: String| {
            let snapshot = progress_jobs.get_job(GetJobOptions {
                project_path: progress_path.clone(),
                expected_project_id: progress_project.clone(),
                job_id: progress_job.clone(),
            });
            if snapshot.as_ref().is_ok_and(|snapshot| {
                snapshot.cancellation_requested || snapshot.status.is_terminal()
            }) {
                progress_cancellation.cancel();
                return;
            }
            let _ = progress_jobs.report_job_progress(ReportJobProgressOptions {
                project_path: progress_path.clone(),
                expected_project_id: progress_project.clone(),
                job_id: progress_job.clone(),
                lease_id: progress_lease.clone(),
                progress: 0.1 + value * 0.65,
                message: Some(message),
            });
            if let Ok(mut heartbeat) = progress_heartbeat.lock() {
                let now = tokio::time::Instant::now();
                if now.duration_since(*heartbeat) >= Duration::from_millis(HEARTBEAT_MS) {
                    let _ = progress_jobs.renew_job_lease(RenewJobLeaseOptions {
                        project_path: progress_path.clone(),
                        expected_project_id: progress_project.clone(),
                        job_id: progress_job.clone(),
                        lease_id: progress_lease.clone(),
                        lease_duration_ms: LEASE_MS,
                    });
                    *heartbeat = now;
                }
            }
        });
        let spec = RenderSpec {
            identity: frozen_identity,
            working_directory: temp.path().to_path_buf(),
            output_path: output_path.clone(),
            audio_path,
            canvas: prepared.config.canvas,
            encoding: prepared.config.encoding(),
            scenes: RendererService::render_scene_specs(&prepared),
        };
        let process_result = match self
            .adapter
            .render(spec, cancellation.clone(), progress)
            .await
        {
            Ok(value) => value,
            Err(error) if error.code == RendererErrorCode::Canceled => {
                let _ = self.acknowledge_cancellation(project_path, project_id, &job_id, &lease_id);
                return;
            }
            Err(error) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_adapter(error),
                );
                return;
            }
        };
        if self
            .acknowledge_cancellation(project_path, project_id, &job_id, &lease_id)
            .unwrap_or(false)
        {
            return;
        }
        let commit = match self.renderer.commit_render(CommitRenderOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            prepared,
            renderer_identity: identity,
            rendered_file_path: output_path.to_string_lossy().into_owned(),
            process_result,
        }) {
            Ok(value) => value,
            Err(error) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_service(error),
                );
                return;
            }
        };
        for artifact_id in &commit.artifact_ids {
            if let Err(error) = self.jobs.record_job_artifact(RecordJobArtifactOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.clone(),
                lease_id: lease_id.clone(),
                artifact_id: artifact_id.clone(),
            }) {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_job(error),
                );
                return;
            }
        }
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.95,
            message: Some("不可变 Render Artifact 已提交，正在写入 StageRun".to_owned()),
        });
        if let Err(error) = self.jobs.complete_job(CompleteJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            artifact_ids: commit.artifact_ids,
            log_summary: commit.log_summary,
        }) {
            self.fail(
                project_path,
                project_id,
                &job_id,
                &lease_id,
                RendererWorkerFailure::from_job(error),
            );
        }
    }

    fn load_request(
        &self,
        project_path: &str,
        project_id: &str,
        snapshot: &JobSnapshotData,
    ) -> Result<EnqueueRenderOptions, RendererRuntimeError> {
        if !self.supports_renderer_job(snapshot) || snapshot.owner_project_id != project_id {
            return Err(RendererRuntimeError::InvalidReceipt);
        }
        let job_id = snapshot
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .ok_or(RendererRuntimeError::InvalidReceipt)?;
        let receipt = self
            .jobs
            .get_stage_job_request(GetStageJobRequestOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            })
            .map_err(RendererRuntimeError::Job)?;
        let request: EnqueueRenderOptions = serde_json::from_value(receipt.request)
            .map_err(|_| RendererRuntimeError::InvalidReceipt)?;
        let expected_input_refs = json!([{
            "refId": format!("renderer_timeline_{}", request.timeline_input.artifact_id), "referenceType": "artifact", "kind": "timeline", "artifactId": request.timeline_input.artifact_id,
            "sourceRunId": request.timeline_input.run_id, "reviewRecordId": request.timeline_input.review_record_id, "contentHash": request.timeline_input.content_hash,
            "claimIds": request.timeline_input.claim_ids, "evidenceRefs": request.timeline_input.evidence_refs
        }]);
        if request.expected_project_id != project_id
            || snapshot.job.get("stageRunId").and_then(Value::as_str)
                != Some(request.run_id.as_str())
            || snapshot.job.get("inputRefs") != Some(&expected_input_refs)
        {
            return Err(RendererRuntimeError::InvalidReceipt);
        }
        Ok(request)
    }

    fn acknowledge_cancellation(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
    ) -> Result<bool, RendererRuntimeError> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            })
            .map_err(RendererRuntimeError::Job)?;
        if !snapshot.cancellation_requested {
            return Ok(false);
        }
        self.jobs
            .acknowledge_cancellation(AcknowledgeCancellationOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
                lease_id: lease_id.to_owned(),
            })
            .map_err(RendererRuntimeError::Job)?;
        Ok(true)
    }

    fn fail(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        failure: RendererWorkerFailure,
    ) {
        let _ = self.jobs.fail_job(FailJobOptions { project_path: project_path.to_owned(), expected_project_id: project_id.to_owned(), job_id: job_id.to_owned(), lease_id: lease_id.to_owned(), error: JobFailureData { code: failure.code.clone(), message: failure.message, retryable: failure.retryable, details: Map::new() }, log_summary: json!({ "message": "Renderer 任务未完成。", "warnings": [], "errors": [failure.code] }) });
    }
}

fn controlled_temp_root(project_path: &str) -> Result<std::path::PathBuf, RendererRuntimeError> {
    let root = Path::new(project_path).join("renders").join(".tmp");
    let renders = Path::new(project_path).join("renders");
    if renders.exists()
        && fs::symlink_metadata(&renders)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(true)
    {
        return Err(RendererRuntimeError::UnsafePath);
    }
    if root.exists()
        && fs::symlink_metadata(&root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(true)
    {
        return Err(RendererRuntimeError::UnsafePath);
    }
    fs::create_dir_all(&root).map_err(|_| RendererRuntimeError::UnsafePath)?;
    let canonical_project =
        fs::canonicalize(project_path).map_err(|_| RendererRuntimeError::UnsafePath)?;
    let canonical_root = fs::canonicalize(&root).map_err(|_| RendererRuntimeError::UnsafePath)?;
    if !canonical_root.starts_with(canonical_project) {
        return Err(RendererRuntimeError::UnsafePath);
    }
    Ok(canonical_root)
}

#[derive(Debug)]
pub enum RendererRuntimeError {
    Job(JobServiceError),
    Service(RendererServiceError),
    InvalidReceipt,
    UnsafePath,
}

impl std::fmt::Display for RendererRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Job(error) => error.fmt(formatter),
            Self::Service(error) => error.fmt(formatter),
            Self::InvalidReceipt => write!(formatter, "Renderer 任务收据无效。"),
            Self::UnsafePath => write!(formatter, "渲染临时目录不安全。"),
        }
    }
}
impl std::error::Error for RendererRuntimeError {}

struct RendererWorkerFailure {
    code: String,
    message: String,
    retryable: bool,
}
impl RendererWorkerFailure {
    fn from_runtime(error: RendererRuntimeError) -> Self {
        match error {
            RendererRuntimeError::Job(error) => Self::from_job(error),
            RendererRuntimeError::Service(error) => Self::from_service(error),
            RendererRuntimeError::InvalidReceipt => Self {
                code: "invalid_renderer_request_receipt".into(),
                message: "Renderer 请求收据未通过身份校验。".into(),
                retryable: false,
            },
            RendererRuntimeError::UnsafePath => Self {
                code: "resource_rejected".into(),
                message: "渲染临时目录越界或包含链接。".into(),
                retryable: false,
            },
        }
    }
    fn from_job(error: JobServiceError) -> Self {
        Self {
            code: format!("renderer_job_{}", error.code.as_str()),
            message: error.message,
            retryable: error.code == narracut_core::JobErrorCode::IoError,
        }
    }
    fn from_service(error: RendererServiceError) -> Self {
        Self {
            code: error.code.as_str().into(),
            message: error.message,
            retryable: error.retryable,
        }
    }
    fn from_adapter(error: RendererError) -> Self {
        let code = match error.code {
            RendererErrorCode::Unavailable => "renderer_unavailable",
            RendererErrorCode::Unsupported => "renderer_unsupported",
            RendererErrorCode::IdentityChanged => "renderer_identity_changed",
            RendererErrorCode::InvalidSpec => "invalid_request",
            RendererErrorCode::ResourceLimit => "resource_limit_exceeded",
            RendererErrorCode::SpawnFailed | RendererErrorCode::ProcessFailed => "ffmpeg_failed",
            RendererErrorCode::Timeout => "timeout",
            RendererErrorCode::Canceled => "canceled",
            RendererErrorCode::CleanupFailed => "cleanup_failed",
            RendererErrorCode::Io => "io_error",
        };
        Self {
            code: code.into(),
            message: error.message,
            retryable: error.retryable,
        }
    }
    fn io(message: &str) -> Self {
        Self {
            code: "io_error".into(),
            message: message.into(),
            retryable: true,
        }
    }
}
