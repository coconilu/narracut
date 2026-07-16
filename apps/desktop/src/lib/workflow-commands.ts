import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  type GetWorkflowRequest,
  type InitializeWorkflowRequest,
  type ListStageHistoryRequest,
  type PrepareStageRunRequest,
  type PreviewRegenerationRequest,
  type RecordStageRunRequest,
  type RegenerationImpactResult,
  type ReviewRecord,
  type ReviewStageRunRequest,
  type StageConfig,
  type StageConfigUpdateResult,
  type StageDefinition,
  type StageHistoryResult,
  type StageReviewResult,
  type StageRun,
  type StageRunCommitResult,
  type StageRunPreparationResult,
  type StageExecutionSnapshot,
  type UpdateStageConfigRequest,
  type WorkflowCommandError,
  type WorkflowOperation,
  type WorkflowSnapshot,
  type WorkflowStageState,
} from "@narracut/contracts";

export type InitializeWorkflowInput = Omit<
  InitializeWorkflowRequest,
  "apiVersion" | "command"
>;
export type UpdateStageConfigInput = Omit<
  UpdateStageConfigRequest,
  "apiVersion" | "command"
>;
export type RecordStageRunInput = Omit<
  RecordStageRunRequest,
  "apiVersion" | "command"
>;
export type PrepareStageRunInput = Omit<
  PrepareStageRunRequest,
  "apiVersion" | "command"
>;
export type ReviewStageRunInput = Omit<
  ReviewStageRunRequest,
  "apiVersion" | "command"
>;
export type ListStageHistoryInput = Omit<
  ListStageHistoryRequest,
  "apiVersion" | "command"
>;

export type WorkflowSnapshotView = Omit<
  WorkflowSnapshot,
  "stageDefinitions" | "stageStates" | "configs"
> & {
  readonly stageDefinitions: readonly StageDefinition[];
  readonly stageStates: readonly WorkflowStageState[];
  readonly configs: readonly StageConfig[];
};
export type StageConfigUpdate = Omit<StageConfigUpdateResult, "config"> & {
  readonly config: StageConfig;
};
export type StageRunCommit = Omit<StageRunCommitResult, "run"> & {
  readonly run: StageRun;
};
export type StageRunPreparation = Omit<
  StageRunPreparationResult,
  "executionSnapshot"
> & {
  readonly executionSnapshot: StageExecutionSnapshot;
};
export type StageReview = Omit<StageReviewResult, "review"> & {
  readonly review: ReviewRecord;
};
export type StageHistory = Omit<StageHistoryResult, "runs" | "reviews"> & {
  readonly runs: readonly StageRun[];
  readonly reviews: readonly ReviewRecord[];
};

const errorCodes: Record<WorkflowCommandError["code"], true> = {
  invalid_request: true,
  invalid_path: true,
  path_contains_symlink: true,
  project_not_found: true,
  project_identity_mismatch: true,
  invalid_project: true,
  migration_required: true,
  unsupported_newer_version: true,
  workflow_not_initialized: true,
  unsupported_workflow: true,
  invalid_stage_graph: true,
  stage_not_found: true,
  stage_not_ready: true,
  config_conflict: true,
  run_not_found: true,
  run_conflict: true,
  review_conflict: true,
  artifact_mismatch: true,
  immutable_conflict: true,
  scan_limit_exceeded: true,
  io_error: true,
  internal_contract_error: true,
};

const operations: Record<WorkflowOperation, true> = {
  initialize_workflow: true,
  get_workflow: true,
  update_stage_config: true,
  prepare_stage_run: true,
  record_stage_run: true,
  review_stage_run: true,
  preview_regeneration: true,
  list_stage_history: true,
};

export const workflowCommands = {
  initialize(input: InitializeWorkflowInput): Promise<WorkflowSnapshotView> {
    return invoke("initialize_project_workflow", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "initialize_project_workflow",
        ...input,
      } satisfies InitializeWorkflowRequest,
    });
  },

  get(projectPath: string): Promise<WorkflowSnapshotView> {
    return invoke("get_project_workflow", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "get_project_workflow",
        projectPath,
      } satisfies GetWorkflowRequest,
    });
  },

  updateConfig(input: UpdateStageConfigInput): Promise<StageConfigUpdate> {
    return invoke("update_stage_config", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "update_stage_config",
        ...input,
      } satisfies UpdateStageConfigRequest,
    });
  },

  prepareRun(input: PrepareStageRunInput): Promise<StageRunPreparation> {
    return invoke("prepare_stage_run", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "prepare_stage_run",
        ...input,
      } satisfies PrepareStageRunRequest,
    });
  },

  recordRun(input: RecordStageRunInput): Promise<StageRunCommit> {
    return invoke("record_stage_run", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "record_stage_run",
        ...input,
      } satisfies RecordStageRunRequest,
    });
  },

  reviewRun(input: ReviewStageRunInput): Promise<StageReview> {
    return invoke("review_stage_run", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "review_stage_run",
        ...input,
      } satisfies ReviewStageRunRequest,
    });
  },

  preview(
    projectPath: string,
    changedStageIds: PreviewRegenerationRequest["changedStageIds"],
  ): Promise<RegenerationImpactResult> {
    return invoke("preview_regeneration", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "preview_regeneration",
        projectPath,
        changedStageIds,
      } satisfies PreviewRegenerationRequest,
    });
  },

  listHistory(input: ListStageHistoryInput): Promise<StageHistory> {
    return invoke("list_stage_history", {
      request: {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        command: "list_stage_history",
        ...input,
      } satisfies ListStageHistoryRequest,
    });
  },
} as const;

export function isWorkflowCommandError(value: unknown): value is WorkflowCommandError {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    candidate.apiVersion === NARRACUT_WORKFLOW_COMMAND_API_VERSION &&
    typeof candidate.code === "string" &&
    Object.prototype.hasOwnProperty.call(errorCodes, candidate.code) &&
    typeof candidate.operation === "string" &&
    Object.prototype.hasOwnProperty.call(operations, candidate.operation) &&
    typeof candidate.message === "string" &&
    (candidate.path === undefined || typeof candidate.path === "string") &&
    (candidate.stageId === undefined || typeof candidate.stageId === "string") &&
    (candidate.runId === undefined || typeof candidate.runId === "string")
  );
}
