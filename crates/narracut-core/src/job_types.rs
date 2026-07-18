use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatusData {
    Queued,
    Running,
    Retrying,
    Succeeded,
    Failed,
    Canceled,
}

impl JobStatusData {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicyData {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub backoff_multiplier: u32,
    pub max_backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFailureData {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnqueueStageJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub input_refs: Vec<Value>,
    pub executor: Value,
    pub idempotency_key: String,
    pub retry_policy: RetryPolicyData,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimStageJobRequestOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub idempotency_key: String,
    pub request: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StageJobRequestClaimData {
    pub owner_project_id: String,
    pub job_id: String,
    pub request: Value,
    pub request_uri: String,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetStageJobRequestOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageJobRequestData {
    pub owner_project_id: String,
    pub job_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub request_receipt_hash: String,
    pub request_uri: String,
    pub request: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListJobsOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub statuses: Vec<JobStatusData>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListJobEventsOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub after_sequence: Option<u32>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryStageJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub source_job_id: String,
    pub new_run_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverJobsOptions {
    pub project_path: String,
    pub expected_project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimNextJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub worker_id: String,
    pub lease_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub lease_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewJobLeaseOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
    pub lease_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportJobProgressOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
    pub progress: f64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordJobArtifactOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
    pub artifact_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompleteJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
    pub artifact_ids: Vec<String>,
    pub log_summary: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailJobOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
    pub error: JobFailureData,
    pub log_summary: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeCancellationOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub lease_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobLeaseData {
    pub worker_id: String,
    pub lease_id: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshotData {
    pub api_version: String,
    pub owner_project_id: String,
    pub job: Value,
    pub job_uri: String,
    pub status: JobStatusData,
    pub attempt: u32,
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub cancellation_requested: bool,
    pub finalization_pending: bool,
    pub artifact_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<JobFailureData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<JobLeaseData>,
    pub last_sequence: u32,
    pub created_at: String,
    pub updated_at: String,
    pub historical: bool,
    pub index_synchronized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobListResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub jobs: Vec<JobSnapshotData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEventsResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub job_id: String,
    pub events: Vec<Value>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecoveryResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub recovered_job_ids: Vec<String>,
    pub finalized_job_ids: Vec<String>,
    pub skipped_live_job_ids: Vec<String>,
    pub reindexed_jobs: u32,
    pub index_warnings: u32,
}
