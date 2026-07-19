import { buildReviewedInputReference } from "../media/media-stage-model.js";

export function resolveApprovedTimelineCandidate(input) {
  if (!input || typeof input !== "object") return { valid: false, error: "缺少渲染输入上下文。" };
  const { projectId, stageState, runs, reviews, artifacts } = input;
  if (!stageState || stageState.stageId !== "timeline" || stageState.status !== "approved" || !stageState.approvedRunId) {
    return { valid: false, error: "时间轴尚未形成当前有效的 approved 版本。" };
  }
  const run = Array.isArray(runs) ? runs.find((item) => item.runId === stageState.approvedRunId) : undefined;
  if (!run || run.status !== "succeeded") return { valid: false, error: "approvedRunId 未指向成功的时间轴运行。" };
  const review = (Array.isArray(reviews) ? reviews : [])
    .filter((item) => item.projectId === projectId && item.stageId === "timeline" && item.runId === run.runId && item.decision === "approved")
    .sort((left, right) => right.createdAt.localeCompare(left.createdAt) || right.reviewId.localeCompare(left.reviewId))[0];
  if (!review) return { valid: false, error: "时间轴缺少匹配的 approved ReviewRecord。" };
  const candidates = (Array.isArray(artifacts) ? artifacts : []).filter((item) => item.kind === "timeline");
  if (candidates.length !== 1) return { valid: false, error: candidates.length ? "批准运行包含多个时间轴产物，必须先消除歧义。" : "批准运行没有 timeline 产物。" };
  const artifact = candidates[0];
  const result = buildReviewedInputReference({
    expectedProjectId: projectId,
    expectedStageId: "timeline",
    expectedArtifactKinds: ["timeline"],
    stageState,
    run,
    review,
    artifact: {
      projectId,
      stageId: "timeline",
      runId: run.runId,
      artifactId: artifact.artifactId,
      kind: artifact.kind,
      contentHash: artifact.contentHash,
      provenance: artifact.provenance,
    },
  });
  return result.valid ? { valid: true, value: result.value } : { valid: false, error: result.errors.join("；") };
}

export function defaultRenderConfig(timeline) {
  if (!timeline || timeline.documentType !== "timeline" || !timeline.canvas || !Number.isInteger(timeline.durationMs) || timeline.durationMs < 100) return null;
  return {
    canvas: { ...timeline.canvas },
    videoCodec: "libx264",
    audioCodec: "aac",
    pixelFormat: "yuv420p",
    preset: "veryfast",
    crf: 23,
    maxDurationMs: Math.min(86_400_000, Math.max(100, timeline.durationMs)),
    maxTemporaryBytes: 2 * 1024 * 1024 * 1024,
    timeoutMs: Math.min(7_200_000, Math.max(60_000, timeline.durationMs * 10)),
  };
}
