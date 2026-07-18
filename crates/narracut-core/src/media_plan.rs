use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use narracut_contracts::{validate_media_document, NARRACUT_MEDIA_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{BuildScenePlanOptions, ScenePlanEditData};

const MAX_AUDIO_DURATION_MS: u64 = 86_400_000;
const MAX_SCENES: usize = 10_000;
const MAX_CUES_PER_GENERATED_SCENE: usize = 3;
const MAX_TITLE_CHARS: usize = 96;
const MAX_ROLE_CHARS: usize = 512;
const MAX_SUMMARY_CHARS: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenePlanErrorCode {
    InvalidRequest,
    InvalidCaptions,
    InvalidScenePlan,
    InvalidEdit,
    ResourceLimitExceeded,
    ContractViolation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePlanError {
    pub code: ScenePlanErrorCode,
    pub message: String,
}

impl ScenePlanError {
    fn new(code: ScenePlanErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ScenePlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ScenePlanError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProvenancePair {
    claim_id: String,
    evidence_ref: String,
}

pub fn build_scene_plan_document(options: BuildScenePlanOptions) -> Result<Value, ScenePlanError> {
    validate_build_options(&options)?;
    validate_media_document(&options.captions_document).map_err(|_| {
        ScenePlanError::new(
            ScenePlanErrorCode::InvalidCaptions,
            "Captions 文档未通过 media v1 契约。",
        )
    })?;
    if options.captions_document["projectId"].as_str() != Some(options.project_id.as_str()) {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidCaptions,
            "Captions 文档与目标 Scene Plan 不属于同一项目。",
        ));
    }
    let cues = validated_caption_cues(&options.captions_document, options.audio_duration_ms)?;
    let captions_hash = canonical_hash(&options.captions_document);
    let mut scenes = Vec::new();
    for cue_group in cues.chunks(MAX_CUES_PER_GENERATED_SCENE) {
        scenes.push(scene_from_cues(
            cue_group,
            &captions_hash,
            &options.stable_seed,
        )?);
    }
    assign_covering_times(&mut scenes, &cues, options.audio_duration_ms)?;
    reindex_scenes(&mut scenes);
    let mut changed_scene_ids = scenes.iter().map(scene_id).collect::<Result<Vec<_>, _>>()?;
    changed_scene_ids.sort();
    let cue_traceability = cues
        .iter()
        .map(|cue| {
            Ok(json!({
                "cueId": required_string(cue, "cueId", ScenePlanErrorCode::InvalidCaptions)?,
                "provenance": cue["provenance"].clone(),
            }))
        })
        .collect::<Result<Vec<_>, ScenePlanError>>()?;
    let document = json!({
        "schemaVersion": NARRACUT_MEDIA_SCHEMA_VERSION,
        "documentType": "scene_plan",
        "scenePlanId": stable_id("sceneplan", &[&options.project_id, &options.run_id, &options.stable_seed, &captions_hash]),
        "projectId": options.project_id,
        "runId": options.run_id,
        "inputRefs": options.input_refs,
        "configSnapshot": options.config_snapshot,
        "cueTraceability": cue_traceability,
        "scenes": scenes,
        "diagnostics": [],
        "changeSummary": {
            "summary": "Deterministic scene plan generated from approved caption cues.",
            "changedSceneIds": changed_scene_ids,
        },
        "createdAt": options.created_at,
    });
    validate_scene_plan_document(&document, options.audio_duration_ms, Some(&cues))?;
    Ok(document)
}

pub fn apply_scene_plan_edits(
    base: &Value,
    edits: &[ScenePlanEditData],
    summary: &str,
    new_run_id: &str,
    new_scene_plan_id: &str,
    created_at: &str,
    base_artifact_id: &str,
) -> Result<Value, ScenePlanError> {
    let audio_duration_ms = scene_plan_duration(base)?;
    validate_scene_plan_document(base, audio_duration_ms, None)?;
    validate_edit_request(
        edits,
        summary,
        new_run_id,
        new_scene_plan_id,
        created_at,
        base_artifact_id,
    )?;
    if base["runId"].as_str() == Some(new_run_id)
        || base["scenePlanId"].as_str() == Some(new_scene_plan_id)
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidRequest,
            "编辑后的 Scene Plan 必须使用新的 runId 与 scenePlanId。",
        ));
    }

    let mut candidate = base.clone();
    let cue_traceability = normalize_scene_plan_traceability(&mut candidate)?;
    let mut scenes = candidate["scenes"]
        .as_array()
        .expect("validated ScenePlanDocument has scenes")
        .clone();
    let mut changed_scene_ids = BTreeSet::new();
    for edit in edits {
        apply_edit(&mut scenes, edit, &cue_traceability, &mut changed_scene_ids)?;
        if scenes.is_empty() || scenes.len() > MAX_SCENES {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::ResourceLimitExceeded,
                "Scene 编辑后的场景数量超过契约上限。",
            ));
        }
        reindex_scenes(&mut scenes);
    }

    candidate["scenePlanId"] = json!(new_scene_plan_id);
    candidate["schemaVersion"] = json!(NARRACUT_MEDIA_SCHEMA_VERSION);
    candidate["runId"] = json!(new_run_id);
    candidate["supersedesArtifactId"] = json!(base_artifact_id);
    candidate["scenes"] = Value::Array(scenes);
    candidate["changeSummary"] = json!({
        "summary": summary,
        "changedSceneIds": changed_scene_ids.into_iter().collect::<Vec<_>>(),
    });
    candidate["createdAt"] = json!(created_at);
    validate_scene_plan_document(&candidate, audio_duration_ms, None)?;
    Ok(candidate)
}

pub fn validate_scene_plan_semantics(
    document: &Value,
    audio_duration_ms: u64,
) -> Result<(), ScenePlanError> {
    validate_scene_plan_document(document, audio_duration_ms, None)
}

fn validate_build_options(options: &BuildScenePlanOptions) -> Result<(), ScenePlanError> {
    if options.audio_duration_ms == 0
        || options.audio_duration_ms > MAX_AUDIO_DURATION_MS
        || !portable_id(&options.project_id)
        || !run_id(&options.run_id)
        || !bounded_text(&options.stable_seed, 512)
        || !bounded_text(&options.created_at, 40)
        || options.input_refs.len() < 2
        || options.input_refs.len() > 32
        || !options.config_snapshot.is_object()
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidRequest,
            "Scene Plan 构建参数无效或超过资源上限。",
        ));
    }
    let mut artifact_ids = BTreeSet::new();
    if options.input_refs.iter().any(|input| {
        input["projectId"].as_str() != Some(options.project_id.as_str())
            || input["artifactId"]
                .as_str()
                .is_none_or(|artifact_id| !artifact_ids.insert(artifact_id))
    }) {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidRequest,
            "Scene Plan inputRefs 必须属于目标项目且 Artifact 引用唯一。",
        ));
    }
    Ok(())
}

fn validate_edit_request(
    edits: &[ScenePlanEditData],
    summary: &str,
    new_run_id: &str,
    new_scene_plan_id: &str,
    created_at: &str,
    base_artifact_id: &str,
) -> Result<(), ScenePlanError> {
    if edits.is_empty()
        || edits.len() > 1_024
        || !bounded_text(summary, MAX_SUMMARY_CHARS)
        || !run_id(new_run_id)
        || !portable_id(new_scene_plan_id)
        || !bounded_text(created_at, 40)
        || !artifact_id(base_artifact_id)
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidRequest,
            "Scene Plan 编辑请求无效或超过资源上限。",
        ));
    }
    Ok(())
}

fn apply_edit(
    scenes: &mut Vec<Value>,
    edit: &ScenePlanEditData,
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
    changed: &mut BTreeSet<String>,
) -> Result<(), ScenePlanError> {
    match edit {
        ScenePlanEditData::Split {
            scene_id: target_id,
            boundary_cue_id,
        } => split_scene(
            scenes,
            target_id,
            boundary_cue_id,
            cue_traceability,
            changed,
        ),
        ScenePlanEditData::Merge {
            first_scene_id,
            second_scene_id,
        } => merge_scenes(
            scenes,
            first_scene_id,
            second_scene_id,
            cue_traceability,
            changed,
        ),
        ScenePlanEditData::Update {
            scene_id: target_id,
            title,
            narrative_role,
        } => update_scene(
            scenes,
            target_id,
            title.as_deref(),
            narrative_role.as_deref(),
            changed,
        ),
        ScenePlanEditData::MoveBoundary {
            left_scene_id,
            right_scene_id,
            boundary_cue_id,
        } => move_boundary(
            scenes,
            left_scene_id,
            right_scene_id,
            boundary_cue_id,
            cue_traceability,
            changed,
        ),
    }
}

fn split_scene(
    scenes: &mut Vec<Value>,
    target_id: &str,
    boundary_cue_id: &str,
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
    changed: &mut BTreeSet<String>,
) -> Result<(), ScenePlanError> {
    validate_edit_id(target_id)?;
    validate_edit_id(boundary_cue_id)?;
    let index = find_scene(scenes, target_id)?;
    let original = scenes[index].clone();
    let cue_ids = scene_cue_ids(&original)?;
    let boundary = cue_ids
        .iter()
        .position(|cue_id| cue_id == boundary_cue_id)
        .filter(|position| *position > 0 && *position < cue_ids.len())
        .ok_or_else(invalid_boundary)?;
    let start = scene_start(&original)?;
    let end = scene_end(&original)?;
    let split_time = proportional_boundary(start, end, boundary, cue_ids.len())?;
    let left_cues = cue_ids[..boundary].to_vec();
    let right_cues = cue_ids[boundary..].to_vec();
    let mut left = original.clone();
    left["cueIds"] = json!(left_cues);
    left["suggestedEndMs"] = json!(split_time);
    set_scene_traceability_from_map(&mut left, cue_traceability)?;

    let mut right = original;
    let right_id = stable_id(
        "scene",
        &[
            "split",
            target_id,
            right_cues.first().expect("right cues non-empty"),
            right_cues.last().expect("right cues non-empty"),
        ],
    );
    right["sceneId"] = json!(right_id);
    right["cueIds"] = json!(right_cues);
    right["suggestedStartMs"] = json!(split_time);
    right["title"] = json!(safe_excerpt(original_title(&right)?, MAX_TITLE_CHARS));
    set_scene_traceability_from_map(&mut right, cue_traceability)?;
    scenes[index] = left;
    scenes.insert(index + 1, right);
    changed.insert(target_id.to_owned());
    changed.insert(right_id);
    Ok(())
}

fn merge_scenes(
    scenes: &mut Vec<Value>,
    first_id: &str,
    second_id: &str,
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
    changed: &mut BTreeSet<String>,
) -> Result<(), ScenePlanError> {
    validate_edit_id(first_id)?;
    validate_edit_id(second_id)?;
    let first_index = find_scene(scenes, first_id)?;
    if scenes
        .get(first_index + 1)
        .and_then(|scene| scene["sceneId"].as_str())
        != Some(second_id)
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidEdit,
            "merge 仅允许按顺序合并相邻 Scene。",
        ));
    }
    let second = scenes.remove(first_index + 1);
    let mut cue_ids = scene_cue_ids(&scenes[first_index])?;
    cue_ids.extend(scene_cue_ids(&second)?);
    scenes[first_index]["cueIds"] = json!(cue_ids);
    set_scene_traceability_from_map(&mut scenes[first_index], cue_traceability)?;
    scenes[first_index]["suggestedEndMs"] = json!(scene_end(&second)?);
    changed.insert(first_id.to_owned());
    changed.insert(second_id.to_owned());
    Ok(())
}

fn update_scene(
    scenes: &mut [Value],
    target_id: &str,
    title: Option<&str>,
    narrative_role: Option<&str>,
    changed: &mut BTreeSet<String>,
) -> Result<(), ScenePlanError> {
    validate_edit_id(target_id)?;
    if title.is_none() && narrative_role.is_none() {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidEdit,
            "update 必须至少修改 title 或 narrativeRole。",
        ));
    }
    if title.is_some_and(|value| !bounded_text(value, MAX_TITLE_CHARS))
        || narrative_role.is_some_and(|value| !bounded_text(value, MAX_ROLE_CHARS))
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidEdit,
            "Scene title 或 narrativeRole 为空、包含控制字符或超过上限。",
        ));
    }
    let index = find_scene(scenes, target_id)?;
    if let Some(title) = title {
        scenes[index]["title"] = json!(title);
    }
    if let Some(narrative_role) = narrative_role {
        scenes[index]["narrativeRole"] = json!(narrative_role);
    }
    changed.insert(target_id.to_owned());
    Ok(())
}

fn move_boundary(
    scenes: &mut [Value],
    left_id: &str,
    right_id: &str,
    boundary_cue_id: &str,
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
    changed: &mut BTreeSet<String>,
) -> Result<(), ScenePlanError> {
    validate_edit_id(left_id)?;
    validate_edit_id(right_id)?;
    validate_edit_id(boundary_cue_id)?;
    let left_index = find_scene(scenes, left_id)?;
    if scenes
        .get(left_index + 1)
        .and_then(|scene| scene["sceneId"].as_str())
        != Some(right_id)
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidEdit,
            "move_boundary 仅允许调整相邻 Scene。",
        ));
    }
    let right_index = left_index + 1;
    let left_count = scene_cue_ids(&scenes[left_index])?.len();
    let mut combined_cues = scene_cue_ids(&scenes[left_index])?;
    combined_cues.extend(scene_cue_ids(&scenes[right_index])?);
    let boundary = combined_cues
        .iter()
        .position(|cue_id| cue_id == boundary_cue_id)
        .filter(|position| {
            *position > 0 && *position < combined_cues.len() && *position != left_count
        })
        .ok_or_else(invalid_boundary)?;
    let start = scene_start(&scenes[left_index])?;
    let end = scene_end(&scenes[right_index])?;
    let boundary_time = proportional_boundary(start, end, boundary, combined_cues.len())?;
    let left_cues = combined_cues[..boundary].to_vec();
    let right_cues = combined_cues[boundary..].to_vec();
    scenes[left_index]["cueIds"] = json!(left_cues);
    set_scene_traceability_from_map(&mut scenes[left_index], cue_traceability)?;
    scenes[left_index]["suggestedEndMs"] = json!(boundary_time);
    scenes[right_index]["cueIds"] = json!(right_cues);
    set_scene_traceability_from_map(&mut scenes[right_index], cue_traceability)?;
    scenes[right_index]["suggestedStartMs"] = json!(boundary_time);
    changed.insert(left_id.to_owned());
    changed.insert(right_id.to_owned());
    Ok(())
}

fn proportional_boundary(
    start: u64,
    end: u64,
    left_count: usize,
    total_count: usize,
) -> Result<u64, ScenePlanError> {
    let duration = end.checked_sub(start).ok_or_else(invalid_boundary)?;
    if left_count == 0 || left_count >= total_count || duration < 2 {
        return Err(invalid_boundary());
    }
    let proportional =
        start + ((u128::from(duration) * left_count as u128) / total_count as u128) as u64;
    Ok(proportional.clamp(start + 1, end - 1))
}

fn invalid_boundary() -> ScenePlanError {
    ScenePlanError::new(
        ScenePlanErrorCode::InvalidEdit,
        "Scene cue 边界无效，边界两侧都必须保留至少一个 cue 和一毫秒。",
    )
}

fn find_scene(scenes: &[Value], target_id: &str) -> Result<usize, ScenePlanError> {
    scenes
        .iter()
        .position(|scene| scene["sceneId"].as_str() == Some(target_id))
        .ok_or_else(|| ScenePlanError::new(ScenePlanErrorCode::InvalidEdit, "目标 Scene 不存在。"))
}

fn validate_edit_id(value: &str) -> Result<(), ScenePlanError> {
    if portable_id(value) {
        Ok(())
    } else {
        Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidEdit,
            "Scene 编辑引用不是安全可移植 ID。",
        ))
    }
}

fn scene_plan_duration(document: &Value) -> Result<u64, ScenePlanError> {
    document["scenes"]
        .as_array()
        .and_then(|scenes| scenes.last())
        .and_then(|scene| scene["suggestedEndMs"].as_u64())
        .filter(|duration| *duration > 0 && *duration <= MAX_AUDIO_DURATION_MS)
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "无法从基础 Scene Plan 推导音频时长。",
            )
        })
}

fn scene_cue_ids(scene: &Value) -> Result<Vec<String>, ScenePlanError> {
    scene["cueIds"]
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene cueIds 必须为非空数组。",
            )
        })?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "Scene cueId 必须为字符串。",
                )
            })
        })
        .collect()
}

fn scene_start(scene: &Value) -> Result<u64, ScenePlanError> {
    required_u64(
        scene,
        "suggestedStartMs",
        ScenePlanErrorCode::InvalidScenePlan,
    )
}

fn scene_end(scene: &Value) -> Result<u64, ScenePlanError> {
    required_u64(
        scene,
        "suggestedEndMs",
        ScenePlanErrorCode::InvalidScenePlan,
    )
}

fn original_title(scene: &Value) -> Result<&str, ScenePlanError> {
    scene["title"].as_str().ok_or_else(|| {
        ScenePlanError::new(ScenePlanErrorCode::InvalidScenePlan, "Scene title 无效。")
    })
}

fn validated_caption_cues(
    captions: &Value,
    audio_duration_ms: u64,
) -> Result<Vec<Value>, ScenePlanError> {
    if captions.get("documentType").and_then(Value::as_str) != Some("captions_media") {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidCaptions,
            "Scene Plan 只能消费 CaptionsMediaDocument。",
        ));
    }
    let cues = captions
        .get("cues")
        .and_then(Value::as_array)
        .filter(|cues| !cues.is_empty() && cues.len() <= MAX_SCENES)
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidCaptions,
                "Captions cue 清单为空或超过上限。",
            )
        })?;
    let mut seen = BTreeSet::new();
    let mut previous_end = 0;
    let mut normalized_cues = Vec::with_capacity(cues.len());
    for (index, cue) in cues.iter().enumerate() {
        let id = required_string(cue, "cueId", ScenePlanErrorCode::InvalidCaptions)?;
        let start = required_u64(cue, "startMs", ScenePlanErrorCode::InvalidCaptions)?;
        let end = required_u64(cue, "endMs", ScenePlanErrorCode::InvalidCaptions)?;
        let text = required_string(cue, "text", ScenePlanErrorCode::InvalidCaptions)?;
        if cue["sourceIndex"].as_u64() != Some(index as u64 + 1)
            || !portable_id(&id)
            || !valid_caption_text(&text)
            || !seen.insert(id)
            || start < previous_end
            || start >= end
            || end > audio_duration_ms
        {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidCaptions,
                "Captions cue 必须连续编号、唯一、有序、不重叠且位于音频时长内。",
            ));
        }
        let provenance = resolved_traceability_pairs(cue, ScenePlanErrorCode::InvalidCaptions)?;
        let mut normalized = cue.clone();
        set_traceability(&mut normalized, &provenance);
        normalized_cues.push(normalized);
        previous_end = end;
    }
    Ok(normalized_cues)
}

fn scene_from_cues(
    cues: &[Value],
    captions_hash: &str,
    stable_seed: &str,
) -> Result<Value, ScenePlanError> {
    let first_id = required_string(&cues[0], "cueId", ScenePlanErrorCode::InvalidCaptions)?;
    let last_id = required_string(
        cues.last().expect("cue group is non-empty"),
        "cueId",
        ScenePlanErrorCode::InvalidCaptions,
    )?;
    let title_source = required_string(&cues[0], "text", ScenePlanErrorCode::InvalidCaptions)?;
    let provenance = ordered_cue_provenance(cues)?;
    let (claim_ids, evidence_refs) = project_provenance(&provenance);
    Ok(json!({
        "sceneId": stable_id("scene", &[captions_hash, stable_seed, &first_id, &last_id]),
        "order": 0,
        "title": safe_excerpt(&title_source, MAX_TITLE_CHARS),
        "narrativeRole": "caption_sequence",
        "suggestedStartMs": 0,
        "suggestedEndMs": 1,
        "cueIds": cues.iter().map(|cue| cue["cueId"].clone()).collect::<Vec<_>>(),
        "provenance": provenance_values(&provenance),
        "claimIds": claim_ids,
        "evidenceRefs": evidence_refs,
    }))
}

fn resolved_traceability_pairs(
    value: &Value,
    code: ScenePlanErrorCode,
) -> Result<Vec<ProvenancePair>, ScenePlanError> {
    let claims = trace_values(value, "claimIds", code)?;
    let evidence = trace_values(value, "evidenceRefs", code)?;
    validate_trace_values(&claims, code)?;
    validate_trace_values(&evidence, code)?;

    let pairs = if let Some(provenance) = value.get("provenance") {
        parse_provenance_pairs(provenance, code)?
    } else {
        match (claims.as_slice(), evidence.as_slice()) {
            ([], []) => Vec::new(),
            ([claim_id], [evidence_ref]) => vec![ProvenancePair {
                claim_id: claim_id.clone(),
                evidence_ref: evidence_ref.clone(),
            }],
            _ => {
                return Err(ScenePlanError::new(
                    code,
                    "多值追溯集合缺少显式 provenance pair，无法安全迁移。",
                ));
            }
        }
    };

    let (projected_claims, projected_evidence) = project_provenance(&pairs);
    if projected_claims != claims || projected_evidence != evidence {
        return Err(ScenePlanError::new(
            code,
            "claimIds/evidenceRefs 必须是 provenance pair 的有序投影。",
        ));
    }
    Ok(pairs)
}

fn parse_provenance_pairs(
    provenance: &Value,
    code: ScenePlanErrorCode,
) -> Result<Vec<ProvenancePair>, ScenePlanError> {
    let provenance = provenance.as_array().ok_or_else(|| {
        ScenePlanError::new(code, "provenance 必须为 claimId/evidenceRef pair 数组。")
    })?;
    if provenance.len() > 1_024 {
        return Err(ScenePlanError::new(code, "provenance pair 超过契约上限。"));
    }
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::with_capacity(provenance.len());
    for item in provenance {
        let claim_id = required_string(item, "claimId", code)?;
        let evidence_ref = required_string(item, "evidenceRef", code)?;
        if !bounded_text(&claim_id, 512) || !bounded_text(&evidence_ref, 512) {
            return Err(ScenePlanError::new(code, "provenance pair 字段无效。"));
        }
        let pair = ProvenancePair {
            claim_id,
            evidence_ref,
        };
        if !seen.insert(pair.clone()) {
            return Err(ScenePlanError::new(code, "provenance pair 必须唯一。"));
        }
        pairs.push(pair);
    }
    Ok(pairs)
}

fn trace_values(
    value: &Value,
    field: &str,
    code: ScenePlanErrorCode,
) -> Result<Vec<String>, ScenePlanError> {
    value[field]
        .as_array()
        .ok_or_else(|| ScenePlanError::new(code, "追溯投影字段必须为数组。"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ScenePlanError::new(code, "追溯投影项必须为字符串。"))
        })
        .collect()
}

fn validate_trace_values(
    values: &[String],
    code: ScenePlanErrorCode,
) -> Result<(), ScenePlanError> {
    let mut seen = BTreeSet::new();
    if values.len() > 1_024
        || values
            .iter()
            .any(|value| !bounded_text(value, 512) || !seen.insert(value))
    {
        return Err(ScenePlanError::new(code, "追溯投影必须有界且唯一。"));
    }
    Ok(())
}

fn ordered_cue_provenance(cues: &[Value]) -> Result<Vec<ProvenancePair>, ScenePlanError> {
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for cue in cues {
        for pair in resolved_traceability_pairs(cue, ScenePlanErrorCode::InvalidCaptions)? {
            if seen.insert(pair.clone()) {
                pairs.push(pair);
            }
        }
    }
    Ok(pairs)
}

fn project_provenance(pairs: &[ProvenancePair]) -> (Vec<String>, Vec<String>) {
    let mut seen_claims = BTreeSet::new();
    let mut seen_evidence = BTreeSet::new();
    let mut claims = Vec::new();
    let mut evidence = Vec::new();
    for pair in pairs {
        if seen_claims.insert(pair.claim_id.clone()) {
            claims.push(pair.claim_id.clone());
        }
        if seen_evidence.insert(pair.evidence_ref.clone()) {
            evidence.push(pair.evidence_ref.clone());
        }
    }
    (claims, evidence)
}

fn provenance_values(pairs: &[ProvenancePair]) -> Vec<Value> {
    pairs
        .iter()
        .map(|pair| {
            json!({
                "claimId": pair.claim_id,
                "evidenceRef": pair.evidence_ref,
            })
        })
        .collect()
}

fn set_traceability(value: &mut Value, pairs: &[ProvenancePair]) {
    let (claim_ids, evidence_refs) = project_provenance(pairs);
    value["provenance"] = json!(provenance_values(pairs));
    value["claimIds"] = json!(claim_ids);
    value["evidenceRefs"] = json!(evidence_refs);
}

fn provenance_for_cue_ids(
    cue_ids: &[String],
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
) -> Result<Vec<ProvenancePair>, ScenePlanError> {
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for cue_id in cue_ids {
        let cue_pairs = cue_traceability.get(cue_id).ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene cue 缺少 cueTraceability。",
            )
        })?;
        for pair in cue_pairs {
            if seen.insert(pair.clone()) {
                pairs.push(pair.clone());
            }
        }
    }
    Ok(pairs)
}

fn set_scene_traceability_from_map(
    scene: &mut Value,
    cue_traceability: &BTreeMap<String, Vec<ProvenancePair>>,
) -> Result<(), ScenePlanError> {
    let cue_ids = scene_cue_ids(scene)?;
    let pairs = provenance_for_cue_ids(&cue_ids, cue_traceability)?;
    set_traceability(scene, &pairs);
    Ok(())
}

fn resolved_document_cue_traceability(
    document: &Value,
    scenes: &[Value],
) -> Result<BTreeMap<String, Vec<ProvenancePair>>, ScenePlanError> {
    let expected_cue_ids = scenes
        .iter()
        .map(scene_cue_ids)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if let Some(entries) = document.get("cueTraceability") {
        let entries = entries
            .as_array()
            .filter(|entries| entries.len() <= MAX_SCENES)
            .ok_or_else(|| {
                ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "cueTraceability 必须为有界数组。",
                )
            })?;
        if entries.len() != expected_cue_ids.len() {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "cueTraceability 必须覆盖全部 cue。",
            ));
        }
        let mut traceability = BTreeMap::new();
        for (entry, expected_cue_id) in entries.iter().zip(&expected_cue_ids) {
            let cue_id = required_string(entry, "cueId", ScenePlanErrorCode::InvalidScenePlan)?;
            if &cue_id != expected_cue_id || traceability.contains_key(&cue_id) {
                return Err(ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "cueTraceability 的 cue 顺序必须与 Scene cue 顺序一致且唯一。",
                ));
            }
            let provenance = entry.get("provenance").ok_or_else(|| {
                ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "cueTraceability 缺少 provenance。",
                )
            })?;
            traceability.insert(
                cue_id,
                parse_provenance_pairs(provenance, ScenePlanErrorCode::InvalidScenePlan)?,
            );
        }
        return Ok(traceability);
    }

    let mut traceability = BTreeMap::new();
    for scene in scenes {
        let pairs = resolved_traceability_pairs(scene, ScenePlanErrorCode::InvalidScenePlan)?;
        if pairs.len() > 1 {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "多 pair Scene 缺少 cueTraceability，无法安全迁移。",
            ));
        }
        for cue_id in scene_cue_ids(scene)? {
            traceability.insert(cue_id, pairs.clone());
        }
    }
    Ok(traceability)
}

fn normalize_scene_plan_traceability(
    document: &mut Value,
) -> Result<BTreeMap<String, Vec<ProvenancePair>>, ScenePlanError> {
    let mut scenes = document["scenes"]
        .as_array()
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene Plan scenes 无效。",
            )
        })?
        .clone();
    let cue_traceability = resolved_document_cue_traceability(document, &scenes)?;
    for scene in &mut scenes {
        set_scene_traceability_from_map(scene, &cue_traceability)?;
    }
    let entries = scenes
        .iter()
        .map(scene_cue_ids)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .map(|cue_id| {
            json!({
                "cueId": cue_id,
                "provenance": provenance_values(
                    cue_traceability
                        .get(&cue_id)
                        .expect("resolved cueTraceability covers every cue"),
                ),
            })
        })
        .collect::<Vec<_>>();
    document["cueTraceability"] = json!(entries);
    document["scenes"] = json!(scenes);
    Ok(cue_traceability)
}

fn assign_covering_times(
    scenes: &mut [Value],
    all_cues: &[Value],
    audio_duration_ms: u64,
) -> Result<(), ScenePlanError> {
    let mut boundaries = vec![0_u64];
    let mut consumed = 0;
    for scene in scenes.iter().take(scenes.len().saturating_sub(1)) {
        consumed += scene["cueIds"].as_array().map(Vec::len).unwrap_or(0);
        let left_end = required_u64(
            &all_cues[consumed - 1],
            "endMs",
            ScenePlanErrorCode::InvalidCaptions,
        )?;
        let right_start = required_u64(
            &all_cues[consumed],
            "startMs",
            ScenePlanErrorCode::InvalidCaptions,
        )?;
        boundaries.push(left_end + (right_start - left_end) / 2);
    }
    boundaries.push(audio_duration_ms);
    for (index, scene) in scenes.iter_mut().enumerate() {
        scene["suggestedStartMs"] = json!(boundaries[index]);
        scene["suggestedEndMs"] = json!(boundaries[index + 1]);
    }
    Ok(())
}

fn reindex_scenes(scenes: &mut [Value]) {
    for (index, scene) in scenes.iter_mut().enumerate() {
        scene["order"] = json!(index);
    }
}

fn validate_scene_plan_document(
    document: &Value,
    audio_duration_ms: u64,
    captions_cues: Option<&[Value]>,
) -> Result<(), ScenePlanError> {
    validate_media_document(document).map_err(|_| {
        ScenePlanError::new(
            ScenePlanErrorCode::ContractViolation,
            "ScenePlanDocument 未通过 media v1 契约。",
        )
    })?;
    if audio_duration_ms == 0 || audio_duration_ms > MAX_AUDIO_DURATION_MS {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidScenePlan,
            "Scene Plan 音频时长无效。",
        ));
    }
    if document["documentType"].as_str() != Some("scene_plan") {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidScenePlan,
            "文档不是 ScenePlanDocument。",
        ));
    }
    let scenes = document["scenes"]
        .as_array()
        .filter(|scenes| !scenes.is_empty() && scenes.len() <= MAX_SCENES)
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene 清单缺失、为空或超过上限。",
            )
        })?;
    let cue_traceability = resolved_document_cue_traceability(document, scenes)?;
    let caption_by_id = captions_cues
        .map(|cues| {
            cues.iter()
                .filter_map(|cue| cue["cueId"].as_str().map(|id| (id, cue)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut previous_end = 0;
    let mut seen_scenes = BTreeSet::new();
    let mut seen_cues = BTreeSet::new();
    for (index, scene) in scenes.iter().enumerate() {
        let id = scene_id(scene)?;
        let title = required_string(scene, "title", ScenePlanErrorCode::InvalidScenePlan)?;
        let role = required_string(scene, "narrativeRole", ScenePlanErrorCode::InvalidScenePlan)?;
        if !portable_id(&id)
            || !seen_scenes.insert(id)
            || !bounded_text(&title, MAX_TITLE_CHARS)
            || !bounded_text(&role, MAX_ROLE_CHARS)
        {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene ID 必须唯一，title/role 必须有界且不含控制字符。",
            ));
        }
        let start = required_u64(
            scene,
            "suggestedStartMs",
            ScenePlanErrorCode::InvalidScenePlan,
        )?;
        let end = required_u64(
            scene,
            "suggestedEndMs",
            ScenePlanErrorCode::InvalidScenePlan,
        )?;
        if scene["order"].as_u64() != Some(index as u64)
            || start != previous_end
            || start >= end
            || end > audio_duration_ms
        {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene 时间轴必须按序完整覆盖音频且无洞无重叠。",
            ));
        }
        let cue_ids = scene_cue_ids(scene)?;
        for cue_id in &cue_ids {
            if !portable_id(cue_id) || !seen_cues.insert(cue_id.to_owned()) {
                return Err(ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "每个 cue 必须且只能属于一个 Scene。",
                ));
            }
        }
        let scene_provenance =
            resolved_traceability_pairs(scene, ScenePlanErrorCode::InvalidScenePlan)?;
        if provenance_for_cue_ids(&cue_ids, &cue_traceability)? != scene_provenance {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene provenance 必须是所属 cue pair 的有序并集。",
            ));
        }
        if captions_cues.is_some() {
            let scene_cues = cue_ids
                .iter()
                .map(|cue_id| {
                    caption_by_id
                        .get(cue_id.as_str())
                        .map(|cue| (*cue).clone())
                        .ok_or_else(|| {
                            ScenePlanError::new(
                                ScenePlanErrorCode::InvalidScenePlan,
                                "Scene 引用了 Captions 中不存在的 cue。",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (cue_id, cue) in cue_ids.iter().zip(&scene_cues) {
                if cue_traceability.get(cue_id)
                    != Some(&resolved_traceability_pairs(
                        cue,
                        ScenePlanErrorCode::InvalidCaptions,
                    )?)
                {
                    return Err(ScenePlanError::new(
                        ScenePlanErrorCode::InvalidScenePlan,
                        "cueTraceability 与 Captions cue provenance 不一致。",
                    ));
                }
            }
            if scene_cues.iter().any(|cue| {
                cue["startMs"]
                    .as_u64()
                    .is_none_or(|cue_start| cue_start < start)
                    || cue["endMs"].as_u64().is_none_or(|cue_end| cue_end > end)
            }) {
                return Err(ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "Scene 时间或追溯集合与所属 Captions cue 不一致。",
                ));
            }
        }
        previous_end = end;
    }
    if previous_end != audio_duration_ms {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidScenePlan,
            "Scene 时间轴没有覆盖完整音频时长。",
        ));
    }
    if let Some(captions_cues) = captions_cues {
        let expected = captions_cues
            .iter()
            .map(|cue| cue["cueId"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        let actual = scenes
            .iter()
            .flat_map(|scene| scene["cueIds"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "Scene cue 顺序必须与 Captions 完全一致。",
            ));
        }
    }
    let changed_ids = document["changeSummary"]["changedSceneIds"]
        .as_array()
        .ok_or_else(|| {
            ScenePlanError::new(
                ScenePlanErrorCode::InvalidScenePlan,
                "changeSummary.changedSceneIds 无效。",
            )
        })?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ScenePlanError::new(
                    ScenePlanErrorCode::InvalidScenePlan,
                    "changedSceneId 必须为字符串。",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut sorted_changed = changed_ids.clone();
    sorted_changed.sort();
    sorted_changed.dedup();
    if sorted_changed != changed_ids {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidScenePlan,
            "changedSceneIds 必须去重并按稳定字典序排列。",
        ));
    }
    if document
        .get("supersedesArtifactId")
        .and_then(Value::as_str)
        .is_some_and(|value| !artifact_id(value))
    {
        return Err(ScenePlanError::new(
            ScenePlanErrorCode::InvalidScenePlan,
            "supersedesArtifactId 无效。",
        ));
    }
    Ok(())
}

fn safe_excerpt(value: &str, max_chars: usize) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    trimmed.chars().take(max_chars).collect()
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:scene-plan:v1\0");
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    format!("{prefix}_{}", lowercase_hex(&hasher.finalize()))
}

fn canonical_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(&canonicalize(value)).expect("Value serialization cannot fail");
    format!("sha256:{}", lowercase_hex(&Sha256::digest(bytes)))
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

fn lowercase_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn required_string(
    value: &Value,
    field: &str,
    code: ScenePlanErrorCode,
) -> Result<String, ScenePlanError> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ScenePlanError::new(code, format!("{field} 缺失或无效。")))
}

fn required_u64(
    value: &Value,
    field: &str,
    code: ScenePlanErrorCode,
) -> Result<u64, ScenePlanError> {
    value[field]
        .as_u64()
        .ok_or_else(|| ScenePlanError::new(code, format!("{field} 缺失或无效。")))
}

fn scene_id(scene: &Value) -> Result<String, ScenePlanError> {
    required_string(scene, "sceneId", ScenePlanErrorCode::InvalidScenePlan)
}

fn bounded_text(value: &str, max_chars: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_chars
        && value.chars().any(|character| !character.is_whitespace())
        && !value.chars().any(|character| character.is_control())
}

fn valid_caption_text(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 2_000
        && value.chars().any(|character| !character.is_whitespace())
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
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

fn run_id(value: &str) -> bool {
    value.len() <= 160 && value.strip_prefix("run_").is_some_and(portable_id)
}

fn artifact_id(value: &str) -> bool {
    value.len() <= 160 && value.strip_prefix("artifact_").is_some_and(portable_id)
}

#[cfg(test)]
mod tests {
    use narracut_contracts::{parse_media_document, validate_media_document};
    use serde_json::{json, Value};

    use super::{
        apply_scene_plan_edits, build_scene_plan_document, validate_scene_plan_document,
        validate_scene_plan_semantics, ScenePlanEditData, ScenePlanErrorCode,
    };
    use crate::BuildScenePlanOptions;

    const AUDIO_DURATION_MS: u64 = 200;

    #[test]
    fn generation_is_deterministic_schema_valid_traceable_and_covers_silence() {
        let captions = captions_document();
        let options = build_options(captions.clone());
        let first = build_scene_plan_document(options.clone()).expect("build Scene Plan");
        let second = build_scene_plan_document(options).expect("repeat Scene Plan");
        assert_eq!(first, second);
        validate_media_document(&first).expect("Scene Plan schema");
        parse_media_document(first.clone()).expect("typed Scene Plan roundtrip");
        validate_scene_plan_semantics(&first, AUDIO_DURATION_MS).expect("Scene semantics");

        let scenes = first["scenes"].as_array().expect("scenes");
        assert_eq!(scenes.len(), 3);
        assert_eq!(scenes[0]["suggestedStartMs"], 0);
        assert_eq!(scenes[0]["suggestedEndMs"], 70);
        assert_eq!(scenes[1]["suggestedStartMs"], 70);
        assert_eq!(scenes[1]["suggestedEndMs"], 165);
        assert_eq!(scenes[2]["suggestedStartMs"], 165);
        assert_eq!(scenes[2]["suggestedEndMs"], AUDIO_DURATION_MS);
        assert_eq!(scenes[0]["cueIds"], json!(["cue_1", "cue_2", "cue_3"]));
        assert_eq!(scenes[1]["cueIds"], json!(["cue_4", "cue_5", "cue_6"]));
        assert_eq!(scenes[2]["cueIds"], json!(["cue_7"]));
        assert_eq!(
            scenes[0]["claimIds"],
            json!(["claim_1", "claim_2", "claim_3"])
        );
        assert_eq!(
            scenes[0]["evidenceRefs"],
            json!(["evidence_1", "evidence_2"])
        );
        assert_eq!(scenes[1]["claimIds"], json!(["claim_4", "claim_5"]));
        assert_eq!(
            scenes[1]["evidenceRefs"],
            json!(["evidence_3", "evidence_4"])
        );
        assert_eq!(scenes[0]["title"], "Opening cue.");
        assert_eq!(scenes[0]["narrativeRole"], "caption_sequence");

        let all_cues = scenes
            .iter()
            .flat_map(|scene| scene["cueIds"].as_array().expect("cue ids"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            all_cues,
            (1..=7)
                .map(|index| json!(format!("cue_{index}")))
                .collect::<Vec<_>>()
        );
        let changed = first["changeSummary"]["changedSceneIds"]
            .as_array()
            .expect("changed scene ids");
        assert!(changed
            .windows(2)
            .all(|pair| pair[0].as_str() < pair[1].as_str()));

        let mut changed_seed = build_options(captions);
        changed_seed.stable_seed = "different-seed".to_owned();
        let changed_seed = build_scene_plan_document(changed_seed).expect("changed seed");
        assert_ne!(first["scenePlanId"], changed_seed["scenePlanId"]);
        assert_ne!(
            first["scenes"][0]["sceneId"],
            changed_seed["scenes"][0]["sceneId"]
        );
    }

    #[test]
    fn split_merge_update_and_move_boundary_are_pure_and_precise() {
        let base = generated_plan();
        let base_before = base.clone();
        let first_id = scene_id(&base, 0);
        let second_id = scene_id(&base, 1);
        let third_id = scene_id(&base, 2);

        let split = apply_scene_plan_edits(
            &base,
            &[ScenePlanEditData::Split {
                scene_id: first_id.clone(),
                boundary_cue_id: "cue_2".to_owned(),
            }],
            "Split the first scene at an exact cue boundary.",
            "run_scene_split",
            "sceneplan_split",
            "2026-07-18T09:00:00Z",
            "artifact_scene_base",
        )
        .expect("split Scene");
        assert_eq!(base, base_before, "base must remain immutable");
        assert_eq!(split["supersedesArtifactId"], "artifact_scene_base");
        assert_eq!(split["scenes"].as_array().map(Vec::len), Some(4));
        assert_eq!(split["scenes"][0]["sceneId"], first_id);
        assert_eq!(split["scenes"][0]["cueIds"], json!(["cue_1"]));
        assert_eq!(split["scenes"][1]["cueIds"], json!(["cue_2", "cue_3"]));
        assert_eq!(split["scenes"][0]["suggestedStartMs"], 0);
        assert_eq!(split["scenes"][1]["suggestedEndMs"], 70);
        let split_right_id = scene_id(&split, 1);
        assert_ne!(split_right_id, first_id);
        assert_changed_ids(&split, &[&first_id, &split_right_id]);
        validate_scene_plan_semantics(&split, AUDIO_DURATION_MS).expect("split semantics");

        let sequential = apply_scene_plan_edits(
            &base,
            &[
                ScenePlanEditData::Update {
                    scene_id: first_id.clone(),
                    title: Some("Updated before split".to_owned()),
                    narrative_role: None,
                },
                ScenePlanEditData::Split {
                    scene_id: first_id.clone(),
                    boundary_cue_id: "cue_2".to_owned(),
                },
                ScenePlanEditData::Update {
                    scene_id: split_right_id.clone(),
                    title: Some("Updated after split".to_owned()),
                    narrative_role: None,
                },
            ],
            "Apply edits sequentially and deduplicate changed IDs.",
            "run_scene_sequential",
            "sceneplan_sequential",
            "2026-07-18T09:00:30Z",
            "artifact_scene_base",
        )
        .expect("sequential edits");
        assert_eq!(sequential["scenes"][0]["title"], "Updated before split");
        assert_eq!(sequential["scenes"][1]["title"], "Updated after split");
        assert_changed_ids(&sequential, &[&first_id, &split_right_id]);

        let merged = apply_scene_plan_edits(
            &split,
            &[ScenePlanEditData::Merge {
                first_scene_id: first_id.clone(),
                second_scene_id: split_right_id.clone(),
            }],
            "Merge the adjacent split scenes.",
            "run_scene_merge",
            "sceneplan_merge",
            "2026-07-18T09:01:00Z",
            "artifact_scene_split",
        )
        .expect("merge Scenes");
        assert_eq!(merged["scenes"].as_array().map(Vec::len), Some(3));
        assert_eq!(merged["scenes"][0]["sceneId"], first_id);
        assert_eq!(
            merged["scenes"][0]["cueIds"],
            json!(["cue_1", "cue_2", "cue_3"])
        );
        assert_changed_ids(&merged, &[&first_id, &split_right_id]);
        assert_eq!(
            merged["scenes"][0]["claimIds"],
            base["scenes"][0]["claimIds"]
        );
        assert_eq!(
            merged["scenes"][0]["evidenceRefs"],
            base["scenes"][0]["evidenceRefs"]
        );

        let updated = apply_scene_plan_edits(
            &base,
            &[ScenePlanEditData::Update {
                scene_id: second_id.clone(),
                title: Some("Reviewed scene title".to_owned()),
                narrative_role: Some("reviewed_caption_sequence".to_owned()),
            }],
            "Update the reviewed scene labels.",
            "run_scene_update",
            "sceneplan_update",
            "2026-07-18T09:02:00Z",
            "artifact_scene_base",
        )
        .expect("update Scene");
        assert_eq!(updated["scenes"][1]["title"], "Reviewed scene title");
        assert_eq!(
            updated["scenes"][1]["narrativeRole"],
            "reviewed_caption_sequence"
        );
        assert_changed_ids(&updated, &[&second_id]);

        let moved = apply_scene_plan_edits(
            &base,
            &[ScenePlanEditData::MoveBoundary {
                left_scene_id: first_id.clone(),
                right_scene_id: second_id.clone(),
                boundary_cue_id: "cue_3".to_owned(),
            }],
            "Move an adjacent cue boundary.",
            "run_scene_move",
            "sceneplan_move",
            "2026-07-18T09:03:00Z",
            "artifact_scene_base",
        )
        .expect("move boundary");
        assert_eq!(moved["scenes"][0]["cueIds"], json!(["cue_1", "cue_2"]));
        assert_eq!(
            moved["scenes"][1]["cueIds"],
            json!(["cue_3", "cue_4", "cue_5", "cue_6"])
        );
        assert_eq!(
            moved["scenes"][0]["suggestedEndMs"],
            moved["scenes"][1]["suggestedStartMs"]
        );
        assert_changed_ids(&moved, &[&first_id, &second_id]);
        assert_eq!(
            moved["scenes"][0]["claimIds"],
            json!(["claim_1", "claim_2"])
        );
        assert_eq!(moved["scenes"][0]["evidenceRefs"], json!(["evidence_1"]));
        assert_eq!(
            moved["scenes"][0]["provenance"],
            json!([
                {"claimId":"claim_1","evidenceRef":"evidence_1"},
                {"claimId":"claim_2","evidenceRef":"evidence_1"}
            ])
        );
        assert_eq!(
            moved["scenes"][1]["claimIds"],
            json!(["claim_3", "claim_4", "claim_5"])
        );
        assert_eq!(
            moved["scenes"][1]["evidenceRefs"],
            json!(["evidence_2", "evidence_3", "evidence_4"])
        );
        assert_eq!(
            moved["scenes"][1]["provenance"],
            json!([
                {"claimId":"claim_3","evidenceRef":"evidence_2"},
                {"claimId":"claim_4","evidenceRef":"evidence_3"},
                {"claimId":"claim_5","evidenceRef":"evidence_4"}
            ])
        );
        assert_eq!(moved["scenes"][2]["sceneId"], third_id);
        validate_scene_plan_semantics(&moved, AUDIO_DURATION_MS).expect("move semantics");
    }

    #[test]
    fn edit_validation_rejects_illegal_boundaries_non_adjacent_scenes_controls_and_limits_atomically(
    ) {
        let base = generated_plan();
        let original = base.clone();
        let first_id = scene_id(&base, 0);
        let second_id = scene_id(&base, 1);
        let third_id = scene_id(&base, 2);

        for edit in [
            ScenePlanEditData::Split {
                scene_id: first_id.clone(),
                boundary_cue_id: "cue_1".to_owned(),
            },
            ScenePlanEditData::Split {
                scene_id: third_id.clone(),
                boundary_cue_id: "cue_7".to_owned(),
            },
            ScenePlanEditData::Merge {
                first_scene_id: first_id.clone(),
                second_scene_id: third_id.clone(),
            },
            ScenePlanEditData::Merge {
                first_scene_id: second_id.clone(),
                second_scene_id: first_id.clone(),
            },
            ScenePlanEditData::Update {
                scene_id: first_id.clone(),
                title: None,
                narrative_role: None,
            },
            ScenePlanEditData::Update {
                scene_id: first_id.clone(),
                title: Some("bad\ncontrol".to_owned()),
                narrative_role: None,
            },
            ScenePlanEditData::Update {
                scene_id: first_id.clone(),
                title: Some("x".repeat(97)),
                narrative_role: None,
            },
            ScenePlanEditData::MoveBoundary {
                left_scene_id: first_id.clone(),
                right_scene_id: third_id.clone(),
                boundary_cue_id: "cue_4".to_owned(),
            },
            ScenePlanEditData::MoveBoundary {
                left_scene_id: first_id.clone(),
                right_scene_id: second_id.clone(),
                boundary_cue_id: "cue_4".to_owned(),
            },
        ] {
            assert_eq!(apply_error(&base, &[edit]), ScenePlanErrorCode::InvalidEdit);
            assert_eq!(base, original);
        }

        assert_eq!(apply_error(&base, &[]), ScenePlanErrorCode::InvalidRequest);
        let too_many = (0..1_025)
            .map(|_| ScenePlanEditData::Update {
                scene_id: first_id.clone(),
                title: Some("bounded".to_owned()),
                narrative_role: None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            apply_error(&base, &too_many),
            ScenePlanErrorCode::InvalidRequest
        );

        let error = apply_scene_plan_edits(
            &base,
            &[
                ScenePlanEditData::Update {
                    scene_id: first_id.clone(),
                    title: Some("would have changed".to_owned()),
                    narrative_role: None,
                },
                ScenePlanEditData::Split {
                    scene_id: third_id,
                    boundary_cue_id: "cue_7".to_owned(),
                },
            ],
            "Atomic edit sequence.",
            "run_scene_atomic",
            "sceneplan_atomic",
            "2026-07-18T09:10:00Z",
            "artifact_scene_base",
        )
        .expect_err("later invalid edit rejects whole sequence");
        assert_eq!(error.code, ScenePlanErrorCode::InvalidEdit);
        assert_eq!(base, original, "failed edit sequence cannot mutate base");

        let error = apply_scene_plan_edits(
            &base,
            &[ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("valid".to_owned()),
                narrative_role: None,
            }],
            "bad\nsummary",
            "run_scene_invalid_summary",
            "sceneplan_invalid_summary",
            "2026-07-18T09:11:00Z",
            "artifact_scene_base",
        )
        .expect_err("control character in summary");
        assert_eq!(error.code, ScenePlanErrorCode::InvalidRequest);

        let error = apply_scene_plan_edits(
            &base,
            &[ScenePlanEditData::Update {
                scene_id: scene_id(&base, 0),
                title: Some("valid".to_owned()),
                narrative_role: None,
            }],
            "Identity must advance.",
            "run_scene_base",
            "sceneplan_same_is_rejected",
            "2026-07-18T09:12:00Z",
            "artifact_scene_base",
        )
        .expect_err("same run identity");
        assert_eq!(error.code, ScenePlanErrorCode::InvalidRequest);
    }

    #[test]
    fn legacy_v1_traceability_only_migrates_empty_or_single_unambiguous_pairs() {
        let mut single = generated_plan();
        single
            .as_object_mut()
            .expect("Scene Plan object")
            .remove("cueTraceability");
        single["schemaVersion"] = json!("1.0.0");
        for scene in single["scenes"].as_array_mut().expect("scenes") {
            scene
                .as_object_mut()
                .expect("scene object")
                .remove("provenance");
            scene["claimIds"] = json!(["claim_legacy"]);
            scene["evidenceRefs"] = json!(["evidence_legacy"]);
        }
        validate_scene_plan_semantics(&single, AUDIO_DURATION_MS)
            .expect("single legacy pair is unambiguous");
        let first_id = scene_id(&single, 0);
        let migrated = apply_scene_plan_edits(
            &single,
            &[ScenePlanEditData::Update {
                scene_id: first_id,
                title: Some("Migrated legacy scene".to_owned()),
                narrative_role: None,
            }],
            "Migrate unambiguous legacy traceability.",
            "run_scene_legacy_migrated",
            "sceneplan_legacy_migrated",
            "2026-07-18T09:04:00Z",
            "artifact_scene_legacy",
        )
        .expect("migrate single legacy pair");
        assert_eq!(
            migrated["cueTraceability"].as_array().map(Vec::len),
            Some(7)
        );
        assert!(migrated["scenes"]
            .as_array()
            .expect("scenes")
            .iter()
            .all(|scene| scene["provenance"]
                == json!([{"claimId":"claim_legacy","evidenceRef":"evidence_legacy"}])));

        let mut empty = single.clone();
        for scene in empty["scenes"].as_array_mut().expect("scenes") {
            scene["claimIds"] = json!([]);
            scene["evidenceRefs"] = json!([]);
        }
        validate_scene_plan_semantics(&empty, AUDIO_DURATION_MS)
            .expect("empty legacy traceability is unambiguous");

        let mut ambiguous = generated_plan();
        ambiguous
            .as_object_mut()
            .expect("Scene Plan object")
            .remove("cueTraceability");
        ambiguous["schemaVersion"] = json!("1.0.0");
        for scene in ambiguous["scenes"].as_array_mut().expect("scenes") {
            scene
                .as_object_mut()
                .expect("scene object")
                .remove("provenance");
        }
        assert!(validate_scene_plan_semantics(&ambiguous, AUDIO_DURATION_MS).is_err());

        let mut ambiguous_caption = captions_document();
        ambiguous_caption["schemaVersion"] = json!("1.0.0");
        for cue in ambiguous_caption["cues"].as_array_mut().expect("cues") {
            cue.as_object_mut()
                .expect("cue object")
                .remove("provenance");
        }
        assert_eq!(
            build_scene_plan_document(build_options(ambiguous_caption))
                .expect_err("multi-valued legacy cue needs explicit pairs")
                .code,
            ScenePlanErrorCode::InvalidCaptions
        );
    }

    #[test]
    fn semantic_validator_rejects_duplicates_missing_cues_holes_overlaps_trace_drift_and_unstable_changes(
    ) {
        let captions = captions_document();
        let caption_cues = captions["cues"].as_array().expect("caption cues");
        let base = generated_plan();

        let mut duplicate_cue = base.clone();
        duplicate_cue["scenes"][1]["cueIds"][0] = json!("cue_3");
        assert_semantic_error(&duplicate_cue);

        let mut missing_cue = base.clone();
        missing_cue["scenes"][0]["cueIds"] = json!(["cue_1", "cue_2"]);
        assert!(
            validate_scene_plan_document(&missing_cue, AUDIO_DURATION_MS, Some(caption_cues))
                .is_err()
        );

        let mut hole = base.clone();
        hole["scenes"][1]["suggestedStartMs"] = json!(71);
        assert_semantic_error(&hole);

        let mut overlap = base.clone();
        overlap["scenes"][1]["suggestedStartMs"] = json!(69);
        assert_semantic_error(&overlap);

        let mut duplicate_scene = base.clone();
        duplicate_scene["scenes"][1]["sceneId"] = duplicate_scene["scenes"][0]["sceneId"].clone();
        assert_semantic_error(&duplicate_scene);

        let mut bad_order = base.clone();
        bad_order["scenes"][1]["order"] = json!(7);
        assert_semantic_error(&bad_order);

        let mut incomplete_duration = base.clone();
        incomplete_duration["scenes"][2]["suggestedEndMs"] = json!(199);
        assert_semantic_error(&incomplete_duration);

        let mut trace_drift = base.clone();
        trace_drift["scenes"][0]["claimIds"] = json!(["claim_wrong"]);
        assert!(
            validate_scene_plan_document(&trace_drift, AUDIO_DURATION_MS, Some(caption_cues))
                .is_err()
        );

        let mut unstable_changes = base.clone();
        unstable_changes["changeSummary"]["changedSceneIds"] = json!(["scene_z", "scene_a"]);
        assert_semantic_error(&unstable_changes);
    }

    #[test]
    fn generation_rejects_project_duration_and_caption_semantic_mismatches_and_safely_truncates_titles(
    ) {
        let mut wrong_project_captions = captions_document();
        wrong_project_captions["projectId"] = json!("project_other");
        assert_eq!(
            build_scene_plan_document(build_options(wrong_project_captions))
                .expect_err("cross-project Captions")
                .code,
            ScenePlanErrorCode::InvalidCaptions
        );

        let mut cross_project_ref = build_options(captions_document());
        cross_project_ref.input_refs[0]["projectId"] = json!("project_other");
        assert_eq!(
            build_scene_plan_document(cross_project_ref)
                .expect_err("cross-project inputRef")
                .code,
            ScenePlanErrorCode::InvalidRequest
        );

        let mut short_audio = build_options(captions_document());
        short_audio.audio_duration_ms = 189;
        assert_eq!(
            build_scene_plan_document(short_audio)
                .expect_err("cue outside Audio")
                .code,
            ScenePlanErrorCode::InvalidCaptions
        );

        let mut overlapping = captions_document();
        overlapping["cues"][3]["startMs"] = json!(59);
        assert_eq!(
            build_scene_plan_document(build_options(overlapping))
                .expect_err("overlapping Captions")
                .code,
            ScenePlanErrorCode::InvalidCaptions
        );

        let mut long_title = captions_document();
        long_title["cues"][0]["text"] = json!(format!("Opening\n{}", "界".repeat(150)));
        let plan = build_scene_plan_document(build_options(long_title)).expect("long title plan");
        let title = plan["scenes"][0]["title"].as_str().expect("scene title");
        assert_eq!(title.chars().count(), 96);
        assert!(!title.chars().any(char::is_control));
        assert!(title.starts_with("Opening "));
    }

    fn apply_error(base: &Value, edits: &[ScenePlanEditData]) -> ScenePlanErrorCode {
        apply_scene_plan_edits(
            base,
            edits,
            "Bounded edit fixture.",
            "run_scene_error",
            "sceneplan_error",
            "2026-07-18T09:20:00Z",
            "artifact_scene_base",
        )
        .expect_err("edit must fail")
        .code
    }

    fn assert_semantic_error(document: &Value) {
        assert!(validate_scene_plan_semantics(document, AUDIO_DURATION_MS).is_err());
    }

    fn generated_plan() -> Value {
        build_scene_plan_document(build_options(captions_document())).expect("fixture Scene Plan")
    }

    fn build_options(captions_document: Value) -> BuildScenePlanOptions {
        BuildScenePlanOptions {
            captions_document,
            audio_duration_ms: AUDIO_DURATION_MS,
            input_refs: frozen_inputs(),
            config_snapshot: json!({"grouping":"three-cues","locale":"zh-CN"}),
            project_id: "project_scene".to_owned(),
            run_id: "run_scene_base".to_owned(),
            stable_seed: "scene-seed-v1".to_owned(),
            created_at: "2026-07-18T08:00:00Z".to_owned(),
        }
    }

    fn captions_document() -> Value {
        let cues = vec![
            cue(1, 10, 20, "Opening cue.", &[("claim_1", "evidence_1")]),
            cue(
                2,
                25,
                40,
                "Second cue.",
                &[("claim_2", "evidence_1"), ("claim_1", "evidence_1")],
            ),
            cue(3, 50, 60, "Third cue.", &[("claim_3", "evidence_2")]),
            cue(4, 80, 90, "Fourth cue.", &[("claim_4", "evidence_3")]),
            cue(
                5,
                100,
                120,
                "Fifth cue.",
                &[("claim_4", "evidence_3"), ("claim_5", "evidence_4")],
            ),
            cue(6, 130, 150, "Sixth cue.", &[]),
            cue(7, 180, 190, "Final cue.", &[("claim_7", "evidence_7")]),
        ];
        json!({
            "schemaVersion": narracut_contracts::NARRACUT_MEDIA_SCHEMA_VERSION,
            "documentType": "captions_media",
            "captionsId": "captions_fixture",
            "projectId": "project_scene",
            "runId": "run_captions_fixture",
            "rawArtifactId": "artifact_captions_raw",
            "rawContentHash": sha('a'),
            "source": {
                "sourceFileName": "fixture.srt",
                "sourceContentHash": sha('a'),
                "byteLength": 256,
            },
            "audioInput": frozen_inputs()[1].clone(),
            "cues": cues,
            "mappings": [{
                "mappingId": "mapping_fixture",
                "level": "cue",
                "sourceCueId": "cue_1",
                "startMs": 10,
                "endMs": 20,
                "text": "Opening cue.",
                "timingPrecision": "cue_exact",
                "timingBasis": "srt_cue",
            }],
            "diagnostics": [],
            "inputRefs": frozen_inputs(),
            "configSnapshot": {"language":"en"},
            "createdAt": "2026-07-18T07:00:00Z",
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
        let mut seen_claims = std::collections::BTreeSet::new();
        let mut seen_evidence = std::collections::BTreeSet::new();
        for (claim_id, evidence_ref) in provenance {
            if seen_claims.insert(*claim_id) {
                claim_ids.push(*claim_id);
            }
            if seen_evidence.insert(*evidence_ref) {
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

    fn frozen_inputs() -> Vec<Value> {
        vec![
            json!({
                "projectId": "project_scene",
                "stageId": "script",
                "runId": "run_script_fixture",
                "artifactId": "artifact_script_fixture",
                "contentHash": sha('b'),
                "reviewRecordId": "review_script_fixture",
                "claimIds": ["claim_1"],
                "evidenceRefs": ["evidence_1"],
            }),
            json!({
                "projectId": "project_scene",
                "stageId": "audio",
                "runId": "run_audio_fixture",
                "artifactId": "artifact_audio_fixture",
                "contentHash": sha('c'),
                "reviewRecordId": "review_audio_fixture",
                "claimIds": ["claim_1"],
                "evidenceRefs": ["evidence_1"],
            }),
        ]
    }

    fn sha(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn scene_id(document: &Value, index: usize) -> String {
        document["scenes"][index]["sceneId"]
            .as_str()
            .expect("scene id")
            .to_owned()
    }

    fn assert_changed_ids(document: &Value, expected: &[&str]) {
        let mut expected = expected
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            document["changeSummary"]["changedSceneIds"],
            json!(expected)
        );
    }
}
