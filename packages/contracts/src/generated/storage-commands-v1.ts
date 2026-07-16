/* eslint-disable */
/**
 * 此文件由 schema/narracut-storage-commands-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut Artifact Store、可重建 SQLite 索引与缓存维护的有界命令契约。Artifact 草稿与完整 Artifact 仍由持久化契约校验。
 */
export type NarraCutStorageCommandMessage =
  | ImportArtifactFileRequest
  | GetArtifactRequest
  | VerifyArtifactRequest
  | RebuildProjectIndexRequest
  | ListRecentProjectsRequest
  | ListIndexedJobsRequest
  | ForgetProjectRequest
  | CleanProjectCacheRequest
  | ArtifactCommitResult
  | ArtifactReadResult
  | ArtifactVerificationResult
  | ProjectIndexRebuildResult
  | RecentProjectsResult
  | IndexedJobsResult
  | ForgetProjectResult
  | CacheCleanupResult
  | StorageCommandError;
export type ApiVersion = "1.0.0";
export type ProjectPath = string;
export type ArtifactId = string;
export type JobStatus = "queued" | "running" | "retrying" | "succeeded" | "failed" | "canceled";
export type IndexStatus = "up_to_date" | "rebuild_required";
export type StorageOperation =
  | "import_artifact"
  | "get_artifact"
  | "verify_artifact"
  | "rebuild_project_index"
  | "list_recent_projects"
  | "list_indexed_jobs"
  | "forget_project"
  | "clean_project_cache";

export interface ImportArtifactFileRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "import_artifact_file";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
  readonly sourcePath: string;
  readonly artifact: {
    [k: string]: unknown | undefined;
  };
}
export interface GetArtifactRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "get_artifact";
  readonly projectPath: ProjectPath;
  readonly artifactId: ArtifactId;
}
export interface VerifyArtifactRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "verify_artifact";
  readonly projectPath: ProjectPath;
  readonly artifactId: ArtifactId;
}
export interface RebuildProjectIndexRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "rebuild_project_index";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
}
export interface ListRecentProjectsRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "list_recent_projects";
  readonly limit: number;
  readonly includeMissing: boolean;
}
export interface ListIndexedJobsRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "list_indexed_jobs";
  readonly ownerProjectId?: string;
  readonly statuses: readonly JobStatus[];
  readonly limit: number;
}
export interface ForgetProjectRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "forget_project";
  readonly ownerProjectId: string;
}
export interface CleanProjectCacheRequest {
  readonly apiVersion: ApiVersion;
  readonly command: "clean_project_cache";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: string;
}
export interface ArtifactCommitResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly artifact: {
    [k: string]: unknown | undefined;
  };
  readonly metadataUri: string;
  readonly contentUri: string;
  readonly deduplicated: boolean;
  readonly indexStatus: IndexStatus;
}
export interface ArtifactReadResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly artifact: {
    [k: string]: unknown | undefined;
  };
  readonly metadataUri: string;
  readonly contentUri: string;
  readonly contentAvailable: boolean;
}
export interface ArtifactVerificationResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly artifactId: ArtifactId;
  readonly status: "verified" | "missing_content" | "hash_mismatch" | "byte_length_mismatch";
  readonly expectedContentHash: string;
  readonly actualContentHash?: string;
  readonly expectedByteLength: number;
  readonly actualByteLength?: number;
}
export interface ProjectIndexRebuildResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly artifactsIndexed: number;
  readonly missingContentCount: number;
  readonly indexStatus: "up_to_date";
}
export interface RecentProjectsResult {
  readonly apiVersion: ApiVersion;
  readonly projects: readonly RecentProject[];
}
export interface RecentProject {
  readonly projectId: string;
  readonly projectPath: string;
  readonly name: string;
  readonly workflowDefinitionId: string;
  readonly projectFormatVersion: number;
  readonly archived: boolean;
  readonly lastOpenedAt: string;
  readonly markerUpdatedAt: string;
  readonly pathAvailable: boolean;
}
export interface IndexedJobsResult {
  readonly apiVersion: ApiVersion;
  readonly jobs: readonly IndexedJob[];
}
export interface IndexedJob {
  readonly ownerProjectId: string;
  readonly jobId: string;
  readonly stageRunId: string;
  readonly stageId: string;
  readonly status: JobStatus;
  readonly attempt: number;
  readonly progress: number;
  readonly message?: string;
  readonly createdAt: string;
  readonly updatedAt: string;
}
export interface ForgetProjectResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly removed: boolean;
}
export interface CacheCleanupResult {
  readonly apiVersion: ApiVersion;
  readonly ownerProjectId: string;
  readonly entriesRemoved: number;
  readonly bytesRemoved: number;
}
export interface StorageCommandError {
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
    | "source_not_found"
    | "source_changed"
    | "source_too_large"
    | "artifact_too_large"
    | "invalid_artifact"
    | "artifact_not_found"
    | "artifact_conflict"
    | "content_corrupt"
    | "index_unavailable"
    | "index_migration_failed"
    | "scan_limit_exceeded"
    | "cache_cleanup_failed"
    | "io_error"
    | "internal_contract_error";
  readonly operation: StorageOperation;
  readonly message: string;
  readonly path?: string;
  readonly artifactId?: ArtifactId;
}
