export function sortRunsNewestFirst(runs) {
  return [...runs].sort((left, right) => {
    const leftTime = left.completedAt ?? left.startedAt ?? left.createdAt;
    const rightTime = right.completedAt ?? right.startedAt ?? right.createdAt;
    const byTime = rightTime.localeCompare(leftTime);
    return byTime !== 0 ? byTime : right.runId.localeCompare(left.runId);
  });
}

export function chooseRunIds(runs, latestRunId, approvedRunId) {
  const ordered = sortRunsNewestFirst(runs);
  const ids = new Set(ordered.map((run) => run.runId));
  const selectedRunId = ids.has(latestRunId) ? latestRunId : ordered[0]?.runId;
  const compareRunId =
    approvedRunId && approvedRunId !== selectedRunId && ids.has(approvedRunId)
      ? approvedRunId
      : ordered.find((run) => run.runId !== selectedRunId)?.runId;

  return { selectedRunId, compareRunId };
}

export function parseConfigDraft(value) {
  let parsed;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error("配置必须是有效的 JSON 对象。");
  }

  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("配置顶层必须是 JSON 对象，不能是数组或基础值。");
  }

  return parsed;
}

function normalizeJson(value) {
  if (Array.isArray(value)) return value.map(normalizeJson);
  if (typeof value !== "object" || value === null) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, normalizeJson(value[key])]),
  );
}

export function sameJsonValue(left, right) {
  return JSON.stringify(normalizeJson(left)) === JSON.stringify(normalizeJson(right));
}

export function reuseStableIntent(current, signature, createValue) {
  if (current?.signature === signature) return current;
  return { signature, ...createValue() };
}

export function canReviewRun(run) {
  return run?.status === "succeeded";
}

export function uniqueArtifactIds(run) {
  return [...new Set(run?.artifactIds ?? [])];
}
