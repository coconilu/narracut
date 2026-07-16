/* eslint-disable */
/**
 * 此文件由 schema/narracut-contracts-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut v1 portable project and execution contracts.
 */
export type NarraCutContractDocument =
  | Project
  | StageDefinition
  | StageConfig
  | StageRun
  | Artifact
  | ReviewRecord
  | JobEvent
  | RenderManifest;
export type SchemaVersion = "1.0.0";
export type StageStatus =
  "draft" | "ready" | "running" | "needs_review" | "approved" | "failed" | "stale";
export type StageRunStatus = "queued" | "running" | "succeeded" | "failed" | "canceled";
export type ArtifactSource =
  GeneratedArtifactSource | ImportedArtifactSource | DerivedArtifactSource;
export type ReviewDecision = "approved" | "rejected" | "changes_requested";
export type JobStatus = "queued" | "running" | "retrying" | "succeeded" | "failed" | "canceled";

export interface Project {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "project";
  readonly projectFormatVersion: 1;
  readonly projectId: string;
  readonly name: string;
  readonly workflowDefinitionId: string;
  readonly defaultLocale?: string;
  readonly stages: readonly StageState[];
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly metadata: JsonObject;
}
export interface StageState {
  readonly stageId: string;
  readonly status: StageStatus;
  readonly approvedRunId?: string;
  readonly latestRunId?: string;
  readonly staleBecauseStageIds: readonly string[];
}
export interface JsonObject {
  readonly [k: string]: unknown | undefined;
}
export interface StageDefinition {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "stage_definition";
  readonly stageId: string;
  readonly definitionVersion: string;
  readonly title: string;
  readonly description: string;
  readonly dependencies: readonly string[];
  readonly inputKinds: readonly string[];
  readonly outputKinds: readonly string[];
  readonly configSchemaRef: string;
  readonly requiresApprovedInputs: boolean;
  readonly supportsPartialRegeneration: boolean;
}
export interface StageConfig {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "stage_config";
  readonly configId: string;
  readonly projectId: string;
  readonly stageId: string;
  readonly revision: number;
  readonly values: JsonObject;
  readonly decisions: readonly DecisionRecord[];
  readonly updatedAt: string;
}
export interface DecisionRecord {
  readonly decisionId: string;
  readonly key: string;
  readonly value: unknown;
  readonly rationale?: string;
  readonly madeBy: string;
  readonly madeAt: string;
}
export interface StageRun {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "stage_run";
  readonly runId: string;
  readonly projectId: string;
  readonly stageId: string;
  readonly stageDefinitionVersion: string;
  readonly status: StageRunStatus;
  readonly jobId: string;
  readonly inputHash: string;
  readonly configHash: string;
  readonly idempotencyKey: string;
  readonly inputRefs: readonly InputReference[];
  readonly configSnapshot: StageConfig;
  readonly executor: ExecutorReference;
  readonly artifactIds: readonly string[];
  readonly logSummary: StageLogSummary;
  readonly supersedesRunId?: string;
  readonly createdAt: string;
  readonly startedAt?: string;
  readonly completedAt?: string;
}
export interface InputReference {
  readonly refId: string;
  readonly kind: string;
  readonly contentHash: string;
  readonly uri?: string;
  readonly artifactId?: string;
  readonly sourceRunId?: string;
  readonly reviewRecordId?: string;
  readonly claimIds: readonly string[];
  readonly evidenceRefs: readonly string[];
}
export interface ExecutorReference {
  readonly providerId: string;
  readonly providerVersion: string;
  readonly executionMode: "remote_api" | "codex_cli" | "local";
  readonly model?: string;
}
export interface StageLogSummary {
  readonly message: string;
  readonly warnings: readonly string[];
  readonly errors: readonly string[];
  readonly logArtifactId?: string;
}
export interface Artifact {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "artifact";
  readonly artifactId: string;
  readonly projectId: string;
  readonly stageId: string;
  readonly runId: string;
  readonly kind: string;
  readonly uri: string;
  readonly contentHash: string;
  readonly byteLength: number;
  readonly mediaType?: string;
  readonly evidenceRole: "factual_evidence" | "expressive_material" | "non_evidence";
  readonly source: ArtifactSource;
  readonly provenance: readonly ProvenanceReference[];
  readonly createdAt: string;
}
export interface GeneratedArtifactSource {
  readonly origin: "generated";
  readonly providerId: string;
  readonly model?: string;
  readonly promptArtifactId?: string;
}
export interface ImportedArtifactSource {
  readonly origin: "imported";
  readonly sourceUri: string;
  readonly author: string;
  readonly license: string;
  readonly attributionText: string;
  readonly sourceContentHash: string;
  readonly authorizationRecordIds: readonly string[];
}
export interface DerivedArtifactSource {
  readonly origin: "derived";
  /**
   * @minItems 1
   */
  readonly sourceArtifactIds: readonly [string, ...string[]];
}
export interface ProvenanceReference {
  readonly claimId: string;
  readonly evidenceRef: string;
}
export interface ReviewRecord {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "review_record";
  readonly reviewId: string;
  readonly projectId: string;
  readonly stageId: string;
  readonly runId: string;
  readonly decision: ReviewDecision;
  readonly reviewer: ReviewerReference;
  readonly comments: string;
  readonly artifactIds: readonly string[];
  readonly createdAt: string;
}
export interface ReviewerReference {
  readonly kind: "human" | "agent" | "system";
  readonly reviewerId: string;
  readonly displayName: string;
}
export interface JobEvent {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "job_event";
  readonly eventId: string;
  readonly jobId: string;
  readonly stageRunId: string;
  readonly sequence: number;
  readonly eventType:
    | "queued"
    | "started"
    | "progress"
    | "log"
    | "artifact_created"
    | "attempt_failed"
    | "retrying"
    | "completed"
    | "failed"
    | "canceled";
  readonly status: JobStatus;
  readonly attempt: number;
  readonly progress?: number;
  readonly message?: string;
  readonly artifactId?: string;
  readonly error?: JobError;
  readonly createdAt: string;
}
export interface JobError {
  readonly code: string;
  readonly message: string;
  readonly retryable: boolean;
  readonly details: JsonObject;
}
export interface RenderManifest {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "render_manifest";
  readonly manifestId: string;
  readonly projectId: string;
  readonly exportRunId: string;
  readonly renderer: RendererReference;
  readonly timelineArtifactId: string;
  readonly inputs: readonly RenderInput[];
  readonly assets: readonly ManifestAsset[];
  /**
   * @minItems 1
   */
  readonly outputs: readonly [ManifestOutput, ...ManifestOutput[]];
  readonly claimEvidenceMap: readonly ProvenanceReference[];
  readonly createdAt: string;
}
export interface RendererReference {
  readonly rendererId: string;
  readonly rendererVersion: string;
}
export interface RenderInput {
  readonly role: "timeline" | "audio" | "captions" | "scene" | "asset" | "avatar";
  readonly artifactId: string;
  readonly contentHash: string;
  readonly provenance: readonly ProvenanceReference[];
}
export interface ManifestAsset {
  readonly artifactId: string;
  readonly contentHash: string;
  readonly source: ArtifactSource;
}
export interface ManifestOutput {
  readonly artifactId: string;
  readonly kind: string;
  readonly uri: string;
  readonly contentHash: string;
}
