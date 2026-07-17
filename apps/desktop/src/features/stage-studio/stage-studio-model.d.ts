import type { JsonObject, StageRun } from "@narracut/contracts";

export interface StableIntent {
  readonly signature: string;
}

export function sortRunsNewestFirst(
  runs: readonly StageRun[],
): readonly StageRun[];

export function chooseRunIds(
  runs: readonly StageRun[],
  latestRunId?: string,
  approvedRunId?: string,
): {
  readonly selectedRunId?: string;
  readonly compareRunId?: string;
};

export function parseConfigDraft(value: string): JsonObject;

export function sameJsonValue(left: unknown, right: unknown): boolean;

export function reuseStableIntent<Value extends object>(
  current: (StableIntent & Value) | null,
  signature: string,
  createValue: () => Value,
): StableIntent & Value;

export function canReviewRun(run?: StageRun): boolean;

export function uniqueArtifactIds(run?: StageRun): readonly string[];
