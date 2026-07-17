use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use narracut_contracts::{validate_contract_document, NARRACUT_CONTRACT_VERSION};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AcknowledgeCancellationOptions, CancelJobOptions, ClaimNextJobOptions, CompleteJobOptions,
    EnqueueStageJobOptions, FailJobOptions, GetJobOptions, IndexedJobStatusData,
    IndexedJobUpsertData, JobErrorCode, JobEventsResultData, JobFailureData, JobLeaseData,
    JobListResultData, JobOperation, JobRecoveryResultData, JobServiceError, JobSnapshotData,
    JobStatusData, ListJobEventsOptions, ListJobsOptions, PrepareStageRunOptions,
    ProjectDescriptorData, ProjectErrorCode, ProjectService, ProjectServiceError,
    RecordJobArtifactOptions, RecordStageRunOptions, RecoverJobsOptions, RenewJobLeaseOptions,
    ReportJobProgressOptions, RetryPolicyData, RetryStageJobOptions, StorageService,
    TerminalRunStatusData, WorkflowErrorCode, WorkflowService, WorkflowServiceError,
};

pub const JOB_COMMAND_API_VERSION: &str = "1.0.0";
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_JOBS: usize = 1024;
const MAX_EVENTS_PER_JOB: usize = 4096;
const MAX_LIST_JOBS: u32 = 200;
const MAX_LIST_EVENTS: u32 = 500;
const MAX_LEASE_MS: u64 = 5 * 60 * 1000;

pub trait JobClock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug, Default)]
pub struct SystemJobClock;

impl JobClock for SystemJobClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Clone)]
pub struct JobService {
    project_service: ProjectService,
    storage_service: StorageService,
    workflow_service: WorkflowService,
    clock: Arc<dyn JobClock>,
}

impl JobService {
    pub fn new(
        project_service: ProjectService,
        storage_service: StorageService,
        workflow_service: WorkflowService,
    ) -> Self {
        Self::with_clock(
            project_service,
            storage_service,
            workflow_service,
            Arc::new(SystemJobClock),
        )
    }

    pub fn with_clock(
        project_service: ProjectService,
        storage_service: StorageService,
        workflow_service: WorkflowService,
        clock: Arc<dyn JobClock>,
    ) -> Self {
        Self {
            project_service,
            storage_service,
            workflow_service,
            clock,
        }
    }

    pub fn enqueue_stage_job(
        &self,
        options: EnqueueStageJobOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::EnqueueStageJob;
        validate_enqueue_options(&options, operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = PathBuf::from(&descriptor.project_path);
        ensure_project_directories(&project_dir, &["jobs"], operation)?;

        let idempotency_hash = hash_bytes(options.idempotency_key.as_bytes());
        let job_id = deterministic_job_id(&descriptor.project_id, &options.idempotency_key);
        let request_hash = hash_json(
            &json!({
                "projectId": descriptor.project_id,
                "stageId": options.stage_id,
                "stageRunId": options.run_id,
                "inputRefs": options.input_refs,
                "executor": options.executor,
                "retryPolicy": options.retry_policy,
            }),
            operation,
        )?;
        let job_path = job_definition_path(&project_dir, &job_id);
        let existing_job = inspect_project_path(&project_dir, &job_path, operation)?.is_some();
        if !existing_job {
            ensure_job_slot_available(&project_dir, operation)?;
        }
        ensure_project_directories(&project_dir, &["jobs", &job_id], operation)?;
        let candidate = json!({
            "schemaVersion": NARRACUT_CONTRACT_VERSION,
            "documentType": "job_definition",
            "jobId": job_id,
            "projectId": descriptor.project_id,
            "jobType": "stage_run",
            "stageId": options.stage_id,
            "stageRunId": options.run_id,
            "executionSnapshotUri": format!(
                "runs/{}/{}/execution.json",
                options.stage_id, options.run_id
            ),
            "idempotencyHash": idempotency_hash,
            "requestHash": request_hash,
            "inputRefs": options.input_refs,
            "executor": options.executor,
            "retryPolicy": options.retry_policy,
            "createdAt": format_timestamp(self.clock.now(), operation)?,
        });
        validate_persistent_document(&candidate, operation, "JobDefinition")?;
        let job = claim_job_definition(&project_dir, &job_path, &candidate, operation)?;

        let events = scan_job_events(&project_dir, &job, operation)?;
        if !events.is_empty() {
            if let Some(error) = preparation_failure_error(&job, &events, operation)? {
                return Err(error);
            }
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }

        self.prepare_and_queue_existing_job(&descriptor, &job, operation)?;
        let events = scan_job_events(&project_dir, &job, operation)?;
        if let Some(error) = preparation_failure_error(&job, &events, operation)? {
            return Err(error);
        }
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn get_job(&self, options: GetJobOptions) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::GetJob;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let (job, events) = load_job(
            Path::new(&descriptor.project_path),
            &options.job_id,
            operation,
        )?;
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn list_jobs(
        &self,
        options: ListJobsOptions,
    ) -> Result<JobListResultData, JobServiceError> {
        let operation = JobOperation::ListJobs;
        if !(1..=MAX_LIST_JOBS).contains(&options.limit) {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                format!("limit 必须位于 1..={MAX_LIST_JOBS}。"),
            ));
        }
        let statuses = options.statuses.iter().copied().collect::<HashSet<_>>();
        if statuses.len() != options.statuses.len() {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                "status 过滤器不能重复。",
            ));
        }
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let mut jobs = Vec::new();
        for job_id in scan_job_ids(Path::new(&descriptor.project_path), operation)? {
            let (job, events) = load_job(Path::new(&descriptor.project_path), &job_id, operation)?;
            if events.is_empty() {
                continue;
            }
            let snapshot = self.snapshot_and_index(&descriptor, job, events, operation)?;
            if statuses.is_empty() || statuses.contains(&snapshot.status) {
                let updated_at = parse_timestamp(&snapshot.updated_at, operation)?;
                jobs.push((updated_at, snapshot));
            }
        }
        jobs.sort_by_key(|(updated_at, job)| Reverse((*updated_at, job.job_uri.clone())));
        jobs.truncate(options.limit as usize);
        Ok(JobListResultData {
            api_version: JOB_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            jobs: jobs.into_iter().map(|(_, job)| job).collect(),
        })
    }

    pub fn list_job_events(
        &self,
        options: ListJobEventsOptions,
    ) -> Result<JobEventsResultData, JobServiceError> {
        let operation = JobOperation::ListJobEvents;
        if !(1..=MAX_LIST_EVENTS).contains(&options.limit) {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                format!("limit 必须位于 1..={MAX_LIST_EVENTS}。"),
            ));
        }
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let (job, events) = load_job(
            Path::new(&descriptor.project_path),
            &options.job_id,
            operation,
        )?;
        let filtered = events
            .into_iter()
            .filter(|event| {
                options.after_sequence.is_none_or(|sequence| {
                    event.get("sequence").and_then(Value::as_u64) > Some(u64::from(sequence))
                })
            })
            .collect::<Vec<_>>();
        let has_more = filtered.len() > options.limit as usize;
        let events = filtered.into_iter().take(options.limit as usize).collect();
        Ok(JobEventsResultData {
            api_version: JOB_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id,
            job_id: required_string(&job, "jobId", operation)?,
            events,
            has_more,
        })
    }

    pub fn cancel_job(
        &self,
        options: CancelJobOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::CancelJob;
        validate_text(&options.message, "取消原因", 4096, operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let mut projection = project_job(&job, &events, operation)?;
        if projection.status.is_terminal() {
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }
        if projection.finalization_event.is_some() {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "任务已进入终态提交，不能再请求取消。",
            ));
        }
        if !projection.cancellation_requested {
            let mut object =
                self.base_event_object(&job, &projection, "cancel_requested", operation)?;
            object.insert(
                "status".to_owned(),
                Value::String(projection.status.as_str().to_owned()),
            );
            object.insert("message".to_owned(), Value::String(options.message));
            if let Some(lease) = &projection.lease {
                object.insert("leaseId".to_owned(), Value::String(lease.lease_id.clone()));
            }
            match append_job_event(project_dir, &job, &Value::Object(object), operation, false) {
                Ok(_) => {}
                Err(error) if error.code == JobErrorCode::EventConflict => {
                    let current = project_job(
                        &job,
                        &scan_job_events(project_dir, &job, operation)?,
                        operation,
                    )?;
                    if !current.cancellation_requested {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
            projection = project_job(
                &job,
                &scan_job_events(project_dir, &job, operation)?,
                operation,
            )?;
        }
        if projection.status != JobStatusData::Running {
            return self.finalize_cancellation(&descriptor, job, projection, operation);
        }
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn retry_stage_job(
        &self,
        options: RetryStageJobOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::RetryStageJob;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let (job, events) = load_job(
            Path::new(&descriptor.project_path),
            &options.source_job_id,
            operation,
        )?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        if !matches!(
            projection.status,
            JobStatusData::Failed | JobStatusData::Canceled
        ) {
            return Err(invalid_transition(
                operation,
                &options.source_job_id,
                "只有 failed 或 canceled 任务可以创建新的重试运行。",
            ));
        }
        let retry_policy: RetryPolicyData = serde_json::from_value(
            job.get("retryPolicy")
                .cloned()
                .ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 retryPolicy。"))?,
        )
        .map_err(|error| invalid_job(operation, format!("retryPolicy 无效：{error}")))?;
        self.enqueue_stage_job(EnqueueStageJobOptions {
            project_path: descriptor.project_path,
            expected_project_id: descriptor.project_id,
            stage_id: required_string(&job, "stageId", operation)?,
            run_id: options.new_run_id,
            input_refs: required_array(&job, "inputRefs", operation)?,
            executor: job
                .get("executor")
                .cloned()
                .ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 executor。"))?,
            idempotency_key: options.idempotency_key,
            retry_policy,
        })
        .map_err(|mut error| {
            error.operation = operation;
            error
        })
    }

    pub fn recover_project_jobs(
        &self,
        options: RecoverJobsOptions,
    ) -> Result<JobRecoveryResultData, JobServiceError> {
        let operation = JobOperation::RecoverJobs;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        ensure_project_directories(project_dir, &["jobs"], operation)?;
        let mut result = JobRecoveryResultData {
            api_version: JOB_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id.clone(),
            recovered_job_ids: Vec::new(),
            finalized_job_ids: Vec::new(),
            skipped_live_job_ids: Vec::new(),
            reindexed_jobs: 0,
            index_warnings: 0,
        };
        for job_id in scan_job_ids(project_dir, operation)? {
            let (job, events) = load_job(project_dir, &job_id, operation)?;
            if required_string(&job, "projectId", operation)? != descriptor.project_id {
                if !events.is_empty() {
                    let snapshot = self.snapshot_and_index(&descriptor, job, events, operation)?;
                    if snapshot.index_synchronized {
                        result.reindexed_jobs += 1;
                    } else {
                        result.index_warnings += 1;
                    }
                }
                continue;
            }
            if events.is_empty() {
                self.prepare_and_queue_existing_job(&descriptor, &job, operation)?;
                result.recovered_job_ids.push(job_id.clone());
            }
            let (_, current_events) = load_job(project_dir, &job_id, operation)?;
            let projection = project_job(&job, &current_events, operation)?;
            if projection.finalization_event.is_some() {
                self.finalize_pending(&descriptor, job.clone(), projection, operation)?;
                result.finalized_job_ids.push(job_id.clone());
            } else if projection.cancellation_requested
                && (projection.status != JobStatusData::Running
                    || lease_is_expired(&projection, self.clock.now(), operation)?)
            {
                self.finalize_cancellation(&descriptor, job.clone(), projection, operation)?;
                result.finalized_job_ids.push(job_id.clone());
            } else if projection.status == JobStatusData::Running {
                if lease_is_expired(&projection, self.clock.now(), operation)? {
                    self.recover_expired_attempt(&descriptor, &job, projection, operation)?;
                    result.recovered_job_ids.push(job_id.clone());
                } else {
                    result.skipped_live_job_ids.push(job_id.clone());
                }
            } else if projection.status == JobStatusData::Retrying
                && projection.next_attempt_at.is_none()
            {
                self.schedule_retry_after_attempt_failure(
                    &descriptor,
                    &job,
                    projection,
                    operation,
                )?;
                result.recovered_job_ids.push(job_id.clone());
            }
            let (job, events) = load_job(project_dir, &job_id, operation)?;
            let snapshot = self.snapshot_and_index(&descriptor, job, events, operation)?;
            if snapshot.index_synchronized {
                result.reindexed_jobs += 1;
            } else {
                result.index_warnings += 1;
            }
        }
        Ok(result)
    }

    pub fn claim_next_job(
        &self,
        options: ClaimNextJobOptions,
    ) -> Result<Option<JobSnapshotData>, JobServiceError> {
        let operation = JobOperation::ClaimNextJob;
        validate_portable_component(&options.worker_id, "workerId", operation)?;
        validate_lease_duration(options.lease_duration_ms, operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let mut candidates = Vec::new();
        for job_id in scan_job_ids(project_dir, operation)? {
            let (job, events) = load_job(project_dir, &job_id, operation)?;
            if events.is_empty()
                || required_string(&job, "projectId", operation)? != descriptor.project_id
            {
                continue;
            }
            candidates.push((
                parse_timestamp(&required_string(&job, "createdAt", operation)?, operation)?,
                job_id,
            ));
        }
        candidates.sort();
        for (_, job_id) in candidates {
            let (job, events) = load_job(project_dir, &job_id, operation)?;
            let projection = project_job(&job, &events, operation)?;
            let now = self.monotonic_now(&projection, operation)?;
            let runnable = match projection.status {
                JobStatusData::Queued => true,
                JobStatusData::Retrying => projection
                    .next_attempt_at
                    .as_deref()
                    .is_some_and(|value| timestamp_is_due(value, now)),
                _ => false,
            };
            if !runnable || projection.cancellation_requested {
                continue;
            }
            let lease_id = format!("lease_{}", Uuid::new_v4().simple());
            let expires_at = now
                .checked_add(Duration::milliseconds(
                    i64::try_from(options.lease_duration_ms).map_err(|_| {
                        JobServiceError::new(
                            JobErrorCode::InvalidRequest,
                            operation,
                            "leaseDurationMs 超出可表示范围。",
                        )
                    })?,
                ))
                .ok_or_else(|| {
                    JobServiceError::new(
                        JobErrorCode::InvalidRequest,
                        operation,
                        "租约到期时间溢出。",
                    )
                })?;
            let mut event = self.base_event_object(&job, &projection, "started", operation)?;
            event.insert("status".to_owned(), Value::String("running".to_owned()));
            event.insert(
                "workerId".to_owned(),
                Value::String(options.worker_id.clone()),
            );
            event.insert("leaseId".to_owned(), Value::String(lease_id));
            event.insert(
                "leaseExpiresAt".to_owned(),
                Value::String(format_timestamp(expires_at, operation)?),
            );
            match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
                Ok(_) => {
                    let events = scan_job_events(project_dir, &job, operation)?;
                    return self
                        .snapshot_and_index(&descriptor, job, events, operation)
                        .map(Some);
                }
                Err(error) if error.code == JobErrorCode::EventConflict => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    pub fn renew_job_lease(
        &self,
        options: RenewJobLeaseOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::RenewLease;
        validate_lease_duration(options.lease_duration_ms, operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        let now = self.monotonic_now(&projection, operation)?;
        require_active_lease(
            &projection,
            &options.lease_id,
            now,
            operation,
            &options.job_id,
        )?;
        if projection.cancellation_requested || projection.finalization_event.is_some() {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "任务已收到取消请求或进入终态提交，不能继续延长 worker 租约。",
            ));
        }
        let current_expires_at = parse_timestamp(
            &projection
                .lease
                .as_ref()
                .expect("active lease is present")
                .expires_at,
            operation,
        )?;
        let lease_duration =
            Duration::milliseconds(i64::try_from(options.lease_duration_ms).map_err(|_| {
                JobServiceError::new(
                    JobErrorCode::InvalidRequest,
                    operation,
                    "leaseDurationMs 超出可表示范围。",
                )
            })?);
        let requested_expires_at = now.checked_add(lease_duration).ok_or_else(|| {
            JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                "租约到期时间溢出。",
            )
        })?;
        let minimally_extended = current_expires_at
            .checked_add(Duration::nanoseconds(1))
            .ok_or_else(|| {
                JobServiceError::new(
                    JobErrorCode::InvalidRequest,
                    operation,
                    "租约到期时间溢出。",
                )
            })?;
        let expires_at = requested_expires_at.max(minimally_extended);
        let mut event = self.base_event_object(&job, &projection, "heartbeat", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert("leaseId".to_owned(), Value::String(options.lease_id));
        event.insert(
            "leaseExpiresAt".to_owned(),
            Value::String(format_timestamp(expires_at, operation)?),
        );
        append_job_event(project_dir, &job, &Value::Object(event), operation, false)?;
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn report_job_progress(
        &self,
        options: ReportJobProgressOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::ReportProgress;
        if !options.progress.is_finite() || !(0.0..=1.0).contains(&options.progress) {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                "progress 必须是 0..=1 的有限数值。",
            ));
        }
        if let Some(message) = &options.message {
            validate_text(message, "进度消息", 4096, operation)?;
        }
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        require_worker_update_allowed(
            &projection,
            &options.lease_id,
            self.clock.now(),
            operation,
            &options.job_id,
        )?;
        if options.progress < projection.progress {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidTransition,
                operation,
                "同一次 attempt 的 progress 不能倒退。",
            )
            .for_job(&options.job_id));
        }
        let mut event = self.base_event_object(&job, &projection, "progress", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert("leaseId".to_owned(), Value::String(options.lease_id));
        event.insert("progress".to_owned(), json!(options.progress));
        if let Some(message) = options.message {
            event.insert("message".to_owned(), Value::String(message));
        }
        append_job_event(project_dir, &job, &Value::Object(event), operation, false)?;
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn record_job_artifact(
        &self,
        options: RecordJobArtifactOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::RecordArtifact;
        validate_portable_component(&options.artifact_id, "artifactId", operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        require_worker_update_allowed(
            &projection,
            &options.lease_id,
            self.clock.now(),
            operation,
            &options.job_id,
        )?;
        self.workflow_service
            .validate_stage_run_artifacts(
                &descriptor.project_path,
                &descriptor.project_id,
                &required_string(&job, "stageId", operation)?,
                &required_string(&job, "stageRunId", operation)?,
                std::slice::from_ref(&options.artifact_id),
            )
            .map_err(|error| workflow_error_to_job(error, operation))?;
        if projection.artifact_ids.contains(&options.artifact_id) {
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }
        let mut event = self.base_event_object(&job, &projection, "artifact_created", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert(
            "leaseId".to_owned(),
            Value::String(options.lease_id.clone()),
        );
        event.insert(
            "artifactId".to_owned(),
            Value::String(options.artifact_id.clone()),
        );
        match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
            Ok(_) => {}
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let current_events = scan_job_events(project_dir, &job, operation)?;
                if project_job(&job, &current_events, operation)?
                    .artifact_ids
                    .contains(&options.artifact_id)
                {
                    return self.snapshot_and_index(&descriptor, job, current_events, operation);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(&descriptor, job, events, operation)
    }

    pub fn complete_job(
        &self,
        options: CompleteJobOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::CompleteJob;
        if options.artifact_ids.is_empty() || options.artifact_ids.len() > 256 {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                "成功任务必须提交 1..=256 个 Artifact。",
            ));
        }
        if options.artifact_ids.iter().collect::<BTreeSet<_>>().len() != options.artifact_ids.len()
        {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidRequest,
                operation,
                "artifactIds 不能重复。",
            ));
        }
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        if events.iter().any(|event| {
            completion_request_matches(
                event,
                &options.lease_id,
                &options.artifact_ids,
                &options.log_summary,
            )
        }) {
            if projection.finalization_event.is_some() {
                return self.finalize_pending(&descriptor, job, projection, operation);
            }
            if projection.status == JobStatusData::Succeeded {
                return self.snapshot_and_index(&descriptor, job, events, operation);
            }
        }
        if projection.status.is_terminal() || projection.finalization_event.is_some() {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "任务已经终结或正在提交另一种终态，complete_job 载荷不匹配。",
            ));
        }
        require_worker_update_allowed(
            &projection,
            &options.lease_id,
            self.clock.now(),
            operation,
            &options.job_id,
        )?;
        let submitted_artifacts = options.artifact_ids.iter().collect::<BTreeSet<_>>();
        if !submitted_artifacts
            .iter()
            .all(|artifact_id| projection.artifact_ids.contains(*artifact_id))
        {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "成功终态中的每个 Artifact 必须先写入 artifact_created 事件。",
            ));
        }
        let mut event =
            self.base_event_object(&job, &projection, "completion_requested", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert(
            "leaseId".to_owned(),
            Value::String(options.lease_id.clone()),
        );
        event.insert(
            "artifactIds".to_owned(),
            json!(options.artifact_ids.clone()),
        );
        event.insert("logSummary".to_owned(), options.log_summary.clone());
        let event = Value::Object(event);
        validate_persistent_document(&event, operation, "completion_requested JobEvent")?;
        self.workflow_service
            .validate_stage_run_artifacts(
                &descriptor.project_path,
                &descriptor.project_id,
                &required_string(&job, "stageId", operation)?,
                &required_string(&job, "stageRunId", operation)?,
                &options.artifact_ids,
            )
            .map_err(|error| workflow_error_to_job(error, operation))?;
        match append_job_event(project_dir, &job, &event, operation, false) {
            Ok(_) => {}
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let current_events = scan_job_events(project_dir, &job, operation)?;
                let current = project_job(&job, &current_events, operation)?;
                if current_events.iter().any(|event| {
                    completion_request_matches(
                        event,
                        &options.lease_id,
                        &options.artifact_ids,
                        &options.log_summary,
                    )
                }) {
                    if current.finalization_event.is_some() {
                        return self.finalize_pending(&descriptor, job, current, operation);
                    }
                    if current.status == JobStatusData::Succeeded {
                        return self.snapshot_and_index(
                            &descriptor,
                            job,
                            current_events,
                            operation,
                        );
                    }
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        let projection = project_job(
            &job,
            &scan_job_events(project_dir, &job, operation)?,
            operation,
        )?;
        self.finalize_pending(&descriptor, job, projection, operation)
    }

    pub fn fail_job(&self, options: FailJobOptions) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::FailJob;
        validate_failure(&options.error, operation)?;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        let error_value = serde_json::to_value(&options.error).map_err(|error| {
            JobServiceError::new(
                JobErrorCode::InternalContractError,
                operation,
                format!("序列化任务失败信息失败：{error}"),
            )
        })?;
        if events.iter().any(|event| {
            attempt_failure_matches(event, &options.lease_id, &error_value, &options.log_summary)
        }) {
            if projection.status == JobStatusData::Retrying
                && projection.next_attempt_at.is_none()
                && !projection.cancellation_requested
            {
                self.schedule_retry_after_attempt_failure(
                    &descriptor,
                    &job,
                    projection,
                    operation,
                )?;
                let events = scan_job_events(project_dir, &job, operation)?;
                return self.snapshot_and_index(&descriptor, job, events, operation);
            }
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }
        if events.iter().any(|event| {
            failure_request_matches(event, &options.lease_id, &error_value, &options.log_summary)
        }) {
            if projection.finalization_event.is_some() {
                return self.finalize_pending(&descriptor, job, projection, operation);
            }
            if projection.status == JobStatusData::Failed {
                return self.snapshot_and_index(&descriptor, job, events, operation);
            }
        }
        if projection.status == JobStatusData::Canceled
            && events.iter().any(|event| {
                event.get("eventType").and_then(Value::as_str) == Some("cancel_requested")
                    && event.get("leaseId").and_then(Value::as_str)
                        == Some(options.lease_id.as_str())
            })
        {
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }
        if projection.status.is_terminal() || projection.finalization_event.is_some() {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "任务已经终结或正在提交另一种终态，fail_job 载荷不匹配。",
            ));
        }
        require_active_lease(
            &projection,
            &options.lease_id,
            self.clock.now(),
            operation,
            &options.job_id,
        )?;
        if projection.cancellation_requested {
            return self.finalize_cancellation(&descriptor, job, projection, operation);
        }
        let retry_policy = retry_policy_from_job(&job, operation)?;
        if options.error.retryable && projection.attempt < retry_policy.max_attempts {
            let mut event =
                self.base_event_object(&job, &projection, "attempt_failed", operation)?;
            event.insert("status".to_owned(), Value::String("retrying".to_owned()));
            event.insert(
                "leaseId".to_owned(),
                Value::String(options.lease_id.clone()),
            );
            event.insert("error".to_owned(), error_value.clone());
            event.insert("logSummary".to_owned(), options.log_summary.clone());
            match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
                Ok(_) => {}
                Err(error) if error.code == JobErrorCode::EventConflict => {
                    return self.fail_job(options);
                }
                Err(error) => return Err(error),
            }
            let projection = project_job(
                &job,
                &scan_job_events(project_dir, &job, operation)?,
                operation,
            )?;
            self.schedule_retry_after_attempt_failure(&descriptor, &job, projection, operation)?;
            let events = scan_job_events(project_dir, &job, operation)?;
            return self.snapshot_and_index(&descriptor, job, events, operation);
        }
        let mut event =
            self.base_event_object(&job, &projection, "failure_requested", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert(
            "leaseId".to_owned(),
            Value::String(options.lease_id.clone()),
        );
        event.insert("error".to_owned(), error_value);
        event.insert("logSummary".to_owned(), options.log_summary.clone());
        match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
            Ok(_) => {}
            Err(error) if error.code == JobErrorCode::EventConflict => {
                return self.fail_job(options);
            }
            Err(error) => return Err(error),
        }
        let projection = project_job(
            &job,
            &scan_job_events(project_dir, &job, operation)?,
            operation,
        )?;
        self.finalize_pending(&descriptor, job, projection, operation)
    }

    pub fn acknowledge_cancellation(
        &self,
        options: AcknowledgeCancellationOptions,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let operation = JobOperation::AcknowledgeCancellation;
        let descriptor = self.open_project(&options.project_path, operation)?;
        require_project_identity(&descriptor, &options.expected_project_id, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let (job, events) = load_job(project_dir, &options.job_id, operation)?;
        require_current_project_job(&descriptor, &job, operation)?;
        let projection = project_job(&job, &events, operation)?;
        require_active_lease(
            &projection,
            &options.lease_id,
            self.clock.now(),
            operation,
            &options.job_id,
        )?;
        if !projection.cancellation_requested {
            return Err(invalid_transition(
                operation,
                &options.job_id,
                "任务尚未收到取消请求。",
            ));
        }
        self.finalize_cancellation(&descriptor, job, projection, operation)
    }

    fn base_event_object(
        &self,
        job: &Value,
        projection: &JobProjection,
        event_type: &str,
        operation: JobOperation,
    ) -> Result<Map<String, Value>, JobServiceError> {
        let sequence = projection.last_sequence.checked_add(1).ok_or_else(|| {
            JobServiceError::new(
                JobErrorCode::ScanLimitExceeded,
                operation,
                "JobEvent sequence 溢出。",
            )
        })?;
        if sequence as usize >= MAX_EVENTS_PER_JOB {
            return Err(JobServiceError::new(
                JobErrorCode::ScanLimitExceeded,
                operation,
                format!("单任务事件已达到上限 {MAX_EVENTS_PER_JOB}。"),
            )
            .for_job(required_string(job, "jobId", operation)?));
        }
        let now = self.clock.now();
        let last_timestamp = parse_timestamp(&projection.updated_at, operation)?;
        let created_at = if now < last_timestamp {
            last_timestamp
        } else {
            now
        };
        Ok(Map::from_iter([
            (
                "schemaVersion".to_owned(),
                Value::String(NARRACUT_CONTRACT_VERSION.to_owned()),
            ),
            (
                "documentType".to_owned(),
                Value::String("job_event".to_owned()),
            ),
            (
                "eventId".to_owned(),
                Value::String(format!("event_{}", Uuid::new_v4().simple())),
            ),
            (
                "jobId".to_owned(),
                Value::String(required_string(job, "jobId", operation)?),
            ),
            (
                "stageRunId".to_owned(),
                Value::String(required_string(job, "stageRunId", operation)?),
            ),
            ("sequence".to_owned(), json!(sequence)),
            ("eventType".to_owned(), Value::String(event_type.to_owned())),
            ("attempt".to_owned(), json!(projection.attempt)),
            (
                "createdAt".to_owned(),
                Value::String(format_timestamp(created_at, operation)?),
            ),
        ]))
    }

    fn open_project(
        &self,
        project_path: impl AsRef<Path>,
        operation: JobOperation,
    ) -> Result<ProjectDescriptorData, JobServiceError> {
        self.project_service
            .open_project(project_path)
            .map_err(|error| project_error_to_job(error, operation))
    }

    fn snapshot_and_index(
        &self,
        descriptor: &ProjectDescriptorData,
        job: Value,
        events: Vec<Value>,
        operation: JobOperation,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let projection = project_job(&job, &events, operation)?;
        let mut snapshot = projection.snapshot(descriptor, job, false, operation)?;
        snapshot.index_synchronized = self
            .storage_service
            .upsert_job_summary(
                descriptor,
                IndexedJobUpsertData {
                    job_id: required_string(&snapshot.job, "jobId", operation)?,
                    stage_run_id: required_string(&snapshot.job, "stageRunId", operation)?,
                    stage_id: required_string(&snapshot.job, "stageId", operation)?,
                    status: indexed_status(snapshot.status),
                    attempt: snapshot.attempt,
                    progress: snapshot.progress,
                    message: snapshot.message.clone(),
                    created_at: snapshot.created_at.clone(),
                    updated_at: snapshot.updated_at.clone(),
                },
            )
            .is_ok();
        Ok(snapshot)
    }

    fn prepare_and_queue_existing_job(
        &self,
        descriptor: &ProjectDescriptorData,
        job: &Value,
        operation: JobOperation,
    ) -> Result<(), JobServiceError> {
        let preparation = self
            .workflow_service
            .prepare_stage_run(PrepareStageRunOptions {
                project_path: descriptor.project_path.clone(),
                expected_project_id: descriptor.project_id.clone(),
                stage_id: required_string(job, "stageId", operation)?,
                run_id: required_string(job, "stageRunId", operation)?,
                job_id: required_string(job, "jobId", operation)?,
                input_refs: required_array(job, "inputRefs", operation)?,
                executor: job
                    .get("executor")
                    .cloned()
                    .ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 executor。"))?,
            });
        let job_id = required_string(job, "jobId", operation)?;
        let stage_run_id = required_string(job, "stageRunId", operation)?;
        let created_at = required_string(job, "createdAt", operation)?;
        let event = match preparation {
            Ok(_) => json!({
                "schemaVersion": NARRACUT_CONTRACT_VERSION,
                "documentType": "job_event",
                "eventId": deterministic_event_id(&job_id, 0),
                "jobId": job_id,
                "stageRunId": stage_run_id,
                "sequence": 0,
                "eventType": "queued",
                "status": "queued",
                "attempt": 1,
                "createdAt": created_at,
            }),
            Err(error) => {
                let error = workflow_error_to_job(error, operation)
                    .for_job(&job_id)
                    .for_stage(required_string(job, "stageId", operation)?)
                    .for_run(&stage_run_id);
                if preparation_error_is_deferred(error.code) {
                    return Err(error);
                }
                json!({
                    "schemaVersion": NARRACUT_CONTRACT_VERSION,
                    "documentType": "job_event",
                    "eventId": deterministic_event_id(&job_id, 0),
                    "jobId": job_id,
                    "stageRunId": stage_run_id,
                    "sequence": 0,
                    "eventType": "preparation_failed",
                    "status": "failed",
                    "attempt": 1,
                    "error": service_error_failure(&error),
                    "createdAt": created_at,
                })
            }
        };
        match append_job_event(
            Path::new(&descriptor.project_path),
            job,
            &event,
            operation,
            true,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let events = scan_job_events(Path::new(&descriptor.project_path), job, operation)?;
                if events.is_empty() {
                    Err(error)
                } else {
                    project_job(job, &events, operation)?;
                    Ok(())
                }
            }
            Err(error) => Err(error),
        }
    }

    fn finalize_pending(
        &self,
        descriptor: &ProjectDescriptorData,
        job: Value,
        projection: JobProjection,
        operation: JobOperation,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let pending = projection.finalization_event.clone().ok_or_else(|| {
            invalid_transition(
                operation,
                &required_string(&job, "jobId", operation).unwrap_or_default(),
                "任务没有待完成的终态提交。",
            )
        })?;
        let event_type = required_string(&pending, "eventType", operation)?;
        let terminal_status = match event_type.as_str() {
            "completion_requested" => TerminalRunStatusData::Succeeded,
            "failure_requested" => TerminalRunStatusData::Failed,
            _ => {
                return Err(invalid_job(
                    operation,
                    "终态请求事件类型不是 completion_requested 或 failure_requested。",
                ));
            }
        };
        let artifact_ids = if terminal_status == TerminalRunStatusData::Succeeded {
            required_string_array(&pending, "artifactIds", operation)?
        } else {
            Vec::new()
        };
        let log_summary = pending
            .get("logSummary")
            .cloned()
            .ok_or_else(|| invalid_job(operation, "终态请求缺少 logSummary。"))?;
        self.workflow_service
            .record_stage_run(RecordStageRunOptions {
                project_path: descriptor.project_path.clone(),
                expected_project_id: descriptor.project_id.clone(),
                stage_id: required_string(&job, "stageId", operation)?,
                run_id: required_string(&job, "stageRunId", operation)?,
                status: terminal_status,
                job_id: required_string(&job, "jobId", operation)?,
                artifact_ids,
                log_summary,
            })
            .map_err(|error| workflow_error_to_job(error, operation))?;

        let mut event = self.base_event_object(
            &job,
            &projection,
            if terminal_status == TerminalRunStatusData::Succeeded {
                "completed"
            } else {
                "failed"
            },
            operation,
        )?;
        let expected_status = if terminal_status == TerminalRunStatusData::Succeeded {
            JobStatusData::Succeeded
        } else {
            JobStatusData::Failed
        };
        event.insert(
            "status".to_owned(),
            Value::String(expected_status.as_str().to_owned()),
        );
        if let Some(lease_id) = pending.get("leaseId").and_then(Value::as_str) {
            event.insert("leaseId".to_owned(), Value::String(lease_id.to_owned()));
        }
        if terminal_status == TerminalRunStatusData::Succeeded {
            event.insert("progress".to_owned(), json!(1));
        } else {
            event.insert(
                "error".to_owned(),
                pending
                    .get("error")
                    .cloned()
                    .ok_or_else(|| invalid_job(operation, "失败请求缺少 error。"))?,
            );
        }
        let project_dir = Path::new(&descriptor.project_path);
        match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
            Ok(_) => {}
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let events = scan_job_events(project_dir, &job, operation)?;
                let current = project_job(&job, &events, operation)?;
                if current.status != expected_status {
                    return Err(error);
                }
                return self.snapshot_and_index(descriptor, job, events, operation);
            }
            Err(error) => return Err(error),
        }
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(descriptor, job, events, operation)
    }

    fn finalize_cancellation(
        &self,
        descriptor: &ProjectDescriptorData,
        job: Value,
        projection: JobProjection,
        operation: JobOperation,
    ) -> Result<JobSnapshotData, JobServiceError> {
        if projection.status == JobStatusData::Canceled {
            let events = scan_job_events(Path::new(&descriptor.project_path), &job, operation)?;
            return self.snapshot_and_index(descriptor, job, events, operation);
        }
        if !projection.cancellation_requested {
            return Err(invalid_transition(
                operation,
                &required_string(&job, "jobId", operation)?,
                "任务没有待处理的取消请求。",
            ));
        }
        let message = projection
            .message
            .clone()
            .unwrap_or_else(|| "任务已取消。".to_owned());
        let log_summary = json!({
            "message": message,
            "warnings": [],
            "errors": [],
        });
        self.workflow_service
            .record_stage_run(RecordStageRunOptions {
                project_path: descriptor.project_path.clone(),
                expected_project_id: descriptor.project_id.clone(),
                stage_id: required_string(&job, "stageId", operation)?,
                run_id: required_string(&job, "stageRunId", operation)?,
                status: TerminalRunStatusData::Canceled,
                job_id: required_string(&job, "jobId", operation)?,
                artifact_ids: Vec::new(),
                log_summary,
            })
            .map_err(|error| workflow_error_to_job(error, operation))?;
        let mut event = self.base_event_object(&job, &projection, "canceled", operation)?;
        event.insert("status".to_owned(), Value::String("canceled".to_owned()));
        event.insert("message".to_owned(), Value::String(message));
        if let Some(lease) = &projection.lease {
            event.insert("leaseId".to_owned(), Value::String(lease.lease_id.clone()));
        }
        let project_dir = Path::new(&descriptor.project_path);
        match append_job_event(project_dir, &job, &Value::Object(event), operation, false) {
            Ok(_) => {}
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let events = scan_job_events(project_dir, &job, operation)?;
                if project_job(&job, &events, operation)?.status != JobStatusData::Canceled {
                    return Err(error);
                }
                return self.snapshot_and_index(descriptor, job, events, operation);
            }
            Err(error) => return Err(error),
        }
        let events = scan_job_events(project_dir, &job, operation)?;
        self.snapshot_and_index(descriptor, job, events, operation)
    }

    fn recover_expired_attempt(
        &self,
        descriptor: &ProjectDescriptorData,
        job: &Value,
        projection: JobProjection,
        operation: JobOperation,
    ) -> Result<(), JobServiceError> {
        let error = JobFailureData {
            code: "worker_interrupted".to_owned(),
            message: "任务租约已过期，执行进程可能异常退出。".to_owned(),
            retryable: true,
            details: Map::new(),
        };
        let retry_policy = retry_policy_from_job(job, operation)?;
        let project_dir = Path::new(&descriptor.project_path);
        let lease_id = projection
            .lease
            .as_ref()
            .expect("running projection has lease")
            .lease_id
            .clone();
        let error_value =
            serde_json::to_value(&error).expect("JobFailureData serialization is infallible");
        let log_summary = json!({
            "message": error.message,
            "warnings": [],
            "errors": ["worker_interrupted"],
        });
        if projection.attempt < retry_policy.max_attempts {
            let mut event =
                self.base_event_object(job, &projection, "attempt_failed", operation)?;
            event.insert("status".to_owned(), Value::String("retrying".to_owned()));
            event.insert("leaseId".to_owned(), Value::String(lease_id.clone()));
            event.insert("error".to_owned(), error_value.clone());
            event.insert("logSummary".to_owned(), log_summary.clone());
            match append_job_event(project_dir, job, &Value::Object(event), operation, false) {
                Ok(_) => {}
                Err(conflict) if conflict.code == JobErrorCode::EventConflict => {
                    let events = scan_job_events(project_dir, job, operation)?;
                    let current = project_job(job, &events, operation)?;
                    if current.cancellation_requested {
                        self.finalize_cancellation(descriptor, job.clone(), current, operation)?;
                        return Ok(());
                    }
                    if current.status == JobStatusData::Running
                        && !lease_is_expired(&current, self.clock.now(), operation)?
                    {
                        return Ok(());
                    }
                    if !events.iter().any(|event| {
                        attempt_failure_matches(event, &lease_id, &error_value, &log_summary)
                    }) {
                        return Err(conflict);
                    }
                }
                Err(error) => return Err(error),
            }
            let projection = project_job(
                job,
                &scan_job_events(project_dir, job, operation)?,
                operation,
            )?;
            self.schedule_retry_after_attempt_failure(descriptor, job, projection, operation)?;
            return Ok(());
        }
        let mut event = self.base_event_object(job, &projection, "failure_requested", operation)?;
        event.insert("status".to_owned(), Value::String("running".to_owned()));
        event.insert("leaseId".to_owned(), Value::String(lease_id.clone()));
        event.insert("error".to_owned(), error_value.clone());
        event.insert("logSummary".to_owned(), log_summary.clone());
        match append_job_event(project_dir, job, &Value::Object(event), operation, false) {
            Ok(_) => {}
            Err(conflict) if conflict.code == JobErrorCode::EventConflict => {
                let events = scan_job_events(project_dir, job, operation)?;
                let current = project_job(job, &events, operation)?;
                if current.cancellation_requested {
                    self.finalize_cancellation(descriptor, job.clone(), current, operation)?;
                    return Ok(());
                }
                if current.status == JobStatusData::Running
                    && !lease_is_expired(&current, self.clock.now(), operation)?
                {
                    return Ok(());
                }
                if !events.iter().any(|event| {
                    failure_request_matches(event, &lease_id, &error_value, &log_summary)
                }) && current.status != JobStatusData::Failed
                {
                    return Err(conflict);
                }
            }
            Err(error) => return Err(error),
        }
        let projection = project_job(
            job,
            &scan_job_events(project_dir, job, operation)?,
            operation,
        )?;
        if projection.status == JobStatusData::Failed {
            return Ok(());
        }
        self.finalize_pending(descriptor, job.clone(), projection, operation)?;
        Ok(())
    }

    fn schedule_retry_after_attempt_failure(
        &self,
        descriptor: &ProjectDescriptorData,
        job: &Value,
        projection: JobProjection,
        operation: JobOperation,
    ) -> Result<(), JobServiceError> {
        if projection.status == JobStatusData::Retrying
            && projection.next_attempt_at.is_some()
            && !projection.cancellation_requested
        {
            return Ok(());
        }
        if projection.status != JobStatusData::Retrying || projection.cancellation_requested {
            return Err(invalid_transition(
                operation,
                &required_string(job, "jobId", operation)?,
                "只有尚未安排退避时间的 attempt_failed 状态可以进入下一次重试。",
            ));
        }
        let retry_policy = retry_policy_from_job(job, operation)?;
        if projection.attempt >= retry_policy.max_attempts {
            return Err(invalid_job(
                operation,
                "attempt_failed 已达到最大尝试次数，不能继续安排重试。",
            ));
        }
        let next_at = self
            .monotonic_now(&projection, operation)?
            .checked_add(Duration::milliseconds(
                i64::try_from(backoff_ms(&retry_policy, projection.attempt))
                    .map_err(|_| invalid_job(operation, "重试退避超出可表示范围。"))?,
            ))
            .ok_or_else(|| invalid_job(operation, "重试时间溢出。"))?;
        let mut event = self.base_event_object(job, &projection, "retrying", operation)?;
        event.insert("status".to_owned(), Value::String("retrying".to_owned()));
        event.insert("attempt".to_owned(), json!(projection.attempt + 1));
        event.insert(
            "error".to_owned(),
            serde_json::to_value(
                projection
                    .last_error
                    .as_ref()
                    .expect("attempt_failed projection has last_error"),
            )
            .expect("error serializes"),
        );
        event.insert(
            "nextAttemptAt".to_owned(),
            Value::String(format_timestamp(next_at, operation)?),
        );
        match append_job_event(
            Path::new(&descriptor.project_path),
            job,
            &Value::Object(event),
            operation,
            false,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.code == JobErrorCode::EventConflict => {
                let events = scan_job_events(Path::new(&descriptor.project_path), job, operation)?;
                let current = project_job(job, &events, operation)?;
                if current.status == JobStatusData::Retrying
                    && current.attempt == projection.attempt + 1
                    && current.next_attempt_at.is_some()
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn monotonic_now(
        &self,
        projection: &JobProjection,
        operation: JobOperation,
    ) -> Result<OffsetDateTime, JobServiceError> {
        let now = self.clock.now();
        let updated_at = parse_timestamp(&projection.updated_at, operation)?;
        Ok(if now < updated_at { updated_at } else { now })
    }
}

#[derive(Debug, Clone)]
struct JobProjection {
    status: JobStatusData,
    attempt: u32,
    progress: f64,
    message: Option<String>,
    cancellation_requested: bool,
    finalization_event: Option<Value>,
    artifact_ids: BTreeSet<String>,
    last_error: Option<JobFailureData>,
    next_attempt_at: Option<String>,
    lease: Option<JobLeaseData>,
    last_sequence: u32,
    created_at: String,
    updated_at: String,
}

impl JobProjection {
    fn snapshot(
        self,
        descriptor: &ProjectDescriptorData,
        job: Value,
        index_synchronized: bool,
        operation: JobOperation,
    ) -> Result<JobSnapshotData, JobServiceError> {
        let job_id = required_string(&job, "jobId", operation)?;
        let historical = required_string(&job, "projectId", operation)? != descriptor.project_id;
        Ok(JobSnapshotData {
            api_version: JOB_COMMAND_API_VERSION.to_owned(),
            owner_project_id: descriptor.project_id.clone(),
            job,
            job_uri: format!("jobs/{job_id}/job.json"),
            status: self.status,
            attempt: self.attempt,
            progress: self.progress,
            message: self.message,
            cancellation_requested: self.cancellation_requested,
            finalization_pending: self.finalization_event.is_some(),
            artifact_ids: self.artifact_ids.into_iter().collect(),
            last_error: self.last_error,
            next_attempt_at: self.next_attempt_at,
            lease: self.lease,
            last_sequence: self.last_sequence,
            created_at: self.created_at,
            updated_at: self.updated_at,
            historical,
            index_synchronized,
        })
    }
}

fn project_job(
    job: &Value,
    events: &[Value],
    operation: JobOperation,
) -> Result<JobProjection, JobServiceError> {
    if events.is_empty() {
        return Err(invalid_job(
            operation,
            "JobDefinition 尚未产生 queued 事件。",
        ));
    }
    let job_id = required_string(job, "jobId", operation)?;
    let run_id = required_string(job, "stageRunId", operation)?;
    let first = &events[0];
    let first_event_type = required_string(first, "eventType", operation)?;
    if !matches!(first_event_type.as_str(), "queued" | "preparation_failed")
        || required_u32(first, "sequence", operation)? != 0
        || required_u32(first, "attempt", operation)? != 1
    {
        return Err(invalid_job(
            operation,
            "任务事件流必须从 sequence=0、attempt=1 的 queued 或 preparation_failed 事件开始。",
        )
        .for_job(&job_id));
    }
    let created_at = required_string(job, "createdAt", operation)?;
    let created_timestamp = parse_timestamp(&created_at, operation)?;
    let first_created_at = required_string(first, "createdAt", operation)?;
    if first_created_at != created_at {
        return Err(invalid_job(
            operation,
            "queued 事件的 createdAt 必须与 JobDefinition.createdAt 一致。",
        )
        .for_job(&job_id));
    }
    let mut latest_timestamp = created_timestamp;
    let mut event_ids = HashSet::new();
    let preparation_failure = if first_event_type == "preparation_failed" {
        Some(parse_failure(first, operation)?)
    } else {
        None
    };
    let mut projection = JobProjection {
        status: if preparation_failure.is_some() {
            JobStatusData::Failed
        } else {
            JobStatusData::Queued
        },
        attempt: 1,
        progress: 0.0,
        message: preparation_failure
            .as_ref()
            .map(|failure| failure.message.clone()),
        cancellation_requested: false,
        finalization_event: None,
        artifact_ids: BTreeSet::new(),
        last_error: preparation_failure,
        next_attempt_at: None,
        lease: None,
        last_sequence: 0,
        created_at,
        updated_at: first_created_at,
    };

    for (index, event) in events.iter().enumerate() {
        validate_persistent_document(event, operation, "JobEvent")?;
        let event_id = required_string(event, "eventId", operation)?;
        if !event_ids.insert(event_id) {
            return Err(invalid_job(operation, "JobEvent.eventId 不能重复。").for_job(&job_id));
        }
        let sequence = required_u32(event, "sequence", operation)?;
        if usize::try_from(sequence).ok() != Some(index)
            || event.get("jobId").and_then(Value::as_str) != Some(job_id.as_str())
            || event.get("stageRunId").and_then(Value::as_str) != Some(run_id.as_str())
        {
            return Err(invalid_job(
                operation,
                "JobEvent 路径、sequence、jobId 或 stageRunId 不一致。",
            )
            .for_job(&job_id));
        }
        let event_created_at = required_string(event, "createdAt", operation)?;
        let event_timestamp = parse_timestamp(&event_created_at, operation)?;
        if event_timestamp < latest_timestamp {
            return Err(invalid_job(operation, "JobEvent.createdAt 不能倒退。").for_job(&job_id));
        }
        if index == 0 {
            continue;
        }
        let attempt = required_u32(event, "attempt", operation)?;
        let event_type = required_string(event, "eventType", operation)?;
        match event_type.as_str() {
            "started" => {
                if !matches!(
                    projection.status,
                    JobStatusData::Queued | JobStatusData::Retrying
                ) || projection.cancellation_requested
                    || attempt != projection.attempt
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "started 只能从可运行的 queued/retrying 状态进入。",
                    ));
                }
                projection.status = JobStatusData::Running;
                projection.progress = 0.0;
                projection.next_attempt_at = None;
                projection.lease = Some(JobLeaseData {
                    worker_id: required_string(event, "workerId", operation)?,
                    lease_id: required_string(event, "leaseId", operation)?,
                    expires_at: required_string(event, "leaseExpiresAt", operation)?,
                });
                if parse_timestamp(
                    &projection
                        .lease
                        .as_ref()
                        .expect("started creates lease")
                        .expires_at,
                    operation,
                )? <= event_timestamp
                {
                    return Err(invalid_job(
                        operation,
                        "started.leaseExpiresAt 必须晚于事件时间。",
                    )
                    .for_job(&job_id));
                }
            }
            "progress" => {
                require_running_event(&projection, event, attempt, operation, &job_id, true)?;
                let progress = required_f64(event, "progress", operation)?;
                if progress < projection.progress {
                    return Err(
                        invalid_job(operation, "JobEvent progress 发生倒退。").for_job(&job_id)
                    );
                }
                projection.progress = progress;
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    projection.message = Some(message.to_owned());
                }
            }
            "log" => {
                require_running_event(&projection, event, attempt, operation, &job_id, false)?;
                projection.message = Some(required_string(event, "message", operation)?);
            }
            "artifact_created" => {
                require_running_event(&projection, event, attempt, operation, &job_id, true)?;
                let artifact_id = required_string(event, "artifactId", operation)?;
                if !projection.artifact_ids.insert(artifact_id) {
                    return Err(
                        invalid_job(operation, "JobEvent 重复声明同一 Artifact。").for_job(&job_id)
                    );
                }
            }
            "heartbeat" => {
                require_running_event(&projection, event, attempt, operation, &job_id, true)?;
                let lease = projection
                    .lease
                    .as_mut()
                    .expect("running projection has a lease");
                let expires_at = required_string(event, "leaseExpiresAt", operation)?;
                if parse_timestamp(&expires_at, operation)? <= event_timestamp {
                    return Err(invalid_job(
                        operation,
                        "heartbeat.leaseExpiresAt 必须晚于事件时间。",
                    )
                    .for_job(&job_id));
                }
                lease.expires_at = expires_at;
            }
            "cancel_requested" => {
                if projection.status.is_terminal()
                    || projection.finalization_event.is_some()
                    || attempt != projection.attempt
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "终态或正在终态提交的任务不能追加 cancel_requested。",
                    ));
                }
                if projection.status == JobStatusData::Running {
                    require_event_lease(&projection, event, operation, &job_id)?;
                }
                projection.cancellation_requested = true;
                projection.message = Some(required_string(event, "message", operation)?);
            }
            "retrying" => {
                if projection.status != JobStatusData::Retrying
                    || projection.cancellation_requested
                    || projection.finalization_event.is_some()
                    || projection.next_attempt_at.is_some()
                    || attempt != projection.attempt + 1
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "retrying 必须紧接尚未安排退避的 attempt_failed，并进入下一个 attempt。",
                    ));
                }
                projection.status = JobStatusData::Retrying;
                projection.attempt = attempt;
                projection.progress = 0.0;
                projection.lease = None;
                let next_attempt_at = required_string(event, "nextAttemptAt", operation)?;
                if parse_timestamp(&next_attempt_at, operation)? < event_timestamp {
                    return Err(invalid_job(
                        operation,
                        "retrying.nextAttemptAt 不能早于事件时间。",
                    )
                    .for_job(&job_id));
                }
                projection.next_attempt_at = Some(next_attempt_at);
                projection.last_error = Some(parse_failure(event, operation)?);
            }
            "attempt_failed" => {
                if projection.status != JobStatusData::Running
                    || projection.cancellation_requested
                    || projection.finalization_event.is_some()
                    || attempt != projection.attempt
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "attempt_failed 只能结束当前 running attempt。",
                    ));
                }
                require_event_lease(&projection, event, operation, &job_id)?;
                projection.status = JobStatusData::Retrying;
                projection.lease = None;
                projection.last_error = Some(parse_failure(event, operation)?);
            }
            "completion_requested" | "failure_requested" => {
                require_running_event(&projection, event, attempt, operation, &job_id, true)?;
                projection.finalization_event = Some(event.clone());
                if event_type == "completion_requested" {
                    let artifact_ids = required_string_array(event, "artifactIds", operation)?;
                    if artifact_ids.iter().collect::<BTreeSet<_>>().len() != artifact_ids.len() {
                        return Err(invalid_job(
                            operation,
                            "completion_requested.artifactIds 不能重复。",
                        )
                        .for_job(&job_id));
                    }
                    projection.artifact_ids = artifact_ids.into_iter().collect();
                } else {
                    projection.last_error = Some(parse_failure(event, operation)?);
                }
            }
            "completed" => {
                if projection
                    .finalization_event
                    .as_ref()
                    .and_then(|value| value.get("eventType"))
                    .and_then(Value::as_str)
                    != Some("completion_requested")
                    || attempt != projection.attempt
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "completed 缺少同一 attempt 的 completion_requested。",
                    ));
                }
                projection.status = JobStatusData::Succeeded;
                projection.progress = 1.0;
                projection.lease = None;
                projection.finalization_event = None;
            }
            "failed" => {
                if projection
                    .finalization_event
                    .as_ref()
                    .and_then(|value| value.get("eventType"))
                    .and_then(Value::as_str)
                    != Some("failure_requested")
                    || attempt != projection.attempt
                {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "failed 缺少同一 attempt 的 failure_requested。",
                    ));
                }
                projection.status = JobStatusData::Failed;
                projection.lease = None;
                projection.finalization_event = None;
                projection.last_error = Some(parse_failure(event, operation)?);
            }
            "canceled" => {
                if !projection.cancellation_requested || attempt != projection.attempt {
                    return Err(invalid_transition(
                        operation,
                        &job_id,
                        "canceled 缺少同一 attempt 的 cancel_requested。",
                    ));
                }
                projection.status = JobStatusData::Canceled;
                projection.lease = None;
                projection.finalization_event = None;
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    projection.message = Some(message.to_owned());
                }
            }
            "queued" => {
                return Err(invalid_transition(
                    operation,
                    &job_id,
                    "queued 只能是任务的首个事件。",
                ));
            }
            "preparation_failed" => {
                return Err(invalid_transition(
                    operation,
                    &job_id,
                    "preparation_failed 只能是任务的首个事件。",
                ));
            }
            _ => return Err(invalid_job(operation, "JobEvent 包含未知 eventType。")),
        }
        projection.last_sequence = sequence;
        projection.updated_at = event_created_at;
        latest_timestamp = event_timestamp;
    }
    Ok(projection)
}

fn require_running_event(
    projection: &JobProjection,
    event: &Value,
    attempt: u32,
    operation: JobOperation,
    job_id: &str,
    require_open: bool,
) -> Result<(), JobServiceError> {
    if projection.status != JobStatusData::Running
        || attempt != projection.attempt
        || (require_open
            && (projection.cancellation_requested || projection.finalization_event.is_some()))
    {
        return Err(invalid_transition(
            operation,
            job_id,
            "worker 事件不符合当前 running attempt 或任务已开始收尾。",
        ));
    }
    require_event_lease(projection, event, operation, job_id)
}

fn require_event_lease(
    projection: &JobProjection,
    event: &Value,
    operation: JobOperation,
    job_id: &str,
) -> Result<(), JobServiceError> {
    let lease_id = event.get("leaseId").and_then(Value::as_str);
    if projection
        .lease
        .as_ref()
        .map(|lease| lease.lease_id.as_str())
        != lease_id
    {
        return Err(JobServiceError::new(
            JobErrorCode::LeaseConflict,
            operation,
            "JobEvent.leaseId 与当前 worker 租约不一致。",
        )
        .for_job(job_id));
    }
    Ok(())
}

fn load_job(
    project_dir: &Path,
    job_id: &str,
    operation: JobOperation,
) -> Result<(Value, Vec<Value>), JobServiceError> {
    validate_job_id(job_id, operation)?;
    let path = job_definition_path(project_dir, job_id);
    let job = read_json_file(project_dir, &path, operation, JobErrorCode::JobNotFound)?;
    validate_persistent_document(&job, operation, "JobDefinition")
        .map_err(|error| error.at_path(&path))?;
    validate_job_definition_hashes(&job, operation).map_err(|error| error.at_path(&path))?;
    if job.get("documentType").and_then(Value::as_str) != Some("job_definition")
        || job.get("jobId").and_then(Value::as_str) != Some(job_id)
    {
        return Err(
            invalid_job(operation, "JobDefinition 路径与文档身份不一致。")
                .at_path(&path)
                .for_job(job_id),
        );
    }
    let events = scan_job_events(project_dir, &job, operation)?;
    Ok((job, events))
}

fn scan_job_ids(
    project_dir: &Path,
    operation: JobOperation,
) -> Result<Vec<String>, JobServiceError> {
    let root = project_dir.join("jobs");
    let Some(metadata) = inspect_project_path(project_dir, &root, operation)? else {
        return Ok(Vec::new());
    };
    if !metadata.is_dir() {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "jobs 路径不是目录。",
        )
        .at_path(&root));
    }
    let mut job_ids = Vec::new();
    for entry in fs::read_dir(&root)
        .map_err(|error| JobServiceError::io(operation, &root, "读取 jobs 目录失败", &error))?
    {
        let entry = entry
            .map_err(|error| JobServiceError::io(operation, &root, "遍历 jobs 目录失败", &error))?;
        let path = entry.path();
        let metadata = inspect_project_path(project_dir, &path, operation)?.ok_or_else(|| {
            JobServiceError::new(JobErrorCode::IoError, operation, "job 目录项在扫描时消失。")
                .at_path(&path)
        })?;
        if !metadata.is_dir() {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "jobs 目录只能包含 job 目录。",
            )
            .at_path(&path));
        }
        let job_id = entry.file_name().into_string().map_err(|_| {
            JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "job 目录名必须是 Unicode。",
            )
            .at_path(&path)
        })?;
        validate_job_id(&job_id, operation).map_err(|error| error.at_path(&path))?;
        job_ids.push(job_id);
        if job_ids.len() > MAX_JOBS {
            return Err(JobServiceError::new(
                JobErrorCode::ScanLimitExceeded,
                operation,
                format!("项目任务数超过扫描上限 {MAX_JOBS}。"),
            )
            .at_path(&root));
        }
    }
    job_ids.sort();
    Ok(job_ids)
}

fn scan_job_events(
    project_dir: &Path,
    job: &Value,
    operation: JobOperation,
) -> Result<Vec<Value>, JobServiceError> {
    let job_id = required_string(job, "jobId", operation)?;
    let run_id = required_string(job, "stageRunId", operation)?;
    let root = job_events_dir(project_dir, &job_id);
    let Some(metadata) = inspect_project_path(project_dir, &root, operation)? else {
        return Ok(Vec::new());
    };
    if !metadata.is_dir() {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "JobEvent 路径不是目录。",
        )
        .at_path(&root)
        .for_job(&job_id));
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(&root)
        .map_err(|error| JobServiceError::io(operation, &root, "读取 JobEvent 目录失败", &error))?
    {
        let path = entry
            .map_err(|error| {
                JobServiceError::io(operation, &root, "遍历 JobEvent 目录失败", &error)
            })?
            .path();
        let metadata = inspect_project_path(project_dir, &path, operation)?.ok_or_else(|| {
            JobServiceError::new(JobErrorCode::IoError, operation, "JobEvent 在扫描时消失。")
                .at_path(&path)
        })?;
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "JobEvent 目录只能包含序号 JSON 文件。",
            )
            .at_path(&path)
            .for_job(&job_id));
        }
        paths.push(path);
        if paths.len() > MAX_EVENTS_PER_JOB {
            return Err(JobServiceError::new(
                JobErrorCode::ScanLimitExceeded,
                operation,
                format!("单任务事件数超过扫描上限 {MAX_EVENTS_PER_JOB}。"),
            )
            .at_path(&root)
            .for_job(&job_id));
        }
    }
    paths.sort();
    let mut events = Vec::with_capacity(paths.len());
    for (index, path) in paths.into_iter().enumerate() {
        let expected_name = format!("{index:010}.json");
        if path.file_name().and_then(|value| value.to_str()) != Some(expected_name.as_str()) {
            return Err(invalid_job(
                operation,
                "JobEvent 文件必须从 0000000000.json 开始连续编号。",
            )
            .at_path(&path)
            .for_job(&job_id));
        }
        let event = read_json_file(project_dir, &path, operation, JobErrorCode::InvalidProject)?;
        validate_persistent_document(&event, operation, "JobEvent")
            .map_err(|error| error.at_path(&path))?;
        if event.get("documentType").and_then(Value::as_str) != Some("job_event")
            || event.get("jobId").and_then(Value::as_str) != Some(job_id.as_str())
            || event.get("stageRunId").and_then(Value::as_str) != Some(run_id.as_str())
            || event.get("sequence").and_then(Value::as_u64) != Some(index as u64)
        {
            return Err(invalid_job(operation, "JobEvent 路径与文档身份不一致。")
                .at_path(&path)
                .for_job(&job_id));
        }
        events.push(event);
    }
    Ok(events)
}

fn claim_job_definition(
    project_dir: &Path,
    path: &Path,
    candidate: &Value,
    operation: JobOperation,
) -> Result<Value, JobServiceError> {
    match write_immutable_json(project_dir, path, candidate, operation) {
        Ok(false) => Ok(candidate.clone()),
        Ok(true)
        | Err(JobServiceError {
            code: JobErrorCode::EventConflict,
            ..
        }) => {
            let existing = read_json_file(
                project_dir,
                path,
                operation,
                JobErrorCode::IdempotencyConflict,
            )?;
            validate_persistent_document(&existing, operation, "既有 JobDefinition")
                .map_err(|error| error.at_path(path))?;
            validate_job_definition_hashes(&existing, operation)
                .map_err(|error| error.at_path(path))?;
            let fields = [
                "documentType",
                "jobId",
                "projectId",
                "jobType",
                "stageId",
                "stageRunId",
                "executionSnapshotUri",
                "idempotencyHash",
                "requestHash",
                "inputRefs",
                "executor",
                "retryPolicy",
            ];
            if fields
                .iter()
                .any(|field| existing.get(*field) != candidate.get(*field))
            {
                return Err(JobServiceError::new(
                    JobErrorCode::IdempotencyConflict,
                    operation,
                    "相同 idempotencyKey 已绑定不同的阶段任务请求。",
                )
                .at_path(path)
                .for_job(required_string(candidate, "jobId", operation)?));
            }
            Ok(existing)
        }
        Err(error) => Err(error),
    }
}

fn append_job_event(
    project_dir: &Path,
    job: &Value,
    event: &Value,
    operation: JobOperation,
    allow_exact_replay: bool,
) -> Result<bool, JobServiceError> {
    validate_persistent_document(event, operation, "JobEvent")?;
    let job_id = required_string(job, "jobId", operation)?;
    if event.get("jobId").and_then(Value::as_str) != Some(job_id.as_str())
        || event.get("stageRunId").and_then(Value::as_str)
            != job.get("stageRunId").and_then(Value::as_str)
    {
        return Err(
            invalid_job(operation, "待写 JobEvent 与 JobDefinition 身份不一致。").for_job(job_id),
        );
    }
    let sequence = required_u32(event, "sequence", operation)?;
    if sequence as usize >= MAX_EVENTS_PER_JOB {
        return Err(JobServiceError::new(
            JobErrorCode::ScanLimitExceeded,
            operation,
            format!("单任务事件已达到上限 {MAX_EVENTS_PER_JOB}。"),
        )
        .for_job(job_id));
    }
    ensure_project_directories(project_dir, &["jobs", &job_id, "events"], operation)?;
    let path = job_event_path(project_dir, &job_id, sequence);
    match write_immutable_json(project_dir, &path, event, operation) {
        Ok(replay) if !replay || allow_exact_replay => Ok(replay),
        Ok(_) => Err(JobServiceError::new(
            JobErrorCode::EventConflict,
            operation,
            "JobEvent sequence 已被并发事件占用。",
        )
        .at_path(&path)
        .for_job(job_id)),
        Err(error) if error.code == JobErrorCode::EventConflict => {
            if allow_exact_replay {
                let existing =
                    read_json_file(project_dir, &path, operation, JobErrorCode::EventConflict)?;
                if existing == *event {
                    return Ok(true);
                }
            }
            Err(error.for_job(job_id))
        }
        Err(error) => Err(error.for_job(job_id)),
    }
}

fn write_immutable_json(
    project_dir: &Path,
    path: &Path,
    value: &Value,
    operation: JobOperation,
) -> Result<bool, JobServiceError> {
    if inspect_project_path(project_dir, path, operation)?.is_some() {
        let existing = read_json_file(project_dir, path, operation, JobErrorCode::EventConflict)?;
        if existing == *value {
            return Ok(true);
        }
        return Err(JobServiceError::new(
            JobErrorCode::EventConflict,
            operation,
            "不可变任务文档已存在且内容不同。",
        )
        .at_path(path));
    }
    let parent = path.parent().ok_or_else(|| {
        JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "不可变任务文档缺少父目录。",
        )
        .at_path(path)
    })?;
    let metadata = inspect_project_path(project_dir, parent, operation)?.ok_or_else(|| {
        JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "不可变任务文档父目录不存在。",
        )
        .at_path(parent)
    })?;
    if !metadata.is_dir() {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "不可变任务文档父路径不是目录。",
        )
        .at_path(parent));
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        JobServiceError::new(
            JobErrorCode::InternalContractError,
            operation,
            format!("序列化任务文档失败：{error}"),
        )
        .at_path(path)
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(JobServiceError::new(
            JobErrorCode::ScanLimitExceeded,
            operation,
            format!("任务文档超过 {MAX_DOCUMENT_BYTES} 字节。"),
        )
        .at_path(path));
    }
    ensure_project_directories(project_dir, &["cache", "job-writes"], operation)?;
    let temporary_dir = project_dir.join("cache/job-writes");
    let mut temporary = NamedTempFile::new_in(&temporary_dir).map_err(|error| {
        JobServiceError::io(operation, &temporary_dir, "创建任务临时文件失败", &error)
    })?;
    temporary.write_all(&bytes).map_err(|error| {
        JobServiceError::io(operation, temporary.path(), "写入任务临时文件失败", &error)
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        JobServiceError::io(operation, temporary.path(), "同步任务临时文件失败", &error)
    })?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(false),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing =
                read_json_file(project_dir, path, operation, JobErrorCode::EventConflict)?;
            if existing == *value {
                Ok(true)
            } else {
                Err(JobServiceError::new(
                    JobErrorCode::EventConflict,
                    operation,
                    "不可变任务文档并发创建冲突。",
                )
                .at_path(path))
            }
        }
        Err(error) => Err(JobServiceError::io(
            operation,
            path,
            "提交不可变任务文档失败",
            &error.error,
        )),
    }
}

fn read_json_file(
    project_dir: &Path,
    path: &Path,
    operation: JobOperation,
    missing_code: JobErrorCode,
) -> Result<Value, JobServiceError> {
    let metadata = inspect_project_path(project_dir, path, operation)?.ok_or_else(|| {
        JobServiceError::new(missing_code, operation, "所需任务文档不存在。").at_path(path)
    })?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "任务文档必须是大小受限的普通文件。",
        )
        .at_path(path));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        JobServiceError::new(
            JobErrorCode::ScanLimitExceeded,
            operation,
            "任务文档大小无法在当前平台表示。",
        )
        .at_path(path)
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)
        .map_err(|error| JobServiceError::io(operation, path, "打开任务文档失败", &error))?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| JobServiceError::io(operation, path, "读取任务文档失败", &error))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(JobServiceError::new(
            JobErrorCode::ScanLimitExceeded,
            operation,
            "任务文档在读取时超过大小上限。",
        )
        .at_path(path));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        JobServiceError::new(
            JobErrorCode::InvalidProject,
            operation,
            format!("任务文档不是有效 JSON：{error}"),
        )
        .at_path(path)
    })
}

fn ensure_project_directories(
    project_dir: &Path,
    components: &[&str],
    operation: JobOperation,
) -> Result<PathBuf, JobServiceError> {
    let mut current = project_dir.to_path_buf();
    for component in components {
        if !portable_component_is_valid(component) {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "任务目录组件不安全。",
            )
            .at_path(&current));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => require_safe_directory_metadata(&current, &metadata, operation)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(JobServiceError::io(
                            operation,
                            &current,
                            "创建任务目录失败",
                            &error,
                        ));
                    }
                }
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    JobServiceError::io(operation, &current, "检查任务目录失败", &error)
                })?;
                require_safe_directory_metadata(&current, &metadata, operation)?;
            }
            Err(error) => {
                return Err(JobServiceError::io(
                    operation,
                    &current,
                    "检查任务目录失败",
                    &error,
                ));
            }
        }
    }
    Ok(current)
}

fn inspect_project_path(
    project_dir: &Path,
    path: &Path,
    operation: JobOperation,
) -> Result<Option<fs::Metadata>, JobServiceError> {
    let relative = path.strip_prefix(project_dir).map_err(|_| {
        JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "任务路径逃逸出项目根目录。",
        )
        .at_path(path)
    })?;
    let root_metadata = fs::symlink_metadata(project_dir).map_err(|error| {
        JobServiceError::io(operation, project_dir, "读取项目根目录失败", &error)
    })?;
    require_safe_directory_metadata(project_dir, &root_metadata, operation)?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Ok(Some(root_metadata));
    }
    let mut current = project_dir.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "任务路径包含非法组件。",
            )
            .at_path(path));
        };
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(JobServiceError::io(
                    operation,
                    &current,
                    "逐级检查任务路径失败",
                    &error,
                ));
            }
        };
        if metadata_is_link(&metadata) {
            return Err(JobServiceError::new(
                JobErrorCode::PathContainsSymlink,
                operation,
                "任务路径不能经过符号链接或重解析点。",
            )
            .at_path(&current));
        }
        if index + 1 != components.len() && !metadata.is_dir() {
            return Err(JobServiceError::new(
                JobErrorCode::InvalidPath,
                operation,
                "任务路径中间组件不是目录。",
            )
            .at_path(&current));
        }
        if index + 1 == components.len() {
            return Ok(Some(metadata));
        }
    }
    unreachable!("non-empty components return inside loop")
}

fn require_safe_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if metadata_is_link(metadata) {
        return Err(JobServiceError::new(
            JobErrorCode::PathContainsSymlink,
            operation,
            "任务目录不能是符号链接或重解析点。",
        )
        .at_path(path));
    }
    if !metadata.is_dir() {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidPath,
            operation,
            "任务目录路径不是目录。",
        )
        .at_path(path));
    }
    Ok(())
}

fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn job_definition_path(project_dir: &Path, job_id: &str) -> PathBuf {
    project_dir.join("jobs").join(job_id).join("job.json")
}

fn job_events_dir(project_dir: &Path, job_id: &str) -> PathBuf {
    project_dir.join("jobs").join(job_id).join("events")
}

fn job_event_path(project_dir: &Path, job_id: &str, sequence: u32) -> PathBuf {
    job_events_dir(project_dir, job_id).join(format!("{sequence:010}.json"))
}

fn ensure_job_slot_available(
    project_dir: &Path,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if scan_job_ids(project_dir, operation)?.len() >= MAX_JOBS {
        return Err(JobServiceError::new(
            JobErrorCode::ScanLimitExceeded,
            operation,
            format!("项目任务数已达到上限 {MAX_JOBS}。"),
        ));
    }
    Ok(())
}

fn validate_enqueue_options(
    options: &EnqueueStageJobOptions,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    validate_portable_component(&options.stage_id, "stageId", operation)?;
    if !options.run_id.starts_with("run_") {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "runId 必须以 run_ 开头。",
        ));
    }
    validate_portable_component(&options.run_id, "runId", operation)?;
    let key = options.idempotency_key.as_str();
    if key.trim().is_empty()
        || key != key.trim()
        || key.chars().count() > 256
        || key.chars().any(char::is_control)
    {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "idempotencyKey 必须是 1..=256 个无首尾空白、无控制符的字符。",
        ));
    }
    if options.input_refs.len() > 256 {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "单任务最多包含 256 个输入引用。",
        ));
    }
    validate_retry_policy(&options.retry_policy, operation)
}

fn validate_retry_policy(
    policy: &RetryPolicyData,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if !(1..=10).contains(&policy.max_attempts)
        || policy.initial_backoff_ms > 86_400_000
        || !(1..=10).contains(&policy.backoff_multiplier)
        || policy.max_backoff_ms > 86_400_000
        || policy.max_backoff_ms < policy.initial_backoff_ms
    {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "retryPolicy 必须满足 maxAttempts 1..=10、退避不超过 24 小时且 maxBackoffMs 不小于 initialBackoffMs。",
        ));
    }
    Ok(())
}

fn validate_failure(
    error: &JobFailureData,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    validate_portable_component(&error.code, "error.code", operation)?;
    validate_text(&error.message, "error.message", 4096, operation)?;
    Ok(())
}

fn validate_lease_duration(value: u64, operation: JobOperation) -> Result<(), JobServiceError> {
    if !(1_000..=MAX_LEASE_MS).contains(&value) {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            format!("leaseDurationMs 必须位于 1000..={MAX_LEASE_MS}。"),
        ));
    }
    Ok(())
}

fn validate_text(
    value: &str,
    label: &str,
    max_characters: usize,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if value.trim().is_empty()
        || value.chars().count() > max_characters
        || value.chars().any(|character| character == '\0')
    {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            format!("{label} 不能为空、不能包含 NUL，且长度不能超过 {max_characters}。"),
        ));
    }
    Ok(())
}

fn validate_portable_component(
    value: &str,
    label: &str,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if portable_component_is_valid(value) {
        Ok(())
    } else {
        Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            format!("{label} 只能包含 ASCII 字母、数字、点、下划线和连字符，且长度不能超过 160。"),
        ))
    }
}

fn portable_component_is_valid(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 160
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_job_id(value: &str, operation: JobOperation) -> Result<(), JobServiceError> {
    let suffix = value.strip_prefix("job_").ok_or_else(|| {
        JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "jobId 必须以 job_ 开头。",
        )
    })?;
    if suffix.len() != 64
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidRequest,
            operation,
            "jobId 必须包含 64 位小写十六进制摘要。",
        ));
    }
    Ok(())
}

fn validate_job_definition_hashes(
    job: &Value,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    let project_id = required_string(job, "projectId", operation)?;
    let idempotency_hash = required_string(job, "idempotencyHash", operation)?;
    let expected_job_id = job_id_from_hash(&project_id, &idempotency_hash);
    if job.get("jobId").and_then(Value::as_str) != Some(expected_job_id.as_str()) {
        return Err(invalid_job(
            operation,
            "JobDefinition.jobId 与项目和 idempotencyHash 不一致。",
        ));
    }
    let stage_id = required_string(job, "stageId", operation)?;
    let run_id = required_string(job, "stageRunId", operation)?;
    let expected_uri = format!("runs/{stage_id}/{run_id}/execution.json");
    if job.get("executionSnapshotUri").and_then(Value::as_str) != Some(expected_uri.as_str()) {
        return Err(invalid_job(
            operation,
            "JobDefinition.executionSnapshotUri 与阶段运行身份不一致。",
        ));
    }
    let retry_policy: RetryPolicyData = serde_json::from_value(
        job.get("retryPolicy")
            .cloned()
            .ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 retryPolicy。"))?,
    )
    .map_err(|error| invalid_job(operation, format!("retryPolicy 无效：{error}")))?;
    validate_retry_policy(&retry_policy, operation).map_err(|error| JobServiceError {
        code: JobErrorCode::InvalidProject,
        ..error
    })?;
    let request_hash = hash_json(
        &json!({
            "projectId": project_id,
            "stageId": stage_id,
            "stageRunId": run_id,
            "inputRefs": required_array(job, "inputRefs", operation)?,
            "executor": job.get("executor").cloned().ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 executor。"))?,
            "retryPolicy": retry_policy,
        }),
        operation,
    )?;
    if job.get("requestHash").and_then(Value::as_str) != Some(request_hash.as_str()) {
        return Err(invalid_job(
            operation,
            "JobDefinition.requestHash 与不可变请求不一致。",
        ));
    }
    Ok(())
}

fn deterministic_job_id(project_id: &str, idempotency_key: &str) -> String {
    job_id_from_hash(project_id, &hash_bytes(idempotency_key.as_bytes()))
}

fn job_id_from_hash(project_id: &str, idempotency_hash: &str) -> String {
    let digest = hash_bytes(format!("{project_id}\0{idempotency_hash}").as_bytes());
    format!("job_{}", digest.trim_start_matches("sha256:"))
}

fn deterministic_event_id(job_id: &str, sequence: u32) -> String {
    format!("event_{}_{sequence:010}", job_id.trim_start_matches("job_"))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in digest {
        value.push_str(&format!("{byte:02x}"));
    }
    value
}

fn hash_json(value: &Value, operation: JobOperation) -> Result<String, JobServiceError> {
    serde_json::to_vec(value)
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|error| {
            JobServiceError::new(
                JobErrorCode::InternalContractError,
                operation,
                format!("计算任务 JSON 哈希失败：{error}"),
            )
        })
}

fn validate_persistent_document(
    document: &Value,
    operation: JobOperation,
    label: &str,
) -> Result<(), JobServiceError> {
    validate_contract_document(document).map_err(|error| {
        JobServiceError::new(
            JobErrorCode::InvalidProject,
            operation,
            format!("{label} 未通过持久化 v1 契约：{error}"),
        )
    })
}

fn required_string(
    value: &Value,
    field: &str,
    operation: JobOperation,
) -> Result<String, JobServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_job(operation, format!("任务文档缺少字符串字段 {field}。")))
}

fn required_u32(
    value: &Value,
    field: &str,
    operation: JobOperation,
) -> Result<u32, JobServiceError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| invalid_job(operation, format!("任务文档缺少 u32 字段 {field}。")))
}

fn required_f64(
    value: &Value,
    field: &str,
    operation: JobOperation,
) -> Result<f64, JobServiceError> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .ok_or_else(|| invalid_job(operation, format!("任务文档缺少有限数值字段 {field}。")))
}

fn required_array(
    value: &Value,
    field: &str,
    operation: JobOperation,
) -> Result<Vec<Value>, JobServiceError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| invalid_job(operation, format!("任务文档缺少数组字段 {field}。")))
}

fn required_string_array(
    value: &Value,
    field: &str,
    operation: JobOperation,
) -> Result<Vec<String>, JobServiceError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_job(operation, format!("任务文档缺少数组字段 {field}。")))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_job(operation, format!("{field} 必须只包含字符串。")))
        })
        .collect()
}

fn service_error_failure(error: &JobServiceError) -> JobFailureData {
    let mut details = Map::new();
    if let Some(path) = &error.path {
        details.insert("path".to_owned(), Value::String(path.clone()));
    }
    if let Some(stage_id) = &error.stage_id {
        details.insert("stageId".to_owned(), Value::String(stage_id.clone()));
    }
    if let Some(run_id) = &error.run_id {
        details.insert("runId".to_owned(), Value::String(run_id.clone()));
    }
    JobFailureData {
        code: error.code.as_str().to_owned(),
        message: error.message.clone(),
        retryable: false,
        details,
    }
}

fn preparation_error_is_deferred(code: JobErrorCode) -> bool {
    matches!(
        code,
        JobErrorCode::WorkflowNotInitialized
            | JobErrorCode::IoError
            | JobErrorCode::InternalContractError
    )
}

fn preparation_failure_error(
    job: &Value,
    events: &[Value],
    operation: JobOperation,
) -> Result<Option<JobServiceError>, JobServiceError> {
    let Some(event) = events.first().filter(|event| {
        event.get("eventType").and_then(Value::as_str) == Some("preparation_failed")
    }) else {
        return Ok(None);
    };
    let failure = parse_failure(event, operation)?;
    let code = job_error_code_from_str(&failure.code).unwrap_or(JobErrorCode::InvalidRequest);
    let mut error = JobServiceError::new(code, operation, failure.message)
        .for_job(required_string(job, "jobId", operation)?)
        .for_stage(required_string(job, "stageId", operation)?)
        .for_run(required_string(job, "stageRunId", operation)?);
    error.path = failure
        .details
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Some(error))
}

fn job_error_code_from_str(value: &str) -> Option<JobErrorCode> {
    Some(match value {
        "invalid_request" => JobErrorCode::InvalidRequest,
        "invalid_path" => JobErrorCode::InvalidPath,
        "path_contains_symlink" => JobErrorCode::PathContainsSymlink,
        "project_not_found" => JobErrorCode::ProjectNotFound,
        "project_identity_mismatch" => JobErrorCode::ProjectIdentityMismatch,
        "invalid_project" => JobErrorCode::InvalidProject,
        "migration_required" => JobErrorCode::MigrationRequired,
        "unsupported_newer_version" => JobErrorCode::UnsupportedNewerVersion,
        "workflow_not_initialized" => JobErrorCode::WorkflowNotInitialized,
        "stage_not_ready" => JobErrorCode::StageNotReady,
        "job_not_found" => JobErrorCode::JobNotFound,
        "idempotency_conflict" => JobErrorCode::IdempotencyConflict,
        "invalid_transition" => JobErrorCode::InvalidTransition,
        "lease_conflict" => JobErrorCode::LeaseConflict,
        "lease_expired" => JobErrorCode::LeaseExpired,
        "event_conflict" => JobErrorCode::EventConflict,
        "scan_limit_exceeded" => JobErrorCode::ScanLimitExceeded,
        "io_error" => JobErrorCode::IoError,
        "internal_contract_error" => JobErrorCode::InternalContractError,
        _ => return None,
    })
}

fn completion_request_matches(
    event: &Value,
    lease_id: &str,
    artifact_ids: &[String],
    log_summary: &Value,
) -> bool {
    event.get("eventType").and_then(Value::as_str) == Some("completion_requested")
        && event.get("leaseId").and_then(Value::as_str) == Some(lease_id)
        && event.get("artifactIds") == Some(&json!(artifact_ids))
        && event.get("logSummary") == Some(log_summary)
}

fn failure_request_matches(
    event: &Value,
    lease_id: &str,
    error: &Value,
    log_summary: &Value,
) -> bool {
    event.get("eventType").and_then(Value::as_str) == Some("failure_requested")
        && event.get("leaseId").and_then(Value::as_str) == Some(lease_id)
        && event.get("error") == Some(error)
        && event.get("logSummary") == Some(log_summary)
}

fn attempt_failure_matches(
    event: &Value,
    lease_id: &str,
    error: &Value,
    log_summary: &Value,
) -> bool {
    event.get("eventType").and_then(Value::as_str) == Some("attempt_failed")
        && event.get("leaseId").and_then(Value::as_str) == Some(lease_id)
        && event.get("error") == Some(error)
        && event.get("logSummary") == Some(log_summary)
}

fn parse_failure(
    event: &Value,
    operation: JobOperation,
) -> Result<JobFailureData, JobServiceError> {
    serde_json::from_value(
        event
            .get("error")
            .cloned()
            .ok_or_else(|| invalid_job(operation, "失败类 JobEvent 缺少 error。"))?,
    )
    .map_err(|error| invalid_job(operation, format!("JobEvent.error 无效：{error}")))
}

fn retry_policy_from_job(
    job: &Value,
    operation: JobOperation,
) -> Result<RetryPolicyData, JobServiceError> {
    serde_json::from_value(
        job.get("retryPolicy")
            .cloned()
            .ok_or_else(|| invalid_job(operation, "JobDefinition 缺少 retryPolicy。"))?,
    )
    .map_err(|error| invalid_job(operation, format!("retryPolicy 无效：{error}")))
}

fn backoff_ms(policy: &RetryPolicyData, failed_attempt: u32) -> u64 {
    let exponent = failed_attempt.saturating_sub(1);
    let multiplier = u64::from(policy.backoff_multiplier).saturating_pow(exponent);
    policy
        .initial_backoff_ms
        .saturating_mul(multiplier)
        .min(policy.max_backoff_ms)
}

fn require_active_lease(
    projection: &JobProjection,
    lease_id: &str,
    now: OffsetDateTime,
    operation: JobOperation,
    job_id: &str,
) -> Result<(), JobServiceError> {
    if projection.status != JobStatusData::Running {
        return Err(invalid_transition(
            operation,
            job_id,
            "任务当前不处于 running。",
        ));
    }
    let lease = projection
        .lease
        .as_ref()
        .ok_or_else(|| invalid_job(operation, "running 任务缺少 worker 租约。").for_job(job_id))?;
    if lease.lease_id != lease_id {
        return Err(JobServiceError::new(
            JobErrorCode::LeaseConflict,
            operation,
            "leaseId 与当前 worker 租约不一致。",
        )
        .for_job(job_id));
    }
    if now >= parse_timestamp(&lease.expires_at, operation)? {
        return Err(JobServiceError::new(
            JobErrorCode::LeaseExpired,
            operation,
            "worker 租约已过期，必须先由恢复流程重新排队。",
        )
        .for_job(job_id));
    }
    Ok(())
}

fn require_worker_update_allowed(
    projection: &JobProjection,
    lease_id: &str,
    now: OffsetDateTime,
    operation: JobOperation,
    job_id: &str,
) -> Result<(), JobServiceError> {
    require_active_lease(projection, lease_id, now, operation, job_id)?;
    if projection.cancellation_requested || projection.finalization_event.is_some() {
        return Err(invalid_transition(
            operation,
            job_id,
            "任务已收到取消请求或进入终态提交，不能再写执行进度。",
        ));
    }
    Ok(())
}

fn lease_is_expired(
    projection: &JobProjection,
    now: OffsetDateTime,
    operation: JobOperation,
) -> Result<bool, JobServiceError> {
    let lease = projection
        .lease
        .as_ref()
        .ok_or_else(|| invalid_job(operation, "running 任务缺少 worker 租约。"))?;
    Ok(now >= parse_timestamp(&lease.expires_at, operation)?)
}

fn timestamp_is_due(value: &str, now: OffsetDateTime) -> bool {
    OffsetDateTime::parse(value, &Rfc3339).is_ok_and(|timestamp| timestamp <= now)
}

fn parse_timestamp(
    value: &str,
    operation: JobOperation,
) -> Result<OffsetDateTime, JobServiceError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| invalid_job(operation, format!("任务时间戳不是 RFC3339：{error}")))
}

fn format_timestamp(
    value: OffsetDateTime,
    operation: JobOperation,
) -> Result<String, JobServiceError> {
    value.format(&Rfc3339).map_err(|error| {
        JobServiceError::new(
            JobErrorCode::InternalContractError,
            operation,
            format!("格式化任务时间戳失败：{error}"),
        )
    })
}

fn indexed_status(status: JobStatusData) -> IndexedJobStatusData {
    match status {
        JobStatusData::Queued => IndexedJobStatusData::Queued,
        JobStatusData::Running => IndexedJobStatusData::Running,
        JobStatusData::Retrying => IndexedJobStatusData::Retrying,
        JobStatusData::Succeeded => IndexedJobStatusData::Succeeded,
        JobStatusData::Failed => IndexedJobStatusData::Failed,
        JobStatusData::Canceled => IndexedJobStatusData::Canceled,
    }
}

fn require_project_identity(
    descriptor: &ProjectDescriptorData,
    expected_project_id: &str,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if descriptor.project_id != expected_project_id {
        return Err(JobServiceError::new(
            JobErrorCode::ProjectIdentityMismatch,
            operation,
            "expectedProjectId 与项目标识不一致。",
        )
        .at_path(Path::new(&descriptor.marker_path)));
    }
    Ok(())
}

fn require_current_project_job(
    descriptor: &ProjectDescriptorData,
    job: &Value,
    operation: JobOperation,
) -> Result<(), JobServiceError> {
    if job.get("projectId").and_then(Value::as_str) != Some(descriptor.project_id.as_str()) {
        return Err(JobServiceError::new(
            JobErrorCode::InvalidTransition,
            operation,
            "复制项目保留的历史任务只能查看，不能在新项目身份下继续执行。",
        )
        .for_job(required_string(job, "jobId", operation)?));
    }
    Ok(())
}

fn invalid_job(operation: JobOperation, message: impl Into<String>) -> JobServiceError {
    JobServiceError::new(JobErrorCode::InvalidProject, operation, message)
}

fn invalid_transition(
    operation: JobOperation,
    job_id: &str,
    message: impl Into<String>,
) -> JobServiceError {
    JobServiceError::new(JobErrorCode::InvalidTransition, operation, message).for_job(job_id)
}

fn project_error_to_job(error: ProjectServiceError, operation: JobOperation) -> JobServiceError {
    let code = match error.code {
        ProjectErrorCode::InvalidPath | ProjectErrorCode::InvalidName => JobErrorCode::InvalidPath,
        ProjectErrorCode::PathContainsSymlink => JobErrorCode::PathContainsSymlink,
        ProjectErrorCode::ProjectNotFound | ProjectErrorCode::MarkerMissing => {
            JobErrorCode::ProjectNotFound
        }
        ProjectErrorCode::MigrationRequired => JobErrorCode::MigrationRequired,
        ProjectErrorCode::UnsupportedNewerVersion => JobErrorCode::UnsupportedNewerVersion,
        ProjectErrorCode::InvalidProject | ProjectErrorCode::MarkerTooLarge => {
            JobErrorCode::InvalidProject
        }
        ProjectErrorCode::IoError => JobErrorCode::IoError,
        _ => JobErrorCode::InternalContractError,
    };
    let mut mapped = JobServiceError::new(code, operation, error.message);
    mapped.path = error.path;
    mapped
}

fn workflow_error_to_job(error: WorkflowServiceError, operation: JobOperation) -> JobServiceError {
    let code = match error.code {
        WorkflowErrorCode::InvalidRequest | WorkflowErrorCode::StageNotFound => {
            JobErrorCode::InvalidRequest
        }
        WorkflowErrorCode::InvalidPath => JobErrorCode::InvalidPath,
        WorkflowErrorCode::PathContainsSymlink => JobErrorCode::PathContainsSymlink,
        WorkflowErrorCode::ProjectNotFound => JobErrorCode::ProjectNotFound,
        WorkflowErrorCode::ProjectIdentityMismatch => JobErrorCode::ProjectIdentityMismatch,
        WorkflowErrorCode::InvalidProject
        | WorkflowErrorCode::UnsupportedWorkflow
        | WorkflowErrorCode::InvalidStageGraph => JobErrorCode::InvalidProject,
        WorkflowErrorCode::MigrationRequired => JobErrorCode::MigrationRequired,
        WorkflowErrorCode::UnsupportedNewerVersion => JobErrorCode::UnsupportedNewerVersion,
        WorkflowErrorCode::WorkflowNotInitialized => JobErrorCode::WorkflowNotInitialized,
        WorkflowErrorCode::StageNotReady | WorkflowErrorCode::ArtifactMismatch => {
            JobErrorCode::StageNotReady
        }
        WorkflowErrorCode::RunConflict | WorkflowErrorCode::ImmutableConflict => {
            JobErrorCode::IdempotencyConflict
        }
        WorkflowErrorCode::ScanLimitExceeded => JobErrorCode::ScanLimitExceeded,
        WorkflowErrorCode::IoError => JobErrorCode::IoError,
        _ => JobErrorCode::InternalContractError,
    };
    let mut mapped = JobServiceError::new(code, operation, error.message);
    mapped.path = error.path;
    mapped.job_id = None;
    mapped.stage_id = error.stage_id;
    mapped.run_id = error.run_id;
    mapped
}
