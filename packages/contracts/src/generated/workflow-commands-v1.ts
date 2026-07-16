/* eslint-disable */
/**
 * 此文件由 schema/narracut-workflow-commands-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut 阶段图、配置、不可变运行、审核采用与 stale 传播的有界命令契约。
 */
export type NarraCutWorkflowCommandMessage =
  | InitializeWorkflowRequest
  | GetWorkflowRequest
  | UpdateStageConfigRequest
  | PrepareStageRunRequest
  | RecordStageRunRequest
  | ReviewStageRunRequest
  | PreviewRegenerationRequest
  | ListStageHistoryRequest
  | WorkflowSnapshot
  | StageConfigUpdateResult
  | StageRunPreparationResult
  | StageRunCommitResult
  | StageReviewResult
  | RegenerationImpactResult
  | StageHistoryResult
  | WorkflowCommandError;
export type ApiVersion = "1.0.0";
export type ProjectPath = string;
export type PortableId = string;
export type RunId = string;
export type InputReference = ArtifactInputReference | ProjectDocumentInputReference;
export type ArtifactId = string;
export type ReviewId = string;
export type TerminalRunStatus = "succeeded" | "failed" | "canceled";
export type ReviewDecision = "approved" | "rejected" | "changes_requested";
export type StageState =
  MutableWorkflowStageState | ApprovedWorkflowStageState | StaleWorkflowStageState;
export type StageStatus =
  "draft" | "ready" | "running" | "needs_review" | "approved" | "failed" | "stale";
export type WorkflowOperation =
  | "initialize_workflow"
  | "get_workflow"
  | "update_stage_config"
  | "prepare_stage_run"
  | "record_stage_run"
  | "review_stage_run"
  | "preview_regeneration"
  | "list_stage_history";

export interface InitializeWorkflowRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "initialize_project_workflow";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
}
export interface GetWorkflowRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "get_project_workflow";
  readonly projectPath: ProjectPath;
}
export interface UpdateStageConfigRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "update_stage_config";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly stageId: PortableId;
  readonly expectedRevision: number;
  readonly values: {
    [k: string]: unknown | undefined;
  };
  /**
   * @maxItems 256
   */
  readonly decisions: {
    [k: string]: unknown | undefined;
  }[];
}
export interface PrepareStageRunRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "prepare_stage_run";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly jobId: string;
  /**
   * @maxItems 256
   */
  readonly inputRefs: readonly InputReference[];
  readonly executor: {
    [k: string]: unknown | undefined;
  };
}
export interface ArtifactInputReference {
  readonly refId: PortableId;
  readonly referenceType: "artifact";
  readonly kind: PortableId;
  readonly contentHash: string;
  readonly artifactId: ArtifactId;
  readonly sourceRunId: RunId;
  readonly reviewRecordId: ReviewId;
  /**
   * @maxItems 4096
   */
  readonly claimIds: readonly PortableId[];
  /**
   * @maxItems 4096
   */
  readonly evidenceRefs: readonly string[];
}
export interface ProjectDocumentInputReference {
  readonly refId: PortableId;
  readonly referenceType: "project_document";
  readonly kind: PortableId;
  readonly contentHash: string;
  readonly uri: string;
  /**
   * @maxItems 4096
   */
  readonly claimIds: readonly PortableId[];
  /**
   * @maxItems 4096
   */
  readonly evidenceRefs: readonly string[];
}
export interface RecordStageRunRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "record_stage_run";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly status: TerminalRunStatus;
  readonly jobId: string;
  /**
   * @maxItems 256
   */
  readonly artifactIds: readonly ArtifactId[];
  readonly logSummary: {
    [k: string]: unknown | undefined;
  };
}
export interface ReviewStageRunRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "review_stage_run";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly reviewId: ReviewId;
  readonly decision: ReviewDecision;
  readonly reviewer: ReviewerReference;
  readonly comments: string;
  /**
   * @maxItems 256
   */
  readonly artifactIds: readonly ArtifactId[];
}
export interface ReviewerReference {
  readonly kind: "human" | "agent" | "system";
  readonly reviewerId: string;
  readonly displayName: string;
}
export interface PreviewRegenerationRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "preview_regeneration";
  readonly projectPath: ProjectPath;
  /**
   * @minItems 1
   * @maxItems 64
   */
  readonly changedStageIds: readonly [PortableId, ...PortableId[]];
}
export interface ListStageHistoryRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "list_stage_history";
  readonly projectPath: ProjectPath;
  readonly stageId: PortableId;
  readonly limit: number;
}
export interface WorkflowSnapshot {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly workflowDefinitionId: string;
  /**
   * @maxItems 64
   */
  readonly stageDefinitions: readonly ContractDocument[];
  /**
   * @maxItems 64
   */
  readonly stageStates: readonly StageState[];
  /**
   * @maxItems 64
   */
  readonly configs: readonly ContractDocument[];
}
export interface ContractDocument {
  readonly schemaVersion: string;
  readonly documentType: string;
  readonly [k: string]: unknown | undefined;
}
export interface MutableWorkflowStageState {
  readonly stageId: PortableId;
  readonly status: "draft" | "ready" | "running" | "needs_review" | "failed";
  readonly approvedRunId?: RunId;
  readonly latestRunId?: RunId;
  /**
   * @maxItems 0
   */
  readonly staleBecauseStageIds: readonly [];
}
export interface ApprovedWorkflowStageState {
  readonly stageId: PortableId;
  readonly status: "approved";
  readonly approvedRunId: RunId;
  readonly latestRunId: RunId;
  /**
   * @maxItems 0
   */
  readonly staleBecauseStageIds: readonly [];
}
export interface StaleWorkflowStageState {
  readonly stageId: PortableId;
  readonly status: "stale";
  readonly approvedRunId: RunId;
  readonly latestRunId: RunId;
  /**
   * @minItems 1
   * @maxItems 64
   */
  readonly staleBecauseStageIds: readonly [PortableId, ...PortableId[]];
}
export interface StageConfigUpdateResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly config: ContractDocument;
  readonly configUri: string;
  /**
   * @maxItems 64
   */
  readonly affectedStages: readonly AffectedStage[];
}
export interface AffectedStage {
  readonly stageId: PortableId;
  readonly distance: number;
  /**
   * @maxItems 64
   */
  readonly directCauseStageIds: readonly PortableId[];
  readonly currentStatus: StageStatus;
  readonly hasApprovedRun: boolean;
  readonly supportsPartialRegeneration: boolean;
}
export interface StageRunPreparationResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly executionSnapshot: ContractDocument;
  readonly executionSnapshotUri: string;
  readonly idempotentReplay: boolean;
}
export interface StageRunCommitResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly run: ContractDocument;
  readonly runUri: string;
  readonly stageState: StageState;
  readonly reviewRequired: boolean;
  readonly executionOutdated: boolean;
  readonly idempotentReplay: boolean;
}
export interface StageReviewResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly review: ContractDocument;
  readonly reviewUri: string;
  /**
   * @maxItems 64
   */
  readonly stageStates: readonly StageState[];
  /**
   * @maxItems 64
   */
  readonly invalidatedStageIds: readonly PortableId[];
  readonly applied: boolean;
  readonly idempotentReplay: boolean;
}
export interface RegenerationImpactResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  /**
   * @maxItems 64
   */
  readonly changedStageIds: readonly PortableId[];
  /**
   * @maxItems 64
   */
  readonly affectedStages: readonly AffectedStage[];
}
export interface StageHistoryResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly stageId: PortableId;
  /**
   * @maxItems 100
   */
  readonly runs: readonly ContractDocument[];
  /**
   * @maxItems 1024
   */
  readonly reviews: readonly ContractDocument[];
}
export interface WorkflowCommandError {
  readonly apiVersion: ApiVersion;
  readonly code:
    | "invalid_request"
    | "invalid_path"
    | "path_contains_symlink"
    | "project_not_found"
    | "project_identity_mismatch"
    | "invalid_project"
    | "migration_required"
    | "unsupported_newer_version"
    | "workflow_not_initialized"
    | "unsupported_workflow"
    | "invalid_stage_graph"
    | "stage_not_found"
    | "stage_not_ready"
    | "config_conflict"
    | "run_not_found"
    | "run_conflict"
    | "review_conflict"
    | "artifact_mismatch"
    | "immutable_conflict"
    | "scan_limit_exceeded"
    | "io_error"
    | "internal_contract_error";
  readonly operation: WorkflowOperation;
  readonly message: string;
  readonly path?: string;
  readonly stageId?: PortableId;
  readonly runId?: RunId;
}
