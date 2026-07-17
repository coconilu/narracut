use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelCapabilityData {
    pub model_id: String,
    pub display_name: String,
    pub supported_tasks: Vec<String>,
    pub structured_outputs: bool,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilityData {
    pub provider_id: String,
    pub display_name: String,
    pub transport: String,
    pub credential_storage: String,
    pub supports_streaming: bool,
    pub supports_cancellation: bool,
    pub reports_usage: bool,
    pub default_model: String,
    pub models: Vec<ProviderModelCapabilityData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogData {
    pub api_version: String,
    pub message_type: String,
    pub providers: Vec<ProviderCapabilityData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialStatusData {
    pub api_version: String,
    pub message_type: String,
    pub provider_id: String,
    pub configured: bool,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialMutationData {
    pub api_version: String,
    pub message_type: String,
    pub provider_id: String,
    pub action: String,
    pub configured: bool,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInputArtifactData {
    pub artifact_id: String,
    pub kind: String,
    pub content_hash: String,
    pub source_run_id: String,
    pub review_record_id: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptGenerationConfigData {
    pub language: String,
    pub max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredProviderRequestData {
    pub api_version: String,
    pub message_type: String,
    pub provider_request_id: String,
    pub provider_id: String,
    pub model: String,
    pub task: String,
    pub project_id: String,
    pub stage_id: String,
    pub run_id: String,
    pub inputs: Vec<ProviderInputArtifactData>,
    pub config: ScriptGenerationConfigData,
    pub output_schema_version: String,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptSegmentData {
    pub segment_id: String,
    pub order: u32,
    pub title: String,
    pub narration: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredScriptOutputData {
    pub schema_version: String,
    pub title: String,
    pub language: String,
    pub summary: String,
    pub estimated_duration_seconds: f64,
    pub segments: Vec<ScriptSegmentData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageData {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredProviderResultData {
    pub api_version: String,
    pub message_type: String,
    pub provider_request_id: String,
    pub provider_id: String,
    pub model: String,
    pub response_id: String,
    pub status: String,
    pub output: StructuredScriptOutputData,
    pub usage: ProviderUsageData,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderExecutionData {
    pub result: StructuredProviderResultData,
}
