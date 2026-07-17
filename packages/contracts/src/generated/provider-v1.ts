/* eslint-disable */
/**
 * 此文件由 schema/narracut-provider-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut AI Provider v1 的能力、凭据命令、结构化请求、事件、结果与脚本阶段入队契约。
 */
export type NarraCutProviderMessage =
  | GetProviderCatalogRequest
  | ProviderCatalogResult
  | GetProviderCredentialStatusRequest
  | ProviderCredentialStatus
  | SetProviderCredentialRequest
  | DeleteProviderCredentialRequest
  | ProviderCredentialMutationResult
  | ScriptStageEnqueueRequest
  | ScriptStageEnqueueResult
  | StructuredProviderRequest
  | ProviderEvent
  | StructuredProviderResult
  | ProviderCommandError;
export type ApiVersion = "1.0.0";
export type ProviderId = string;
export type ModelId = string;
export type ProviderTask = "script_generation";
export type PortableId = string;
export type ContentHash = string;
export type Timestamp = string;
export type ProviderEvent =
  | ProviderEventStarted
  | ProviderEventOutputDelta
  | ProviderEventUsage
  | ProviderEventCompleted
  | ProviderEventFailed
  | ProviderEventCanceled;
export type ProviderErrorCode =
  | "invalid_request"
  | "idempotency_conflict"
  | "credential_missing"
  | "provider_unavailable"
  | "provider_response_invalid"
  | "rate_limited"
  | "canceled"
  | "job_error"
  | "storage_error"
  | "workflow_error"
  | "internal";
export type ProviderOperation =
  | "get_provider_catalog"
  | "get_provider_credential_status"
  | "set_provider_credential"
  | "delete_provider_credential"
  | "enqueue_script_stage"
  | "execute_provider_request";

export interface GetProviderCatalogRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "get_provider_catalog_request";
}
export interface ProviderCatalogResult {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_catalog_result";
  /**
   * @minItems 1
   * @maxItems 32
   */
  readonly providers: readonly [ProviderCapability, ...ProviderCapability[]];
}
export interface ProviderCapability {
  readonly providerId: ProviderId;
  readonly displayName: string;
  readonly transport: "remote_api" | "local_cli" | "local_model";
  readonly credentialStorage: "system_keyring" | "none";
  readonly supportsStreaming: boolean;
  readonly supportsCancellation: boolean;
  readonly reportsUsage: boolean;
  readonly defaultModel: ModelId;
  /**
   * @minItems 1
   * @maxItems 32
   */
  readonly models: readonly [ProviderModelCapability, ...ProviderModelCapability[]];
}
export interface ProviderModelCapability {
  readonly modelId: ModelId;
  readonly displayName: string;
  /**
   * @minItems 1
   */
  readonly supportedTasks: readonly [ProviderTask, ...ProviderTask[]];
  readonly structuredOutputs: boolean;
  readonly maxOutputTokens: number;
}
export interface GetProviderCredentialStatusRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "get_provider_credential_status_request";
  readonly providerId: ProviderId;
}
export interface ProviderCredentialStatus {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_credential_status";
  readonly providerId: ProviderId;
  readonly configured: boolean;
  readonly storage: "system_keyring";
}
export interface SetProviderCredentialRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "set_provider_credential_request";
  readonly providerId: ProviderId;
  readonly secret: string;
}
export interface DeleteProviderCredentialRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "delete_provider_credential_request";
  readonly providerId: ProviderId;
}
export interface ProviderCredentialMutationResult {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_credential_mutation_result";
  readonly providerId: ProviderId;
  readonly action: "stored" | "deleted";
  readonly configured: boolean;
  readonly storage: "system_keyring";
}
export interface ScriptStageEnqueueRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "script_stage_enqueue_request";
  readonly projectPath: string;
  readonly expectedProjectId: PortableId;
  readonly stageId: "script";
  readonly providerId: ProviderId;
  readonly model: ModelId;
  readonly runId: string;
  readonly idempotencyKey: PortableId;
  readonly language: string;
  readonly maxOutputTokens: number;
}
export interface ScriptStageEnqueueResult {
  readonly apiVersion: ApiVersion;
  readonly messageType: "script_stage_enqueue_result";
  readonly ownerProjectId: PortableId;
  readonly providerRequestId: string;
  readonly jobId: string;
  readonly runId: string;
  readonly status: "queued" | "running" | "retrying" | "succeeded" | "failed" | "canceled";
}
export interface StructuredProviderRequest {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_request";
  readonly providerRequestId: string;
  readonly providerId: ProviderId;
  readonly model: ModelId;
  readonly task: ProviderTask;
  readonly projectId: PortableId;
  readonly stageId: "script";
  readonly runId: string;
  /**
   * @minItems 2
   * @maxItems 32
   */
  readonly inputs: readonly [ProviderInputArtifact, ProviderInputArtifact, ...ProviderInputArtifact[]];
  readonly config: ScriptGenerationConfig;
  readonly outputSchemaVersion: "narracut.script/v1";
  readonly requestedAt: Timestamp;
}
export interface ProviderInputArtifact {
  readonly artifactId: PortableId;
  readonly kind: "brief" | "claim_set" | "evidence_set";
  readonly contentHash: ContentHash;
  readonly sourceRunId: string;
  readonly reviewRecordId: PortableId;
  /**
   * @maxItems 4096
   */
  readonly provenance: readonly ProvenanceReference[];
  readonly content: string;
}
export interface ProvenanceReference {
  readonly claimId: PortableId;
  readonly evidenceRef: PortableId;
}
export interface ScriptGenerationConfig {
  readonly language: string;
  readonly maxOutputTokens: number;
  readonly targetDurationSeconds?: number;
}
export interface ProviderEventStarted {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "started";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
}
export interface ProviderEventOutputDelta {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "output_delta";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
  readonly delta: string;
}
export interface ProviderEventUsage {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "usage";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
  readonly usage: ProviderUsage;
}
export interface ProviderUsage {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly totalTokens: number;
  readonly cachedInputTokens?: number;
  readonly reasoningTokens?: number;
}
export interface ProviderEventCompleted {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "completed";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
  readonly responseId: PortableId;
}
export interface ProviderEventFailed {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "failed";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
  readonly code: ProviderErrorCode;
  readonly message: string;
  readonly retryable: boolean;
}
export interface ProviderEventCanceled {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_event";
  readonly eventType: "canceled";
  readonly providerRequestId: PortableId;
  readonly sequence: number;
  readonly occurredAt: Timestamp;
}
export interface StructuredProviderResult {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_result";
  readonly providerRequestId: PortableId;
  readonly providerId: ProviderId;
  readonly model: ModelId;
  readonly responseId: PortableId;
  readonly status: "completed";
  readonly output: StructuredScriptOutput;
  readonly usage: ProviderUsage;
  readonly completedAt: Timestamp;
}
export interface StructuredScriptOutput {
  readonly schemaVersion: "narracut.script/v1";
  readonly title: string;
  readonly language: string;
  readonly summary: string;
  readonly estimatedDurationSeconds: number;
  /**
   * @minItems 1
   * @maxItems 128
   */
  readonly segments: readonly [ScriptSegment, ...ScriptSegment[]];
}
export interface ScriptSegment {
  readonly segmentId: string;
  readonly order: number;
  readonly title: string;
  readonly narration: string;
  /**
   * @minItems 1
   * @maxItems 128
   */
  readonly provenance: readonly [ProvenanceReference, ...ProvenanceReference[]];
}
export interface ProviderCommandError {
  readonly apiVersion: ApiVersion;
  readonly messageType: "provider_command_error";
  readonly operation: ProviderOperation;
  readonly code: ProviderErrorCode;
  readonly message: string;
  readonly retryable: boolean;
  readonly providerId?: ProviderId;
  readonly jobId?: PortableId;
  readonly runId?: PortableId;
}
