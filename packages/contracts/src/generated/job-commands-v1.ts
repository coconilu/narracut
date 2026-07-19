/* eslint-disable */
/**
 * 此文件由 schema/narracut-job-commands-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut 持久化任务队列的排队、查询、取消、人工重试与崩溃恢复命令契约。worker 租约与执行事件只在 Rust 内部接口开放。
 */
export type NarraCutJobCommandMessage =
  | EnqueueStageJobRequest
  | GetJobRequest
  | ListJobsRequest
  | ListJobEventsRequest
  | CancelJobRequest
  | RetryStageJobRequest
  | RecoverJobsRequest
  | JobSnapshot
  | JobListResult
  | JobEventsResult
  | JobRecoveryResult
  | JobCommandError;
export type ApiVersion = "1.0.0";
export type ProjectPath = string;
export type PortableId = string;
export type RunId = string;
export type InputReference = ArtifactInputReference | ProjectDocumentInputReference;
export type ContentHash = string;
export type ArtifactId = string;
export type JobId = string;
export type JobStatus = "queued" | "running" | "retrying" | "succeeded" | "failed" | "canceled";
export type JobOperation =
  | "enqueue_stage_job"
  | "get_job"
  | "list_jobs"
  | "list_job_events"
  | "cancel_job"
  | "retry_stage_job"
  | "recover_jobs";

export interface EnqueueStageJobRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "enqueue_stage_job";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly stageId: PortableId;
  readonly runId: RunId;
  /**
   * @maxItems 256
   */
  readonly inputRefs: readonly InputReference[];
  readonly executor: ExecutorReference;
  readonly idempotencyKey: string;
  readonly retryPolicy: RetryPolicy;
}
export interface ArtifactInputReference {
  readonly refId: PortableId;
  readonly referenceType: "artifact";
  readonly kind: PortableId;
  readonly contentHash: ContentHash;
  readonly artifactId: ArtifactId;
  readonly sourceRunId: RunId;
  readonly reviewRecordId: string;
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
  readonly contentHash: ContentHash;
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
export interface ExecutorReference {
  readonly providerId: string;
  readonly providerVersion: string;
  readonly executionMode: "remote_api" | "codex_cli" | "local";
  readonly model?: string;
}
export interface RetryPolicy {
  readonly maxAttempts: number;
  readonly initialBackoffMs: number;
  readonly backoffMultiplier: number;
  readonly maxBackoffMs: number;
}
export interface GetJobRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "get_job";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly jobId: JobId;
}
export interface ListJobsRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "list_jobs";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  /**
   * @maxItems 6
   */
  readonly statuses:
    | []
    | [JobStatus]
    | [JobStatus, JobStatus]
    | [JobStatus, JobStatus, JobStatus]
    | [JobStatus, JobStatus, JobStatus, JobStatus]
    | [JobStatus, JobStatus, JobStatus, JobStatus, JobStatus]
    | [JobStatus, JobStatus, JobStatus, JobStatus, JobStatus, JobStatus];
  readonly limit: number;
}
export interface ListJobEventsRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "list_job_events";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly jobId: JobId;
  readonly afterSequence?: number;
  readonly limit: number;
}
export interface CancelJobRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "cancel_job";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly jobId: JobId;
  readonly message: string;
}
export interface RetryStageJobRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "retry_stage_job";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly sourceJobId: JobId;
  readonly newRunId: RunId;
  readonly idempotencyKey: string;
}
export interface RecoverJobsRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "recover_jobs";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
}
export interface JobSnapshot {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly job: ContractDocument;
  readonly jobUri: string;
  readonly status: JobStatus;
  readonly attempt: number;
  readonly progress: number;
  readonly message?: string;
  readonly cancellationRequested: boolean;
  readonly finalizationPending: boolean;
  readonly finalizationMode: "immediate" | "external_commit" | null;
  /**
   * @maxItems 256
   */
  readonly artifactIds: readonly ArtifactId[];
  readonly lastError?: JobFailure;
  readonly nextAttemptAt?: string;
  readonly lease?: JobLease;
  readonly lastSequence: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly historical: boolean;
  readonly indexSynchronized: boolean;
}
export interface ContractDocument {
  readonly schemaVersion: string;
  readonly documentType: string;
  readonly [k: string]: unknown | undefined;
}
export interface JobFailure {
  readonly code: PortableId;
  readonly message: string;
  readonly retryable: boolean;
  readonly details: {
    [k: string]: unknown | undefined;
  };
}
export interface JobLease {
  readonly workerId: PortableId;
  readonly leaseId: PortableId;
  readonly expiresAt: string;
}
export interface JobListResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  /**
   * @maxItems 200
   */
  readonly jobs: readonly JobSnapshot[];
}
export interface JobEventsResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly jobId: JobId;
  /**
   * @maxItems 500
   */
  readonly events: readonly ContractDocument[];
  readonly hasMore: boolean;
}
export interface JobRecoveryResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  /**
   * @maxItems 1024
   */
  readonly recoveredJobIds: readonly JobId[];
  /**
   * @maxItems 1024
   */
  readonly finalizedJobIds: readonly JobId[];
  /**
   * @maxItems 1024
   */
  readonly skippedLiveJobIds: readonly JobId[];
  readonly reindexedJobs: number;
  readonly indexWarnings: number;
}
export interface JobCommandError {
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
    | "stage_not_ready"
    | "job_not_found"
    | "idempotency_conflict"
    | "invalid_transition"
    | "lease_conflict"
    | "lease_expired"
    | "event_conflict"
    | "scan_limit_exceeded"
    | "io_error"
    | "internal_contract_error";
  readonly operation: JobOperation;
  readonly message: string;
  readonly path?: string;
  readonly jobId?: JobId;
  readonly stageId?: PortableId;
  readonly runId?: RunId;
}
