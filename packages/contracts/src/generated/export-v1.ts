/* eslint-disable */
/**
 * 此文件由 schema/narracut-export-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * Versioned, fail-closed QA, export and portable manifest contracts.
 */
export type NarraCutExportMessage =
  | RunExportQaRequest
  | EnqueueExportRequest
  | GetExportResultRequest
  | VerifyExportRequest
  | ExportQaResult
  | ExportJobAcceptedResult
  | ExportResult
  | ExportVerificationResult
  | ExportManifest
  | ExportCommandError;
export type ApiVersion = "1.0.0";
export type ProjectPath = string;
export type PortableId = string;
export type RunId = string;
export type ArtifactId = string;
export type Sha256 = string;
/**
 * @maxItems 4096
 */
export type StringSet = string[];
export type IdempotencyKey = string;
export type JobId = string;
export type Timestamp = string;
export type ManifestVersion = "1.0.0";
export type ProjectUri = string;
export type RelativeExportPath = string;

export interface RunExportQaRequest {
  readonly apiVersion: ApiVersion;
  readonly operation: "run_export_qa";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly renderInput: ExportRenderInputReference;
}
export interface ExportRenderInputReference {
  readonly stageId: "render";
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  readonly resultArtifactId: ArtifactId;
  readonly contentHash: Sha256;
  readonly reviewRecordId: PortableId;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
export interface EnqueueExportRequest {
  readonly apiVersion: ApiVersion;
  readonly operation: "enqueue_export";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly runId: RunId;
  readonly renderInput: ExportRenderInputReference;
  readonly qaHash: Sha256;
  readonly destinationDirectory: ProjectPath;
  readonly exportName: string;
  readonly idempotencyKey: IdempotencyKey;
  readonly maxTemporaryBytes: number;
}
export interface GetExportResultRequest {
  readonly apiVersion: ApiVersion;
  readonly operation: "get_export_result";
  readonly projectPath: ProjectPath;
  readonly expectedProjectId: PortableId;
  readonly jobId: JobId;
}
export interface VerifyExportRequest {
  readonly apiVersion: ApiVersion;
  readonly operation: "verify_export";
  readonly exportDirectory: ProjectPath;
}
export interface ExportQaResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "run_export_qa";
  readonly ownerProjectId: PortableId;
  readonly renderInput: ExportRenderInputReference;
  readonly qa: ExportQaSummary;
}
export interface ExportQaSummary {
  readonly status: "passed" | "blocked";
  readonly passed: boolean;
  readonly warningCount: number;
  readonly blockingCount: number;
  /**
   * @minItems 1
   * @maxItems 128
   */
  readonly checks: readonly [ExportQaCheck, ...ExportQaCheck[]];
  /**
   * @maxItems 256
   */
  readonly diagnostics: readonly ExportQaDiagnostic[];
  readonly checkedAt: Timestamp;
  readonly qaHash: Sha256;
}
export interface ExportQaCheck {
  readonly checkId: PortableId;
  readonly category:
    | "canvas"
    | "duration"
    | "audio"
    | "scenes"
    | "captions"
    | "text_layout"
    | "provenance"
    | "rights"
    | "hash"
    | "renderer"
    | "probe";
  readonly status: "passed" | "warning" | "blocked";
  readonly message: string;
  readonly sceneIds: StringSet;
  /**
   * @maxItems 256
   */
  readonly artifactIds: readonly ArtifactId[];
}
export interface ExportQaDiagnostic {
  readonly diagnosticId: PortableId;
  readonly severity: "blocking" | "warning";
  readonly code: PortableId;
  readonly message: string;
  readonly sceneIds: StringSet;
  /**
   * @maxItems 256
   */
  readonly artifactIds: readonly ArtifactId[];
}
export interface ExportJobAcceptedResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "enqueue_export";
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly jobId: JobId;
  readonly status: "queued" | "running" | "retrying";
  readonly idempotentReplay: boolean;
}
export interface ExportResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "get_export_result";
  readonly ownerProjectId: PortableId;
  readonly runId: RunId;
  readonly jobId: JobId;
  readonly status: "succeeded";
  readonly exportId: PortableId;
  readonly exportPath: ProjectPath;
  readonly manifest: ExportManifest;
  readonly manifestHash: Sha256;
  readonly idempotentReplay: boolean;
}
export interface ExportManifest {
  readonly manifestVersion: ManifestVersion;
  readonly documentType: "export_manifest";
  readonly projectId: PortableId;
  readonly projectFormatVersion: number;
  readonly exportId: PortableId;
  readonly createdAt: Timestamp;
  readonly exportRunId: RunId;
  readonly renderRunId: RunId;
  readonly renderReviewRecordId: PortableId;
  /**
   * @minItems 2
   * @maxItems 256
   */
  readonly adoptedArtifacts: readonly [ExportAdoptedArtifact, ExportAdoptedArtifact, ...ExportAdoptedArtifact[]];
  readonly rendererIdentity: ExportRendererIdentity;
  readonly media: ExportMediaInfo;
  /**
   * @minItems 4
   * @maxItems 256
   */
  readonly files: readonly [
    ExportManifestFile,
    ExportManifestFile,
    ExportManifestFile,
    ExportManifestFile,
    ...ExportManifestFile[],
  ];
  /**
   * @maxItems 4096
   */
  readonly provenance: readonly ExportProvenanceReference[];
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
  /**
   * @minItems 1
   * @maxItems 256
   */
  readonly licenses: readonly [ExportLicenseRecord, ...ExportLicenseRecord[]];
  readonly qa: ExportQaSummary;
  readonly integrity: "complete";
}
export interface ExportAdoptedArtifact {
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  readonly kind: PortableId;
  readonly uri: ProjectUri;
  readonly contentHash: Sha256;
  readonly reviewRecordId: PortableId;
}
export interface ExportRendererIdentity {
  readonly adapterId: "narracut.ffmpeg";
  readonly adapterVersion: "1.0.0";
  readonly executableFileName: "ffmpeg" | "ffmpeg.exe";
  readonly executableHash: Sha256;
  readonly ffmpegVersion: string;
  readonly ffprobeFileName: "ffprobe" | "ffprobe.exe";
  readonly ffprobeHash: Sha256;
  readonly ffprobeVersion: string;
  readonly capabilityHash: Sha256;
}
export interface ExportMediaInfo {
  readonly width: number;
  readonly height: number;
  readonly durationMs: number;
  readonly frameRateNumerator: number;
  readonly frameRateDenominator: number;
  readonly videoCodec: "libx264";
  readonly audioCodec: "aac";
  readonly pixelFormat: "yuv420p";
  readonly hasAudio: boolean;
}
export interface ExportManifestFile {
  readonly role:
    "video" | "audio_reference" | "captions" | "timeline" | "manifest" | "licenses" | "checksums";
  readonly relativePath: RelativeExportPath;
  readonly sourceUri: ProjectUri;
  readonly contentHash: Sha256;
  readonly byteLength: number;
  readonly mediaType: string;
}
export interface ExportProvenanceReference {
  readonly claimId: string;
  readonly evidenceRef: string;
}
export interface ExportLicenseRecord {
  readonly artifactId: ArtifactId;
  readonly sourceFileName: string;
  readonly author: string;
  readonly licenseId: string;
  readonly rightsStatement: string;
  readonly attributionText: string;
  readonly authorizationRecordIds: StringSet;
}
export interface ExportVerificationResult {
  readonly apiVersion: ApiVersion;
  readonly operation: "verify_export";
  readonly status: "verified" | "corrupt";
  readonly manifestHash: Sha256;
  readonly filesChecked: number;
  /**
   * @maxItems 256
   */
  readonly diagnostics: readonly ExportQaDiagnostic[];
}
export interface ExportCommandError {
  readonly apiVersion: ApiVersion;
  readonly operation: "run_export_qa" | "enqueue_export" | "get_export_result" | "verify_export";
  readonly code:
    | "invalid_request"
    | "invalid_project"
    | "project_mismatch"
    | "render_not_approved"
    | "render_stale"
    | "artifact_not_found"
    | "hash_mismatch"
    | "qa_blocked"
    | "qa_changed"
    | "rights_incomplete"
    | "renderer_identity_changed"
    | "destination_invalid"
    | "destination_conflict"
    | "disk_space_insufficient"
    | "canceled"
    | "io_error"
    | "internal_contract_error";
  readonly message: string;
  readonly retryable: boolean;
  readonly details?: {
    [k: string]: unknown | undefined;
  };
}
