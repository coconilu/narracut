import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_JOB_COMMAND_API_VERSION,
  type CancelJobRequest,
  type EnqueueStageJobRequest,
  type GetJobRequest,
  type JobCommandError,
  type JobEventsResult,
  type JobListResult,
  type JobOperation,
  type JobRecoveryResult,
  type JobSnapshot,
  type ListJobEventsRequest,
  type ListJobsRequest,
  type RecoverJobsRequest,
  type RetryStageJobRequest,
} from "@narracut/contracts";

export type EnqueueStageJobInput = Omit<
  EnqueueStageJobRequest,
  "apiVersion" | "command"
>;
export type GetJobInput = Omit<GetJobRequest, "apiVersion" | "command">;
export type ListJobsInput = Omit<ListJobsRequest, "apiVersion" | "command">;
export type ListJobEventsInput = Omit<
  ListJobEventsRequest,
  "apiVersion" | "command"
>;
export type CancelJobInput = Omit<CancelJobRequest, "apiVersion" | "command">;
export type RetryStageJobInput = Omit<
  RetryStageJobRequest,
  "apiVersion" | "command"
>;
export type RecoverJobsInput = Omit<RecoverJobsRequest, "apiVersion" | "command">;

const errorCodes: Record<JobCommandError["code"], true> = {
  invalid_request: true,
  invalid_path: true,
  path_contains_symlink: true,
  project_not_found: true,
  project_identity_mismatch: true,
  invalid_project: true,
  migration_required: true,
  unsupported_newer_version: true,
  workflow_not_initialized: true,
  stage_not_ready: true,
  job_not_found: true,
  idempotency_conflict: true,
  invalid_transition: true,
  lease_conflict: true,
  lease_expired: true,
  event_conflict: true,
  scan_limit_exceeded: true,
  io_error: true,
  internal_contract_error: true,
};

const operations: Record<JobOperation, true> = {
  enqueue_stage_job: true,
  get_job: true,
  list_jobs: true,
  list_job_events: true,
  cancel_job: true,
  retry_stage_job: true,
  recover_jobs: true,
};

export const jobCommands = {
  enqueue(input: EnqueueStageJobInput): Promise<JobSnapshot> {
    return invoke("enqueue_stage_job", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "enqueue_stage_job",
        ...input,
      } satisfies EnqueueStageJobRequest,
    });
  },

  get(input: GetJobInput): Promise<JobSnapshot> {
    return invoke("get_job", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "get_job",
        ...input,
      } satisfies GetJobRequest,
    });
  },

  list(input: ListJobsInput): Promise<JobListResult> {
    return invoke("list_jobs", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "list_jobs",
        ...input,
      } satisfies ListJobsRequest,
    });
  },

  listEvents(input: ListJobEventsInput): Promise<JobEventsResult> {
    return invoke("list_job_events", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "list_job_events",
        ...input,
      } satisfies ListJobEventsRequest,
    });
  },

  cancel(input: CancelJobInput): Promise<JobSnapshot> {
    return invoke("cancel_job", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "cancel_job",
        ...input,
      } satisfies CancelJobRequest,
    });
  },

  retry(input: RetryStageJobInput): Promise<JobSnapshot> {
    return invoke("retry_stage_job", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "retry_stage_job",
        ...input,
      } satisfies RetryStageJobRequest,
    });
  },

  recover(input: RecoverJobsInput): Promise<JobRecoveryResult> {
    return invoke("recover_jobs", {
      request: {
        apiVersion: NARRACUT_JOB_COMMAND_API_VERSION,
        command: "recover_jobs",
        ...input,
      } satisfies RecoverJobsRequest,
    });
  },
} as const;

export function isJobCommandError(value: unknown): value is JobCommandError {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    candidate.apiVersion === NARRACUT_JOB_COMMAND_API_VERSION &&
    typeof candidate.code === "string" &&
    Object.prototype.hasOwnProperty.call(errorCodes, candidate.code) &&
    typeof candidate.operation === "string" &&
    Object.prototype.hasOwnProperty.call(operations, candidate.operation) &&
    typeof candidate.message === "string" &&
    (candidate.path === undefined || typeof candidate.path === "string") &&
    (candidate.jobId === undefined || typeof candidate.jobId === "string") &&
    (candidate.stageId === undefined || typeof candidate.stageId === "string") &&
    (candidate.runId === undefined || typeof candidate.runId === "string")
  );
}
