import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_RENDERER_API_VERSION,
  type CreateSceneSnapshotRequest,
  type EnqueueSceneRenderRequest,
  type EnqueueTimelineRenderRequest,
  type GetRenderResultRequest,
  type ProbeRendererRequest,
  type RenderJobAcceptedResult,
  type RenderResult,
  type RendererCapabilitiesResult,
  type RendererCommandError,
  type SceneSnapshotResult,
} from "@narracut/contracts";

export type CreateSceneSnapshotInput = Omit<CreateSceneSnapshotRequest, "apiVersion" | "command">;
export type EnqueueSceneRenderInput = Omit<EnqueueSceneRenderRequest, "apiVersion" | "command">;
export type EnqueueTimelineRenderInput = Omit<EnqueueTimelineRenderRequest, "apiVersion" | "command">;
export type GetRenderResultInput = Omit<GetRenderResultRequest, "apiVersion" | "command">;

export const rendererCommands = {
  probe(): Promise<RendererCapabilitiesResult> {
    return invoke("probe_renderer", { request: {
      apiVersion: NARRACUT_RENDERER_API_VERSION,
      command: "probe_renderer",
    } satisfies ProbeRendererRequest });
  },
  createSnapshot(input: CreateSceneSnapshotInput): Promise<SceneSnapshotResult> {
    return invoke("create_scene_snapshot", { request: {
      apiVersion: NARRACUT_RENDERER_API_VERSION,
      command: "create_scene_snapshot",
      ...input,
    } satisfies CreateSceneSnapshotRequest });
  },
  enqueueScene(input: EnqueueSceneRenderInput): Promise<RenderJobAcceptedResult> {
    return invoke("enqueue_scene_render", { request: {
      apiVersion: NARRACUT_RENDERER_API_VERSION,
      command: "enqueue_scene_render",
      ...input,
    } satisfies EnqueueSceneRenderRequest });
  },
  enqueueTimeline(input: EnqueueTimelineRenderInput): Promise<RenderJobAcceptedResult> {
    return invoke("enqueue_timeline_render", { request: {
      apiVersion: NARRACUT_RENDERER_API_VERSION,
      command: "enqueue_timeline_render",
      ...input,
    } satisfies EnqueueTimelineRenderRequest });
  },
  getResult(input: GetRenderResultInput): Promise<RenderResult> {
    return invoke("get_render_result", { request: {
      apiVersion: NARRACUT_RENDERER_API_VERSION,
      command: "get_render_result",
      ...input,
    } satisfies GetRenderResultRequest });
  },
} as const;

const rendererCodes = new Set<RendererCommandError["code"]>([
  "invalid_request", "project_not_found", "project_identity_mismatch", "review_required",
  "input_stale", "input_hash_mismatch", "cross_project_reference", "traceability_incomplete",
  "snapshot_too_large", "resource_rejected", "resource_limit_exceeded", "renderer_unavailable",
  "renderer_unsupported", "renderer_identity_changed", "scene_not_found", "ffmpeg_failed",
  "timeout", "canceled", "artifact_not_found", "job_conflict", "io_error", "internal_contract_error",
]);

export function isRendererCommandError(value: unknown): value is RendererCommandError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<RendererCommandError>;
  return candidate.apiVersion === NARRACUT_RENDERER_API_VERSION &&
    typeof candidate.code === "string" && rendererCodes.has(candidate.code as RendererCommandError["code"]) &&
    typeof candidate.message === "string" && typeof candidate.retryable === "boolean";
}
