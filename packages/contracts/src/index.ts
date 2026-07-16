export const NARRACUT_CONTRACT_VERSION = "1.0.0" as const;

export type StageRunStatus =
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "retrying"
  | "canceled";

export interface StageInputRef {
  readonly id: string;
  readonly kind: string;
  readonly contentHash: string;
}

export interface ProvenanceRef {
  readonly claimId: string;
  readonly evidenceRef: string;
}

export interface ArtifactDescriptor {
  readonly artifactId: string;
  readonly kind: string;
  readonly uri: string;
  readonly contentHash: string;
  readonly mediaType?: string;
  readonly provenance?: readonly ProvenanceRef[];
}

export interface StageLogSummary {
  readonly message: string;
  readonly warningCount: number;
  readonly errorCount: number;
}

export interface StageRunContract<
  TConfig extends object = Record<string, unknown>,
> {
  readonly contractVersion: typeof NARRACUT_CONTRACT_VERSION;
  readonly runId: string;
  readonly stageId: string;
  readonly status: StageRunStatus;
  readonly inputRefs: readonly StageInputRef[];
  readonly configSnapshot: Readonly<TConfig>;
  readonly artifacts: readonly ArtifactDescriptor[];
  readonly logSummary: StageLogSummary;
  readonly createdAt: string;
  readonly updatedAt: string;
}
