use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaOperation {
    ImportAudio,
    ImportCaptions,
    GenerateScenePlan,
    GenerateTimeline,
    SaveScenePlan,
    SaveTimeline,
    ValidateApprovedInputs,
    ReadMediaDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaErrorCode {
    InvalidRequest,
    InvalidSourceName,
    SourceHashMismatch,
    SourceChanged,
    RightsRequired,
    VoiceCloneNotAllowed,
    InputNotApproved,
    InputReferenceMismatch,
    CrossProjectReference,
    ArtifactVerificationFailed,
    IdempotencyConflict,
    ResourceLimitExceeded,
    InvalidMedia,
    ContractViolation,
    StorageUnavailable,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaServiceError {
    pub code: MediaErrorCode,
    pub operation: MediaOperation,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<Box<str>>,
}

impl MediaServiceError {
    pub(crate) fn new(
        code: MediaErrorCode,
        operation: MediaOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            project_id: None,
            stage_id: None,
            run_id: None,
            artifact_id: None,
        }
    }

    pub(crate) fn with_safe_context(
        mut self,
        project_id: Option<&str>,
        stage_id: Option<&str>,
        run_id: Option<&str>,
        artifact_id: Option<&str>,
    ) -> Self {
        self.project_id = project_id.map(Box::<str>::from);
        self.stage_id = stage_id.map(Box::<str>::from);
        self.run_id = run_id.map(Box::<str>::from);
        self.artifact_id = artifact_id.map(Box::<str>::from);
        self
    }
}

impl fmt::Display for MediaServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MediaServiceError {}
