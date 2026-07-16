use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectOptions {
    pub parent_path: String,
    pub directory_name: String,
    pub name: String,
    pub workflow_definition_id: String,
    pub default_locale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyProjectOptions {
    pub source_project_path: String,
    pub destination_parent_path: String,
    pub directory_name: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDescriptorData {
    pub api_version: String,
    pub project_path: String,
    pub marker_path: String,
    pub project_id: String,
    pub name: String,
    pub workflow_definition_id: String,
    pub project_format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_locale: Option<String>,
    pub archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copied_from_project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copied_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ProjectMigrationStatusData {
    Current {
        format_version: u32,
    },
    Required {
        from_version: u32,
        to_version: u32,
        steps: Vec<String>,
    },
    UnsupportedNewer {
        detected_version: u32,
        supported_version: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInspectionData {
    pub api_version: String,
    pub project_path: String,
    pub marker_path: String,
    pub detected_format_version: u32,
    pub current_format_version: u32,
    pub migration: ProjectMigrationStatusData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectDescriptorData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMigrationResultData {
    pub api_version: String,
    pub project: ProjectDescriptorData,
    pub from_version: u32,
    pub to_version: u32,
    pub applied_steps: Vec<String>,
    pub backup_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCopyResultData {
    pub api_version: String,
    pub project: ProjectDescriptorData,
    pub source_project_id: String,
    pub history_policy: String,
    pub files_copied: u64,
    pub bytes_copied: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrashResultData {
    pub api_version: String,
    pub project_id: String,
    pub trashed_path: String,
}
