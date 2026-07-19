export function resolveApprovedRenderCandidate(input) {
  if (!input || typeof input !== "object") return { valid: false, error: "缺少导出输入上下文。" };
  const { projectId, stageState, runs, reviews, artifacts } = input;
  if (!stageState || stageState.stageId !== "render" || stageState.status !== "approved" || !stageState.approvedRunId || stageState.staleBecauseStageIds?.length) return { valid: false, error: "渲染阶段尚未形成当前有效的 approved 版本。" };
  const run = Array.isArray(runs) ? runs.find((item) => item.runId === stageState.approvedRunId && item.status === "succeeded" && item.projectId === projectId) : undefined;
  if (!run) return { valid: false, error: "approvedRunId 未指向当前项目的成功 Render StageRun。" };
  const review = (Array.isArray(reviews) ? reviews : []).filter((item) => item.projectId === projectId && item.stageId === "render" && item.runId === run.runId && item.decision === "approved").sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.reviewId.localeCompare(a.reviewId))[0];
  if (!review) return { valid: false, error: "Render 缺少匹配的 approved ReviewRecord。" };
  const video = (Array.isArray(artifacts) ? artifacts : []).filter((item) => item.kind === "rendered_video");
  const logs = (Array.isArray(artifacts) ? artifacts : []).filter((item) => item.kind === "render_log");
  if (video.length !== 1 || logs.length !== 1) return { valid: false, error: "批准 Render 必须且只能包含一个 rendered_video 与一个 render_log。" };
  if (![video[0].artifactId, logs[0].artifactId].every((id) => run.artifactIds.includes(id) && review.artifactIds.includes(id))) return { valid: false, error: "批准记录未同时覆盖 rendered_video 与 render_log。" };
  if (!video[0].contentAvailable || !logs[0].contentAvailable || !video[0].contentHash || !video[0].provenance?.length) return { valid: false, error: "Render Artifact 内容、哈希或追溯不完整。" };
  const claimIds = [...new Set(video[0].provenance.map((item) => item.claimId))].sort();
  const evidenceRefs = [...new Set(video[0].provenance.map((item) => item.evidenceRef))].sort();
  return { valid: true, value: { stageId: "render", runId: run.runId, artifactId: video[0].artifactId, resultArtifactId: logs[0].artifactId, contentHash: video[0].contentHash, reviewRecordId: review.reviewId, claimIds, evidenceRefs } };
}

export function safeExportName(projectName) {
  const normalized = String(projectName ?? "narracut-export").normalize("NFKD").replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 64);
  return normalized && /^[A-Za-z0-9]/.test(normalized) ? normalized : "narracut-export";
}
