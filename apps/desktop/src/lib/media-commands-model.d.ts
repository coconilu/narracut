import type { MediaCommandError } from "@narracut/contracts";

export const mediaCommandErrorCodes: readonly [
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
];

export const mediaOperations: readonly [
  "enqueue_audio_import",
  "enqueue_captions_import",
  "get_media_document",
  "generate_scene_plan",
  "save_scene_plan",
  "generate_timeline",
  "save_timeline",
];

export function isMediaCommandError(value: unknown): value is MediaCommandError;
