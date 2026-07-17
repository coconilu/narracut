import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_PROVIDER_API_VERSION,
  type DeleteProviderCredentialRequest,
  type GetProviderCatalogRequest,
  type GetProviderCredentialStatusRequest,
  type ProviderCatalogResult,
  type ProviderCommandError,
  type ProviderCredentialMutationResult,
  type ProviderCredentialStatus,
  type ProviderOperation,
  type ScriptStageEnqueueRequest,
  type ScriptStageEnqueueResult,
  type SetProviderCredentialRequest,
} from "@narracut/contracts";

export type ScriptStageEnqueueInput = Omit<
  ScriptStageEnqueueRequest,
  "apiVersion" | "messageType" | "stageId"
>;

const errorCodes: Record<ProviderCommandError["code"], true> = {
  invalid_request: true,
  credential_missing: true,
  provider_unavailable: true,
  provider_response_invalid: true,
  rate_limited: true,
  idempotency_conflict: true,
  canceled: true,
  job_error: true,
  storage_error: true,
  workflow_error: true,
  internal: true,
};

const operations: Record<ProviderOperation, true> = {
  get_provider_catalog: true,
  get_provider_credential_status: true,
  set_provider_credential: true,
  delete_provider_credential: true,
  enqueue_script_stage: true,
  execute_provider_request: true,
};

export const providerCommands = {
  catalog(): Promise<ProviderCatalogResult> {
    return invoke("get_provider_catalog", {
      request: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "get_provider_catalog_request",
      } satisfies GetProviderCatalogRequest,
    });
  },

  credentialStatus(providerId: string): Promise<ProviderCredentialStatus> {
    return invoke("get_provider_credential_status", {
      request: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "get_provider_credential_status_request",
        providerId,
      } satisfies GetProviderCredentialStatusRequest,
    });
  },

  setCredential(
    providerId: string,
    secret: string,
  ): Promise<ProviderCredentialMutationResult> {
    return invoke("set_provider_credential", {
      request: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "set_provider_credential_request",
        providerId,
        secret,
      } satisfies SetProviderCredentialRequest,
    });
  },

  deleteCredential(providerId: string): Promise<ProviderCredentialMutationResult> {
    return invoke("delete_provider_credential", {
      request: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "delete_provider_credential_request",
        providerId,
      } satisfies DeleteProviderCredentialRequest,
    });
  },

  enqueueScript(input: ScriptStageEnqueueInput): Promise<ScriptStageEnqueueResult> {
    return invoke("enqueue_script_stage", {
      request: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "script_stage_enqueue_request",
        stageId: "script",
        ...input,
      } satisfies ScriptStageEnqueueRequest,
    });
  },
} as const;

export function isProviderCommandError(value: unknown): value is ProviderCommandError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    candidate.apiVersion === NARRACUT_PROVIDER_API_VERSION &&
    candidate.messageType === "provider_command_error" &&
    typeof candidate.code === "string" &&
    Object.prototype.hasOwnProperty.call(errorCodes, candidate.code) &&
    typeof candidate.operation === "string" &&
    Object.prototype.hasOwnProperty.call(operations, candidate.operation) &&
    typeof candidate.message === "string" &&
    typeof candidate.retryable === "boolean"
  );
}
