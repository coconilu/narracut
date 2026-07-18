use narracut_renderer::{RenderCanvas, RenderEncoding, RendererIdentity};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererTimelineInputData {
    pub stage_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub review_record_id: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderConfigData {
    pub canvas: RenderCanvas,
    pub video_codec: String,
    pub audio_codec: String,
    pub pixel_format: String,
    pub preset: String,
    pub crf: u8,
    pub max_duration_ms: u64,
    pub max_temporary_bytes: u64,
    pub timeout_ms: u64,
}

impl RenderConfigData {
    pub fn encoding(&self) -> RenderEncoding {
        RenderEncoding {
            preset: self.preset.clone(),
            crf: self.crf,
            timeout_ms: self.timeout_ms,
            max_temporary_bytes: self.max_temporary_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum RenderTargetData {
    Scene { scene_id: String },
    Timeline,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSceneSnapshotOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub timeline_input: RendererTimelineInputData,
    pub scene_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRenderOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub timeline_input: RendererTimelineInputData,
    pub target: RenderTargetData,
    pub config: RenderConfigData,
    /// Frozen when the request is accepted. Recovery and retry must execute with
    /// the same executable and capability identity instead of silently drifting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renderer_identity: Option<RendererIdentity>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneSnapshotData {
    pub snapshot_version: String,
    pub snapshot_id: String,
    pub project_id: String,
    pub timeline_artifact_id: String,
    pub timeline_content_hash: String,
    pub scene_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub canvas: RenderCanvas,
    pub safe_area: Value,
    pub title: String,
    pub narrative_role: String,
    pub caption_cue_ids: Vec<String>,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub csp: String,
    pub resource_uris: Vec<String>,
    pub html: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRenderData {
    pub owner_project_id: String,
    pub run_id: String,
    pub timeline_input: RendererTimelineInputData,
    pub config: RenderConfigData,
    pub target: RenderTargetData,
    pub snapshots: Vec<SceneSnapshotData>,
    pub audio_bytes: Vec<u8>,
    pub source_artifact_ids: Vec<String>,
    pub provenance: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderEnqueueResultData {
    pub api_version: String,
    pub operation: String,
    pub owner_project_id: String,
    pub run_id: String,
    pub job_id: String,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitRenderOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub job_id: String,
    pub prepared: PreparedRenderData,
    pub renderer_identity: RendererIdentity,
    pub rendered_file_path: String,
    pub process_result: narracut_renderer::RenderProcessResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderCommitResultData {
    pub owner_project_id: String,
    pub run_id: String,
    pub artifact_ids: Vec<String>,
    pub video_artifact_id: String,
    pub result_artifact_id: String,
    pub result: Value,
    pub log_summary: Value,
}
