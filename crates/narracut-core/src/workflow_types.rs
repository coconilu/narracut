use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatusData {
    Draft,
    Ready,
    Running,
    NeedsReview,
    Approved,
    Failed,
    Stale,
}

impl StageStatusData {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::NeedsReview => "needs_review",
            Self::Approved => "approved",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRunStatusData {
    Succeeded,
    Failed,
    Canceled,
}

impl TerminalRunStatusData {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecisionData {
    Approved,
    Rejected,
    ChangesRequested,
}

impl ReviewDecisionData {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::ChangesRequested => "changes_requested",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewerReferenceData {
    pub kind: String,
    pub reviewer_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitializeWorkflowOptions {
    pub project_path: String,
    pub expected_project_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStageConfigOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub expected_revision: u32,
    pub values: Map<String, Value>,
    pub decisions: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrepareStageRunOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub job_id: String,
    pub input_refs: Vec<Value>,
    pub executor: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedArtifactInputData {
    pub ref_id: String,
    pub kind: String,
    pub artifact_id: String,
    pub source_run_id: String,
    pub review_record_id: String,
    pub content_hash: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateApprovedMediaInputsOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub target_stage_id: String,
    pub inputs: Vec<ApprovedArtifactInputData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateCurrentApprovedStageArtifactOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub expected_kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordStageRunOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub status: TerminalRunStatusData,
    pub job_id: String,
    pub artifact_ids: Vec<String>,
    pub log_summary: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewStageRunOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub review_id: String,
    pub decision: ReviewDecisionData,
    pub reviewer: ReviewerReferenceData,
    pub comments: String,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageStateData {
    pub stage_id: String,
    pub status: StageStatusData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_run_id: Option<String>,
    pub stale_because_stage_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedStageData {
    pub stage_id: String,
    pub distance: u32,
    pub direct_cause_stage_ids: Vec<String>,
    pub current_status: StageStatusData,
    pub has_approved_run: bool,
    pub supports_partial_regeneration: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSnapshotData {
    pub api_version: String,
    pub owner_project_id: String,
    pub workflow_definition_id: String,
    pub stage_definitions: Vec<Value>,
    pub stage_states: Vec<StageStateData>,
    pub configs: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageConfigUpdateResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub config: Value,
    pub config_uri: String,
    pub affected_stages: Vec<AffectedStageData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRunCommitResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub run: Value,
    pub run_uri: String,
    pub stage_state: StageStateData,
    pub review_required: bool,
    pub execution_outdated: bool,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRunPreparationResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub execution_snapshot: Value,
    pub execution_snapshot_uri: String,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageReviewResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub review: Value,
    pub review_uri: String,
    pub stage_states: Vec<StageStateData>,
    pub invalidated_stage_ids: Vec<String>,
    pub applied: bool,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenerationImpactResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub changed_stage_ids: Vec<String>,
    pub affected_stages: Vec<AffectedStageData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageHistoryResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub stage_id: String,
    pub runs: Vec<Value>,
    pub reviews: Vec<Value>,
}
