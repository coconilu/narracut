import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_PROJECT_COMMAND_API_VERSION,
  type CopyProjectRequest,
  type CreateProjectRequest,
  type MigrateProjectRequest,
  type MoveProjectToTrashRequest,
  type OpenProjectRequest,
  type InspectProjectRequest,
  type ProjectCommandError,
  type ProjectCommandOperation,
  type ProjectCopyResult,
  type ProjectDescriptor,
  type ProjectInspection,
  type ProjectMigrationResult,
  type ProjectTrashResult,
  type RenameProjectRequest,
  type SetProjectArchivedRequest,
} from "@narracut/contracts";

export type CreateProjectInput = Omit<
  CreateProjectRequest,
  "apiVersion" | "command"
>;
export type MigrateProjectInput = Omit<
  MigrateProjectRequest,
  "apiVersion" | "command"
>;
export type RenameProjectInput = Omit<
  RenameProjectRequest,
  "apiVersion" | "command"
>;
export type CopyProjectInput = Omit<
  CopyProjectRequest,
  "apiVersion" | "command"
>;
export type SetProjectArchivedInput = Omit<
  SetProjectArchivedRequest,
  "apiVersion" | "command"
>;
export type MoveProjectToTrashInput = Omit<
  MoveProjectToTrashRequest,
  "apiVersion" | "command"
>;

const errorCodes: Record<ProjectCommandError["code"], true> = {
  invalid_request: true,
  invalid_name: true,
  invalid_path: true,
  path_contains_symlink: true,
  project_not_found: true,
  marker_missing: true,
  marker_too_large: true,
  invalid_project: true,
  migration_required: true,
  migration_conflict: true,
  unsupported_newer_version: true,
  migration_failed: true,
  destination_exists: true,
  copy_too_large: true,
  io_error: true,
  trash_failed: true,
  internal_contract_error: true,
};

const operations: Record<ProjectCommandOperation, true> = {
  inspect: true,
  open: true,
  create: true,
  migrate: true,
  rename: true,
  copy: true,
  set_archived: true,
  move_to_trash: true,
};

export const projectCommands = {
  inspect(projectPath: string): Promise<ProjectInspection> {
    return invoke("inspect_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "inspect_project",
        projectPath,
      } satisfies InspectProjectRequest,
    });
  },

  open(projectPath: string): Promise<ProjectDescriptor> {
    return invoke("open_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "open_project",
        projectPath,
      } satisfies OpenProjectRequest,
    });
  },

  create(input: CreateProjectInput): Promise<ProjectDescriptor> {
    return invoke("create_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "create_project",
        ...input,
      } satisfies CreateProjectRequest,
    });
  },

  migrate(input: MigrateProjectInput): Promise<ProjectMigrationResult> {
    return invoke("migrate_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "migrate_project",
        ...input,
      } satisfies MigrateProjectRequest,
    });
  },

  rename(input: RenameProjectInput): Promise<ProjectDescriptor> {
    return invoke("rename_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "rename_project",
        ...input,
      } satisfies RenameProjectRequest,
    });
  },

  copy(input: CopyProjectInput): Promise<ProjectCopyResult> {
    return invoke("copy_project", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "copy_project",
        ...input,
      } satisfies CopyProjectRequest,
    });
  },

  setArchived(input: SetProjectArchivedInput): Promise<ProjectDescriptor> {
    return invoke("set_project_archived", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "set_project_archived",
        ...input,
      } satisfies SetProjectArchivedRequest,
    });
  },

  moveToTrash(input: MoveProjectToTrashInput): Promise<ProjectTrashResult> {
    return invoke("move_project_to_trash", {
      request: {
        apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
        command: "move_project_to_trash",
        ...input,
      } satisfies MoveProjectToTrashRequest,
    });
  },
} as const;

export function isProjectCommandError(value: unknown): value is ProjectCommandError {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    candidate.apiVersion === NARRACUT_PROJECT_COMMAND_API_VERSION &&
    typeof candidate.code === "string" &&
    Object.prototype.hasOwnProperty.call(errorCodes, candidate.code) &&
    typeof candidate.message === "string" &&
    typeof candidate.operation === "string" &&
    Object.prototype.hasOwnProperty.call(operations, candidate.operation) &&
    (candidate.path === undefined || typeof candidate.path === "string") &&
    isOptionalVersion(candidate.expectedVersion) &&
    isOptionalVersion(candidate.detectedVersion)
  );
}

function isOptionalVersion(value: unknown): boolean {
  return (
    value === undefined ||
    (typeof value === "number" && Number.isInteger(value) && value >= 0)
  );
}
