use narracut_core::{
    AcknowledgeCancellationOptions, ClaimJobOptions, ClaimStageJobRequestOptions,
    CompleteJobOptions, EnqueueStageJobOptions, FailJobOptions, FrozenArtifactInputData,
    GenerateScenePlanOptions, GenerateTimelineOptions, GetJobOptions, GetStageJobRequestOptions,
    ImportAudioOptions, ImportCaptionsOptions, JobErrorCode, JobFailureData, JobOperation,
    JobService, JobServiceError, JobSnapshotData, JobStatusData, ListJobsOptions, MediaErrorCode,
    MediaRightsData, MediaService, MediaServiceError, PcmWavParseLimits, RecordJobArtifactOptions,
    RecoverJobsOptions, RenewJobLeaseOptions, ReportJobProgressOptions,
    ResolveStagedMediaSourceOptions, RetryPolicyData, SrtParseLimits, StageMediaSourceFileOptions,
    StorageErrorCode, StorageService, StorageServiceError, TimelineCanvasData,
    TimelineSafeAreaData,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar,
};
use std::{
    collections::HashSet,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

const MEDIA_RUNTIME_REQUEST_VERSION: &str = "1.0.0";
const MEDIA_RUNTIME_PROVIDER_ID: &str = "narracut_media_runtime";
const MEDIA_RUNTIME_PROVIDER_VERSION: &str = "1.0.0";
const MEDIA_RUNTIME_EXECUTION_MODE: &str = "local";
const MEDIA_RUNTIME_MODEL: &str = "bounded_media_v1";
const MEDIA_STAGE_IDS: [&str; 4] = ["audio", "captions", "scene_plan", "timeline"];
const MEDIA_WORKER_ID: &str = "media_runtime_worker_v1";
const MEDIA_LEASE_MS: u64 = 180_000;
const MEDIA_LEASE_HEARTBEAT_MS: u64 = 60_000;
const MEDIA_WORKER_POLL_MS: u64 = 250;
const MEDIA_RECOVERY_POLL_MS: u64 = 1_000;
const MEDIA_JOB_SCAN_LIMIT: u32 = 200;
const MEDIA_RECENT_PROJECT_SCAN_LIMIT: u32 = 25;

#[derive(Clone)]
pub struct MediaRuntime {
    media: MediaService,
    storage: StorageService,
    jobs: JobService,
    active_jobs: Arc<Mutex<HashSet<String>>>,
    worker_slots: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    execution_test_gate: Option<MediaExecutionTestGate>,
}

impl MediaRuntime {
    pub fn new(media: MediaService, storage: StorageService, jobs: JobService) -> Self {
        Self {
            media,
            storage,
            jobs,
            active_jobs: Arc::new(Mutex::new(HashSet::new())),
            worker_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            #[cfg(test)]
            execution_test_gate: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_execution_test_gate(mut self, gate: MediaExecutionTestGate) -> Self {
        self.execution_test_gate = Some(gate);
        self
    }

    /// Returns true only for the four media stages owned by this local runtime.
    /// Provider-backed script jobs can never pass this executor identity check.
    pub fn supports_media_job(&self, snapshot: &JobSnapshotData) -> bool {
        if snapshot.historical
            || snapshot.job.get("jobType").and_then(Value::as_str) != Some("stage_run")
        {
            return false;
        }
        let Some(stage_id) = snapshot.job.get("stageId").and_then(Value::as_str) else {
            return false;
        };
        if !MEDIA_STAGE_IDS.contains(&stage_id) {
            return false;
        }
        let Some(executor) = snapshot.job.get("executor") else {
            return false;
        };
        executor.get("providerId").and_then(Value::as_str) == Some(MEDIA_RUNTIME_PROVIDER_ID)
            && executor.get("providerVersion").and_then(Value::as_str)
                == Some(MEDIA_RUNTIME_PROVIDER_VERSION)
            && executor.get("executionMode").and_then(Value::as_str)
                == Some(MEDIA_RUNTIME_EXECUTION_MODE)
            && executor.get("model").and_then(Value::as_str) == Some(MEDIA_RUNTIME_MODEL)
    }

    pub fn schedule_supported_job(
        &self,
        project_path: String,
        project_id: String,
        job_id: String,
    ) -> Result<bool, MediaRuntimeError> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.clone(),
                expected_project_id: project_id.clone(),
                job_id: job_id.clone(),
            })
            .map_err(MediaRuntimeError::Job)?;
        if snapshot.status.is_terminal() || !self.supports_media_job(&snapshot) {
            return Ok(false);
        }
        Ok(self.schedule(project_path, project_id, job_id))
    }

    pub fn resume_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, MediaRuntimeError> {
        self.jobs
            .recover_project_jobs(RecoverJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
            })
            .map_err(MediaRuntimeError::Job)?;
        self.schedule_project_jobs(project_path, project_id)
    }

    pub fn schedule_project_jobs(
        &self,
        project_path: &str,
        project_id: &str,
    ) -> Result<usize, MediaRuntimeError> {
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
                limit: MEDIA_JOB_SCAN_LIMIT,
            })
            .map_err(MediaRuntimeError::Job)?;
        let mut scheduled = 0;
        for snapshot in jobs.jobs {
            if !self.supports_media_job(&snapshot) {
                continue;
            }
            let job_id = snapshot
                .job
                .get("jobId")
                .and_then(Value::as_str)
                .ok_or(MediaRuntimeError::InvalidSnapshot("jobId"))?
                .to_owned();
            if self.schedule(project_path.to_owned(), project_id.to_owned(), job_id) {
                scheduled += 1;
            }
        }
        Ok(scheduled)
    }

    pub fn resume_recent_projects(&self) -> usize {
        let Ok(recent) = self
            .storage
            .list_recent_projects(MEDIA_RECENT_PROJECT_SCAN_LIMIT, false)
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

    fn schedule(&self, project_path: String, project_id: String, job_id: String) -> bool {
        let key = format!("{project_id}:{job_id}");
        let should_start = self
            .active_jobs
            .lock()
            .map(|mut active| active.insert(key.clone()))
            .unwrap_or(false);
        if !should_start {
            return false;
        }
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            runtime
                .run_until_terminal(&project_path, &project_id, &job_id)
                .await;
            if let Ok(mut active) = runtime.active_jobs.lock() {
                active.remove(&key);
            }
        });
        true
    }

    pub async fn run_until_terminal(&self, project_path: &str, project_id: &str, job_id: &str) {
        let mut next_recovery = tokio::time::Instant::now();
        loop {
            let snapshot = match self.jobs.get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            }) {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            if snapshot.status.is_terminal() || !self.supports_media_job(&snapshot) {
                return;
            }
            if matches!(
                snapshot.status,
                JobStatusData::Queued | JobStatusData::Retrying
            ) {
                let Ok(worker_permit) = self.worker_slots.clone().try_acquire_owned() else {
                    tokio::time::sleep(Duration::from_millis(MEDIA_WORKER_POLL_MS)).await;
                    continue;
                };
                match self.jobs.claim_job(ClaimJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.to_owned(),
                    worker_id: MEDIA_WORKER_ID.to_owned(),
                    lease_duration_ms: MEDIA_LEASE_MS,
                }) {
                    Ok(Some(claimed)) => {
                        self.run_claimed(project_path, project_id, claimed).await;
                        drop(worker_permit);
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            } else if snapshot.status == JobStatusData::Running
                && tokio::time::Instant::now() >= next_recovery
            {
                let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                });
                next_recovery =
                    tokio::time::Instant::now() + Duration::from_millis(MEDIA_RECOVERY_POLL_MS);
            }
            tokio::time::sleep(Duration::from_millis(MEDIA_WORKER_POLL_MS)).await;
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
            .acknowledge_if_cancellation_requested(project_path, project_id, &job_id, &lease_id)
            .unwrap_or(false)
        {
            return;
        }
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.1,
            message: Some("正在复核媒体任务请求与冻结输入".to_owned()),
        });
        let request = match self.load_execution_request(project_path, project_id, &claimed) {
            Ok(request) => request,
            Err(error) => {
                if !self
                    .acknowledge_if_cancellation_requested(
                        project_path,
                        project_id,
                        &job_id,
                        &lease_id,
                    )
                    .unwrap_or(false)
                {
                    self.fail_claimed(
                        project_path,
                        project_id,
                        &job_id,
                        &lease_id,
                        MediaWorkerFailure::from_runtime(error),
                    );
                }
                return;
            }
        };
        if self
            .acknowledge_if_cancellation_requested(project_path, project_id, &job_id, &lease_id)
            .unwrap_or(false)
        {
            return;
        }
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.35,
            message: Some("正在执行受限媒体处理".to_owned()),
        });
        let output = match self
            .execute_request_off_thread(project_path, project_id, &job_id, &lease_id, request)
            .await
        {
            Ok(output) => output,
            Err(failure) => {
                self.fail_claimed(project_path, project_id, &job_id, &lease_id, failure);
                return;
            }
        };
        for artifact_id in &output.artifact_ids {
            if let Err(error) = self.jobs.record_job_artifact(RecordJobArtifactOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.clone(),
                lease_id: lease_id.clone(),
                artifact_id: artifact_id.clone(),
            }) {
                self.fail_claimed(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    MediaWorkerFailure::from_job(error),
                );
                return;
            }
        }
        let _ = self.jobs.report_job_progress(ReportJobProgressOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            progress: 0.85,
            message: Some("媒体 Artifact 已登记，正在提交终态".to_owned()),
        });
        match self.acknowledge_if_cancellation_requested(
            project_path,
            project_id,
            &job_id,
            &lease_id,
        ) {
            Ok(true) => return,
            Ok(false) => {}
            Err(failure) => {
                self.fail_claimed(project_path, project_id, &job_id, &lease_id, failure);
                return;
            }
        }
        if let Err(error) = self.jobs.complete_job(CompleteJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.clone(),
            lease_id: lease_id.clone(),
            artifact_ids: output.artifact_ids,
            log_summary: output.log_summary,
        }) {
            if self
                .acknowledge_if_cancellation_requested(project_path, project_id, &job_id, &lease_id)
                .unwrap_or(false)
            {
                return;
            }
            let _ = self.jobs.recover_project_jobs(RecoverJobsOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
            });
            let finalized = self
                .jobs
                .get_job(GetJobOptions {
                    project_path: project_path.to_owned(),
                    expected_project_id: project_id.to_owned(),
                    job_id: job_id.clone(),
                })
                .is_ok_and(|snapshot| snapshot.status.is_terminal());
            if !finalized {
                self.fail_claimed(
                    project_path,
                    project_id,
                    &job_id,
                    &lease_id,
                    MediaWorkerFailure::from_job(error),
                );
            }
        }
    }

    async fn execute_request_off_thread(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        request: MediaRuntimeRequest,
    ) -> Result<MediaExecutionOutput, MediaWorkerFailure> {
        let runtime = self.clone();
        let execution_project_path = project_path.to_owned();
        let execution_project_id = project_id.to_owned();
        let mut execution = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(gate) = runtime.execution_test_gate.as_ref() {
                gate.block_worker_until_released();
            }
            runtime.execute_request(execution_project_path, execution_project_id, request)
        });
        let mut next_heartbeat =
            tokio::time::Instant::now() + Duration::from_millis(MEDIA_LEASE_HEARTBEAT_MS);
        loop {
            tokio::select! {
                result = &mut execution => {
                    return result
                        .map_err(|_| MediaWorkerFailure::worker_stopped())?;
                }
                _ = tokio::time::sleep(Duration::from_millis(MEDIA_WORKER_POLL_MS)) => {
                    let Ok(snapshot) = self.jobs.get_job(GetJobOptions {
                        project_path: project_path.to_owned(),
                        expected_project_id: project_id.to_owned(),
                        job_id: job_id.to_owned(),
                    }) else {
                        continue;
                    };
                    if snapshot.cancellation_requested || snapshot.status.is_terminal() {
                        return execution
                            .await
                            .map_err(|_| MediaWorkerFailure::worker_stopped())?;
                    }
                    let now = tokio::time::Instant::now();
                    if now >= next_heartbeat {
                        let _ = self.jobs.renew_job_lease(RenewJobLeaseOptions {
                            project_path: project_path.to_owned(),
                            expected_project_id: project_id.to_owned(),
                            job_id: job_id.to_owned(),
                            lease_id: lease_id.to_owned(),
                            lease_duration_ms: MEDIA_LEASE_MS,
                        });
                        next_heartbeat = now
                            + Duration::from_millis(MEDIA_LEASE_HEARTBEAT_MS);
                    }
                }
            }
        }
    }

    fn acknowledge_if_cancellation_requested(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
    ) -> Result<bool, MediaWorkerFailure> {
        let snapshot = self
            .jobs
            .get_job(GetJobOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            })
            .map_err(MediaWorkerFailure::from_job)?;
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
            .map_err(MediaWorkerFailure::from_job)?;
        Ok(true)
    }

    fn fail_claimed(
        &self,
        project_path: &str,
        project_id: &str,
        job_id: &str,
        lease_id: &str,
        failure: MediaWorkerFailure,
    ) {
        let _ = self.jobs.fail_job(FailJobOptions {
            project_path: project_path.to_owned(),
            expected_project_id: project_id.to_owned(),
            job_id: job_id.to_owned(),
            lease_id: lease_id.to_owned(),
            error: JobFailureData {
                code: failure.code.clone(),
                message: failure.message.clone(),
                retryable: failure.retryable,
                details: Map::new(),
            },
            log_summary: json!({
                "message": "媒体任务执行未完成。",
                "warnings": [],
                "errors": [failure.code],
            }),
        });
    }

    fn load_execution_request(
        &self,
        project_path: &str,
        project_id: &str,
        claimed: &JobSnapshotData,
    ) -> Result<MediaRuntimeRequest, MediaRuntimeError> {
        if claimed.owner_project_id != project_id || !self.supports_media_job(claimed) {
            return Err(MediaRuntimeError::InvalidSnapshot(
                "supported media executor identity",
            ));
        }
        let job_id = claimed
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("jobId"))?;
        let stage_id = claimed
            .job
            .get("stageId")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("stageId"))?;
        let run_id = claimed
            .job
            .get("stageRunId")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("stageRunId"))?;
        let idempotency_hash = claimed
            .job
            .get("idempotencyHash")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("idempotencyHash"))?;

        let receipt = self
            .jobs
            .get_stage_job_request(GetStageJobRequestOptions {
                project_path: project_path.to_owned(),
                expected_project_id: project_id.to_owned(),
                job_id: job_id.to_owned(),
            })
            .map_err(MediaRuntimeError::Job)?;
        if receipt.owner_project_id != project_id
            || receipt.job_id != job_id
            || receipt.stage_id != stage_id
            || receipt.run_id != run_id
        {
            return Err(MediaRuntimeError::InvalidSnapshot(
                "verified media request receipt identity",
            ));
        }

        let request: MediaRuntimeRequest = serde_json::from_value(receipt.request)
            .map_err(|error| MediaRuntimeError::Serialization(error.to_string()))?;
        let (
            request_version,
            request_project_id,
            request_run_id,
            request_stage_id,
            input_refs,
            idempotency_key,
            input_kinds,
        ): (
            &str,
            &str,
            &str,
            &str,
            &[FrozenArtifactInputData],
            &str,
            &[&str],
        ) = match &request {
            MediaRuntimeRequest::EnqueueAudioImport {
                request_version,
                project_id,
                run_id,
                input_refs,
                idempotency_key,
                ..
            } => (
                request_version,
                project_id,
                run_id,
                "audio",
                input_refs,
                idempotency_key,
                &["script"],
            ),
            MediaRuntimeRequest::EnqueueCaptionsImport {
                request_version,
                project_id,
                run_id,
                input_refs,
                idempotency_key,
                ..
            } => (
                request_version,
                project_id,
                run_id,
                "captions",
                input_refs,
                idempotency_key,
                &["script", "voice_audio"],
            ),
            MediaRuntimeRequest::GenerateScenePlan {
                request_version,
                project_id,
                run_id,
                input_refs,
                idempotency_key,
                ..
            } => (
                request_version,
                project_id,
                run_id,
                "scene_plan",
                input_refs,
                idempotency_key,
                &["claim_set", "script", "captions"],
            ),
            MediaRuntimeRequest::GenerateTimeline {
                request_version,
                project_id,
                run_id,
                input_refs,
                idempotency_key,
                ..
            } => (
                request_version,
                project_id,
                run_id,
                "timeline",
                input_refs,
                idempotency_key,
                &["voice_audio", "captions", "scene_plan"],
            ),
        };
        let request_idempotency_hash = Sha256::digest(idempotency_key.as_bytes()).iter().fold(
            "sha256:".to_owned(),
            |mut hash, byte| {
                hash.push_str(&format!("{byte:02x}"));
                hash
            },
        );
        if request_version != MEDIA_RUNTIME_REQUEST_VERSION
            || request_project_id != project_id
            || request_run_id != run_id
            || request_stage_id != stage_id
            || request_idempotency_hash != idempotency_hash
            || input_refs.len() != input_kinds.len()
        {
            return Err(MediaRuntimeError::InvalidSnapshot("media request binding"));
        }
        let expected_input_refs = Value::Array(
            input_refs
                .iter()
                .zip(input_kinds.iter())
                .map(|(input, kind)| job_artifact_input_ref(input, kind))
                .collect(),
        );
        if claimed.job.get("inputRefs") != Some(&expected_input_refs) {
            return Err(MediaRuntimeError::InvalidSnapshot(
                "media request inputRefs binding",
            ));
        }

        Ok(request)
    }

    fn execute_request(
        &self,
        project_path: String,
        project_id: String,
        request: MediaRuntimeRequest,
    ) -> Result<MediaExecutionOutput, MediaWorkerFailure> {
        match request {
            MediaRuntimeRequest::EnqueueAudioImport {
                run_id,
                staged_source_uri,
                source_file_name,
                source_content_hash,
                source_byte_length,
                rights,
                limits,
                input_refs,
                config_snapshot,
                idempotency_key,
                ..
            } => {
                let resolved = self
                    .storage
                    .resolve_staged_media_source(ResolveStagedMediaSourceOptions {
                        project_path: project_path.clone(),
                        expected_project_id: project_id.clone(),
                        staged_source_uri: staged_source_uri.clone(),
                        expected_content_hash: source_content_hash.clone(),
                        expected_byte_length: source_byte_length,
                        max_bytes: limits.max_bytes,
                    })
                    .map_err(MediaWorkerFailure::from_storage)?;
                if resolved.owner_project_id != project_id
                    || resolved.staged_source_uri != staged_source_uri
                    || resolved.source_file_name != source_file_name
                    || resolved.content_hash != source_content_hash
                    || resolved.byte_length != source_byte_length
                {
                    return Err(MediaWorkerFailure::invalid_receipt());
                }
                let [script_input]: [FrozenArtifactInputData; 1] = input_refs
                    .try_into()
                    .map_err(|_| MediaWorkerFailure::invalid_receipt())?;
                let result = self
                    .media
                    .import_audio(ImportAudioOptions {
                        project_path,
                        expected_project_id: project_id.clone(),
                        run_id: run_id.clone(),
                        source_path: resolved.source_path,
                        expected_source_content_hash: Some(source_content_hash),
                        script_input,
                        rights,
                        limits,
                        config_snapshot,
                        idempotency_key,
                    })
                    .map_err(MediaWorkerFailure::from_media)?;
                if result.owner_project_id != project_id || result.run_id != run_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                if result.raw_artifact_id == result.artifact_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                Ok(MediaExecutionOutput {
                    artifact_ids: vec![result.artifact_id],
                    log_summary: media_success_log("音频导入完成。"),
                })
            }
            MediaRuntimeRequest::EnqueueCaptionsImport {
                run_id,
                staged_source_uri,
                source_file_name,
                source_content_hash,
                source_byte_length,
                audio_duration_ms,
                rights,
                limits,
                input_refs,
                config_snapshot,
                idempotency_key,
                ..
            } => {
                let resolved = self
                    .storage
                    .resolve_staged_media_source(ResolveStagedMediaSourceOptions {
                        project_path: project_path.clone(),
                        expected_project_id: project_id.clone(),
                        staged_source_uri: staged_source_uri.clone(),
                        expected_content_hash: source_content_hash.clone(),
                        expected_byte_length: source_byte_length,
                        max_bytes: limits.max_bytes,
                    })
                    .map_err(MediaWorkerFailure::from_storage)?;
                if resolved.owner_project_id != project_id
                    || resolved.staged_source_uri != staged_source_uri
                    || resolved.source_file_name != source_file_name
                    || resolved.content_hash != source_content_hash
                    || resolved.byte_length != source_byte_length
                {
                    return Err(MediaWorkerFailure::invalid_receipt());
                }
                let [script_input, audio_input]: [FrozenArtifactInputData; 2] = input_refs
                    .try_into()
                    .map_err(|_| MediaWorkerFailure::invalid_receipt())?;
                let result = self
                    .media
                    .import_captions(ImportCaptionsOptions {
                        project_path,
                        expected_project_id: project_id.clone(),
                        run_id: run_id.clone(),
                        source_path: resolved.source_path,
                        expected_source_content_hash: Some(source_content_hash),
                        script_input,
                        audio_input,
                        audio_duration_ms,
                        rights,
                        limits,
                        config_snapshot,
                        idempotency_key,
                    })
                    .map_err(MediaWorkerFailure::from_media)?;
                if result.owner_project_id != project_id || result.run_id != run_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                if result.raw_artifact_id == result.artifact_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                Ok(MediaExecutionOutput {
                    artifact_ids: vec![result.artifact_id],
                    log_summary: media_success_log("字幕导入完成。"),
                })
            }
            MediaRuntimeRequest::GenerateScenePlan {
                run_id,
                input_refs,
                idempotency_key,
                ..
            } => {
                let [research_input, script_input, captions_input]: [FrozenArtifactInputData; 3] =
                    input_refs
                        .try_into()
                        .map_err(|_| MediaWorkerFailure::invalid_receipt())?;
                let result = self
                    .media
                    .generate_scene_plan(GenerateScenePlanOptions {
                        project_path,
                        expected_project_id: project_id.clone(),
                        run_id: run_id.clone(),
                        research_input,
                        script_input,
                        captions_input,
                        idempotency_key,
                    })
                    .map_err(MediaWorkerFailure::from_media)?;
                if result.owner_project_id != project_id || result.run_id != run_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                Ok(MediaExecutionOutput {
                    artifact_ids: vec![result.artifact_id],
                    log_summary: media_success_log("分镜计划生成完成。"),
                })
            }
            MediaRuntimeRequest::GenerateTimeline {
                run_id,
                input_refs,
                canvas,
                safe_area,
                idempotency_key,
                ..
            } => {
                let [audio_input, captions_input, scene_plan_input]: [FrozenArtifactInputData; 3] =
                    input_refs
                        .try_into()
                        .map_err(|_| MediaWorkerFailure::invalid_receipt())?;
                let result = self
                    .media
                    .generate_timeline(GenerateTimelineOptions {
                        project_path,
                        expected_project_id: project_id.clone(),
                        run_id: run_id.clone(),
                        audio_input,
                        captions_input,
                        scene_plan_input,
                        canvas,
                        safe_area,
                        idempotency_key,
                    })
                    .map_err(MediaWorkerFailure::from_media)?;
                if result.owner_project_id != project_id || result.run_id != run_id {
                    return Err(MediaWorkerFailure::invalid_output());
                }
                Ok(MediaExecutionOutput {
                    artifact_ids: vec![result.artifact_id],
                    log_summary: media_success_log("时间轴生成完成。"),
                })
            }
        }
    }

    /// Stages an external audio source before enqueueing a durable local media job.
    /// `source_path` is consumed only by staging and is never included in the job receipt.
    pub fn enqueue_audio_import(
        &self,
        options: AudioImportEnqueueOptions,
    ) -> Result<MediaJobEnqueueOutcome, MediaRuntimeError> {
        let staged = self
            .storage
            .stage_media_source_file(StageMediaSourceFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: options.source_path,
                expected_content_hash: options.expected_source_content_hash,
                max_bytes: options.limits.max_bytes,
            })
            .map_err(MediaRuntimeError::Storage)?;

        let input_refs = vec![job_artifact_input_ref(&options.script_input, "script")];
        let request = MediaRuntimeRequest::EnqueueAudioImport {
            request_version: MEDIA_RUNTIME_REQUEST_VERSION.to_owned(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            staged_source_uri: staged.staged_source_uri,
            source_file_name: staged.source_file_name,
            source_content_hash: staged.content_hash,
            source_byte_length: staged.byte_length,
            rights: options.rights,
            limits: options.limits,
            input_refs: vec![options.script_input],
            config_snapshot: options.config_snapshot,
            idempotency_key: options.idempotency_key.clone(),
        };

        self.enqueue_request(
            MediaEnqueueRequestOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                stage_id: "audio",
                run_id: options.run_id,
                input_refs,
                idempotency_key: options.idempotency_key,
            },
            request,
        )
    }

    pub fn enqueue_captions_import(
        &self,
        options: CaptionsImportEnqueueOptions,
    ) -> Result<MediaJobEnqueueOutcome, MediaRuntimeError> {
        let staged = self
            .storage
            .stage_media_source_file(StageMediaSourceFileOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                source_path: options.source_path,
                expected_content_hash: options.expected_source_content_hash,
                max_bytes: options.limits.max_bytes,
            })
            .map_err(MediaRuntimeError::Storage)?;
        let input_refs = vec![
            job_artifact_input_ref(&options.script_input, "script"),
            job_artifact_input_ref(&options.audio_input, "voice_audio"),
        ];
        let request = MediaRuntimeRequest::EnqueueCaptionsImport {
            request_version: MEDIA_RUNTIME_REQUEST_VERSION.to_owned(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            staged_source_uri: staged.staged_source_uri,
            source_file_name: staged.source_file_name,
            source_content_hash: staged.content_hash,
            source_byte_length: staged.byte_length,
            audio_duration_ms: options.audio_duration_ms,
            rights: options.rights,
            limits: options.limits,
            input_refs: vec![options.script_input, options.audio_input],
            config_snapshot: options.config_snapshot,
            idempotency_key: options.idempotency_key.clone(),
        };

        self.enqueue_request(
            MediaEnqueueRequestOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                stage_id: "captions",
                run_id: options.run_id,
                input_refs,
                idempotency_key: options.idempotency_key,
            },
            request,
        )
    }

    pub fn generate_scene_plan(
        &self,
        options: ScenePlanEnqueueOptions,
    ) -> Result<MediaJobEnqueueOutcome, MediaRuntimeError> {
        let input_refs = vec![
            job_artifact_input_ref(&options.research_input, "claim_set"),
            job_artifact_input_ref(&options.script_input, "script"),
            job_artifact_input_ref(&options.captions_input, "captions"),
        ];
        let request = MediaRuntimeRequest::GenerateScenePlan {
            request_version: MEDIA_RUNTIME_REQUEST_VERSION.to_owned(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            input_refs: vec![
                options.research_input,
                options.script_input,
                options.captions_input,
            ],
            config_snapshot: options.config_snapshot,
            idempotency_key: options.idempotency_key.clone(),
        };

        self.enqueue_request(
            MediaEnqueueRequestOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                stage_id: "scene_plan",
                run_id: options.run_id,
                input_refs,
                idempotency_key: options.idempotency_key,
            },
            request,
        )
    }

    pub fn generate_timeline(
        &self,
        options: TimelineEnqueueOptions,
    ) -> Result<MediaJobEnqueueOutcome, MediaRuntimeError> {
        let input_refs = vec![
            job_artifact_input_ref(&options.audio_input, "voice_audio"),
            job_artifact_input_ref(&options.captions_input, "captions"),
            job_artifact_input_ref(&options.scene_plan_input, "scene_plan"),
        ];
        let request = MediaRuntimeRequest::GenerateTimeline {
            request_version: MEDIA_RUNTIME_REQUEST_VERSION.to_owned(),
            project_id: options.expected_project_id.clone(),
            run_id: options.run_id.clone(),
            input_refs: vec![
                options.audio_input,
                options.captions_input,
                options.scene_plan_input,
            ],
            canvas: options.canvas,
            safe_area: options.safe_area,
            config_snapshot: options.config_snapshot,
            idempotency_key: options.idempotency_key.clone(),
        };

        self.enqueue_request(
            MediaEnqueueRequestOptions {
                project_path: options.project_path,
                expected_project_id: options.expected_project_id,
                stage_id: "timeline",
                run_id: options.run_id,
                input_refs,
                idempotency_key: options.idempotency_key,
            },
            request,
        )
    }

    pub fn retry_media_job(
        &self,
        options: MediaRetryOptions,
    ) -> Result<JobSnapshotData, MediaRuntimeError> {
        let source = self
            .jobs
            .get_job(GetJobOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                job_id: options.source_job_id.clone(),
            })
            .map_err(MediaRuntimeError::Job)?;
        if !self.supports_media_job(&source) {
            return Err(media_retry_error(
                JobErrorCode::InvalidRequest,
                &options.source_job_id,
                "只有本地媒体运行时拥有的任务可以通过媒体重试入口重试。",
            ));
        }
        if !matches!(
            source.status,
            JobStatusData::Failed | JobStatusData::Canceled
        ) {
            return Err(media_retry_error(
                JobErrorCode::InvalidTransition,
                &options.source_job_id,
                "只有 failed 或 canceled 媒体任务可以创建新的重试运行。",
            ));
        }

        // The strict loader re-reads and verifies the immutable request receipt, then
        // binds its typed fields back to the source JobDefinition before we retarget it.
        let request = self.load_execution_request(
            &options.project_path,
            &options.expected_project_id,
            &source,
        )?;
        let retry_request = request
            .rebind_retry_identity(options.new_run_id.clone(), options.idempotency_key.clone());
        let retry_request = serde_json::to_value(retry_request)
            .map_err(|error| MediaRuntimeError::Serialization(error.to_string()))?;

        let stage_id = source
            .job
            .get("stageId")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("stageId"))?
            .to_owned();
        let input_refs = source
            .job
            .get("inputRefs")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(MediaRuntimeError::InvalidSnapshot("inputRefs"))?;
        let executor = source
            .job
            .get("executor")
            .cloned()
            .ok_or(MediaRuntimeError::InvalidSnapshot("executor"))?;
        let retry_policy = serde_json::from_value::<RetryPolicyData>(
            source
                .job
                .get("retryPolicy")
                .cloned()
                .ok_or(MediaRuntimeError::InvalidSnapshot("retryPolicy"))?,
        )
        .map_err(|_| MediaRuntimeError::InvalidSnapshot("retryPolicy"))?;

        let claim = self
            .jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: options.project_path.clone(),
                expected_project_id: options.expected_project_id.clone(),
                idempotency_key: options.idempotency_key.clone(),
                request: retry_request.clone(),
            })
            .map_err(MediaRuntimeError::Job)?;
        if claim.owner_project_id != options.expected_project_id || claim.request != retry_request {
            return Err(MediaRuntimeError::InvalidSnapshot(
                "claimed media retry request identity",
            ));
        }

        let snapshot = self
            .jobs
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path: options.project_path,
                    expected_project_id: options.expected_project_id.clone(),
                    stage_id,
                    run_id: options.new_run_id,
                    input_refs,
                    executor,
                    idempotency_key: options.idempotency_key,
                    retry_policy,
                },
                retry_request,
            )
            .map_err(MediaRuntimeError::Job)?;
        if snapshot.owner_project_id != options.expected_project_id
            || snapshot.job.get("jobId").and_then(Value::as_str) != Some(claim.job_id.as_str())
        {
            return Err(MediaRuntimeError::InvalidSnapshot(
                "enqueued media retry job identity",
            ));
        }
        Ok(snapshot)
    }

    fn enqueue_request(
        &self,
        options: MediaEnqueueRequestOptions,
        request: MediaRuntimeRequest,
    ) -> Result<MediaJobEnqueueOutcome, MediaRuntimeError> {
        let MediaEnqueueRequestOptions {
            project_path,
            expected_project_id,
            stage_id,
            run_id,
            input_refs,
            idempotency_key,
        } = options;
        let request = serde_json::to_value(request)
            .map_err(|error| MediaRuntimeError::Serialization(error.to_string()))?;

        let claim = self
            .jobs
            .claim_stage_job_request(ClaimStageJobRequestOptions {
                project_path: project_path.clone(),
                expected_project_id: expected_project_id.clone(),
                idempotency_key: idempotency_key.clone(),
                request: request.clone(),
            })
            .map_err(MediaRuntimeError::Job)?;
        let snapshot = self
            .jobs
            .enqueue_stage_job_with_request(
                EnqueueStageJobOptions {
                    project_path,
                    expected_project_id,
                    stage_id: stage_id.to_owned(),
                    run_id: run_id.clone(),
                    input_refs,
                    executor: media_runtime_executor(),
                    idempotency_key,
                    retry_policy: media_retry_policy(),
                },
                request,
            )
            .map_err(MediaRuntimeError::Job)?;
        let job_id = snapshot
            .job
            .get("jobId")
            .and_then(Value::as_str)
            .ok_or(MediaRuntimeError::InvalidSnapshot("jobId"))?
            .to_owned();

        Ok(MediaJobEnqueueOutcome {
            owner_project_id: snapshot.owner_project_id,
            run_id,
            job_id,
            idempotent_replay: claim.idempotent_replay,
        })
    }
}

struct MediaEnqueueRequestOptions {
    project_path: String,
    expected_project_id: String,
    stage_id: &'static str,
    run_id: String,
    input_refs: Vec<Value>,
    idempotency_key: String,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct MediaExecutionTestGate {
    entered: Arc<AtomicBool>,
    entered_notify: Arc<tokio::sync::Notify>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

#[cfg(test)]
impl MediaExecutionTestGate {
    pub(crate) fn new() -> Self {
        Self {
            entered: Arc::new(AtomicBool::new(false)),
            entered_notify: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    pub(crate) async fn wait_until_entered(&self) {
        loop {
            let notified = self.entered_notify.notified();
            if self.entered.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn release(&self) {
        let (released, condition) = &*self.release;
        if let Ok(mut released) = released.lock() {
            *released = true;
            condition.notify_all();
        }
    }

    fn block_worker_until_released(&self) {
        self.entered.store(true, Ordering::Release);
        self.entered_notify.notify_waiters();
        let (released, condition) = &*self.release;
        let mut released = released.lock().unwrap_or_else(|error| error.into_inner());
        while !*released {
            released = condition
                .wait(released)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioImportEnqueueOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub source_path: String,
    pub expected_source_content_hash: Option<String>,
    pub script_input: FrozenArtifactInputData,
    pub rights: MediaRightsData,
    pub limits: PcmWavParseLimits,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptionsImportEnqueueOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub source_path: String,
    pub expected_source_content_hash: Option<String>,
    pub script_input: FrozenArtifactInputData,
    pub audio_input: FrozenArtifactInputData,
    pub audio_duration_ms: u64,
    pub rights: MediaRightsData,
    pub limits: SrtParseLimits,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScenePlanEnqueueOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub research_input: FrozenArtifactInputData,
    pub script_input: FrozenArtifactInputData,
    pub captions_input: FrozenArtifactInputData,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEnqueueOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub audio_input: FrozenArtifactInputData,
    pub captions_input: FrozenArtifactInputData,
    pub scene_plan_input: FrozenArtifactInputData,
    pub canvas: TimelineCanvasData,
    pub safe_area: TimelineSafeAreaData,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRetryOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub source_job_id: String,
    pub new_run_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaJobEnqueueOutcome {
    pub owner_project_id: String,
    pub run_id: String,
    pub job_id: String,
    pub idempotent_replay: bool,
}

#[derive(Debug)]
pub enum MediaRuntimeError {
    Storage(StorageServiceError),
    Job(JobServiceError),
    Serialization(String),
    InvalidSnapshot(&'static str),
}

impl fmt::Display for MediaRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(formatter),
            Self::Job(error) => error.fmt(formatter),
            Self::Serialization(error) => {
                write!(formatter, "media request serialization failed: {error}")
            }
            Self::InvalidSnapshot(field) => {
                write!(formatter, "queued media job snapshot is missing {field}")
            }
        }
    }
}

impl Error for MediaRuntimeError {}

#[derive(Debug, Clone, PartialEq)]
struct MediaExecutionOutput {
    artifact_ids: Vec<String>,
    log_summary: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaWorkerFailure {
    code: String,
    message: String,
    retryable: bool,
}

impl MediaWorkerFailure {
    fn worker_stopped() -> Self {
        Self {
            code: "media_worker_stopped".to_owned(),
            message: "媒体 worker 在线程边界完成前异常停止。".to_owned(),
            retryable: true,
        }
    }

    fn invalid_receipt() -> Self {
        Self {
            code: "invalid_media_request_receipt".to_owned(),
            message: "媒体任务请求凭据无法通过身份与结构校验。".to_owned(),
            retryable: false,
        }
    }

    fn invalid_output() -> Self {
        Self {
            code: "invalid_media_execution_output".to_owned(),
            message: "媒体执行结果未通过项目、运行或 Artifact 身份校验。".to_owned(),
            retryable: false,
        }
    }

    fn from_job(error: JobServiceError) -> Self {
        let retryable = error.code == JobErrorCode::IoError;
        Self {
            code: if retryable {
                "media_job_io".to_owned()
            } else {
                format!(
                    "media_job_{}_{}",
                    error.operation.as_str(),
                    error.code.as_str()
                )
            },
            message: if retryable {
                "媒体任务状态暂时不可用。"
            } else {
                "媒体任务状态或请求凭据无效。"
            }
            .to_owned(),
            retryable,
        }
    }

    fn from_runtime(error: MediaRuntimeError) -> Self {
        match error {
            MediaRuntimeError::Job(error) => Self::from_job(error),
            MediaRuntimeError::Storage(error) => Self::from_storage(error),
            MediaRuntimeError::Serialization(_) | MediaRuntimeError::InvalidSnapshot(_) => {
                Self::invalid_receipt()
            }
        }
    }

    fn from_storage(error: StorageServiceError) -> Self {
        let retryable = matches!(
            error.code,
            StorageErrorCode::IoError
                | StorageErrorCode::IndexUnavailable
                | StorageErrorCode::IndexMigrationFailed
        );
        Self {
            code: format!("media_source_{}", error.code.as_str()),
            message: if retryable {
                "暂存媒体源当前不可用，可稍后重试。"
            } else {
                "暂存媒体源不存在、越界或完整性校验失败。"
            }
            .to_owned(),
            retryable,
        }
    }

    fn from_media(error: MediaServiceError) -> Self {
        let retryable = matches!(
            error.code,
            MediaErrorCode::StorageUnavailable | MediaErrorCode::Io
        );
        Self {
            code: media_failure_code(error.code).to_owned(),
            message: if retryable {
                "媒体服务暂时不可用，可稍后重试。"
            } else {
                "媒体请求、审核输入、授权或生成结果未通过校验。"
            }
            .to_owned(),
            retryable,
        }
    }
}

fn media_failure_code(code: MediaErrorCode) -> &'static str {
    match code {
        MediaErrorCode::InvalidRequest => "media_invalid_request",
        MediaErrorCode::InvalidSourceName => "media_invalid_source_name",
        MediaErrorCode::SourceHashMismatch => "media_source_hash_mismatch",
        MediaErrorCode::SourceChanged => "media_source_changed",
        MediaErrorCode::RightsRequired => "media_rights_required",
        MediaErrorCode::VoiceCloneNotAllowed => "media_voice_clone_not_allowed",
        MediaErrorCode::InputNotApproved => "media_input_not_approved",
        MediaErrorCode::InputReferenceMismatch => "media_input_reference_mismatch",
        MediaErrorCode::CrossProjectReference => "media_cross_project_reference",
        MediaErrorCode::ArtifactVerificationFailed => "media_artifact_verification_failed",
        MediaErrorCode::IdempotencyConflict => "media_idempotency_conflict",
        MediaErrorCode::ResourceLimitExceeded => "media_resource_limit_exceeded",
        MediaErrorCode::InvalidMedia => "media_invalid_media",
        MediaErrorCode::ContractViolation => "media_contract_violation",
        MediaErrorCode::StorageUnavailable => "media_storage_unavailable",
        MediaErrorCode::Io => "media_io",
    }
}

fn media_success_log(message: &str) -> Value {
    json!({
        "message": message,
        "warnings": [],
        "errors": [],
    })
}

fn media_runtime_executor() -> Value {
    json!({
        "providerId": MEDIA_RUNTIME_PROVIDER_ID,
        "providerVersion": MEDIA_RUNTIME_PROVIDER_VERSION,
        "executionMode": MEDIA_RUNTIME_EXECUTION_MODE,
        "model": MEDIA_RUNTIME_MODEL,
    })
}

fn media_retry_policy() -> RetryPolicyData {
    RetryPolicyData {
        max_attempts: 3,
        initial_backoff_ms: 1_000,
        backoff_multiplier: 2,
        max_backoff_ms: 15_000,
    }
}

fn media_retry_error(
    code: JobErrorCode,
    source_job_id: &str,
    message: &'static str,
) -> MediaRuntimeError {
    MediaRuntimeError::Job(
        JobServiceError::new(code, JobOperation::RetryStageJob, message)
            .for_job(source_job_id.to_owned()),
    )
}

fn job_artifact_input_ref(input: &FrozenArtifactInputData, kind: &str) -> Value {
    json!({
        "refId": format!("media_{}_{}", input.stage_id, input.artifact_id),
        "referenceType": "artifact",
        "kind": kind,
        "contentHash": input.content_hash,
        "artifactId": input.artifact_id,
        "sourceRunId": input.run_id,
        "reviewRecordId": input.review_record_id,
        "claimIds": input.claim_ids,
        "evidenceRefs": input.evidence_refs,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum MediaRuntimeRequest {
    EnqueueAudioImport {
        request_version: String,
        project_id: String,
        run_id: String,
        staged_source_uri: String,
        source_file_name: String,
        source_content_hash: String,
        source_byte_length: u64,
        rights: MediaRightsData,
        limits: PcmWavParseLimits,
        input_refs: Vec<FrozenArtifactInputData>,
        config_snapshot: Value,
        idempotency_key: String,
    },
    EnqueueCaptionsImport {
        request_version: String,
        project_id: String,
        run_id: String,
        staged_source_uri: String,
        source_file_name: String,
        source_content_hash: String,
        source_byte_length: u64,
        audio_duration_ms: u64,
        rights: MediaRightsData,
        limits: SrtParseLimits,
        input_refs: Vec<FrozenArtifactInputData>,
        config_snapshot: Value,
        idempotency_key: String,
    },
    GenerateScenePlan {
        request_version: String,
        project_id: String,
        run_id: String,
        input_refs: Vec<FrozenArtifactInputData>,
        config_snapshot: Value,
        idempotency_key: String,
    },
    GenerateTimeline {
        request_version: String,
        project_id: String,
        run_id: String,
        input_refs: Vec<FrozenArtifactInputData>,
        canvas: TimelineCanvasData,
        safe_area: TimelineSafeAreaData,
        config_snapshot: Value,
        idempotency_key: String,
    },
}

impl MediaRuntimeRequest {
    fn rebind_retry_identity(&self, new_run_id: String, new_idempotency_key: String) -> Self {
        let mut request = self.clone();
        match &mut request {
            Self::EnqueueAudioImport {
                run_id,
                idempotency_key,
                ..
            }
            | Self::EnqueueCaptionsImport {
                run_id,
                idempotency_key,
                ..
            }
            | Self::GenerateScenePlan {
                run_id,
                idempotency_key,
                ..
            }
            | Self::GenerateTimeline {
                run_id,
                idempotency_key,
                ..
            } => {
                *run_id = new_run_id;
                *idempotency_key = new_idempotency_key;
            }
        }
        request
    }
}
