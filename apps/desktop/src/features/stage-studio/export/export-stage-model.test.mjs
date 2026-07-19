import assert from "node:assert/strict";
import test from "node:test";
import { resolveApprovedRenderCandidate, safeExportName } from "./export-stage-model.js";

const hash = `sha256:${"a".repeat(64)}`;
const run = { projectId: "project_demo", stageId: "render", runId: "run_render_demo", status: "succeeded", artifactIds: ["artifact_video_demo", "artifact_log_demo"] };
const review = { projectId: "project_demo", stageId: "render", runId: run.runId, reviewId: "review_render_demo", decision: "approved", artifactIds: run.artifactIds, createdAt: "2026-07-19T00:00:00Z" };
const artifacts = [{ artifactId: "artifact_video_demo", kind: "rendered_video", contentHash: hash, contentAvailable: true, provenance: [{ claimId: "claim_1", evidenceRef: "evidence_1" }] }, { artifactId: "artifact_log_demo", kind: "render_log", contentAvailable: true, provenance: [] }];
test("当前 approved Render 闭合成唯一导出输入", () => { const result = resolveApprovedRenderCandidate({ projectId: "project_demo", stageState: { stageId: "render", status: "approved", approvedRunId: run.runId, staleBecauseStageIds: [] }, runs: [run], reviews: [review], artifacts }); assert.equal(result.valid, true); assert.equal(result.value.resultArtifactId, "artifact_log_demo"); });
test("stale、审核未覆盖和多视频一律 fail-closed", () => { assert.equal(resolveApprovedRenderCandidate({ projectId: "project_demo", stageState: { stageId: "render", status: "approved", approvedRunId: run.runId, staleBecauseStageIds: ["timeline"] }, runs: [run], reviews: [review], artifacts }).valid, false); assert.equal(resolveApprovedRenderCandidate({ projectId: "project_demo", stageState: { stageId: "render", status: "approved", approvedRunId: run.runId, staleBecauseStageIds: [] }, runs: [run], reviews: [{ ...review, artifactIds: ["artifact_video_demo"] }], artifacts }).valid, false); });
test("导出名被限制为单个安全目录组件", () => { assert.equal(safeExportName("月球城市 / Alpha"), "Alpha"); assert.equal(safeExportName("../"), "narracut-export"); });
