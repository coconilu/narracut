import type {
  AudioMediaDocument,
  CaptionsMediaDocument,
  MediaDocumentValue,
  MediaReviewedInputReference,
  MediaRightsInput,
  NarraCutMediaDocument,
  ScenePlanDocument,
  ScenePlanEdit,
  TimelineDocument,
  TimelineEdit,
} from "@narracut/contracts";

export type MediaStageId = "audio" | "captions" | "scene_plan" | "timeline";
export type MediaDocumentType = NarraCutMediaDocument["documentType"];

export interface MediaStageRequirement {
  readonly stageId: string;
  readonly artifactKinds: readonly string[];
}

export type ValidationResult<T> =
  | { readonly valid: true; readonly value: T }
  | { readonly valid: false; readonly errors: readonly string[] };

export interface ReviewedInputBuildInput {
  readonly expectedProjectId: string;
  readonly expectedStageId: string;
  readonly expectedArtifactKinds?: readonly string[];
  readonly stageState: {
    readonly stageId: string;
    readonly status: string;
    readonly approvedRunId?: string;
  };
  readonly run: {
    readonly projectId: string;
    readonly stageId: string;
    readonly runId: string;
    readonly status: string;
    readonly artifactIds: readonly string[];
  };
  readonly review: {
    readonly reviewId: string;
    readonly projectId: string;
    readonly stageId: string;
    readonly runId: string;
    readonly decision: string;
    readonly artifactIds: readonly string[];
  };
  readonly artifact: {
    readonly projectId: string;
    readonly stageId: string;
    readonly runId: string;
    readonly artifactId: string;
    readonly kind: string;
    readonly contentHash?: string;
    readonly provenance: readonly {
      readonly claimId: string;
      readonly evidenceRef: string;
    }[];
  };
}

export interface ImportFormInput {
  readonly sourcePath: string;
  readonly ownership: "self_recorded" | "licensed" | string;
  readonly author: string;
  readonly rightsStatement: string;
  readonly licenseId: string;
  readonly attributionText: string;
}

export interface ValidatedImportForm {
  readonly sourcePath: string;
  readonly rights: MediaRightsInput;
}

export interface TimelineLayoutItem {
  readonly id: string;
  readonly startMs: number;
  readonly endMs: number;
  readonly leftPercent: number;
  readonly widthPercent: number;
}

export interface TimelineTrackLayout {
  readonly durationMs: number;
  readonly tracks: readonly [
    {
      readonly trackId: "audio";
      readonly items: readonly [TimelineLayoutItem];
    },
    {
      readonly trackId: "scenes";
      readonly items: readonly TimelineLayoutItem[];
    },
    {
      readonly trackId: "captions";
      readonly visible: boolean;
      readonly cueCount: number;
      readonly items: readonly [TimelineLayoutItem];
    },
  ];
}

export function isMediaStageId(value: unknown): value is MediaStageId;
export function requirementsForStage(
  stageId: unknown,
): readonly MediaStageRequirement[] | null;

export function narrowMediaDocument(
  value: MediaDocumentValue | unknown,
): NarraCutMediaDocument | null;
export function narrowMediaDocument(
  value: MediaDocumentValue | unknown,
  expectedDocumentType: "audio_media",
): AudioMediaDocument | null;
export function narrowMediaDocument(
  value: MediaDocumentValue | unknown,
  expectedDocumentType: "captions_media",
): CaptionsMediaDocument | null;
export function narrowMediaDocument(
  value: MediaDocumentValue | unknown,
  expectedDocumentType: "scene_plan",
): ScenePlanDocument | null;
export function narrowMediaDocument(
  value: MediaDocumentValue | unknown,
  expectedDocumentType: "timeline",
): TimelineDocument | null;

export function buildReviewedInputReference(
  input: ReviewedInputBuildInput | unknown,
): ValidationResult<MediaReviewedInputReference>;

export function validateImportForm(
  input: ImportFormInput | unknown,
): ValidationResult<ValidatedImportForm>;

export function validateSceneEdit(
  document: ScenePlanDocument | unknown,
  edit: ScenePlanEdit | unknown,
): ValidationResult<ScenePlanEdit>;

export function validateTimelineEdit(
  document: TimelineDocument | unknown,
  edit: TimelineEdit | unknown,
): ValidationResult<TimelineEdit>;

export function formatDuration(milliseconds: number): string;

export function buildTimelineTrackLayout(
  document: TimelineDocument | unknown,
): TimelineTrackLayout | null;
