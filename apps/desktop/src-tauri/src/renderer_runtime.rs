use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use narracut_core::{
    AcknowledgeCancellationOptions, ArtifactTransferAbort, ArtifactTransferObserver,
    ClaimJobOptions, CommitRenderOptions, CompleteJobOptions, EnqueueRenderOptions, FailJobOptions,
    GetJobOptions, GetStageJobRequestOptions, JobFailureData, JobService, JobServiceError,
    JobSnapshotData, JobStatusData, ListJobsOptions, RecordJobArtifactOptions, RecoverJobsOptions,
    RendererService, RendererServiceError, RendererServiceErrorCode, RenewJobLeaseOptions,
    ReportJobProgressOptions, StorageService,
};
use narracut_renderer::{
    FfmpegRenderer, RenderCancellation, RenderSpec, RendererAdapter, RendererError,
    RendererErrorCode,
};
use serde_json::{json, Map, Value};

const WORKER_ID: &str = "renderer_runtime_worker_v1";
const LEASE_MS: u64 = 180_000;
const HEARTBEAT_MS: u64 = 60_000;
const COMMIT_MONITOR_MS: u64 = 1_000;
const COMMIT_PROGRESS_BYTES: u64 = 64 * 1024 * 1024;
const POLL_MS: u64 = 200;
const JOB_SCAN_LIMIT: u32 = 200;
const RECENT_PROJECT_LIMIT: u32 = 25;

const COMMIT_RUNNING: u8 = 0;
const COMMIT_CANCELED: u8 = 1;
const COMMIT_LEASE_LOST: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitTransferProgress {
    artifact_id: String,
    completed_bytes: u64,
    total_bytes: u64,
}

struct RuntimeCommitObserver {
    abort_state: AtomicU8,
    progress_tx: tokio::sync::mpsc::UnboundedSender<CommitTransferProgress>,
}

impl RuntimeCommitObserver {
    fn new(progress_tx: tokio::sync::mpsc::UnboundedSender<CommitTransferProgress>) -> Self {
        Self {
            abort_state: AtomicU8::new(COMMIT_RUNNING),
            progress_tx,
        }
    }

    fn cancel(&self) {
        let _ = self.abort_state.compare_exchange(
            COMMIT_RUNNING,
            COMMIT_CANCELED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn lose_lease(&self) {
        self.abort_state.store(COMMIT_LEASE_LOST, Ordering::Release);
    }
}

impl ArtifactTransferObserver for RuntimeCommitObserver {
    fn checkpoint(
        &self,
        artifact_id: &str,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), ArtifactTransferAbort> {
        match self.abort_state.load(Ordering::Acquire) {
            COMMIT_CANCELED => return Err(ArtifactTransferAbort::Canceled),
            COMMIT_LEASE_LOST => return Err(ArtifactTransferAbort::LeaseLost),
            _ => {}
        }
        if completed_bytes == 0
            || completed_bytes == total_bytes
            || completed_bytes.is_multiple_of(COMMIT_PROGRESS_BYTES)
        {
            let _ = self.progress_tx.send(CommitTransferProgress {
                artifact_id: artifact_id.to_owned(),
                completed_bytes,
                total_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitSupervisorState {
    Continue,
    Canceled,
    LeaseLost,
}

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
        let commit_options = CommitRenderOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            prepared,
            renderer_identity: identity,
            rendered_file_path: output_path.to_string_lossy().into_owned(),
            process_result,
        };
        let commit_renderer = self.renderer.clone();
        let heartbeat_jobs = self.jobs.clone();
        let heartbeat_path = project_path.to_owned();
        let heartbeat_project = project_id.to_owned();
        let heartbeat_job = job_id.clone();
        let heartbeat_lease = lease_id.clone();
        let progress_jobs = self.jobs.clone();
        let commit_path = project_path.to_owned();
        let commit_project = project_id.to_owned();
        let commit_job = job_id.clone();
        let commit_lease = lease_id.clone();
        let mut last_commit_renewal =
            tokio::time::Instant::now() - Duration::from_millis(HEARTBEAT_MS);
        let commit = match run_supervised_blocking_commit(
            Duration::from_millis(COMMIT_MONITOR_MS),
            move |observer| {
                commit_renderer.commit_render_with_control(commit_options, observer.as_ref())
            },
            move || {
                let snapshot = match heartbeat_jobs.get_job(GetJobOptions {
                    project_path: heartbeat_path.clone(),
                    expected_project_id: heartbeat_project.clone(),
                    job_id: heartbeat_job.clone(),
                }) {
                    Ok(snapshot) => snapshot,
                    Err(_) => return CommitSupervisorState::LeaseLost,
                };
                if snapshot.cancellation_requested {
                    return CommitSupervisorState::Canceled;
                }
                if snapshot.status.is_terminal()
                    || snapshot.lease.as_ref().map(|lease| lease.lease_id.as_str())
                        != Some(heartbeat_lease.as_str())
                {
                    return CommitSupervisorState::LeaseLost;
                }
                let now = tokio::time::Instant::now();
                if now.duration_since(last_commit_renewal) < Duration::from_millis(HEARTBEAT_MS) {
                    return CommitSupervisorState::Continue;
                }
                match heartbeat_jobs.renew_job_lease(RenewJobLeaseOptions {
                    project_path: heartbeat_path.clone(),
                    expected_project_id: heartbeat_project.clone(),
                    job_id: heartbeat_job.clone(),
                    lease_id: heartbeat_lease.clone(),
                    lease_duration_ms: LEASE_MS,
                }) {
                    Ok(_) => {
                        last_commit_renewal = now;
                        CommitSupervisorState::Continue
                    }
                    Err(_) => CommitSupervisorState::LeaseLost,
                }
            },
            move |progress| {
                let message = format!(
                    "Committing {}: {} / {} bytes",
                    progress.artifact_id, progress.completed_bytes, progress.total_bytes
                );
                match progress_jobs.report_job_progress(ReportJobProgressOptions {
                    project_path: commit_path.clone(),
                    expected_project_id: commit_project.clone(),
                    job_id: commit_job.clone(),
                    lease_id: commit_lease.clone(),
                    progress: 0.80,
                    message: Some(message),
                }) {
                    Ok(snapshot) if snapshot.cancellation_requested => {
                        CommitSupervisorState::Canceled
                    }
                    Ok(snapshot)
                        if snapshot.status.is_terminal()
                            || snapshot.lease.as_ref().map(|lease| lease.lease_id.as_str())
                                != Some(commit_lease.as_str()) =>
                    {
                        CommitSupervisorState::LeaseLost
                    }
                    Ok(_) => CommitSupervisorState::Continue,
                    Err(_) => CommitSupervisorState::LeaseLost,
                }
            },
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) if error.code == RendererServiceErrorCode::Canceled => {
                let _ = self.acknowledge_cancellation(project_path, project_id, &job_id, &lease_id);
                return;
            }
            Ok(Err(error)) if error.code == RendererServiceErrorCode::JobConflict => {
                // The lease owner may have changed. The pending commit journal and
                // stable Artifact identities are intentionally left for recovery.
                return;
            }
            Ok(Err(error)) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::from_service(error),
                );
                return;
            }
            Err(_) => {
                self.fail(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    RendererWorkerFailure::io("Renderer commit blocking worker failed."),
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

async fn run_supervised_blocking_commit<T, F, H, P>(
    heartbeat_interval: Duration,
    commit: F,
    mut heartbeat: H,
    mut report_progress: P,
) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce(Arc<RuntimeCommitObserver>) -> T + Send + 'static,
    H: FnMut() -> CommitSupervisorState,
    P: FnMut(CommitTransferProgress) -> CommitSupervisorState,
{
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let observer = Arc::new(RuntimeCommitObserver::new(progress_tx));
    let commit_observer = observer.clone();
    let mut task = tokio::task::spawn_blocking(move || commit(commit_observer));
    let mut interval = tokio::time::interval(heartbeat_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut progress_open = true;

    loop {
        tokio::select! {
            result = &mut task => return result,
            _ = interval.tick() => {
                match heartbeat() {
                    CommitSupervisorState::Continue => {}
                    CommitSupervisorState::Canceled => observer.cancel(),
                    CommitSupervisorState::LeaseLost => observer.lose_lease(),
                }
            }
            progress = progress_rx.recv(), if progress_open => {
                match progress {
                    Some(progress) => match report_progress(progress) {
                        CommitSupervisorState::Continue => {}
                        CommitSupervisorState::Canceled => observer.cancel(),
                        CommitSupervisorState::LeaseLost => observer.lose_lease(),
                    },
                    None => progress_open = false,
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_commit_runs_off_runtime_and_renews_past_a_fake_lease() {
        let renewals = Arc::new(AtomicUsize::new(0));
        let renewal_counter = renewals.clone();
        let runtime_ticks = Arc::new(AtomicUsize::new(0));
        let tick_counter = runtime_ticks.clone();
        let ticker = tokio::spawn(async move {
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(2)).await;
                tick_counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        let started = Instant::now();
        let result = run_supervised_blocking_commit(
            Duration::from_millis(5),
            move |observer| {
                for chunk in 1..=30 {
                    std::thread::sleep(Duration::from_millis(4));
                    observer.checkpoint("artifact_slow", chunk, 30)?;
                }
                Ok::<_, ArtifactTransferAbort>(())
            },
            move || {
                renewal_counter.fetch_add(1, Ordering::Relaxed);
                CommitSupervisorState::Continue
            },
            |_| CommitSupervisorState::Continue,
        )
        .await
        .expect("blocking worker joins");
        ticker.await.expect("runtime ticker joins");

        assert_eq!(result, Ok(()));
        assert!(started.elapsed() > Duration::from_millis(100));
        assert!(renewals.load(Ordering::Relaxed) >= 3);
        assert!(runtime_ticks.load(Ordering::Relaxed) >= 20);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_interrupts_a_streaming_commit_promptly() {
        let heartbeats = Arc::new(AtomicUsize::new(0));
        let heartbeat_counter = heartbeats.clone();
        let started = Instant::now();
        let result = run_supervised_blocking_commit(
            Duration::from_millis(5),
            move |observer| {
                for chunk in 1..=1_000 {
                    std::thread::sleep(Duration::from_millis(3));
                    observer.checkpoint("artifact_cancel", chunk, 1_000)?;
                }
                Ok::<_, ArtifactTransferAbort>(())
            },
            move || {
                if heartbeat_counter.fetch_add(1, Ordering::Relaxed) >= 2 {
                    CommitSupervisorState::Canceled
                } else {
                    CommitSupervisorState::Continue
                }
            },
            |_| CommitSupervisorState::Continue,
        )
        .await
        .expect("blocking worker joins");

        assert_eq!(result, Err(ArtifactTransferAbort::Canceled));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lease_loss_interrupts_the_blocking_worker_without_marking_success() {
        let heartbeats = Arc::new(AtomicUsize::new(0));
        let heartbeat_counter = heartbeats.clone();
        let result = run_supervised_blocking_commit(
            Duration::from_millis(5),
            move |observer| {
                for chunk in 1..=1_000 {
                    std::thread::sleep(Duration::from_millis(3));
                    observer.checkpoint("artifact_lease_lost", chunk, 1_000)?;
                }
                Ok::<_, ArtifactTransferAbort>(())
            },
            move || {
                if heartbeat_counter.fetch_add(1, Ordering::Relaxed) >= 1 {
                    CommitSupervisorState::LeaseLost
                } else {
                    CommitSupervisorState::Continue
                }
            },
            |_| CommitSupervisorState::Continue,
        )
        .await
        .expect("blocking worker joins");

        assert_eq!(result, Err(ArtifactTransferAbort::LeaseLost));
    }
}
