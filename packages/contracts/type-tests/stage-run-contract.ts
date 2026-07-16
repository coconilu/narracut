import type { StageRunContract } from "../src/index";

interface ScriptConfig {
  prompt: string;
}

declare const run: StageRunContract<ScriptConfig>;

const prompt: string = run.configSnapshot.prompt;

// @ts-expect-error Stage configuration snapshots are read-only.
run.configSnapshot.prompt = "changed";

void prompt;
