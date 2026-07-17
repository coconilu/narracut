import assert from "node:assert/strict";
import test from "node:test";
import { createRequestGate } from "./request-gate.js";

test("later project request wins when A and B resolve out of order", async () => {
  const gate = createRequestGate();
  const projectA = deferred();
  const projectB = deferred();
  const applied = [];

  const openA = applyWhenCurrent(gate, projectA.promise, applied);
  const openB = applyWhenCurrent(gate, projectB.promise, applied);

  projectB.resolve("project-b");
  await openB;
  projectA.resolve("project-a");
  await openA;

  assert.deepEqual(applied, ["project-b"]);
});

test("returning home invalidates an in-flight workspace result", async () => {
  const gate = createRequestGate();
  const refresh = deferred();
  const applied = [];
  const refreshTask = applyWhenCurrent(gate, refresh.promise, applied);

  gate.invalidate();
  refresh.resolve("stale-workspace");
  await refreshTask;

  assert.deepEqual(applied, []);
});

async function applyWhenCurrent(gate, promise, applied) {
  const token = gate.begin();
  const value = await promise;
  if (token.isCurrent()) applied.push(value);
}

function deferred() {
  let resolve;
  const promise = new Promise((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}
