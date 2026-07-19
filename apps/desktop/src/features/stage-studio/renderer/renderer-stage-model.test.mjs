import assert from "node:assert/strict";
import test from "node:test";
import { defaultRenderConfig, resolveApprovedTimelineCandidate } from "./renderer-stage-model.js";

const hash = `sha256:${"a".repeat(64)}`;
const provenance = [{ claimId: "claim_1", evidenceRef: "evidence_1" }];
const base = {
  projectId: "project_1",
  stageState: { stageId: "timeline", status: "approved", approvedRunId: "run_timeline_1" },
  runs: [{ projectId: "project_1", stageId: "timeline", runId: "run_timeline_1", status: "succeeded", artifactIds: ["artifact_timeline_1"] }],
  reviews: [{ projectId: "project_1", stageId: "timeline", runId: "run_timeline_1", reviewId: "review_1", decision: "approved", artifactIds: ["artifact_timeline_1"], createdAt: "2026-07-19T00:00:00Z" }],
  artifacts: [{ artifactId: "artifact_timeline_1", kind: "timeline", contentHash: hash, provenance }],
};

test("renderer input closes approved run, review, hash and provenance", () => {
  const result = resolveApprovedTimelineCandidate(base);
  assert.equal(result.valid, true);
  assert.deepEqual(result.value.claimIds, ["claim_1"]);
  assert.equal(result.value.reviewRecordId, "review_1");
});

test("renderer input fails closed for stale or ambiguous timelines", () => {
  assert.equal(resolveApprovedTimelineCandidate({ ...base, stageState: { ...base.stageState, status: "stale" } }).valid, false);
  assert.equal(resolveApprovedTimelineCandidate({ ...base, artifacts: [...base.artifacts, { ...base.artifacts[0], artifactId: "artifact_timeline_2" }] }).valid, false);
});

test("default config is fixed to safe codecs and timeline canvas", () => {
  const config = defaultRenderConfig({ documentType: "timeline", durationMs: 12_000, canvas: { width: 1920, height: 1080, frameRateNumerator: 30, frameRateDenominator: 1 } });
  assert.deepEqual(config.canvas, { width: 1920, height: 1080, frameRateNumerator: 30, frameRateDenominator: 1 });
  assert.equal(config.videoCodec, "libx264");
  assert.equal(config.audioCodec, "aac");
  assert.equal(config.preset, "veryfast");
});
