use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRenderInputData {
    pub stage_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub result_artifact_id: String,
    pub content_hash: String,
    pub review_record_id: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunExportQaOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub render_input: ExportRenderInputData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueExportOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub render_input: ExportRenderInputData,
    pub qa_hash: String,
    pub destination_directory: String,
    pub export_name: String,
    pub idempotency_key: String,
    pub max_temporary_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedExportData {
    pub options: EnqueueExportOptions,
    pub qa_result: Value,
    pub render_result: Value,
    pub adopted_artifacts: Vec<Value>,
    pub source_documents: Vec<(Value, Value)>,
    pub video_content_uri: String,
    pub video_byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEnqueueResultData {
    pub api_version: String,
    pub operation: String,
    pub owner_project_id: String,
    pub run_id: String,
    pub job_id: String,
    pub status: String,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportCommitResultData {
    pub owner_project_id: String,
    pub run_id: String,
    pub export_id: String,
    pub export_path: String,
    pub artifact_ids: Vec<String>,
    pub manifest: Value,
    pub manifest_hash: String,
    pub result: Value,
    pub log_summary: Value,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTransferAbort {
    Canceled,
    LeaseLost,
}

pub trait ExportTransferObserver: Send + Sync {
    fn checkpoint(
        &self,
        phase: &str,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), ExportTransferAbort>;
}

#[derive(Debug, Default)]
pub struct NoopExportTransferObserver;

impl ExportTransferObserver for NoopExportTransferObserver {
    fn checkpoint(
        &self,
        _phase: &str,
        _completed_bytes: u64,
        _total_bytes: u64,
    ) -> Result<(), ExportTransferAbort> {
        Ok(())
    }
}
