use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererOperation {
    ProbeRenderer,
    CreateSceneSnapshot,
    EnqueueSceneRender,
    EnqueueTimelineRender,
    ExecuteRender,
    CommitRender,
    GetRenderResult,
}

impl RendererOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProbeRenderer => "probe_renderer",
            Self::CreateSceneSnapshot => "create_scene_snapshot",
            Self::EnqueueSceneRender => "enqueue_scene_render",
            Self::EnqueueTimelineRender => "enqueue_timeline_render",
            Self::ExecuteRender => "execute_render",
            Self::CommitRender => "commit_render",
            Self::GetRenderResult => "get_render_result",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererServiceErrorCode {
    InvalidRequest,
    ProjectNotFound,
    ProjectIdentityMismatch,
    ReviewRequired,
    InputStale,
    InputHashMismatch,
    CrossProjectReference,
    TraceabilityIncomplete,
    SceneNotFound,
    SnapshotTooLarge,
    ResourceRejected,
    ResourceLimitExceeded,
    ArtifactNotFound,
    Canceled,
    JobConflict,
    ContractViolation,
    Io,
}

impl RendererServiceErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectIdentityMismatch => "project_identity_mismatch",
            Self::ReviewRequired => "review_required",
            Self::InputStale => "input_stale",
            Self::InputHashMismatch => "input_hash_mismatch",
            Self::CrossProjectReference => "cross_project_reference",
            Self::TraceabilityIncomplete => "traceability_incomplete",
            Self::SceneNotFound => "scene_not_found",
            Self::SnapshotTooLarge => "snapshot_too_large",
            Self::ResourceRejected => "resource_rejected",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::ArtifactNotFound => "artifact_not_found",
            Self::Canceled => "canceled",
            Self::JobConflict => "job_conflict",
            Self::ContractViolation => "internal_contract_error",
            Self::Io => "io_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererServiceError {
    pub code: RendererServiceErrorCode,
    pub operation: RendererOperation,
    pub message: String,
    pub retryable: bool,
    pub stage_id: Option<String>,
    pub run_id: Option<String>,
    pub artifact_id: Option<String>,
}

impl RendererServiceError {
    pub fn new(
        code: RendererServiceErrorCode,
        operation: RendererOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            retryable: false,
            stage_id: None,
            run_id: None,
            artifact_id: None,
        }
    }

    pub fn retryable(mut self, value: bool) -> Self {
        self.retryable = value;
        self
    }
    pub fn for_run(mut self, value: impl Into<String>) -> Self {
        self.run_id = Some(value.into());
        self
    }
    pub fn for_artifact(mut self, value: impl Into<String>) -> Self {
        self.artifact_id = Some(value.into());
        self
    }
}

impl fmt::Display for RendererServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for RendererServiceError {}
