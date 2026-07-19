use narracut_contracts::ArtifactDraft;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct StoreArtifactFileOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub source_path: String,
    pub artifact: ArtifactDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageMediaSourceFileOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub source_path: String,
    pub expected_content_hash: Option<String>,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveStagedMediaSourceOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub staged_source_uri: String,
    pub expected_content_hash: String,
    pub expected_byte_length: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedMediaSourceData {
    pub owner_project_id: String,
    pub staged_source_uri: String,
    pub source_file_name: String,
    pub content_hash: String,
    pub byte_length: u64,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStagedMediaSourceData {
    pub owner_project_id: String,
    pub staged_source_uri: String,
    pub source_path: String,
    pub source_file_name: String,
    pub content_hash: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageIndexStatusData {
    UpToDate,
    RebuildRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCommitResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub artifact: Value,
    pub metadata_uri: String,
    pub content_uri: String,
    pub deduplicated: bool,
    pub index_status: StorageIndexStatusData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCommitPlanEntryData {
    pub artifact_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCommitJournalStatusData {
    Pending,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCommitJournalData {
    pub document_type: String,
    pub document_version: String,
    pub project_id: String,
    pub job_id: String,
    pub run_id: String,
    pub created_at: String,
    pub status: ArtifactCommitJournalStatusData,
    pub entries: Vec<ArtifactCommitPlanEntryData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactReadResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub artifact: Value,
    pub metadata_uri: String,
    pub content_uri: String,
    pub content_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactVerificationStatusData {
    Verified,
    MissingContent,
    HashMismatch,
    ByteLengthMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactVerificationResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub artifact_id: String,
    pub status: ArtifactVerificationStatusData,
    pub expected_content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_content_hash: Option<String>,
    pub expected_byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_byte_length: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIndexRebuildResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub artifacts_indexed: u64,
    pub missing_content_count: u64,
    pub index_status: StorageIndexStatusData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectData {
    pub project_id: String,
    pub project_path: String,
    pub name: String,
    pub workflow_definition_id: String,
    pub project_format_version: u32,
    pub archived: bool,
    pub last_opened_at: String,
    pub marker_updated_at: String,
    pub path_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectsResultData {
    pub api_version: String,
    pub projects: Vec<RecentProjectData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexedJobStatusData {
    Queued,
    Running,
    Retrying,
    Succeeded,
    Failed,
    Canceled,
}

impl IndexedJobStatusData {
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

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "retrying" => Some(Self::Retrying),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedJobData {
    pub owner_project_id: String,
    pub job_id: String,
    pub stage_run_id: String,
    pub stage_id: String,
    pub status: IndexedJobStatusData,
    pub attempt: u32,
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedJobUpsertData {
    pub job_id: String,
    pub stage_run_id: String,
    pub stage_id: String,
    pub status: IndexedJobStatusData,
    pub attempt: u32,
    pub progress: f64,
    pub message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListIndexedJobsOptions {
    pub owner_project_id: Option<String>,
    pub statuses: Vec<IndexedJobStatusData>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedJobsResultData {
    pub api_version: String,
    pub jobs: Vec<IndexedJobData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetProjectResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCleanupResultData {
    pub api_version: String,
    pub owner_project_id: String,
    pub entries_removed: u64,
    pub bytes_removed: u64,
}
