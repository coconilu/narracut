use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportOperation {
    RunQa,
    Enqueue,
    Prepare,
    Commit,
    GetResult,
    Verify,
}

impl ExportOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunQa => "run_export_qa",
            Self::Enqueue | Self::Prepare | Self::Commit => "enqueue_export",
            Self::GetResult => "get_export_result",
            Self::Verify => "verify_export",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportErrorCode {
    InvalidRequest,
    InvalidProject,
    ProjectMismatch,
    RenderNotApproved,
    RenderStale,
    ArtifactNotFound,
    HashMismatch,
    QaBlocked,
    QaChanged,
    RightsIncomplete,
    RendererIdentityChanged,
    DestinationInvalid,
    DestinationConflict,
    DiskSpaceInsufficient,
    Canceled,
    Io,
    InternalContract,
}

impl ExportErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidProject => "invalid_project",
            Self::ProjectMismatch => "project_mismatch",
            Self::RenderNotApproved => "render_not_approved",
            Self::RenderStale => "render_stale",
            Self::ArtifactNotFound => "artifact_not_found",
            Self::HashMismatch => "hash_mismatch",
            Self::QaBlocked => "qa_blocked",
            Self::QaChanged => "qa_changed",
            Self::RightsIncomplete => "rights_incomplete",
            Self::RendererIdentityChanged => "renderer_identity_changed",
            Self::DestinationInvalid => "destination_invalid",
            Self::DestinationConflict => "destination_conflict",
            Self::DiskSpaceInsufficient => "disk_space_insufficient",
            Self::Canceled => "canceled",
            Self::Io => "io_error",
            Self::InternalContract => "internal_contract_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportServiceError {
    pub code: ExportErrorCode,
    pub operation: ExportOperation,
    pub message: String,
    pub retryable: bool,
}

impl ExportServiceError {
    pub fn new(
        code: ExportErrorCode,
        operation: ExportOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            retryable: matches!(
                code,
                ExportErrorCode::Io | ExportErrorCode::DiskSpaceInsufficient
            ),
        }
    }
}

impl fmt::Display for ExportServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for ExportServiceError {}
