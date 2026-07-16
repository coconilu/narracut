use std::{error::Error, fmt, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowErrorCode {
    InvalidRequest,
    InvalidPath,
    PathContainsSymlink,
    ProjectNotFound,
    ProjectIdentityMismatch,
    InvalidProject,
    MigrationRequired,
    UnsupportedNewerVersion,
    WorkflowNotInitialized,
    UnsupportedWorkflow,
    InvalidStageGraph,
    StageNotFound,
    StageNotReady,
    ConfigConflict,
    RunNotFound,
    RunConflict,
    ReviewConflict,
    ArtifactMismatch,
    ImmutableConflict,
    ScanLimitExceeded,
    IoError,
    InternalContractError,
}

impl WorkflowErrorCode {
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
            Self::UnsupportedWorkflow => "unsupported_workflow",
            Self::InvalidStageGraph => "invalid_stage_graph",
            Self::StageNotFound => "stage_not_found",
            Self::StageNotReady => "stage_not_ready",
            Self::ConfigConflict => "config_conflict",
            Self::RunNotFound => "run_not_found",
            Self::RunConflict => "run_conflict",
            Self::ReviewConflict => "review_conflict",
            Self::ArtifactMismatch => "artifact_mismatch",
            Self::ImmutableConflict => "immutable_conflict",
            Self::ScanLimitExceeded => "scan_limit_exceeded",
            Self::IoError => "io_error",
            Self::InternalContractError => "internal_contract_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOperation {
    InitializeWorkflow,
    GetWorkflow,
    UpdateStageConfig,
    PrepareStageRun,
    RecordStageRun,
    ReviewStageRun,
    PreviewRegeneration,
    ListStageHistory,
}

impl WorkflowOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitializeWorkflow => "initialize_workflow",
            Self::GetWorkflow => "get_workflow",
            Self::UpdateStageConfig => "update_stage_config",
            Self::PrepareStageRun => "prepare_stage_run",
            Self::RecordStageRun => "record_stage_run",
            Self::ReviewStageRun => "review_stage_run",
            Self::PreviewRegeneration => "preview_regeneration",
            Self::ListStageHistory => "list_stage_history",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowServiceError {
    pub code: WorkflowErrorCode,
    pub operation: WorkflowOperation,
    pub message: String,
    pub path: Option<String>,
    pub stage_id: Option<String>,
    pub run_id: Option<String>,
}

impl WorkflowServiceError {
    pub fn new(
        code: WorkflowErrorCode,
        operation: WorkflowOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            path: None,
            stage_id: None,
            run_id: None,
        }
    }

    pub fn at_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
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
        operation: WorkflowOperation,
        path: &Path,
        context: &str,
        error: &std::io::Error,
    ) -> Self {
        Self::new(
            WorkflowErrorCode::IoError,
            operation,
            format!("{context}：{error}"),
        )
        .at_path(path)
    }
}

impl fmt::Display for WorkflowServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for WorkflowServiceError {}
