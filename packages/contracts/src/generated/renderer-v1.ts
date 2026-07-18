/* eslint-disable */
/**
 * 此文件由 schema/narracut-renderer-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * Versioned, high-level renderer commands and immutable renderer results.
 */
export type NarraCutRendererMessage =
  | ProbeRendererRequest
  | CreateSceneSnapshotRequest
  | EnqueueSceneRenderRequest
  | EnqueueTimelineRenderRequest
  | GetRenderResultRequest
  | RendererCapabilitiesResult
  | SceneSnapshotResult
  | RenderJobAcceptedResult
  | RenderResult
  | RenderEvent
  | RendererCommandError;
export type ApiVersion = "1.0.0";
export type ProjectPath = string;
export type PortableId = string;
export type RunId = string;
export type ArtifactId = string;
export type Sha256 = string;
/**
 * @maxItems 1024
 */
export type StringSet = string[];
export type IdempotencyKey = string;
export type JobId = string;
export type ProjectUri = string;
export type RendererOperation =
  | "probe_renderer"
  | "create_scene_snapshot"
  | "enqueue_scene_render"
  | "enqueue_timeline_render"
  | "get_render_result"
  | "render_event";

export interface ProbeRendererRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "probe_renderer";
}
export interface CreateSceneSnapshotRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "create_scene_snapshot";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly timelineInput: RendererTimelineInputReference;
  readonly sceneId: PortableId;
}
export interface RendererTimelineInputReference {
  readonly stageId: "timeline";
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  readonly contentHash: Sha256;
  readonly reviewRecordId: PortableId;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
export interface EnqueueSceneRenderRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "enqueue_scene_render";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly timelineInput: RendererTimelineInputReference;
  readonly sceneId: PortableId;
  readonly config: RendererConfig;
  readonly idempotencyKey: IdempotencyKey;
}
export interface RendererConfig {
  readonly canvas: RendererCanvas;
  readonly videoCodec: "libx264";
  readonly audioCodec: "aac";
  readonly pixelFormat: "yuv420p";
  readonly preset: "veryfast" | "faster" | "fast" | "medium";
  readonly crf: number;
  readonly maxDurationMs: number;
  readonly maxTemporaryBytes: number;
  readonly timeoutMs: number;
}
export interface RendererCanvas {
  readonly width: number;
  readonly height: number;
  readonly frameRateNumerator: number;
  readonly frameRateDenominator: number;
}
export interface EnqueueTimelineRenderRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "enqueue_timeline_render";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly timelineInput: RendererTimelineInputReference;
  readonly config: RendererConfig;
  readonly idempotencyKey: IdempotencyKey;
}
export interface GetRenderResultRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "get_render_result";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly jobId: JobId;
}
export interface RendererCapabilitiesResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "probe_renderer";
  readonly available: boolean;
  readonly supported: boolean;
  readonly identity: RendererIdentity;
  readonly limits: RendererLimits;
  /**
   * @maxItems 1
   */
  readonly videoCodecs: readonly [] | ["libx264"];
  /**
   * @maxItems 1
   */
  readonly audioCodecs: readonly [] | ["aac"];
  /**
   * @maxItems 64
   */
  readonly diagnostics: readonly RendererDiagnostic[];
}
export interface RendererIdentity {
  readonly adapterId: "narracut.ffmpeg";
  readonly adapterVersion: "1.0.0";
  readonly executableFileName: "ffmpeg" | "ffmpeg.exe";
  readonly executableHash: Sha256;
  readonly ffmpegVersion: string;
  readonly capabilityHash: Sha256;
}
export interface RendererLimits {
  readonly maxScenes: number;
  readonly maxSnapshotBytes: number;
  readonly maxResourceBytes: number;
  readonly maxLogBytes: number;
  readonly maxConcurrentJobs: number;
}
export interface RendererDiagnostic {
  readonly diagnosticId: PortableId;
  readonly severity: "info" | "warning" | "error";
  readonly code: PortableId;
  readonly message: string;
}
export interface SceneSnapshotResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "create_scene_snapshot";
  readonly ownerProjectId: PortableId;
  readonly snapshot: SceneSnapshot;
}
export interface SceneSnapshot {
  readonly snapshotVersion: "1.0.0";
  readonly snapshotId: PortableId;
  readonly projectId: PortableId;
  readonly timelineArtifactId: ArtifactId;
  readonly timelineContentHash: Sha256;
  readonly sceneId: PortableId;
  readonly startMs: number;
  readonly endMs: number;
  readonly canvas: RendererCanvas;
  readonly safeArea: RendererSafeArea;
  readonly title: string;
  readonly narrativeRole: string;
  readonly captionCueIds: StringSet;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
  readonly csp: "default-src 'none'; img-src data: narracut:; media-src narracut:; style-src 'unsafe-inline'; font-src narracut:; script-src 'none'; connect-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
  /**
   * @maxItems 64
   */
  readonly resourceUris: readonly ProjectUri[];
  readonly html: string;
  readonly contentHash: Sha256;
}
export interface RendererSafeArea {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}
export interface RenderJobAcceptedResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "enqueue_scene_render" | "enqueue_timeline_render";
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly jobId: JobId;
  readonly idempotentReplay: boolean;
}
export interface RenderResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "get_render_result";
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly jobId: JobId;
  readonly status: "succeeded" | "failed" | "canceled";
  readonly target: "scene" | "timeline";
  readonly timelineInput: RendererTimelineInputReference;
  readonly config: RendererConfig;
  readonly rendererIdentity: RendererIdentity;
  /**
   * @maxItems 10000
   */
  readonly snapshotHashes: readonly Sha256[];
  /**
   * @maxItems 10002
   */
  readonly artifacts: readonly RenderArtifactManifestEntry[];
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly affectedSceneIds: readonly [PortableId, ...PortableId[]];
  /**
   * @maxItems 10000
   */
  readonly reusedSceneIds: readonly PortableId[];
  /**
   * @maxItems 64
   */
  readonly diagnostics: readonly RendererDiagnostic[];
  readonly logSummary: {
    [k: string]: unknown | undefined;
  };
}
export interface RenderArtifactManifestEntry {
  readonly artifactId: ArtifactId;
  readonly kind: "scene_snapshot" | "rendered_scene" | "rendered_video" | "render_log";
  readonly uri: ProjectUri;
  readonly contentHash: Sha256;
  readonly byteLength: number;
  readonly mediaType: "text/html" | "video/mp4" | "application/json";
  readonly durationMs: number;
  readonly width: number;
  readonly height: number;
  readonly hasAudio: boolean;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly sceneIds: readonly [PortableId, ...PortableId[]];
}
export interface RenderEvent {
  readonly apiVersion: ApiVersion;
  readonly operation: "render_event";
  readonly jobId: JobId;
  readonly sequence: number;
  readonly eventType:
    | "queued"
    | "snapshot_ready"
    | "scene_started"
    | "scene_completed"
    | "composition_started"
    | "progress"
    | "cancellation_requested"
    | "retry_scheduled"
    | "succeeded"
    | "failed"
    | "canceled";
  readonly progress: number;
  readonly phase:
    "validation" | "snapshot" | "scene_render" | "composition" | "commit" | "cleanup" | "terminal";
  readonly sceneId?: PortableId;
  readonly message: string;
  readonly occurredAt: string;
}
export interface RendererCommandError {
  readonly apiVersion: ApiVersion;
  readonly code:
    | "invalid_request"
    | "project_not_found"
    | "project_identity_mismatch"
    | "review_required"
    | "input_stale"
    | "input_hash_mismatch"
    | "cross_project_reference"
    | "traceability_incomplete"
    | "snapshot_too_large"
    | "resource_rejected"
    | "resource_limit_exceeded"
    | "renderer_unavailable"
    | "renderer_unsupported"
    | "renderer_identity_changed"
    | "scene_not_found"
    | "ffmpeg_failed"
    | "timeout"
    | "canceled"
    | "artifact_not_found"
    | "job_conflict"
    | "io_error"
    | "internal_contract_error";
  readonly operation: RendererOperation;
  readonly message: string;
  readonly retryable: boolean;
  readonly stageId?: PortableId;
  readonly runId?: RunId;
  readonly artifactId?: ArtifactId;
  readonly diagnosticIds?: StringSet;
}
