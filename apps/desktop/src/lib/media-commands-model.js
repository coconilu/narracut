export const mediaCommandErrorCodes = Object.freeze([
  "invalid_request",
  "project_not_found",
  "project_identity_mismatch",
  "source_not_file",
  "source_link_rejected",
  "source_changed",
  "source_too_large",
  "unsupported_media",
  "invalid_wav",
  "invalid_utf8",
  "invalid_srt",
  "cue_overlap",
  "cue_out_of_audio_range",
  "review_required",
  "input_stale",
  "input_hash_mismatch",
  "cross_project_reference",
  "traceability_incomplete",
  "invalid_scene_boundary",
  "invalid_safe_area",
  "artifact_not_found",
  "job_conflict",
  "canceled",
  "io_error",
  "internal_contract_error",
]);

export const mediaOperations = Object.freeze([
  "enqueue_audio_import",
  "enqueue_captions_import",
  "get_media_document",
  "generate_scene_plan",
  "save_scene_plan",
  "generate_timeline",
  "save_timeline",
]);

const errorCodeSet = new Set(mediaCommandErrorCodes);
const operationSet = new Set(mediaOperations);
const errorKeys = new Set([
  "apiVersion",
  "code",
  "operation",
  "message",
  "retryable",
  "stageId",
  "runId",
  "artifactId",
  "diagnosticIds",
]);
const portableIdPattern = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const runIdPattern = /^run_[A-Za-z0-9][A-Za-z0-9._-]*$/;
const artifactIdPattern = /^artifact_[A-Za-z0-9][A-Za-z0-9._-]*$/;

export function isMediaCommandError(value) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const candidate = value;
  return (
    Object.keys(candidate).every((key) => errorKeys.has(key)) &&
    candidate.apiVersion === "1.0.0" &&
    typeof candidate.code === "string" &&
    errorCodeSet.has(candidate.code) &&
    typeof candidate.operation === "string" &&
    operationSet.has(candidate.operation) &&
    boundedString(candidate.message, 1, 2048) &&
    typeof candidate.retryable === "boolean" &&
    optionalId(candidate.stageId, portableIdPattern, 1) &&
    optionalId(candidate.runId, runIdPattern, 5) &&
    optionalId(candidate.artifactId, artifactIdPattern, 10) &&
    optionalStringSet(candidate.diagnosticIds)
  );
}

function optionalId(value, pattern, minimumLength) {
  return (
    value === undefined ||
    (boundedString(value, minimumLength, 160) && pattern.test(value))
  );
}

function optionalStringSet(value) {
  if (value === undefined) return true;
  if (!Array.isArray(value) || value.length > 1024) return false;
  return (
    value.every((item) => boundedString(item, 1, 512)) &&
    new Set(value).size === value.length
  );
}

function boundedString(value, minimumLength, maximumLength) {
  if (typeof value !== "string") return false;
  const length = [...value].length;
  return length >= minimumLength && length <= maximumLength;
}
