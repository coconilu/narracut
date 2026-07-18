use std::{error::Error, fmt, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageErrorCode {
    InvalidRequest,
    InvalidPath,
    PathContainsSymlink,
    ProjectNotFound,
    ProjectIdentityMismatch,
    InvalidProject,
    MigrationRequired,
    UnsupportedNewerVersion,
    SourceNotFound,
    SourceChanged,
    SourceTooLarge,
    ArtifactTooLarge,
    InvalidArtifact,
    ArtifactNotFound,
    ArtifactConflict,
    ContentCorrupt,
    IndexUnavailable,
    IndexMigrationFailed,
    ScanLimitExceeded,
    CacheCleanupFailed,
    IoError,
    InternalContractError,
}

impl StorageErrorCode {
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
            Self::SourceNotFound => "source_not_found",
            Self::SourceChanged => "source_changed",
            Self::SourceTooLarge => "source_too_large",
            Self::ArtifactTooLarge => "artifact_too_large",
            Self::InvalidArtifact => "invalid_artifact",
            Self::ArtifactNotFound => "artifact_not_found",
            Self::ArtifactConflict => "artifact_conflict",
            Self::ContentCorrupt => "content_corrupt",
            Self::IndexUnavailable => "index_unavailable",
            Self::IndexMigrationFailed => "index_migration_failed",
            Self::ScanLimitExceeded => "scan_limit_exceeded",
            Self::CacheCleanupFailed => "cache_cleanup_failed",
            Self::IoError => "io_error",
            Self::InternalContractError => "internal_contract_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOperation {
    ImportArtifact,
    GetArtifact,
    VerifyArtifact,
    ManageMediaReceipt,
    ManageMediaSource,
    RebuildProjectIndex,
    ListRecentProjects,
    ListIndexedJobs,
    ForgetProject,
    CleanProjectCache,
}

impl StorageOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImportArtifact => "import_artifact",
            Self::GetArtifact => "get_artifact",
            Self::VerifyArtifact => "verify_artifact",
            Self::ManageMediaReceipt => "manage_media_receipt",
            Self::ManageMediaSource => "manage_media_source",
            Self::RebuildProjectIndex => "rebuild_project_index",
            Self::ListRecentProjects => "list_recent_projects",
            Self::ListIndexedJobs => "list_indexed_jobs",
            Self::ForgetProject => "forget_project",
            Self::CleanProjectCache => "clean_project_cache",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageServiceError {
    pub code: StorageErrorCode,
    pub operation: StorageOperation,
    pub message: String,
    pub path: Option<String>,
    pub artifact_id: Option<String>,
}

impl StorageServiceError {
    pub fn new(
        code: StorageErrorCode,
        operation: StorageOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            path: None,
            artifact_id: None,
        }
    }

    pub fn at_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
        self
    }

    pub fn for_artifact(mut self, artifact_id: impl Into<String>) -> Self {
        self.artifact_id = Some(artifact_id.into());
        self
    }

    pub(crate) fn io(
        operation: StorageOperation,
        path: &Path,
        context: &str,
        error: &std::io::Error,
    ) -> Self {
        Self::new(
            StorageErrorCode::IoError,
            operation,
            format!("{context}：{error}"),
        )
        .at_path(path)
    }

    pub(crate) fn index(
        code: StorageErrorCode,
        operation: StorageOperation,
        path: &Path,
        context: &str,
        error: &rusqlite::Error,
    ) -> Self {
        Self::new(code, operation, format!("{context}：{error}")).at_path(path)
    }
}

impl fmt::Display for StorageServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for StorageServiceError {}
