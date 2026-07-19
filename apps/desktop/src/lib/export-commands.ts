import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_EXPORT_API_VERSION,
  NARRACUT_JOB_COMMAND_API_VERSION,
  type EnqueueExportRequest,
  type ExportCommandError,
  type ExportJobAcceptedResult,
  type ExportQaResult,
  type ExportResult,
  type ExportVerificationResult,
  type GetExportResultRequest,
  type RunExportQaRequest,
  type RetryStageJobRequest,
  type VerifyExportRequest,
} from "@narracut/contracts";

export type RunExportQaInput = Omit<RunExportQaRequest, "apiVersion" | "operation">;
export type EnqueueExportInput = Omit<EnqueueExportRequest, "apiVersion" | "operation">;
export type GetExportResultInput = Omit<GetExportResultRequest, "apiVersion" | "operation">;
export type VerifyExportInput = Omit<VerifyExportRequest, "apiVersion" | "operation">;
export type RetryExportInput = Omit<RetryStageJobRequest, "apiVersion" | "command">;

export const exportCommands = {
  runQa(input: RunExportQaInput): Promise<ExportQaResult> {
    return invoke("run_export_qa", { request: { apiVersion: NARRACUT_EXPORT_API_VERSION, operation: "run_export_qa", ...input } satisfies RunExportQaRequest });
  },
  enqueue(input: EnqueueExportInput): Promise<ExportJobAcceptedResult> {
    return invoke("enqueue_export", { request: { apiVersion: NARRACUT_EXPORT_API_VERSION, operation: "enqueue_export", ...input } satisfies EnqueueExportRequest });
  },
  retry(input: RetryExportInput): Promise<ExportJobAcceptedResult> {
    return invoke("retry_export", { request: { apiVersion: NARRACUT_JOB_COMMAND_API_VERSION, command: "retry_stage_job", ...input } satisfies RetryStageJobRequest });
  },
  getResult(input: GetExportResultInput): Promise<ExportResult> {
    return invoke("get_export_result", { request: { apiVersion: NARRACUT_EXPORT_API_VERSION, operation: "get_export_result", ...input } satisfies GetExportResultRequest });
  },
  verify(input: VerifyExportInput): Promise<ExportVerificationResult> {
    return invoke("verify_export", { request: { apiVersion: NARRACUT_EXPORT_API_VERSION, operation: "verify_export", ...input } satisfies VerifyExportRequest });
  },
} as const;

const codes = new Set<ExportCommandError["code"]>([
  "invalid_request", "invalid_project", "project_mismatch", "render_not_approved", "render_stale",
  "artifact_not_found", "hash_mismatch", "qa_blocked", "qa_changed", "rights_incomplete",
  "renderer_identity_changed", "destination_invalid", "destination_conflict", "disk_space_insufficient",
  "canceled", "io_error", "internal_contract_error",
]);

export function isExportCommandError(value: unknown): value is ExportCommandError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<ExportCommandError>;
  return candidate.apiVersion === NARRACUT_EXPORT_API_VERSION && typeof candidate.code === "string" && codes.has(candidate.code as ExportCommandError["code"]) && typeof candidate.message === "string" && typeof candidate.retryable === "boolean";
}
