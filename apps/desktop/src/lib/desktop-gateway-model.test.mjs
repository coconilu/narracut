import assert from "node:assert/strict";
import test from "node:test";
import {
  artifactReadMatchesRun,
  findJobByRunId,
} from "./desktop-gateway-model.js";

const run = {
  projectId: "project_source",
  stageId: "script",
  runId: "run_script_001",
  artifactIds: ["artifact_script_001"],
};

const artifact = {
  projectId: "project_source",
  stageId: "script",
  runId: "run_script_001",
  artifactId: "artifact_script_001",
};

test("复制工程以物理 owner 读取，同时保留不可变 Run/Artifact 源身份", () => {
  assert.equal(
    artifactReadMatchesRun("project_copy", run, {
      ownerProjectId: "project_copy",
      artifact,
    }),
    true,
  );
  assert.equal(
    artifactReadMatchesRun("project_copy", run, {
      ownerProjectId: "project_source",
      artifact,
    }),
    false,
  );
  assert.equal(
    artifactReadMatchesRun("project_copy", run, {
      ownerProjectId: "project_copy",
      artifact: { ...artifact, projectId: "project_copy" },
    }),
    false,
  );
});

test("响应丢失后可按稳定 runId 找回已创建任务", () => {
  const expected = { jobId: "job_002", runId: "run_script_002" };
  assert.equal(
    findJobByRunId(
      [{ jobId: "job_001", runId: "run_script_001" }, expected],
      "run_script_002",
    ),
    expected,
  );
  assert.equal(findJobByRunId([], "run_missing"), undefined);
});
