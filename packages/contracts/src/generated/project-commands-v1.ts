/* eslint-disable */
/**
 * 此文件由 schema/narracut-project-commands-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut 桌面端项目服务的有类型 Tauri command v1 消息。
 */
export type NarraCutProjectCommandMessage =
  | InspectProjectRequest
  | OpenProjectRequest
  | CreateProjectRequest
  | MigrateProjectRequest
  | RenameProjectRequest
  | CopyProjectRequest
  | SetProjectArchivedRequest
  | MoveProjectToTrashRequest
  | ProjectDescriptor
  | ProjectInspection
  | ProjectMigrationResult
  | ProjectCopyResult
  | ProjectTrashResult
  | ProjectCommandError;
export type ProjectCommandApiVersion = "1.0.0";
export type ProjectMigrationStatus =
  CurrentMigrationStatus | RequiredMigrationStatus | UnsupportedNewerMigrationStatus;
export type ProjectCommandErrorCode =
  | "invalid_request"
  | "invalid_name"
  | "invalid_path"
  | "path_contains_symlink"
  | "project_not_found"
  | "marker_missing"
  | "marker_too_large"
  | "invalid_project"
  | "migration_required"
  | "migration_conflict"
  | "unsupported_newer_version"
  | "migration_failed"
  | "destination_exists"
  | "copy_too_large"
  | "io_error"
  | "trash_failed"
  | "internal_contract_error";
export type ProjectCommandOperation =
  "inspect" | "open" | "create" | "migrate" | "rename" | "copy" | "set_archived" | "move_to_trash";

export interface InspectProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "inspect_project";
  readonly projectPath: string;
}
export interface OpenProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "open_project";
  readonly projectPath: string;
}
export interface CreateProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "create_project";
  readonly parentPath: string;
  readonly directoryName: string;
  readonly name: string;
  readonly workflowDefinitionId: string;
  readonly defaultLocale?: string;
}
export interface MigrateProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "migrate_project";
  readonly projectPath: string;
  readonly expectedSourceFormatVersion: number;
}
export interface RenameProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "rename_project";
  readonly projectPath: string;
  readonly newName: string;
}
export interface CopyProjectRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "copy_project";
  readonly sourceProjectPath: string;
  readonly destinationParentPath: string;
  readonly directoryName: string;
  readonly name: string;
}
export interface SetProjectArchivedRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "set_project_archived";
  readonly projectPath: string;
  readonly archived: boolean;
}
export interface MoveProjectToTrashRequest {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly command: "move_project_to_trash";
  readonly projectPath: string;
  readonly expectedProjectId: string;
}
export interface ProjectDescriptor {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly projectPath: string;
  readonly markerPath: string;
  readonly projectId: string;
  readonly name: string;
  readonly workflowDefinitionId: string;
  readonly projectFormatVersion: 1;
  readonly defaultLocale?: string;
  readonly archived: boolean;
  readonly archivedAt?: string;
  readonly copiedFromProjectId?: string;
  readonly copiedAt?: string;
  readonly createdAt: string;
  readonly updatedAt: string;
}
export interface ProjectInspection {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly projectPath: string;
  readonly markerPath: string;
  readonly detectedFormatVersion: number;
  readonly currentFormatVersion: 1;
  readonly migration: ProjectMigrationStatus;
  readonly project?: ProjectDescriptor;
}
export interface CurrentMigrationStatus {
  readonly status: "current";
  readonly formatVersion: 1;
}
export interface RequiredMigrationStatus {
  readonly status: "required";
  readonly fromVersion: number;
  readonly toVersion: 1;
  /**
   * @minItems 1
   */
  readonly steps: readonly [string, ...string[]];
}
export interface UnsupportedNewerMigrationStatus {
  readonly status: "unsupported_newer";
  readonly detectedVersion: number;
  readonly supportedVersion: 1;
}
export interface ProjectMigrationResult {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly project: ProjectDescriptor;
  readonly fromVersion: number;
  readonly toVersion: 1;
  /**
   * @minItems 1
   */
  readonly appliedSteps: readonly [string, ...string[]];
  readonly backupPath: string;
}
export interface ProjectCopyResult {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly project: ProjectDescriptor;
  readonly sourceProjectId: string;
  readonly historyPolicy: "preserve_immutable_source_identity";
  readonly filesCopied: number;
  readonly bytesCopied: number;
}
export interface ProjectTrashResult {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly projectId: string;
  readonly trashedPath: string;
}
export interface ProjectCommandError {
  readonly apiVersion: ProjectCommandApiVersion;
  readonly code: ProjectCommandErrorCode;
  readonly message: string;
  readonly operation: ProjectCommandOperation;
  readonly path?: string;
  readonly expectedVersion?: number;
  readonly detectedVersion?: number;
}
