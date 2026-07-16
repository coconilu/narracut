use std::{error::Error, fmt, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectErrorCode {
    InvalidRequest,
    InvalidName,
    InvalidPath,
    PathContainsSymlink,
    ProjectNotFound,
    MarkerMissing,
    MarkerTooLarge,
    InvalidProject,
    MigrationRequired,
    MigrationConflict,
    UnsupportedNewerVersion,
    MigrationFailed,
    DestinationExists,
    CopyTooLarge,
    IoError,
    TrashFailed,
    InternalContractError,
}

impl ProjectErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidName => "invalid_name",
            Self::InvalidPath => "invalid_path",
            Self::PathContainsSymlink => "path_contains_symlink",
            Self::ProjectNotFound => "project_not_found",
            Self::MarkerMissing => "marker_missing",
            Self::MarkerTooLarge => "marker_too_large",
            Self::InvalidProject => "invalid_project",
            Self::MigrationRequired => "migration_required",
            Self::MigrationConflict => "migration_conflict",
            Self::UnsupportedNewerVersion => "unsupported_newer_version",
            Self::MigrationFailed => "migration_failed",
            Self::DestinationExists => "destination_exists",
            Self::CopyTooLarge => "copy_too_large",
            Self::IoError => "io_error",
            Self::TrashFailed => "trash_failed",
            Self::InternalContractError => "internal_contract_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOperation {
    Inspect,
    Open,
    Create,
    Migrate,
    Rename,
    Copy,
    SetArchived,
    MoveToTrash,
}

impl ProjectOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Open => "open",
            Self::Create => "create",
            Self::Migrate => "migrate",
            Self::Rename => "rename",
            Self::Copy => "copy",
            Self::SetArchived => "set_archived",
            Self::MoveToTrash => "move_to_trash",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectServiceError {
    pub code: ProjectErrorCode,
    pub operation: ProjectOperation,
    pub message: String,
    pub path: Option<String>,
    pub expected_version: Option<u32>,
    pub detected_version: Option<u32>,
}

impl ProjectServiceError {
    pub fn new(
        code: ProjectErrorCode,
        operation: ProjectOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            path: None,
            expected_version: None,
            detected_version: None,
        }
    }

    pub fn at_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn with_versions(mut self, expected: u32, detected: u32) -> Self {
        self.expected_version = Some(expected);
        self.detected_version = Some(detected);
        self
    }

    pub(crate) fn io(
        operation: ProjectOperation,
        path: &Path,
        context: &str,
        error: &std::io::Error,
    ) -> Self {
        Self::new(
            ProjectErrorCode::IoError,
            operation,
            format!("{context}：{error}"),
        )
        .at_path(path)
    }
}

impl fmt::Display for ProjectServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProjectServiceError {}
