use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::{
    ProviderError, ProviderErrorCode, ProviderOperation, StructuredProviderRequestData,
    StructuredScriptOutputData,
};

pub(crate) const SCRIPT_INSTRUCTIONS: &str = "你是 NarraCut 的事实脚本编排器。只根据输入中已审核的资料生成结构化口播脚本；不得新增或重新组合主张与证据；每个片段必须原样采用输入 provenance 中存在的 claimId/evidenceRef 对。只能返回符合给定 JSON Schema 的最终 JSON，不得调用命令、修改文件、访问网络、MCP 或其他工具。";

pub(crate) fn script_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["schemaVersion", "title", "language", "summary", "estimatedDurationSeconds", "segments"],
        "properties": {
            "schemaVersion": {"type": "string", "const": "narracut.script/v1"},
            "title": {"type": "string", "minLength": 1, "maxLength": 240},
            "language": {"type": "string", "minLength": 2, "maxLength": 35},
            "summary": {"type": "string", "minLength": 1, "maxLength": 2000},
            "estimatedDurationSeconds": {"type": "number", "exclusiveMinimum": 0, "maximum": 21600},
            "segments": {
                "type": "array", "minItems": 1, "maxItems": 128,
                "items": {
                    "type": "object", "additionalProperties": false,
                    "required": ["segmentId", "order", "title", "narration", "provenance"],
                    "properties": {
                        "segmentId": {"type": "string", "pattern": "^segment_[A-Za-z0-9][A-Za-z0-9._-]{0,151}$"},
                        "order": {"type": "integer", "minimum": 0, "maximum": 1023},
                        "title": {"type": "string", "minLength": 1, "maxLength": 240},
                        "narration": {"type": "string", "minLength": 1, "maxLength": 8000},
                        "provenance": {
                            "type": "array", "minItems": 1, "maxItems": 128,
                            "items": {
                                "type": "object", "additionalProperties": false,
                                "required": ["claimId", "evidenceRef"],
                                "properties": {
                                    "claimId": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$"},
                                    "evidenceRef": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$"}
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub(crate) fn validate_reference_subset(
    provider_id: &str,
    request: &StructuredProviderRequestData,
    output: &StructuredScriptOutputData,
) -> Result<(), ProviderError> {
    let allowed_pairs = request
        .inputs
        .iter()
        .flat_map(|input| input.provenance.iter())
        .map(|reference| (&reference.claim_id, &reference.evidence_ref))
        .collect::<BTreeSet<_>>();
    let mut segment_ids = BTreeSet::new();
    let mut orders = BTreeSet::new();
    for segment in &output.segments {
        if !segment_ids.insert(&segment.segment_id) || !orders.insert(segment.order) {
            return Err(invalid_response(
                provider_id,
                "脚本片段的 segmentId 与 order 必须唯一。",
            ));
        }
        if segment.provenance.iter().any(|reference| {
            !allowed_pairs.contains(&(&reference.claim_id, &reference.evidence_ref))
        }) {
            return Err(invalid_response(
                provider_id,
                "脚本输出包含未出现在已审核输入中的 claimId/evidenceRef 对。",
            ));
        }
    }
    Ok(())
}

fn invalid_response(provider_id: &str, message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::ProviderResponseInvalid,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider(provider_id)
}
