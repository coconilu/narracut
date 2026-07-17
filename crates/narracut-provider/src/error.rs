use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCode {
    InvalidRequest,
    CredentialMissing,
    ProviderUnavailable,
    ProviderResponseInvalid,
    RateLimited,
    Canceled,
    JobError,
    StorageError,
    WorkflowError,
    Internal,
}

impl ProviderErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::CredentialMissing => "credential_missing",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderResponseInvalid => "provider_response_invalid",
            Self::RateLimited => "rate_limited",
            Self::Canceled => "canceled",
            Self::JobError => "job_error",
            Self::StorageError => "storage_error",
            Self::WorkflowError => "workflow_error",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOperation {
    GetProviderCatalog,
    GetProviderCredentialStatus,
    SetProviderCredential,
    DeleteProviderCredential,
    EnqueueScriptStage,
    ExecuteProviderRequest,
}

impl ProviderOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GetProviderCatalog => "get_provider_catalog",
            Self::GetProviderCredentialStatus => "get_provider_credential_status",
            Self::SetProviderCredential => "set_provider_credential",
            Self::DeleteProviderCredential => "delete_provider_credential",
            Self::EnqueueScriptStage => "enqueue_script_stage",
            Self::ExecuteProviderRequest => "execute_provider_request",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub operation: ProviderOperation,
    pub message: String,
    pub retryable: bool,
    pub provider_id: Option<String>,
}

impl ProviderError {
    pub fn new(
        code: ProviderErrorCode,
        operation: ProviderOperation,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            operation,
            message: message.into(),
            retryable,
            provider_id: None,
        }
    }

    pub fn for_provider(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProviderError {}
