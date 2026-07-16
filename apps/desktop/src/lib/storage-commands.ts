import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_STORAGE_COMMAND_API_VERSION,
  type Artifact,
  type ArtifactCommitResult,
  type ArtifactDraft,
  type ArtifactReadResult,
  type ArtifactVerificationResult,
  type CacheCleanupResult,
  type CleanProjectCacheRequest,
  type ForgetProjectRequest,
  type ForgetProjectResult,
  type GetArtifactRequest,
  type ImportArtifactFileRequest,
  type IndexedJobsResult,
  type ListIndexedJobsRequest,
  type ListRecentProjectsRequest,
  type ProjectIndexRebuildResult,
  type RebuildProjectIndexRequest,
  type RecentProjectsResult,
  type StorageCommandError,
  type StorageOperation,
  type VerifyArtifactRequest,
} from "@narracut/contracts";

export type ImportArtifactFileInput = Omit<
  ImportArtifactFileRequest,
  "apiVersion" | "command" | "artifact"
> & {
  readonly artifact: ArtifactDraft;
};
export type ArtifactCommit = Omit<ArtifactCommitResult, "artifact"> & {
  readonly artifact: Artifact;
};
export type ArtifactRead = Omit<ArtifactReadResult, "artifact"> & {
  readonly artifact: Artifact;
};
export type RebuildProjectIndexInput = Omit<
  RebuildProjectIndexRequest,
  "apiVersion" | "command"
>;
export type ListRecentProjectsInput = Omit<
  ListRecentProjectsRequest,
  "apiVersion" | "command"
>;
export type ListIndexedJobsInput = Omit<
  ListIndexedJobsRequest,
  "apiVersion" | "command"
>;
export type CleanProjectCacheInput = Omit<
  CleanProjectCacheRequest,
  "apiVersion" | "command"
>;

type ImportArtifactFileCommandRequest = Omit<ImportArtifactFileRequest, "artifact"> & {
  readonly artifact: ArtifactDraft;
};

const errorCodes: Record<StorageCommandError["code"], true> = {
  invalid_request: true,
  invalid_path: true,
  path_contains_symlink: true,
  project_not_found: true,
  project_identity_mismatch: true,
  invalid_project: true,
  migration_required: true,
  unsupported_newer_version: true,
  source_not_found: true,
  source_changed: true,
  source_too_large: true,
  artifact_too_large: true,
  invalid_artifact: true,
  artifact_not_found: true,
  artifact_conflict: true,
  content_corrupt: true,
  index_unavailable: true,
  index_migration_failed: true,
  scan_limit_exceeded: true,
  cache_cleanup_failed: true,
  io_error: true,
  internal_contract_error: true,
};

const operations: Record<StorageOperation, true> = {
  import_artifact: true,
  get_artifact: true,
  verify_artifact: true,
  rebuild_project_index: true,
  list_recent_projects: true,
  list_indexed_jobs: true,
  forget_project: true,
  clean_project_cache: true,
};

export const storageCommands = {
  importArtifactFile(input: ImportArtifactFileInput): Promise<ArtifactCommit> {
    const request = {
      apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
      command: "import_artifact_file",
      ...input,
    } satisfies ImportArtifactFileCommandRequest;
    return invoke<ArtifactCommit>("import_artifact_file", { request });
  },

  getArtifact(projectPath: string, artifactId: string): Promise<ArtifactRead> {
    return invoke<ArtifactRead>("get_artifact", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "get_artifact",
        projectPath,
        artifactId,
      } satisfies GetArtifactRequest,
    });
  },

  verifyArtifact(
    projectPath: string,
    artifactId: string,
  ): Promise<ArtifactVerificationResult> {
    return invoke("verify_artifact", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "verify_artifact",
        projectPath,
        artifactId,
      } satisfies VerifyArtifactRequest,
    });
  },

  rebuildProjectIndex(
    input: RebuildProjectIndexInput,
  ): Promise<ProjectIndexRebuildResult> {
    return invoke("rebuild_project_index", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "rebuild_project_index",
        ...input,
      } satisfies RebuildProjectIndexRequest,
    });
  },

  listRecentProjects(input: ListRecentProjectsInput): Promise<RecentProjectsResult> {
    return invoke("list_recent_projects", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "list_recent_projects",
        ...input,
      } satisfies ListRecentProjectsRequest,
    });
  },

  listIndexedJobs(input: ListIndexedJobsInput): Promise<IndexedJobsResult> {
    return invoke("list_indexed_jobs", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "list_indexed_jobs",
        ...input,
      } satisfies ListIndexedJobsRequest,
    });
  },

  forgetProject(ownerProjectId: string): Promise<ForgetProjectResult> {
    return invoke("forget_project", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "forget_project",
        ownerProjectId,
      } satisfies ForgetProjectRequest,
    });
  },

  cleanProjectCache(input: CleanProjectCacheInput): Promise<CacheCleanupResult> {
    return invoke("clean_project_cache", {
      request: {
        apiVersion: NARRACUT_STORAGE_COMMAND_API_VERSION,
        command: "clean_project_cache",
        ...input,
      } satisfies CleanProjectCacheRequest,
    });
  },
} as const;

export function isStorageCommandError(value: unknown): value is StorageCommandError {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    candidate.apiVersion === NARRACUT_STORAGE_COMMAND_API_VERSION &&
    typeof candidate.code === "string" &&
    Object.prototype.hasOwnProperty.call(errorCodes, candidate.code) &&
    typeof candidate.operation === "string" &&
    Object.prototype.hasOwnProperty.call(operations, candidate.operation) &&
    typeof candidate.message === "string" &&
    (candidate.path === undefined || typeof candidate.path === "string") &&
    (candidate.artifactId === undefined || typeof candidate.artifactId === "string")
  );
}
