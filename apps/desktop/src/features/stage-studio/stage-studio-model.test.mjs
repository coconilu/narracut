import assert from "node:assert/strict";
import test from "node:test";
import {
  canReviewRun,
  chooseRunIds,
  parseConfigDraft,
  reuseStableIntent,
  sameJsonValue,
  sortRunsNewestFirst,
  uniqueArtifactIds,
} from "./stage-studio-model.js";

function run(runId, createdAt, status = "succeeded", artifactIds = []) {
  return { runId, createdAt, status, artifactIds };
}

test("运行历史按完成时间倒序，并以 runId 稳定打破平局", () => {
  const ordered = sortRunsNewestFirst([
    run("run_001", "2026-07-16T10:00:00Z"),
    run("run_003", "2026-07-17T10:00:00Z"),
    run("run_002", "2026-07-17T10:00:00Z"),
  ]);

  assert.deepEqual(ordered.map((item) => item.runId), [
    "run_003",
    "run_002",
    "run_001",
  ]);
});

test("优先选最新运行，并把已采用运行作为比较版本", () => {
  const selection = chooseRunIds(
    [
      run("run_script_003", "2026-07-16T10:00:00Z"),
      run("run_script_004", "2026-07-17T10:00:00Z"),
    ],
    "run_script_004",
    "run_script_003",
  );

  assert.deepEqual(selection, {
    selectedRunId: "run_script_004",
    compareRunId: "run_script_003",
  });
});

test("缺失的 latestRunId 不会制造不存在的选择", () => {
  const selection = chooseRunIds(
    [run("run_script_002", "2026-07-16T10:00:00Z")],
    "run_script_preparing",
    undefined,
  );

  assert.deepEqual(selection, {
    selectedRunId: "run_script_002",
    compareRunId: undefined,
  });
});

test("配置编辑器只接受 JSON 对象", () => {
  assert.deepEqual(parseConfigDraft('{"tone":"calm","claims":4}'), {
    tone: "calm",
    claims: 4,
  });
  assert.throws(() => parseConfigDraft("[1,2]"), /顶层必须/);
  assert.throws(() => parseConfigDraft("{"), /有效的 JSON/);
});

test("配置相等判断忽略对象键顺序但保留数组顺序", () => {
  assert.equal(
    sameJsonValue(
      { tone: "calm", nested: { duration: 180, locale: "zh-CN" } },
      { nested: { locale: "zh-CN", duration: 180 }, tone: "calm" },
    ),
    true,
  );
  assert.equal(sameJsonValue({ order: ["a", "b"] }, { order: ["b", "a"] }), false);
});

test("相同请求签名复用审核或重生成意图", () => {
  let created = 0;
  const first = reuseStableIntent(null, "same", () => ({
    requestId: `request_${++created}`,
  }));
  const replay = reuseStableIntent(first, "same", () => ({
    requestId: `request_${++created}`,
  }));
  const changed = reuseStableIntent(first, "changed", () => ({
    requestId: `request_${++created}`,
  }));

  assert.equal(replay, first);
  assert.equal(replay.requestId, "request_1");
  assert.equal(changed.requestId, "request_2");
});

test("只有 succeeded 运行可批准，产物选择自动去重", () => {
  assert.equal(canReviewRun(run("run_ok", "2026-07-17T10:00:00Z")), true);
  assert.equal(
    canReviewRun(run("run_failed", "2026-07-17T10:00:00Z", "failed")),
    false,
  );
  assert.deepEqual(
    uniqueArtifactIds(
      run("run_ok", "2026-07-17T10:00:00Z", "succeeded", ["a", "a", "b"]),
    ),
    ["a", "b"],
  );
});
