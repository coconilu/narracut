import type { StageRun } from "../src/index";

declare const run: StageRun;

const prompt: unknown = run.configSnapshot.values.prompt;

// @ts-expect-error Stage configuration snapshots are read-only.
run.configSnapshot.values = { prompt: "changed" };

// @ts-expect-error Snapshot values are deeply read-only at the contract boundary.
run.configSnapshot.values.prompt = "changed";

// @ts-expect-error Runs cannot enter the stage-only stale state.
run.status = "stale";

void prompt;
