/* eslint-disable */
/**
 * 此文件由 schema/narracut-media-commands-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * Typed high-level media import, query, scene-plan, and timeline command messages.
 */
export type NarraCutMediaCommandMessage =
  | EnqueueAudioImportRequest
  | EnqueueCaptionsImportRequest
  | EnqueueMediaReauthorizationRequest
  | GetMediaDocumentRequest
  | GenerateScenePlanRequest
  | SaveScenePlanRequest
  | GenerateTimelineRequest
  | SaveTimelineRequest
  | MediaJobAcceptedResult
  | MediaDocumentResult
  | MediaSaveResult
  | MediaCommandError;
export type EnqueueAudioImportRequest =
  EnqueueAudioImportRequestV1_0 | EnqueueAudioImportRequestV1_1;
export type LegacyApiVersion = "1.0.0";
export type ProjectPath = string;
export type PortableId = string;
export type RunId = string;
export type SourcePath = string;
export type Sha256 = string;
export type ArtifactId = string;
/**
 * @maxItems 1024
 */
export type StringSet = string[];
export type IdempotencyKey = string;
export type CurrentApiVersion = "1.1.0";
export type EnqueueCaptionsImportRequest =
  EnqueueCaptionsImportRequestV1_0 | EnqueueCaptionsImportRequestV1_1;
export type ApiVersion = "1.0.0" | "1.1.0";
export type ScenePlanEdit =
  SplitSceneEdit | MergeScenesEdit | UpdateSceneEdit | MoveSceneBoundaryEdit;
export type TimelineEdit = MoveSceneBoundaryEdit | SetSafeAreaEdit | SetCaptionVisibilityEdit;
export type MediaOperation =
  | "enqueue_audio_import"
  | "enqueue_captions_import"
  | "enqueue_media_reauthorization"
  | "reauthorize_media"
  | "get_media_document"
  | "generate_scene_plan"
  | "save_scene_plan"
  | "generate_timeline"
  | "save_timeline";
export type JobId = string;

export interface EnqueueAudioImportRequestV1_0 {
  readonly apiVersion: LegacyApiVersion;
  readonly command: "enqueue_audio_import";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly sourcePath: SourcePath;
  readonly expectedSourceContentHash?: Sha256;
  readonly scriptInput: MediaReviewedInputReference;
  readonly rights: LegacyMediaRightsInput;
  readonly limits: MediaImportLimits;
  readonly idempotencyKey: IdempotencyKey;
}
export interface MediaReviewedInputReference {
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  readonly contentHash: Sha256;
  readonly reviewRecordId: PortableId;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
/**
 * Exact 1.0 request/receipt rights shape; accepted only for historical read and fail-closed execution.
 */
export interface LegacyMediaRightsInput {
  readonly ownership: "self_recorded" | "licensed";
  readonly author: string;
  readonly rightsStatement: string;
  readonly licenseId: string;
  readonly attributionText: string;
  readonly voiceAuthorization: "not_voice_clone";
}
export interface MediaImportLimits {
  readonly maxBytes: number;
  readonly maxCueCount?: number;
  readonly maxCueTextBytes?: number;
}
export interface EnqueueAudioImportRequestV1_1 {
  readonly apiVersion: CurrentApiVersion;
  readonly command: "enqueue_audio_import";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly sourcePath: SourcePath;
  readonly expectedSourceContentHash?: Sha256;
  readonly scriptInput: MediaReviewedInputReference;
  readonly rights: MediaRightsInput;
  readonly limits: MediaImportLimits;
  readonly idempotencyKey: IdempotencyKey;
}
export interface MediaRightsInput {
  readonly ownership: "self_recorded" | "licensed";
  readonly author: string;
  readonly rightsStatement: string;
  readonly licenseId: string;
  readonly attributionText: string;
  /**
   * @minItems 1
   * @maxItems 32
   */
  readonly authorizationRecords: readonly [AuthorizationRecordInput, ...AuthorizationRecordInput[]];
  readonly voiceAuthorization: VoiceAuthorizationApplicability;
}
export interface AuthorizationRecordInput {
  readonly authorizationRecordId: string;
  readonly authorizationType: "material_use";
  readonly grantor: string;
  readonly scope: string;
  readonly evidenceRef: string;
  readonly recordedAt: string;
}
export interface VoiceAuthorizationApplicability {
  readonly applicability: "not_applicable";
  readonly reason: "not_voice_clone";
}
export interface EnqueueCaptionsImportRequestV1_0 {
  readonly apiVersion: LegacyApiVersion;
  readonly command: "enqueue_captions_import";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly sourcePath: SourcePath;
  readonly expectedSourceContentHash?: Sha256;
  readonly scriptInput: MediaReviewedInputReference;
  readonly audioInput: MediaReviewedInputReference;
  readonly audioDurationMs: number;
  readonly rights: LegacyMediaRightsInput;
  readonly limits: MediaImportLimits;
  readonly idempotencyKey: IdempotencyKey;
}
export interface EnqueueCaptionsImportRequestV1_1 {
  readonly apiVersion: CurrentApiVersion;
  readonly command: "enqueue_captions_import";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly sourcePath: SourcePath;
  readonly expectedSourceContentHash?: Sha256;
  readonly scriptInput: MediaReviewedInputReference;
  readonly audioInput: MediaReviewedInputReference;
  readonly audioDurationMs: number;
  readonly rights: MediaRightsInput;
  readonly limits: MediaImportLimits;
  readonly idempotencyKey: IdempotencyKey;
}
export interface EnqueueMediaReauthorizationRequest {
  readonly apiVersion: CurrentApiVersion;
  readonly command: "enqueue_media_reauthorization";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly baseArtifactId: ArtifactId;
  readonly rights: MediaRightsInput;
  readonly idempotencyKey: IdempotencyKey;
}
export interface GetMediaDocumentRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "get_media_document";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly artifactId: ArtifactId;
}
export interface GenerateScenePlanRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "generate_scene_plan";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly researchInput: MediaReviewedInputReference;
  readonly scriptInput: MediaReviewedInputReference;
  readonly captionsInput: MediaReviewedInputReference;
  readonly idempotencyKey: IdempotencyKey;
}
export interface SaveScenePlanRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "save_scene_plan";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly baseArtifactId: ArtifactId;
  /**
   * @minItems 1
   * @maxItems 1000
   */
  readonly edits: readonly [ScenePlanEdit, ...ScenePlanEdit[]];
  readonly changeSummary: string;
  readonly idempotencyKey: IdempotencyKey;
}
export interface SplitSceneEdit {
  readonly editType: "split";
  readonly sceneId: PortableId;
  readonly splitAtMs: number;
}
export interface MergeScenesEdit {
  readonly editType: "merge";
  readonly firstSceneId: PortableId;
  readonly secondSceneId: PortableId;
}
export interface UpdateSceneEdit {
  readonly editType: "update";
  readonly sceneId: PortableId;
  readonly title?: string;
  readonly narrativeRole?: string;
}
export interface MoveSceneBoundaryEdit {
  readonly editType: "move_boundary";
  readonly leftSceneId: PortableId;
  readonly rightSceneId: PortableId;
  readonly boundaryMs: number;
}
export interface GenerateTimelineRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "generate_timeline";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly audioInput: MediaReviewedInputReference;
  readonly captionsInput: MediaReviewedInputReference;
  readonly scenePlanInput: MediaReviewedInputReference;
  readonly canvas: TimelineCanvasInput;
  readonly safeArea: TimelineSafeAreaInput;
  readonly idempotencyKey: IdempotencyKey;
}
export interface TimelineCanvasInput {
  readonly width: number;
  readonly height: number;
  readonly frameRateNumerator: number;
  readonly frameRateDenominator: number;
}
export interface TimelineSafeAreaInput {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}
export interface SaveTimelineRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "save_timeline";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly baseArtifactId: ArtifactId;
  /**
   * @minItems 1
   * @maxItems 1000
   */
  readonly edits: readonly [TimelineEdit, ...TimelineEdit[]];
  readonly changeSummary: string;
  readonly idempotencyKey: IdempotencyKey;
}
export interface SetSafeAreaEdit {
  readonly editType: "set_safe_area";
  readonly safeArea: TimelineSafeAreaInput;
}
export interface SetCaptionVisibilityEdit {
  readonly editType: "set_caption_visibility";
  readonly visible: boolean;
}
export interface MediaJobAcceptedResult {
  readonly apiVersion: ApiVersion;
  readonly operation: MediaOperation;
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly jobId: JobId;
  readonly idempotentReplay: boolean;
}
export interface MediaDocumentResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: PortableId;
  readonly artifactId: ArtifactId;
  readonly contentHash: Sha256;
  readonly document: MediaDocumentValue;
}
export interface MediaDocumentValue {
  readonly schemaVersion: "1.0.0" | "1.1.0" | "1.2.0";
  readonly documentType: "audio_media" | "captions_media" | "scene_plan" | "timeline";
  readonly projectId: PortableId;
  readonly runId: RunId;
  readonly [k: string]: unknown | undefined;
}
export interface MediaSaveResult {
  readonly apiVersion: ApiVersion;
  readonly operation: MediaOperation;
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  /**
   * @maxItems 10000
   */
  readonly changedSceneIds: readonly PortableId[];
  /**
   * @maxItems 16
   */
  readonly staleBecauseStageIds:
    | []
    | [PortableId]
    | [PortableId, PortableId]
    | [PortableId, PortableId, PortableId]
    | [PortableId, PortableId, PortableId, PortableId]
    | [PortableId, PortableId, PortableId, PortableId, PortableId]
    | [PortableId, PortableId, PortableId, PortableId, PortableId, PortableId]
    | [PortableId, PortableId, PortableId, PortableId, PortableId, PortableId, PortableId]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ]
    | [
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
        PortableId,
      ];
  readonly idempotentReplay: boolean;
}
export interface MediaCommandError {
  readonly apiVersion: ApiVersion;
  readonly code:
    | "invalid_request"
    | "project_not_found"
    | "project_identity_mismatch"
    | "source_not_file"
    | "source_link_rejected"
    | "source_changed"
    | "source_too_large"
    | "unsupported_media"
    | "invalid_wav"
    | "invalid_utf8"
    | "invalid_srt"
    | "cue_overlap"
    | "cue_out_of_audio_range"
    | "review_required"
    | "rights_upgrade_required"
    | "input_stale"
    | "input_hash_mismatch"
    | "cross_project_reference"
    | "traceability_incomplete"
    | "invalid_scene_boundary"
    | "invalid_safe_area"
    | "artifact_not_found"
    | "job_conflict"
    | "canceled"
    | "io_error"
    | "internal_contract_error";
  readonly operation: MediaOperation;
  readonly message: string;
  readonly retryable: boolean;
  readonly stageId?: PortableId;
  readonly runId?: RunId;
  readonly artifactId?: ArtifactId;
  readonly diagnosticIds?: StringSet;
}
