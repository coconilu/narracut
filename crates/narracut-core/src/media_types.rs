use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{PcmWavParseLimits, SrtParseLimits};

pub trait MediaClock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug, Default)]
pub struct SystemMediaClock;

impl MediaClock for SystemMediaClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

pub(crate) fn system_media_clock() -> Arc<dyn MediaClock> {
    Arc::new(SystemMediaClock)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationRecordInputData {
    pub authorization_record_id: String,
    pub authorization_type: String,
    pub grantor: String,
    pub scope: String,
    pub evidence_ref: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAuthorizationApplicabilityData {
    pub applicability: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRightsData {
    pub ownership: String,
    pub author: String,
    pub rights_statement: String,
    pub license_id: String,
    pub attribution_text: String,
    pub authorization_records: Vec<AuthorizationRecordInputData>,
    pub voice_authorization: VoiceAuthorizationApplicabilityData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenArtifactInputData {
    pub stage_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub review_record_id: String,
    pub claim_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportAudioOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub source_path: String,
    pub expected_source_content_hash: Option<String>,
    pub script_input: FrozenArtifactInputData,
    pub rights: MediaRightsData,
    pub limits: PcmWavParseLimits,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportCaptionsOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub source_path: String,
    pub expected_source_content_hash: Option<String>,
    pub script_input: FrozenArtifactInputData,
    pub audio_input: FrozenArtifactInputData,
    pub audio_duration_ms: u64,
    pub rights: MediaRightsData,
    pub limits: SrtParseLimits,
    pub config_snapshot: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateScenePlanOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub research_input: FrozenArtifactInputData,
    pub script_input: FrozenArtifactInputData,
    pub captions_input: FrozenArtifactInputData,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateTimelineOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub audio_input: FrozenArtifactInputData,
    pub captions_input: FrozenArtifactInputData,
    pub scene_plan_input: FrozenArtifactInputData,
    pub canvas: TimelineCanvasData,
    pub safe_area: TimelineSafeAreaData,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveScenePlanOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub base_artifact_id: String,
    pub edits: Vec<ScenePlanEditData>,
    pub change_summary: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveTimelineOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub base_artifact_id: String,
    pub edits: Vec<TimelineEditData>,
    pub change_summary: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetMediaDocumentOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub artifact_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReauthorizeMediaOptions {
    pub project_path: String,
    pub expected_project_id: String,
    pub run_id: String,
    pub base_artifact_id: String,
    pub rights: MediaRightsData,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaImportResultData {
    pub owner_project_id: String,
    pub run_id: String,
    pub raw_artifact_id: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub document: Value,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDocumentReadResultData {
    pub owner_project_id: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub document: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSaveResultData {
    pub api_version: String,
    pub operation: String,
    pub owner_project_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub changed_scene_ids: Vec<String>,
    pub stale_because_stage_ids: Vec<String>,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ScenePlanEditData {
    Split {
        scene_id: String,
        boundary_cue_id: String,
    },
    Merge {
        first_scene_id: String,
        second_scene_id: String,
    },
    Update {
        scene_id: String,
        title: Option<String>,
        narrative_role: Option<String>,
    },
    MoveBoundary {
        left_scene_id: String,
        right_scene_id: String,
        boundary_cue_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildScenePlanOptions {
    pub captions_document: Value,
    pub audio_duration_ms: u64,
    pub input_refs: Vec<Value>,
    pub config_snapshot: Value,
    pub project_id: String,
    pub run_id: String,
    pub stable_seed: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineCanvasData {
    pub width: u32,
    pub height: u32,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineSafeAreaData {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TimelineEditData {
    MoveSceneBoundary {
        left_scene_id: String,
        right_scene_id: String,
        boundary_ms: u64,
    },
    SetSafeArea {
        safe_area: TimelineSafeAreaData,
    },
    SetCaptionVisibility {
        visible: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildTimelineOptions {
    pub audio_document: Value,
    pub captions_document: Value,
    pub scene_plan_document: Value,
    pub audio_input: FrozenArtifactInputData,
    pub captions_input: FrozenArtifactInputData,
    pub scene_plan_input: FrozenArtifactInputData,
    pub canvas: TimelineCanvasData,
    pub safe_area: TimelineSafeAreaData,
    pub config_snapshot: Value,
    pub project_id: String,
    pub run_id: String,
    pub stable_seed: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyTimelineEditsOptions {
    pub base_timeline_document: Value,
    pub edits: Vec<TimelineEditData>,
    pub change_summary: String,
    pub new_run_id: String,
    pub new_timeline_id: String,
    pub created_at: String,
    pub base_artifact_id: String,
}
