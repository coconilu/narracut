use std::{error::Error, fmt, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobErrorCode {
    InvalidRequest,
    InvalidPath,
    PathContainsSymlink,
    ProjectNotFound,
    ProjectIdentityMismatch,
    InvalidProject,
    MigrationRequired,
    UnsupportedNewerVersion,
    WorkflowNotInitialized,
    StageNotReady,
    JobNotFound,
    IdempotencyConflict,
    InvalidTransition,
    LeaseConflict,
    LeaseExpired,
    EventConflict,
    ScanLimitExceeded,
    IoError,
    InternalContractError,
}

impl JobErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidPath => "invalid_path",
            Self::PathContainsSymlink => "path_contains_symlink",
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectIdentityMismatch => "project_identity_mismatch",
            Self::InvalidProject => "invalid_project",
            Self::MigrationRequired => "migration_required",
            Self::UnsupportedNewerVersion => "unsupported_newer_version",
            Self::WorkflowNotInitialized => "workflow_not_initialized",
            Self::StageNotReady => "stage_not_ready",
            Self::JobNotFound => "job_not_found",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::InvalidTransition => "invalid_transition",
            Self::LeaseConflict => "lease_conflict",
            Self::LeaseExpired => "lease_expired",
            Self::EventConflict => "event_conflict",
            Self::ScanLimitExceeded => "scan_limit_exceeded",
            Self::IoError => "io_error",
            Self::InternalContractError => "internal_contract_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOperation {
    EnqueueStageJob,
    GetJob,
    ListJobs,
    ListJobEvents,
    CancelJob,
    RetryStageJob,
    RecoverJobs,
    ClaimNextJob,
    RenewLease,
    ReportProgress,
    RecordArtifact,
    CompleteJob,
    FailJob,
    AcknowledgeCancellation,
}

impl JobOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnqueueStageJob => "enqueue_stage_job",
            Self::GetJob => "get_job",
            Self::ListJobs => "list_jobs",
            Self::ListJobEvents => "list_job_events",
            Self::CancelJob => "cancel_job",
            Self::RetryStageJob => "retry_stage_job",
            Self::RecoverJobs => "recover_jobs",
            Self::ClaimNextJob => "claim_next_job",
            Self::RenewLease => "renew_lease",
            Self::ReportProgress => "report_progress",
            Self::RecordArtifact => "record_artifact",
            Self::CompleteJob => "complete_job",
            Self::FailJob => "fail_job",
            Self::AcknowledgeCancellation => "acknowledge_cancellation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobServiceError {
    pub code: JobErrorCode,
    pub operation: JobOperation,
    pub message: String,
    pub path: Option<String>,
    pub job_id: Option<Box<str>>,
    pub stage_id: Option<String>,
    pub run_id: Option<String>,
}

impl JobServiceError {
    pub fn new(code: JobErrorCode, operation: JobOperation, message: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            path: None,
            job_id: None,
            stage_id: None,
            run_id: None,
        }
    }

    pub fn at_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn for_job(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into().into_boxed_str());
        self
    }

    pub fn for_stage(mut self, stage_id: impl Into<String>) -> Self {
        self.stage_id = Some(stage_id.into());
        self
    }

    pub fn for_run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub(crate) fn io(
        operation: JobOperation,
        path: &Path,
        context: &str,
        error: &std::io::Error,
    ) -> Self {
        Self::new(
            JobErrorCode::IoError,
            operation,
            format!("{context}：{error}"),
        )
        .at_path(path)
    }
}

impl fmt::Display for JobServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for JobServiceError {}
