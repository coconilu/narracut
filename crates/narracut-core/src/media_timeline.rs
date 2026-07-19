use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use narracut_contracts::{validate_media_document, NARRACUT_MEDIA_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    validate_scene_plan_semantics, ApplyTimelineEditsOptions, BuildTimelineOptions,
    FrozenArtifactInputData, TimelineCanvasData, TimelineEditData, TimelineSafeAreaData,
};

const MAX_AUDIO_DURATION_MS: u64 = 86_400_000;
const MAX_TIMELINE_SCENES: usize = 10_000;
const MAX_TIMELINE_CUES: usize = 10_000;
const MAX_TIMELINE_EDITS: usize = 1_000;
const MAX_CANVAS_DIMENSION: u32 = 16_384;
const MAX_FRAME_RATE_NUMERATOR: u32 = 240_000;
const MAX_FRAME_RATE_DENOMINATOR: u32 = 10_000;
const MAX_FRAME_RATE: u64 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineDomainErrorCode {
    InvalidRequest,
    InvalidInput,
    InvalidTimeline,
    InvalidEdit,
    ResourceLimitExceeded,
    ContractViolation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineDomainError {
    pub code: TimelineDomainErrorCode,
    pub message: String,
}

impl TimelineDomainError {
    fn new(code: TimelineDomainErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for TimelineDomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TimelineDomainError {}

#[derive(Debug, Clone)]
struct CaptionCueClosure {
    cue_id: String,
    start_ms: u64,
    end_ms: u64,
    claim_ids: Vec<String>,
    evidence_refs: Vec<String>,
}

pub fn build_timeline_document(
    options: BuildTimelineOptions,
) -> Result<Value, TimelineDomainError> {
    validate_build_options(&options)?;
    validate_source_document_contracts(&options)?;
    validate_source_identities(&options)?;

    let duration_ms = required_duration(
        &options.audio_document,
        TimelineDomainErrorCode::InvalidInput,
    )?;
    let cues = validated_caption_cues(&options.captions_document, duration_ms)?;
    validate_scene_plan_semantics(&options.scene_plan_document, duration_ms).map_err(|error| {
        TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidInput,
            format!("ScenePlanDocument 语义无效：{}", error.message),
        )
    })?;
    validate_scene_plan_against_captions(&options.scene_plan_document, &cues, duration_ms)?;

    let scene_track = options.scene_plan_document["scenes"]
        .as_array()
        .expect("validated ScenePlanDocument has scenes")
        .iter()
        .map(|scene| {
            json!({
                "sceneId": scene["sceneId"],
                "startMs": scene["suggestedStartMs"],
                "endMs": scene["suggestedEndMs"],
            })
        })
        .collect::<Vec<_>>();
    let cue_ids = cues
        .iter()
        .map(|cue| Value::String(cue.cue_id.clone()))
        .collect::<Vec<_>>();
    let mut changed_scene_ids = scene_track
        .iter()
        .filter_map(|scene| scene["sceneId"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    changed_scene_ids.sort();
    changed_scene_ids.dedup();
    let input_refs = vec![
        frozen_input_document(&options.project_id, &options.audio_input),
        frozen_input_document(&options.project_id, &options.captions_input),
        frozen_input_document(&options.project_id, &options.scene_plan_input),
    ];
    let stable_request = json!({
        "version": 1,
        "projectId": options.project_id,
        "runId": options.run_id,
        "stableSeed": options.stable_seed,
        "audioDocumentHash": canonical_hash(&options.audio_document),
        "captionsDocumentHash": canonical_hash(&options.captions_document),
        "scenePlanDocumentHash": canonical_hash(&options.scene_plan_document),
        "inputRefs": input_refs,
        "canvas": options.canvas,
        "safeArea": options.safe_area,
        "configSnapshot": options.config_snapshot,
    });
    let timeline_id = stable_timeline_id(&stable_request);
    let document = json!({
        "schemaVersion": NARRACUT_MEDIA_SCHEMA_VERSION,
        "documentType": "timeline",
        "timelineId": timeline_id,
        "projectId": options.project_id,
        "runId": options.run_id,
        "durationMs": duration_ms,
        "canvas": options.canvas,
        "audioTrack": {
            "audioArtifactId": options.audio_input.artifact_id,
            "startMs": 0,
            "endMs": duration_ms,
        },
        "sceneTrack": scene_track,
        "captionTrack": {
            "captionsArtifactId": options.captions_input.artifact_id,
            "cueIds": cue_ids,
            "visible": true,
        },
        "safeArea": options.safe_area,
        "inputRefs": input_refs,
        "configSnapshot": options.config_snapshot,
        "changeSummary": {
            "summary": "Deterministic timeline assembled from approved structured media inputs.",
            "changedSceneIds": changed_scene_ids,
        },
        "createdAt": options.created_at,
    });
    validate_media_document(&document).map_err(|_| {
        TimelineDomainError::new(
            TimelineDomainErrorCode::ContractViolation,
            "生成的 TimelineDocument 未通过 media v1 契约。",
        )
    })?;
    validate_timeline_semantics(&document)?;
    validate_timeline_against_sources(&document, &options, &cues)?;
    Ok(document)
}

pub fn apply_timeline_edits(
    options: ApplyTimelineEditsOptions,
) -> Result<Value, TimelineDomainError> {
    validate_timeline_semantics(&options.base_timeline_document)?;
    validate_apply_options(&options)?;
    let mut candidate = options.base_timeline_document.clone();
    let mut changed_scene_ids = BTreeSet::new();

    for edit in &options.edits {
        match edit {
            TimelineEditData::MoveSceneBoundary {
                left_scene_id,
                right_scene_id,
                boundary_ms,
            } => move_scene_boundary(
                &mut candidate,
                left_scene_id,
                right_scene_id,
                *boundary_ms,
                &mut changed_scene_ids,
            )?,
            TimelineEditData::SetSafeArea { safe_area } => {
                let canvas = timeline_canvas(&candidate)?;
                validate_safe_area(*safe_area, canvas)?;
                candidate["safeArea"] = serde_json::to_value(safe_area)
                    .expect("TimelineSafeAreaData serialization cannot fail");
            }
            TimelineEditData::SetCaptionVisibility { visible } => {
                candidate["captionTrack"]["visible"] = json!(visible);
            }
        }
    }

    candidate["timelineId"] = json!(options.new_timeline_id);
    candidate["schemaVersion"] = json!(NARRACUT_MEDIA_SCHEMA_VERSION);
    candidate["runId"] = json!(options.new_run_id);
    candidate["supersedesArtifactId"] = json!(options.base_artifact_id);
    candidate["changeSummary"] = json!({
        "summary": options.change_summary,
        "changedSceneIds": changed_scene_ids.into_iter().collect::<Vec<_>>(),
    });
    candidate["createdAt"] = json!(options.created_at);
    validate_media_document(&candidate).map_err(|_| {
        TimelineDomainError::new(
            TimelineDomainErrorCode::ContractViolation,
            "编辑后的 TimelineDocument 未通过 media v1 契约。",
        )
    })?;
    validate_timeline_semantics(&candidate)?;
    Ok(candidate)
}

pub fn validate_timeline_semantics(document: &Value) -> Result<(), TimelineDomainError> {
    validate_media_document(document).map_err(|_| {
        TimelineDomainError::new(
            TimelineDomainErrorCode::ContractViolation,
            "TimelineDocument 未通过 media v1 契约。",
        )
    })?;
    if document["documentType"].as_str() != Some("timeline") {
        return Err(invalid_timeline("文档不是 TimelineDocument。"));
    }
    let project_id = required_string(
        document,
        "projectId",
        TimelineDomainErrorCode::InvalidTimeline,
    )?;
    let run_id = required_string(document, "runId", TimelineDomainErrorCode::InvalidTimeline)?;
    let timeline_id = required_string(
        document,
        "timelineId",
        TimelineDomainErrorCode::InvalidTimeline,
    )?;
    if !portable_id(&project_id) || !run_id_is_valid(&run_id) || !portable_id(&timeline_id) {
        return Err(invalid_timeline("Timeline 项目、运行或文档身份无效。"));
    }
    if !bounded_text(document["createdAt"].as_str().unwrap_or_default(), 20, 40)
        || !document["configSnapshot"].is_object()
    {
        return Err(invalid_timeline("Timeline 时间戳或配置快照无效。"));
    }
    let duration_ms = required_duration(document, TimelineDomainErrorCode::InvalidTimeline)?;
    let canvas = timeline_canvas(document)?;
    let safe_area = timeline_safe_area(document)?;
    validate_canvas(canvas)?;
    validate_safe_area(safe_area, canvas)?;

    let inputs = validated_timeline_input_refs(document, &project_id)?;
    let audio_input = required_input_for_stage(&inputs, "audio")?;
    let captions_input = required_input_for_stage(&inputs, "captions")?;
    required_input_for_stage(&inputs, "scene_plan")?;
    if document
        .pointer("/audioTrack/audioArtifactId")
        .and_then(Value::as_str)
        != Some(audio_input.artifact_id.as_str())
        || document
            .pointer("/audioTrack/startMs")
            .and_then(Value::as_u64)
            != Some(0)
        || document
            .pointer("/audioTrack/endMs")
            .and_then(Value::as_u64)
            != Some(duration_ms)
    {
        return Err(invalid_timeline(
            "audioTrack 必须精确引用批准 Audio Artifact 并覆盖完整时长。",
        ));
    }
    if document
        .pointer("/captionTrack/captionsArtifactId")
        .and_then(Value::as_str)
        != Some(captions_input.artifact_id.as_str())
    {
        return Err(invalid_timeline(
            "captionTrack 未精确引用批准 Captions Artifact。",
        ));
    }
    let cue_ids = document
        .pointer("/captionTrack/cueIds")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= MAX_TIMELINE_CUES)
        .ok_or_else(|| invalid_timeline("captionTrack cueIds 缺失、为空或超过上限。"))?;
    let mut seen_cues = BTreeSet::new();
    if cue_ids.iter().any(|value| {
        value
            .as_str()
            .is_none_or(|id| !portable_id(id) || !seen_cues.insert(id.to_owned()))
    }) {
        return Err(invalid_timeline(
            "captionTrack cueIds 必须有界、可移植、唯一且保持顺序。",
        ));
    }
    let scene_ids = validate_scene_track(document, duration_ms)?;
    validate_changed_scene_ids(document, &scene_ids)?;
    Ok(())
}

fn validate_build_options(options: &BuildTimelineOptions) -> Result<(), TimelineDomainError> {
    if !portable_id(&options.project_id)
        || !run_id_is_valid(&options.run_id)
        || !bounded_text(&options.stable_seed, 1, 512)
        || !bounded_text(&options.created_at, 20, 40)
        || !options.config_snapshot.is_object()
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "Timeline 构建身份、稳定种子、时间戳或配置无效。",
        ));
    }
    validate_canvas(options.canvas)?;
    validate_safe_area(options.safe_area, options.canvas)?;
    validate_frozen_input(&options.audio_input, "audio")?;
    validate_frozen_input(&options.captions_input, "captions")?;
    validate_frozen_input(&options.scene_plan_input, "scene_plan")?;
    let unique_artifacts = [
        &options.audio_input.artifact_id,
        &options.captions_input.artifact_id,
        &options.scene_plan_input.artifact_id,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if unique_artifacts.len() != 3 {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "Timeline 三个冻结输入必须引用不同 Artifact。",
        ));
    }
    Ok(())
}

fn validate_source_document_contracts(
    options: &BuildTimelineOptions,
) -> Result<(), TimelineDomainError> {
    for document in [
        &options.audio_document,
        &options.captions_document,
        &options.scene_plan_document,
    ] {
        validate_media_document(document).map_err(|_| {
            TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Timeline 输入未通过 media v1 契约。",
            )
        })?;
    }
    Ok(())
}

fn validate_source_identities(options: &BuildTimelineOptions) -> Result<(), TimelineDomainError> {
    for (document, document_type, input) in [
        (&options.audio_document, "audio_media", &options.audio_input),
        (
            &options.captions_document,
            "captions_media",
            &options.captions_input,
        ),
        (
            &options.scene_plan_document,
            "scene_plan",
            &options.scene_plan_input,
        ),
    ] {
        if document["documentType"].as_str() != Some(document_type)
            || document["projectId"].as_str() != Some(options.project_id.as_str())
            || document["runId"].as_str() != Some(input.run_id.as_str())
        {
            return Err(TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Timeline 输入文档的类型、项目或运行身份与冻结引用不一致。",
            ));
        }
    }
    let audio_ref = frozen_input_document(&options.project_id, &options.audio_input);
    if options.captions_document.get("audioInput") != Some(&audio_ref)
        || !document_has_frozen_input(&options.captions_document, &audio_ref)
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidInput,
            "CaptionsMediaDocument 未闭包引用批准 Audio 输入。",
        ));
    }
    let captions_ref = frozen_input_document(&options.project_id, &options.captions_input);
    if !document_has_frozen_input(&options.scene_plan_document, &captions_ref) {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidInput,
            "ScenePlanDocument 未闭包引用批准 Captions 输入。",
        ));
    }
    Ok(())
}

fn validated_caption_cues(
    captions: &Value,
    duration_ms: u64,
) -> Result<Vec<CaptionCueClosure>, TimelineDomainError> {
    let cues = captions["cues"]
        .as_array()
        .filter(|values| !values.is_empty() && values.len() <= MAX_TIMELINE_CUES)
        .ok_or_else(|| {
            TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Captions cue 清单缺失、为空或超过上限。",
            )
        })?;
    let mut result = Vec::with_capacity(cues.len());
    let mut seen = BTreeSet::new();
    let mut previous_end = 0;
    for (index, cue) in cues.iter().enumerate() {
        let cue_id = required_string(cue, "cueId", TimelineDomainErrorCode::InvalidInput)?;
        let start_ms = required_u64(cue, "startMs", TimelineDomainErrorCode::InvalidInput)?;
        let end_ms = required_u64(cue, "endMs", TimelineDomainErrorCode::InvalidInput)?;
        if cue["sourceIndex"].as_u64() != Some(index as u64 + 1)
            || !portable_id(&cue_id)
            || !seen.insert(cue_id.clone())
            || start_ms < previous_end
            || start_ms >= end_ms
            || end_ms > duration_ms
        {
            return Err(TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Captions cue 必须连续编号、唯一、有序、不重叠且位于音频范围内。",
            ));
        }
        let claim_ids = string_set(cue, "claimIds", TimelineDomainErrorCode::InvalidInput)?;
        let evidence_refs = string_set(cue, "evidenceRefs", TimelineDomainErrorCode::InvalidInput)?;
        result.push(CaptionCueClosure {
            cue_id,
            start_ms,
            end_ms,
            claim_ids,
            evidence_refs,
        });
        previous_end = end_ms;
    }
    Ok(result)
}

fn validate_scene_plan_against_captions(
    scene_plan: &Value,
    cues: &[CaptionCueClosure],
    duration_ms: u64,
) -> Result<(), TimelineDomainError> {
    let cue_by_id = cues
        .iter()
        .map(|cue| (cue.cue_id.as_str(), cue))
        .collect::<BTreeMap<_, _>>();
    let scenes = scene_plan["scenes"]
        .as_array()
        .filter(|values| !values.is_empty() && values.len() <= MAX_TIMELINE_SCENES)
        .ok_or_else(|| {
            TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Scene Plan scene 清单缺失、为空或超过上限。",
            )
        })?;
    let mut actual_cues = Vec::new();
    for scene in scenes {
        let start_ms = required_u64(
            scene,
            "suggestedStartMs",
            TimelineDomainErrorCode::InvalidInput,
        )?;
        let end_ms = required_u64(
            scene,
            "suggestedEndMs",
            TimelineDomainErrorCode::InvalidInput,
        )?;
        if end_ms > duration_ms {
            return Err(TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Scene Plan scene 超出 Audio duration。",
            ));
        }
        let cue_ids = scene["cueIds"].as_array().ok_or_else(|| {
            TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Scene Plan scene 缺少 cueIds。",
            )
        })?;
        let mut scene_cues = Vec::with_capacity(cue_ids.len());
        for value in cue_ids {
            let cue_id = value.as_str().ok_or_else(|| {
                TimelineDomainError::new(
                    TimelineDomainErrorCode::InvalidInput,
                    "Scene Plan cue 引用无效。",
                )
            })?;
            let cue = cue_by_id.get(cue_id).ok_or_else(|| {
                TimelineDomainError::new(
                    TimelineDomainErrorCode::InvalidInput,
                    "Scene Plan 引用了 Captions 中不存在的 cue。",
                )
            })?;
            if cue.start_ms < start_ms || cue.end_ms > end_ms {
                return Err(TimelineDomainError::new(
                    TimelineDomainErrorCode::InvalidInput,
                    "Scene Plan cue 不位于所属 scene 的时间范围内。",
                ));
            }
            actual_cues.push(cue_id.to_owned());
            scene_cues.push(*cue);
        }
        if ordered_trace_values(&scene_cues, |cue| &cue.claim_ids)
            != string_set(scene, "claimIds", TimelineDomainErrorCode::InvalidInput)?
            || ordered_trace_values(&scene_cues, |cue| &cue.evidence_refs)
                != string_set(scene, "evidenceRefs", TimelineDomainErrorCode::InvalidInput)?
        {
            return Err(TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidInput,
                "Scene Plan scene 追溯集合与所属 Captions cue 不一致。",
            ));
        }
    }
    let expected_cues = cues
        .iter()
        .map(|cue| cue.cue_id.clone())
        .collect::<Vec<_>>();
    if actual_cues != expected_cues {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidInput,
            "Scene Plan cue 引用必须完整且与 Captions 顺序一致。",
        ));
    }
    Ok(())
}

fn validate_timeline_against_sources(
    timeline: &Value,
    options: &BuildTimelineOptions,
    cues: &[CaptionCueClosure],
) -> Result<(), TimelineDomainError> {
    let expected_inputs = vec![
        frozen_input_document(&options.project_id, &options.audio_input),
        frozen_input_document(&options.project_id, &options.captions_input),
        frozen_input_document(&options.project_id, &options.scene_plan_input),
    ];
    if timeline["inputRefs"] != Value::Array(expected_inputs) {
        return Err(invalid_timeline(
            "Timeline inputRefs 未精确冻结 Audio/Captions/ScenePlan 输入。",
        ));
    }
    let expected_scenes = options.scene_plan_document["scenes"]
        .as_array()
        .expect("validated Scene Plan scenes")
        .iter()
        .map(|scene| {
            json!({
                "sceneId": scene["sceneId"],
                "startMs": scene["suggestedStartMs"],
                "endMs": scene["suggestedEndMs"],
            })
        })
        .collect::<Vec<_>>();
    if timeline["sceneTrack"] != Value::Array(expected_scenes) {
        return Err(invalid_timeline(
            "Timeline sceneTrack 未精确投影 ScenePlanDocument 边界。",
        ));
    }
    let expected_cues = cues
        .iter()
        .map(|cue| Value::String(cue.cue_id.clone()))
        .collect::<Vec<_>>();
    if timeline["captionTrack"]["cueIds"] != Value::Array(expected_cues) {
        return Err(invalid_timeline(
            "Timeline captionTrack 未完整有序引用 Captions cue。",
        ));
    }
    Ok(())
}

fn validate_apply_options(options: &ApplyTimelineEditsOptions) -> Result<(), TimelineDomainError> {
    if options.edits.is_empty()
        || options.edits.len() > MAX_TIMELINE_EDITS
        || !bounded_text(&options.change_summary, 1, 2_048)
        || !run_id_is_valid(&options.new_run_id)
        || !portable_id(&options.new_timeline_id)
        || !bounded_text(&options.created_at, 20, 40)
        || !artifact_id_is_valid(&options.base_artifact_id)
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "Timeline 编辑请求无效或超过资源上限。",
        ));
    }
    if options.base_timeline_document["runId"].as_str() == Some(options.new_run_id.as_str())
        || options.base_timeline_document["timelineId"].as_str()
            == Some(options.new_timeline_id.as_str())
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "编辑后的 Timeline 必须使用新的 runId 与 timelineId。",
        ));
    }
    Ok(())
}

fn move_scene_boundary(
    timeline: &mut Value,
    left_scene_id: &str,
    right_scene_id: &str,
    boundary_ms: u64,
    changed: &mut BTreeSet<String>,
) -> Result<(), TimelineDomainError> {
    if !portable_id(left_scene_id) || !portable_id(right_scene_id) {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidEdit,
            "Timeline scene 边界编辑引用无效。",
        ));
    }
    let scenes = timeline["sceneTrack"]
        .as_array_mut()
        .expect("validated Timeline has sceneTrack");
    let left_index = scenes
        .iter()
        .position(|scene| scene["sceneId"].as_str() == Some(left_scene_id))
        .ok_or_else(|| {
            TimelineDomainError::new(
                TimelineDomainErrorCode::InvalidEdit,
                "move_scene_boundary 左侧 Scene 不存在。",
            )
        })?;
    if scenes
        .get(left_index + 1)
        .and_then(|scene| scene["sceneId"].as_str())
        != Some(right_scene_id)
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidEdit,
            "move_scene_boundary 仅允许调整相邻左右 Scene。",
        ));
    }
    let right_index = left_index + 1;
    let combined_start = scenes[left_index]["startMs"]
        .as_u64()
        .expect("validated scene start");
    let current_boundary = scenes[left_index]["endMs"]
        .as_u64()
        .expect("validated scene end");
    let combined_end = scenes[right_index]["endMs"]
        .as_u64()
        .expect("validated scene end");
    if boundary_ms <= combined_start
        || boundary_ms >= combined_end
        || boundary_ms == current_boundary
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidEdit,
            "新 Scene 边界必须严格位于相邻两端之间且不能是当前边界。",
        ));
    }
    scenes[left_index]["endMs"] = json!(boundary_ms);
    scenes[right_index]["startMs"] = json!(boundary_ms);
    changed.insert(left_scene_id.to_owned());
    changed.insert(right_scene_id.to_owned());
    Ok(())
}

fn validate_canvas(canvas: TimelineCanvasData) -> Result<(), TimelineDomainError> {
    let numerator = u64::from(canvas.frame_rate_numerator);
    let denominator = u64::from(canvas.frame_rate_denominator);
    let rational_valid = canvas.frame_rate_numerator > 0
        && canvas.frame_rate_numerator <= MAX_FRAME_RATE_NUMERATOR
        && canvas.frame_rate_denominator > 0
        && canvas.frame_rate_denominator <= MAX_FRAME_RATE_DENOMINATOR
        && numerator >= denominator
        && numerator <= MAX_FRAME_RATE.saturating_mul(denominator);
    if !(16..=MAX_CANVAS_DIMENSION).contains(&canvas.width)
        || !(16..=MAX_CANVAS_DIMENSION).contains(&canvas.height)
        || !rational_valid
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "Timeline canvas 尺寸或 frameRate rational 无效。",
        ));
    }
    Ok(())
}

fn validate_safe_area(
    safe_area: TimelineSafeAreaData,
    canvas: TimelineCanvasData,
) -> Result<(), TimelineDomainError> {
    let right = safe_area.x.checked_add(safe_area.width);
    let bottom = safe_area.y.checked_add(safe_area.height);
    if safe_area.width == 0
        || safe_area.height == 0
        || safe_area.x > MAX_CANVAS_DIMENSION
        || safe_area.y > MAX_CANVAS_DIMENSION
        || safe_area.width > MAX_CANVAS_DIMENSION
        || safe_area.height > MAX_CANVAS_DIMENSION
        || right.is_none_or(|right| right > canvas.width)
        || bottom.is_none_or(|bottom| bottom > canvas.height)
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            "Timeline safeArea 必须无溢出地位于 canvas 范围内。",
        ));
    }
    Ok(())
}

fn validate_frozen_input(
    input: &FrozenArtifactInputData,
    expected_stage_id: &str,
) -> Result<(), TimelineDomainError> {
    if input.stage_id != expected_stage_id
        || !run_id_is_valid(&input.run_id)
        || !artifact_id_is_valid(&input.artifact_id)
        || !sha256(&input.content_hash)
        || !portable_id(&input.review_record_id)
        || !valid_string_set(&input.claim_ids)
        || !valid_string_set(&input.evidence_refs)
    {
        return Err(TimelineDomainError::new(
            TimelineDomainErrorCode::InvalidRequest,
            format!("Timeline {expected_stage_id} 冻结输入无效。"),
        ));
    }
    Ok(())
}

fn validated_timeline_input_refs(
    document: &Value,
    project_id: &str,
) -> Result<Vec<FrozenArtifactInputData>, TimelineDomainError> {
    let values = document["inputRefs"]
        .as_array()
        .filter(|values| values.len() == 3)
        .ok_or_else(|| invalid_timeline("Timeline 必须精确包含三个冻结 inputRefs。"))?;
    let mut inputs = Vec::with_capacity(3);
    let mut artifact_ids = BTreeSet::new();
    for value in values {
        if value["projectId"].as_str() != Some(project_id) {
            return Err(invalid_timeline(
                "Timeline inputRefs 必须全部属于文档项目。",
            ));
        }
        let input: FrozenArtifactInputData = serde_json::from_value(value.clone())
            .map_err(|_| invalid_timeline("Timeline inputRef 格式无效。"))?;
        validate_frozen_input(&input, &input.stage_id)
            .map_err(|_| invalid_timeline("Timeline inputRef 身份或追溯字段无效。"))?;
        if !matches!(input.stage_id.as_str(), "audio" | "captions" | "scene_plan")
            || !artifact_ids.insert(input.artifact_id.clone())
        {
            return Err(invalid_timeline(
                "Timeline inputRefs 阶段必须为 Audio/Captions/ScenePlan 且 Artifact 唯一。",
            ));
        }
        inputs.push(input);
    }
    for stage_id in ["audio", "captions", "scene_plan"] {
        required_input_for_stage(&inputs, stage_id)?;
    }
    Ok(inputs)
}

fn required_input_for_stage<'a>(
    inputs: &'a [FrozenArtifactInputData],
    stage_id: &str,
) -> Result<&'a FrozenArtifactInputData, TimelineDomainError> {
    let mut matches = inputs.iter().filter(|input| input.stage_id == stage_id);
    let input = matches
        .next()
        .ok_or_else(|| invalid_timeline(format!("Timeline 缺少 {stage_id} inputRef。")))?;
    if matches.next().is_some() {
        return Err(invalid_timeline(format!(
            "Timeline 包含重复 {stage_id} inputRef。"
        )));
    }
    Ok(input)
}

fn validate_scene_track(
    document: &Value,
    duration_ms: u64,
) -> Result<BTreeSet<String>, TimelineDomainError> {
    let scenes = document["sceneTrack"]
        .as_array()
        .filter(|values| !values.is_empty() && values.len() <= MAX_TIMELINE_SCENES)
        .ok_or_else(|| invalid_timeline("sceneTrack 缺失、为空或超过上限。"))?;
    let mut ids = BTreeSet::new();
    let mut previous_end = 0;
    for scene in scenes {
        let scene_id = required_string(scene, "sceneId", TimelineDomainErrorCode::InvalidTimeline)?;
        let start_ms = required_u64(scene, "startMs", TimelineDomainErrorCode::InvalidTimeline)?;
        let end_ms = required_u64(scene, "endMs", TimelineDomainErrorCode::InvalidTimeline)?;
        if !portable_id(&scene_id)
            || !ids.insert(scene_id)
            || start_ms != previous_end
            || start_ms >= end_ms
            || end_ms > duration_ms
        {
            return Err(invalid_timeline(
                "sceneTrack 必须 ID 唯一、按序完整覆盖音频且无洞无重叠。",
            ));
        }
        previous_end = end_ms;
    }
    if previous_end != duration_ms {
        return Err(invalid_timeline(
            "sceneTrack 未覆盖 Timeline 完整 duration。",
        ));
    }
    Ok(ids)
}

fn validate_changed_scene_ids(
    document: &Value,
    scene_ids: &BTreeSet<String>,
) -> Result<(), TimelineDomainError> {
    let summary = document["changeSummary"]["summary"]
        .as_str()
        .unwrap_or_default();
    let changed = document["changeSummary"]["changedSceneIds"]
        .as_array()
        .ok_or_else(|| invalid_timeline("changeSummary.changedSceneIds 无效。"))?;
    let changed = changed
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_timeline("changedSceneId 必须为字符串。"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut stable = changed.clone();
    stable.sort();
    stable.dedup();
    if !bounded_text(summary, 1, 2_048)
        || stable != changed
        || changed.iter().any(|scene_id| !scene_ids.contains(scene_id))
    {
        return Err(invalid_timeline(
            "changeSummary 必须有界，changedSceneIds 必须稳定去重并引用现有 Scene。",
        ));
    }
    Ok(())
}

fn timeline_canvas(document: &Value) -> Result<TimelineCanvasData, TimelineDomainError> {
    serde_json::from_value(document["canvas"].clone())
        .map_err(|_| invalid_timeline("Timeline canvas 格式无效。"))
}

fn timeline_safe_area(document: &Value) -> Result<TimelineSafeAreaData, TimelineDomainError> {
    serde_json::from_value(document["safeArea"].clone())
        .map_err(|_| invalid_timeline("Timeline safeArea 格式无效。"))
}

fn frozen_input_document(project_id: &str, input: &FrozenArtifactInputData) -> Value {
    json!({
        "projectId": project_id,
        "stageId": input.stage_id,
        "runId": input.run_id,
        "artifactId": input.artifact_id,
        "contentHash": input.content_hash,
        "reviewRecordId": input.review_record_id,
        "claimIds": input.claim_ids,
        "evidenceRefs": input.evidence_refs,
    })
}

fn document_has_frozen_input(document: &Value, expected: &Value) -> bool {
    document["inputRefs"]
        .as_array()
        .is_some_and(|values| values.iter().any(|value| value == expected))
}

fn ordered_trace_values(
    cues: &[&CaptionCueClosure],
    values: impl Fn(&CaptionCueClosure) -> &[String],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for cue in cues {
        for value in values(cue) {
            if seen.insert(value.clone()) {
                ordered.push(value.clone());
            }
        }
    }
    ordered
}

fn string_set(
    value: &Value,
    field: &str,
    code: TimelineDomainErrorCode,
) -> Result<Vec<String>, TimelineDomainError> {
    let values = value[field]
        .as_array()
        .ok_or_else(|| TimelineDomainError::new(code, format!("{field} 缺失或不是数组。")))?;
    let result = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| TimelineDomainError::new(code, format!("{field} 包含非字符串。")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !valid_string_set(&result) {
        return Err(TimelineDomainError::new(
            code,
            format!("{field} 不是有界唯一字符串集合。"),
        ));
    }
    Ok(result)
}

fn required_duration(
    value: &Value,
    code: TimelineDomainErrorCode,
) -> Result<u64, TimelineDomainError> {
    value["durationMs"]
        .as_u64()
        .filter(|duration| *duration > 0 && *duration <= MAX_AUDIO_DURATION_MS)
        .ok_or_else(|| TimelineDomainError::new(code, "durationMs 缺失或无效。"))
}

fn required_string(
    value: &Value,
    field: &str,
    code: TimelineDomainErrorCode,
) -> Result<String, TimelineDomainError> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| TimelineDomainError::new(code, format!("{field} 缺失或无效。")))
}

fn required_u64(
    value: &Value,
    field: &str,
    code: TimelineDomainErrorCode,
) -> Result<u64, TimelineDomainError> {
    value[field]
        .as_u64()
        .ok_or_else(|| TimelineDomainError::new(code, format!("{field} 缺失或无效。")))
}

fn invalid_timeline(message: impl Into<String>) -> TimelineDomainError {
    TimelineDomainError::new(TimelineDomainErrorCode::InvalidTimeline, message)
}

fn portable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn run_id_is_valid(value: &str) -> bool {
    value
        .strip_prefix("run_")
        .is_some_and(|suffix| portable_id(suffix) && value.len() <= 160)
}

fn artifact_id_is_valid(value: &str) -> bool {
    value
        .strip_prefix("artifact_")
        .is_some_and(|suffix| portable_id(suffix) && value.len() <= 160)
}

fn sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_string_set(values: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    values.len() <= 1_024
        && values
            .iter()
            .all(|value| bounded_text(value, 1, 512) && seen.insert(value.clone()))
}

fn bounded_text(value: &str, min_chars: usize, max_chars: usize) -> bool {
    let length = value.chars().count();
    (min_chars..=max_chars).contains(&length) && !value.chars().any(char::is_control)
}

fn stable_timeline_id(request: &Value) -> String {
    let digest = canonical_hash(request);
    format!("timeline_{}", &digest["sha256:".len()..])
}

fn canonical_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(&canonicalize(value)).expect("Value serialization cannot fail");
    let mut hash = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        write!(&mut hash, "{byte:02x}").expect("writing to String cannot fail");
    }
    hash
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut map = Map::new();
            for key in keys {
                map.insert(key.clone(), canonicalize(&values[key]));
            }
            Value::Object(map)
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use narracut_contracts::{parse_media_document, validate_media_document};
    use serde_json::{json, Value};

    use super::{
        apply_timeline_edits, build_timeline_document, validate_timeline_semantics,
        TimelineDomainErrorCode,
    };
    use crate::{
        build_scene_plan_document, ApplyTimelineEditsOptions, BuildScenePlanOptions,
        BuildTimelineOptions, FrozenArtifactInputData, TimelineCanvasData, TimelineEditData,
        TimelineSafeAreaData,
    };

    const PROJECT_ID: &str = "project_timeline";
    const AUDIO_DURATION_MS: u64 = 100;

    #[test]
    fn build_is_deterministic_schema_valid_traceable_and_projects_all_three_tracks() {
        let options = build_options();
        let first = build_timeline_document(options.clone()).expect("build Timeline");
        let second = build_timeline_document(options.clone()).expect("repeat Timeline");
        assert_eq!(first, second);
        validate_media_document(&first).expect("Timeline schema");
        parse_media_document(first.clone()).expect("typed Timeline roundtrip");
        validate_timeline_semantics(&first).expect("Timeline semantics");

        assert_eq!(first["durationMs"], AUDIO_DURATION_MS);
        assert_eq!(first["canvas"]["width"], 1_920);
        assert_eq!(first["canvas"]["frameRateNumerator"], 30_000);
        assert_eq!(first["canvas"]["frameRateDenominator"], 1_001);
        assert_eq!(
            first["audioTrack"],
            json!({
                "audioArtifactId": options.audio_input.artifact_id,
                "startMs": 0,
                "endMs": AUDIO_DURATION_MS,
            })
        );
        assert_eq!(
            first["captionTrack"],
            json!({
                "captionsArtifactId": options.captions_input.artifact_id,
                "cueIds": ["cue_1", "cue_2", "cue_3", "cue_4"],
                "visible": true,
            })
        );

        let scenes = first["sceneTrack"].as_array().expect("sceneTrack");
        assert_eq!(scenes.len(), 2);
        assert_eq!(scenes[0]["startMs"], 0);
        assert_eq!(
            scenes.last().expect("last Scene")["endMs"],
            AUDIO_DURATION_MS
        );
        assert!(scenes
            .windows(2)
            .all(|pair| pair[0]["endMs"] == pair[1]["startMs"]));
        for (timeline_scene, plan_scene) in scenes.iter().zip(
            options.scene_plan_document["scenes"]
                .as_array()
                .expect("Scene Plan scenes"),
        ) {
            assert_eq!(timeline_scene["sceneId"], plan_scene["sceneId"]);
            assert_eq!(timeline_scene["startMs"], plan_scene["suggestedStartMs"]);
            assert_eq!(timeline_scene["endMs"], plan_scene["suggestedEndMs"]);
            assert_eq!(timeline_scene.as_object().map(|value| value.len()), Some(3));
        }
        assert_eq!(
            first["inputRefs"]
                .as_array()
                .expect("Timeline inputs")
                .iter()
                .map(|input| input["stageId"].as_str().expect("stage"))
                .collect::<Vec<_>>(),
            vec!["audio", "captions", "scene_plan"]
        );

        let mut expected_changed = scenes
            .iter()
            .map(|scene| scene["sceneId"].as_str().expect("Scene ID").to_owned())
            .collect::<Vec<_>>();
        expected_changed.sort();
        assert_eq!(
            first["changeSummary"]["changedSceneIds"],
            json!(expected_changed)
        );
        let projected_tracks = serde_json::to_string(&json!({
            "sceneTrack": first["sceneTrack"],
            "captionTrack": first["captionTrack"],
        }))
        .expect("serialize projected tracks");
        assert!(!projected_tracks.contains("Opening evidence-backed caption."));
        assert!(!projected_tracks.contains("claim_1"));
        assert!(!projected_tracks.contains("evidence_1"));

        let mut later_time = options.clone();
        later_time.created_at = "2026-07-18T10:00:00Z".to_owned();
        let later_time = build_timeline_document(later_time).expect("stable ID across clock time");
        assert_eq!(first["timelineId"], later_time["timelineId"]);
        let mut changed_seed = options;
        changed_seed.stable_seed = "timeline-seed-v2".to_owned();
        let changed_seed = build_timeline_document(changed_seed).expect("changed stable seed");
        assert_ne!(first["timelineId"], changed_seed["timelineId"]);
    }

    #[test]
    fn edits_are_pure_sequential_atomic_and_report_only_boundary_scene_ids() {
        let base = built_timeline();
        let original = base.clone();
        let left_scene_id = scene_id(&base, 0);
        let right_scene_id = scene_id(&base, 1);
        let edited = apply_timeline_edits(ApplyTimelineEditsOptions {
            base_timeline_document: base.clone(),
            edits: vec![
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: left_scene_id.clone(),
                    right_scene_id: right_scene_id.clone(),
                    boundary_ms: 55,
                },
                TimelineEditData::SetSafeArea {
                    safe_area: TimelineSafeAreaData {
                        x: 100,
                        y: 50,
                        width: 1_600,
                        height: 900,
                    },
                },
                TimelineEditData::SetCaptionVisibility { visible: false },
            ],
            change_summary: "Move an adjacent boundary and update presentation controls."
                .to_owned(),
            new_run_id: "run_timeline_edited".to_owned(),
            new_timeline_id: "timeline_edited".to_owned(),
            created_at: "2026-07-18T09:00:00Z".to_owned(),
            base_artifact_id: "artifact_timeline_base".to_owned(),
        })
        .expect("apply three Timeline edit types");

        assert_eq!(base, original, "base Timeline must remain immutable");
        assert_eq!(edited["runId"], "run_timeline_edited");
        assert_eq!(edited["timelineId"], "timeline_edited");
        assert_eq!(edited["supersedesArtifactId"], "artifact_timeline_base");
        assert_eq!(edited["createdAt"], "2026-07-18T09:00:00Z");
        assert_eq!(edited["sceneTrack"][0]["endMs"], 55);
        assert_eq!(edited["sceneTrack"][1]["startMs"], 55);
        assert_eq!(
            edited["safeArea"],
            json!({"x":100,"y":50,"width":1600,"height":900})
        );
        assert_eq!(edited["captionTrack"]["visible"], false);
        assert_changed_ids(&edited, &[&left_scene_id, &right_scene_id]);
        validate_media_document(&edited).expect("edited Timeline schema");
        validate_timeline_semantics(&edited).expect("edited Timeline semantics");

        let sequential = apply_timeline_edits(ApplyTimelineEditsOptions {
            base_timeline_document: base.clone(),
            edits: vec![
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: left_scene_id.clone(),
                    right_scene_id: right_scene_id.clone(),
                    boundary_ms: 55,
                },
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: left_scene_id.clone(),
                    right_scene_id: right_scene_id.clone(),
                    boundary_ms: 50,
                },
            ],
            change_summary: "Apply boundary edits in sequence and deduplicate scene IDs."
                .to_owned(),
            new_run_id: "run_timeline_sequential".to_owned(),
            new_timeline_id: "timeline_sequential".to_owned(),
            created_at: "2026-07-18T09:01:00Z".to_owned(),
            base_artifact_id: "artifact_timeline_base".to_owned(),
        })
        .expect("apply sequential Timeline edits");
        assert_eq!(sequential["sceneTrack"][0]["endMs"], 50);
        assert_eq!(sequential["sceneTrack"][1]["startMs"], 50);
        assert_changed_ids(&sequential, &[&left_scene_id, &right_scene_id]);

        for (suffix, edit) in [
            (
                "safe_only",
                TimelineEditData::SetSafeArea {
                    safe_area: TimelineSafeAreaData {
                        x: 80,
                        y: 45,
                        width: 1_760,
                        height: 990,
                    },
                },
            ),
            (
                "captions_only",
                TimelineEditData::SetCaptionVisibility { visible: false },
            ),
        ] {
            let controls_only = apply_timeline_edits(ApplyTimelineEditsOptions {
                base_timeline_document: base.clone(),
                edits: vec![edit],
                change_summary: "Update one bounded presentation control.".to_owned(),
                new_run_id: format!("run_timeline_{suffix}"),
                new_timeline_id: format!("timeline_{suffix}"),
                created_at: "2026-07-18T09:02:00Z".to_owned(),
                base_artifact_id: "artifact_timeline_base".to_owned(),
            })
            .expect("apply presentation-only edit");
            assert_eq!(controls_only["changeSummary"]["changedSceneIds"], json!([]));
        }
    }

    #[test]
    fn build_rejects_invalid_canvas_safe_area_identity_duration_and_trace_closure() {
        let mut invalid_canvas = build_options();
        invalid_canvas.canvas.width = 15;
        assert_build_code(invalid_canvas, TimelineDomainErrorCode::InvalidRequest);

        let mut zero_denominator = build_options();
        zero_denominator.canvas.frame_rate_denominator = 0;
        assert_build_code(zero_denominator, TimelineDomainErrorCode::InvalidRequest);

        let mut excessive_rational_rate = build_options();
        excessive_rational_rate.canvas.frame_rate_numerator = 241;
        excessive_rational_rate.canvas.frame_rate_denominator = 1;
        assert_build_code(
            excessive_rational_rate,
            TimelineDomainErrorCode::InvalidRequest,
        );

        let mut overflowing_safe_area = build_options();
        overflowing_safe_area.safe_area.x = u32::MAX;
        overflowing_safe_area.safe_area.width = 2;
        assert_build_code(
            overflowing_safe_area,
            TimelineDomainErrorCode::InvalidRequest,
        );

        let mut outside_canvas = build_options();
        outside_canvas.safe_area.x = 1_900;
        outside_canvas.safe_area.width = 100;
        assert_build_code(outside_canvas, TimelineDomainErrorCode::InvalidRequest);

        let mut wrong_project = build_options();
        wrong_project.audio_document["projectId"] = json!("project_other");
        assert_build_code(wrong_project, TimelineDomainErrorCode::InvalidInput);

        let mut wrong_run = build_options();
        wrong_run.audio_document["runId"] = json!("run_audio_other");
        assert_build_code(wrong_run, TimelineDomainErrorCode::InvalidInput);

        let mut duration_drift = build_options();
        duration_drift.audio_document["durationMs"] = json!(99);
        assert_build_code(duration_drift, TimelineDomainErrorCode::InvalidInput);

        let mut captions_audio_ref_drift = build_options();
        captions_audio_ref_drift.captions_document["audioInput"]["artifactId"] =
            json!("artifact_audio_other");
        assert_build_code(
            captions_audio_ref_drift,
            TimelineDomainErrorCode::InvalidInput,
        );

        let mut scene_captions_ref_drift = build_options();
        scene_captions_ref_drift.scene_plan_document["inputRefs"][2]["contentHash"] =
            json!(sha('9'));
        assert_build_code(
            scene_captions_ref_drift,
            TimelineDomainErrorCode::InvalidInput,
        );

        let mut duplicate_cue = build_options();
        duplicate_cue.captions_document["cues"][1]["cueId"] = json!("cue_1");
        assert_build_code(duplicate_cue, TimelineDomainErrorCode::InvalidInput);

        let mut out_of_audio_cue = build_options();
        out_of_audio_cue.captions_document["cues"][3]["endMs"] = json!(101);
        assert_build_code(out_of_audio_cue, TimelineDomainErrorCode::InvalidInput);

        let mut missing_caption_ref = build_options();
        missing_caption_ref.captions_document["cues"]
            .as_array_mut()
            .expect("caption cues")
            .pop();
        assert_build_code(missing_caption_ref, TimelineDomainErrorCode::InvalidInput);

        let mut trace_drift = build_options();
        trace_drift.scene_plan_document["scenes"][0]["claimIds"] = json!(["claim_other"]);
        assert_build_code(trace_drift, TimelineDomainErrorCode::InvalidInput);

        let mut duplicate_artifact = build_options();
        duplicate_artifact.captions_input.artifact_id =
            duplicate_artifact.audio_input.artifact_id.clone();
        assert_build_code(duplicate_artifact, TimelineDomainErrorCode::InvalidRequest);

        let mut wrong_stage = build_options();
        wrong_stage.scene_plan_input.stage_id = "scene".to_owned();
        assert_build_code(wrong_stage, TimelineDomainErrorCode::InvalidRequest);
    }

    #[test]
    fn semantic_validator_rejects_track_holes_overlaps_ref_drift_and_unstable_ids() {
        let base = built_timeline();

        let mut duplicate_scene = base.clone();
        duplicate_scene["sceneTrack"][1]["sceneId"] =
            duplicate_scene["sceneTrack"][0]["sceneId"].clone();
        assert_schema_valid_semantic_error(&duplicate_scene);

        let mut hole = base.clone();
        hole["sceneTrack"][1]["startMs"] = json!(61);
        assert_schema_valid_semantic_error(&hole);

        let mut overlap = base.clone();
        overlap["sceneTrack"][1]["startMs"] = json!(59);
        assert_schema_valid_semantic_error(&overlap);

        let mut wrong_start = base.clone();
        wrong_start["sceneTrack"][0]["startMs"] = json!(1);
        assert_schema_valid_semantic_error(&wrong_start);

        let mut incomplete_duration = base.clone();
        incomplete_duration["sceneTrack"][1]["endMs"] = json!(99);
        assert_schema_valid_semantic_error(&incomplete_duration);

        let mut audio_track_ref_drift = base.clone();
        audio_track_ref_drift["audioTrack"]["audioArtifactId"] = json!("artifact_audio_other");
        assert_schema_valid_semantic_error(&audio_track_ref_drift);

        let mut audio_track_duration_drift = base.clone();
        audio_track_duration_drift["audioTrack"]["endMs"] = json!(99);
        assert_schema_valid_semantic_error(&audio_track_duration_drift);

        let mut captions_track_ref_drift = base.clone();
        captions_track_ref_drift["captionTrack"]["captionsArtifactId"] =
            json!("artifact_captions_other");
        assert_schema_valid_semantic_error(&captions_track_ref_drift);

        let mut no_caption_ids = base.clone();
        no_caption_ids["captionTrack"]["cueIds"] = json!([]);
        assert_schema_valid_semantic_error(&no_caption_ids);

        let mut cross_project_input = base.clone();
        cross_project_input["inputRefs"][0]["projectId"] = json!("project_other");
        assert_schema_valid_semantic_error(&cross_project_input);

        let mut duplicate_stage = base.clone();
        duplicate_stage["inputRefs"][2]["stageId"] = json!("captions");
        assert_schema_valid_semantic_error(&duplicate_stage);

        let mut unsafe_area = base.clone();
        unsafe_area["safeArea"]["x"] = json!(1_900);
        unsafe_area["safeArea"]["width"] = json!(100);
        assert_schema_valid_semantic_error(&unsafe_area);

        let mut overflowing_area = base.clone();
        overflowing_area["safeArea"]["x"] = json!(16_384);
        overflowing_area["safeArea"]["width"] = json!(16_384);
        assert_schema_valid_semantic_error(&overflowing_area);

        let mut excessive_rate = base.clone();
        excessive_rate["canvas"]["frameRateNumerator"] = json!(241);
        excessive_rate["canvas"]["frameRateDenominator"] = json!(1);
        assert_schema_valid_semantic_error(&excessive_rate);

        let mut changed = base["changeSummary"]["changedSceneIds"]
            .as_array()
            .expect("changed IDs")
            .clone();
        assert!(changed.len() >= 2);
        changed.reverse();
        let mut unstable_changes = base.clone();
        unstable_changes["changeSummary"]["changedSceneIds"] = Value::Array(changed);
        assert_schema_valid_semantic_error(&unstable_changes);

        let mut unknown_change = base;
        unknown_change["changeSummary"]["changedSceneIds"] = json!(["scene_unknown"]);
        assert_schema_valid_semantic_error(&unknown_change);
    }

    #[test]
    fn edit_validation_rejects_illegal_nonadjacent_and_overflowing_sequences_atomically() {
        let base = three_scene_timeline();
        let original = base.clone();

        for edit in [
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_b".to_owned(),
                boundary_ms: 0,
            },
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_b".to_owned(),
                boundary_ms: 30,
            },
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_b".to_owned(),
                boundary_ms: 60,
            },
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_a".to_owned(),
                right_scene_id: "scene_c".to_owned(),
                boundary_ms: 45,
            },
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_b".to_owned(),
                right_scene_id: "scene_a".to_owned(),
                boundary_ms: 45,
            },
            TimelineEditData::MoveSceneBoundary {
                left_scene_id: "scene_missing".to_owned(),
                right_scene_id: "scene_b".to_owned(),
                boundary_ms: 20,
            },
        ] {
            assert_eq!(
                apply_error(&base, vec![edit]),
                TimelineDomainErrorCode::InvalidEdit
            );
            assert_eq!(base, original);
        }

        assert_eq!(
            apply_error(
                &base,
                vec![TimelineEditData::SetSafeArea {
                    safe_area: TimelineSafeAreaData {
                        x: u32::MAX,
                        y: 0,
                        width: 2,
                        height: 10,
                    },
                }]
            ),
            TimelineDomainErrorCode::InvalidRequest
        );
        assert_eq!(base, original);

        assert_eq!(
            apply_error(&base, vec![]),
            TimelineDomainErrorCode::InvalidRequest
        );
        let too_many = (0..=1_000)
            .map(|index| TimelineEditData::SetCaptionVisibility {
                visible: index % 2 == 0,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            apply_error(&base, too_many),
            TimelineDomainErrorCode::InvalidRequest
        );

        let error = apply_timeline_edits(ApplyTimelineEditsOptions {
            base_timeline_document: base.clone(),
            edits: vec![
                TimelineEditData::SetCaptionVisibility { visible: false },
                TimelineEditData::MoveSceneBoundary {
                    left_scene_id: "scene_a".to_owned(),
                    right_scene_id: "scene_b".to_owned(),
                    boundary_ms: 30,
                },
            ],
            change_summary: "A later invalid edit rejects the entire sequence.".to_owned(),
            new_run_id: "run_timeline_atomic".to_owned(),
            new_timeline_id: "timeline_atomic".to_owned(),
            created_at: "2026-07-18T09:10:00Z".to_owned(),
            base_artifact_id: "artifact_timeline_base".to_owned(),
        })
        .expect_err("late invalid edit must reject candidate");
        assert_eq!(error.code, TimelineDomainErrorCode::InvalidEdit);
        assert_eq!(base, original, "failed sequence cannot mutate base");

        let mut invalid_controls = edit_options(
            &base,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
        );
        invalid_controls.change_summary = "bad\nsummary".to_owned();
        assert_eq!(
            apply_timeline_edits(invalid_controls)
                .expect_err("control character in summary")
                .code,
            TimelineDomainErrorCode::InvalidRequest
        );

        let mut same_run = edit_options(
            &base,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
        );
        same_run.new_run_id = base["runId"].as_str().expect("base run").to_owned();
        assert_eq!(
            apply_timeline_edits(same_run)
                .expect_err("run identity must advance")
                .code,
            TimelineDomainErrorCode::InvalidRequest
        );

        let mut same_timeline = edit_options(
            &base,
            vec![TimelineEditData::SetCaptionVisibility { visible: false }],
        );
        same_timeline.new_timeline_id = base["timelineId"]
            .as_str()
            .expect("base Timeline ID")
            .to_owned();
        assert_eq!(
            apply_timeline_edits(same_timeline)
                .expect_err("Timeline identity must advance")
                .code,
            TimelineDomainErrorCode::InvalidRequest
        );
    }

    fn built_timeline() -> Value {
        build_timeline_document(build_options()).expect("fixture Timeline")
    }

    fn three_scene_timeline() -> Value {
        let mut timeline = built_timeline();
        timeline["sceneTrack"] = json!([
            {"sceneId":"scene_a","startMs":0,"endMs":30},
            {"sceneId":"scene_b","startMs":30,"endMs":60},
            {"sceneId":"scene_c","startMs":60,"endMs":AUDIO_DURATION_MS},
        ]);
        timeline["changeSummary"]["changedSceneIds"] = json!(["scene_a", "scene_b", "scene_c"]);
        validate_timeline_semantics(&timeline).expect("three-Scene fixture semantics");
        timeline
    }

    fn edit_options(base: &Value, edits: Vec<TimelineEditData>) -> ApplyTimelineEditsOptions {
        ApplyTimelineEditsOptions {
            base_timeline_document: base.clone(),
            edits,
            change_summary: "Bounded Timeline edit fixture.".to_owned(),
            new_run_id: "run_timeline_error".to_owned(),
            new_timeline_id: "timeline_error".to_owned(),
            created_at: "2026-07-18T09:20:00Z".to_owned(),
            base_artifact_id: "artifact_timeline_base".to_owned(),
        }
    }

    fn apply_error(base: &Value, edits: Vec<TimelineEditData>) -> TimelineDomainErrorCode {
        apply_timeline_edits(edit_options(base, edits))
            .expect_err("Timeline edit must fail")
            .code
    }

    fn assert_build_code(options: BuildTimelineOptions, expected: TimelineDomainErrorCode) {
        assert_eq!(
            build_timeline_document(options)
                .expect_err("Timeline build must fail")
                .code,
            expected
        );
    }

    fn assert_schema_valid_semantic_error(document: &Value) {
        validate_media_document(document).expect("mutation remains schema valid");
        assert!(
            validate_timeline_semantics(document).is_err(),
            "semantic validator unexpectedly accepted {document}"
        );
    }

    fn build_options() -> BuildTimelineOptions {
        let audio_input = frozen("audio", 'a');
        let captions_input = frozen("captions", 'b');
        let scene_plan_input = frozen("scene_plan", 'c');
        let captions_document = captions_document(&audio_input);
        let scene_plan_document = build_scene_plan_document(BuildScenePlanOptions {
            captions_document: captions_document.clone(),
            audio_duration_ms: AUDIO_DURATION_MS,
            input_refs: vec![
                frozen_value(&frozen("research", 'd')),
                frozen_value(&frozen("script", 'e')),
                frozen_value(&captions_input),
            ],
            config_snapshot: json!({"grouping":"three-cues","language":"en"}),
            project_id: PROJECT_ID.to_owned(),
            run_id: scene_plan_input.run_id.clone(),
            stable_seed: "scene-plan-seed-v1".to_owned(),
            created_at: "2026-07-18T08:00:00Z".to_owned(),
        })
        .expect("fixture Scene Plan");
        BuildTimelineOptions {
            audio_document: audio_document(&audio_input),
            captions_document,
            scene_plan_document,
            audio_input,
            captions_input,
            scene_plan_input,
            canvas: TimelineCanvasData {
                width: 1_920,
                height: 1_080,
                frame_rate_numerator: 30_000,
                frame_rate_denominator: 1_001,
            },
            safe_area: TimelineSafeAreaData {
                x: 96,
                y: 54,
                width: 1_728,
                height: 972,
            },
            config_snapshot: json!({"layout":"landscape","captionsVisible":true}),
            project_id: PROJECT_ID.to_owned(),
            run_id: "run_timeline_base".to_owned(),
            stable_seed: "timeline-seed-v1".to_owned(),
            created_at: "2026-07-18T08:30:00Z".to_owned(),
        }
    }

    fn audio_document(audio_input: &FrozenArtifactInputData) -> Value {
        json!({
            "schemaVersion": narracut_contracts::NARRACUT_MEDIA_SCHEMA_VERSION,
            "documentType": "audio_media",
            "mediaId": "audio_timeline_fixture",
            "projectId": PROJECT_ID,
            "runId": audio_input.run_id,
            "artifactUri": format!("artifacts/objects/sha256/aa/{}", "a".repeat(64)),
            "source": {
                "sourceFileName": "narration.wav",
                "sourceContentHash": sha('a'),
                "byteLength": 3_244,
            },
            "rights": {
                "ownership": "self_recorded",
                "author": "Timeline fixture narrator",
                "rightsStatement": "Recorded for the Timeline domain fixture.",
                "licenseId": "self-recorded",
                "attributionText": "",
                "authorizationRecords": [{
                    "authorizationRecordId": "authorization_timeline_fixture_audio",
                    "authorizationType": "material_use",
                    "grantor": "Timeline fixture narrator",
                    "scope": "Recorded for the Timeline domain fixture.",
                    "evidenceRef": "self-recorded",
                    "recordedAt": "2026-07-18T00:00:00Z"
                }],
                "voiceAuthorization": {
                    "applicability": "not_applicable",
                    "reason": "not_voice_clone"
                },
            },
            "durationMs": AUDIO_DURATION_MS,
            "sampleRateHz": 16_000,
            "bitsPerSample": 16,
            "channels": 1,
            "blockAlign": 2,
            "byteRate": 32_000,
            "dataBytes": 3_200,
            "inputRefs": [frozen_value(&frozen("script", 'e'))],
            "configSnapshot": {"format":"pcm-wav"},
            "createdAt": "2026-07-18T07:00:00Z",
        })
    }

    fn captions_document(audio_input: &FrozenArtifactInputData) -> Value {
        json!({
            "schemaVersion": narracut_contracts::NARRACUT_MEDIA_SCHEMA_VERSION,
            "documentType": "captions_media",
            "captionsId": "captions_timeline_fixture",
            "projectId": PROJECT_ID,
            "runId": frozen("captions", 'b').run_id,
            "rawArtifactId": "artifact_captions_raw_timeline",
            "rawContentHash": sha('f'),
            "source": {
                "sourceFileName": "narration.srt",
                "sourceContentHash": sha('f'),
                "byteLength": 512,
            },
            "rights": {
                "ownership": "self_recorded",
                "author": "Fixture Captioner",
                "rightsStatement": "Fixture captions are authorized for test use.",
                "licenseId": "fixture-owned-captions",
                "attributionText": "",
                "authorizationRecords": [{
                    "authorizationRecordId": "authorization_fixture_captions",
                    "authorizationType": "material_use",
                    "grantor": "Fixture Captioner",
                    "scope": "Fixture captions are authorized for test use.",
                    "evidenceRef": "fixture-owned-captions",
                    "recordedAt": "2026-07-18T00:00:00Z"
                }],
                "voiceAuthorization": {
                    "applicability": "not_applicable",
                    "reason": "not_voice_clone"
                }
            },
            "audioInput": frozen_value(audio_input),
            "cues": [
                cue(1, 5, 20, "Opening evidence-backed caption.", &[("claim_1", "evidence_1")]),
                cue(2, 25, 40, "Second caption.", &[("claim_2", "evidence_1"), ("claim_1", "evidence_1")]),
                cue(3, 45, 60, "Third caption.", &[("claim_3", "evidence_2")]),
                cue(4, 70, 90, "Closing caption.", &[("claim_4", "evidence_3")]),
            ],
            "mappings": [{
                "mappingId": "mapping_timeline_fixture",
                "level": "cue",
                "sourceCueId": "cue_1",
                "startMs": 5,
                "endMs": 20,
                "text": "Opening evidence-backed caption.",
                "timingPrecision": "cue_exact",
                "timingBasis": "srt_cue",
            }],
            "diagnostics": [],
            "inputRefs": [
                frozen_value(&frozen("script", 'e')),
                frozen_value(audio_input),
            ],
            "configSnapshot": {"language":"en"},
            "createdAt": "2026-07-18T07:30:00Z",
        })
    }

    fn cue(
        index: u64,
        start_ms: u64,
        end_ms: u64,
        text: &str,
        provenance: &[(&str, &str)],
    ) -> Value {
        let mut claim_ids = Vec::new();
        let mut evidence_refs = Vec::new();
        for (claim_id, evidence_ref) in provenance {
            if !claim_ids.contains(claim_id) {
                claim_ids.push(*claim_id);
            }
            if !evidence_refs.contains(evidence_ref) {
                evidence_refs.push(*evidence_ref);
            }
        }
        json!({
            "cueId": format!("cue_{index}"),
            "sourceIndex": index,
            "startMs": start_ms,
            "endMs": end_ms,
            "text": text,
            "provenance": provenance.iter().map(|(claim_id, evidence_ref)| json!({
                "claimId": claim_id,
                "evidenceRef": evidence_ref,
            })).collect::<Vec<_>>(),
            "claimIds": claim_ids,
            "evidenceRefs": evidence_refs,
        })
    }

    fn frozen(stage_id: &str, hash_character: char) -> FrozenArtifactInputData {
        FrozenArtifactInputData {
            stage_id: stage_id.to_owned(),
            run_id: format!("run_{stage_id}_timeline"),
            artifact_id: format!("artifact_{stage_id}_timeline"),
            content_hash: sha(hash_character),
            review_record_id: format!("review_{stage_id}_timeline"),
            claim_ids: vec!["claim_1".to_owned()],
            evidence_refs: vec!["evidence_1".to_owned()],
        }
    }

    fn frozen_value(input: &FrozenArtifactInputData) -> Value {
        json!({
            "projectId": PROJECT_ID,
            "stageId": input.stage_id,
            "runId": input.run_id,
            "artifactId": input.artifact_id,
            "contentHash": input.content_hash,
            "reviewRecordId": input.review_record_id,
            "claimIds": input.claim_ids,
            "evidenceRefs": input.evidence_refs,
        })
    }

    fn sha(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn scene_id(document: &Value, index: usize) -> String {
        document["sceneTrack"][index]["sceneId"]
            .as_str()
            .expect("Scene ID")
            .to_owned()
    }

    fn assert_changed_ids(document: &Value, expected: &[&str]) {
        let mut expected = expected
            .iter()
            .map(|scene_id| (*scene_id).to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        expected.dedup();
        assert_eq!(
            document["changeSummary"]["changedSceneIds"],
            json!(expected)
        );
    }
}
