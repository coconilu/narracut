/* eslint-disable */
/**
 * 此文件由 schema/narracut-media-v1.schema.json 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */

/**
 * NarraCut v1 reviewed media, scene-plan, and minimal timeline contracts.
 */
export type NarraCutMediaDocument =
  AudioMediaDocument | CaptionsMediaDocument | ScenePlanDocument | TimelineDocument;
export type SchemaVersion = "1.0.0";
export type PortableId = string;
export type RunId = string;
export type ProjectUri = string;
export type Sha256 = string;
export type ArtifactId = string;
/**
 * @maxItems 1024
 */
export type StringSet = string[];
export type Timestamp = string;

export interface AudioMediaDocument {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "audio_media";
  readonly mediaId: PortableId;
  readonly projectId: PortableId;
  readonly runId: RunId;
  readonly artifactUri: ProjectUri;
  readonly source: ImportedSourceIdentity;
  readonly rights: MediaRights;
  readonly durationMs: number;
  readonly sampleRateHz: number;
  readonly bitsPerSample: 8 | 16 | 24 | 32;
  readonly channels: number;
  readonly blockAlign: number;
  readonly byteRate: number;
  readonly dataBytes: number;
  /**
   * @minItems 1
   * @maxItems 8
   */
  readonly inputRefs:
    | [FrozenArtifactInput]
    | [FrozenArtifactInput, FrozenArtifactInput]
    | [FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput]
    | [FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ];
  readonly configSnapshot: JsonObject;
  readonly createdAt: Timestamp;
}
export interface ImportedSourceIdentity {
  readonly sourceFileName: string;
  readonly sourceContentHash: Sha256;
  readonly byteLength: number;
}
export interface MediaRights {
  readonly ownership: "self_recorded" | "licensed";
  readonly author: string;
  readonly rightsStatement: string;
  readonly licenseId: string;
  readonly attributionText: string;
  readonly voiceAuthorization: "not_voice_clone";
}
export interface FrozenArtifactInput {
  readonly projectId: PortableId;
  readonly stageId: PortableId;
  readonly runId: RunId;
  readonly artifactId: ArtifactId;
  readonly contentHash: Sha256;
  readonly reviewRecordId: PortableId;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
export interface JsonObject {
  readonly [k: string]: unknown | undefined;
}
export interface CaptionsMediaDocument {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "captions_media";
  readonly captionsId: PortableId;
  readonly projectId: PortableId;
  readonly runId: RunId;
  readonly rawArtifactId: ArtifactId;
  readonly rawContentHash: Sha256;
  readonly source: ImportedSourceIdentity;
  readonly audioInput: FrozenArtifactInput;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly cues: readonly [CaptionCue, ...CaptionCue[]];
  /**
   * @minItems 1
   * @maxItems 200000
   */
  readonly mappings: readonly [TimingMapping, ...TimingMapping[]];
  /**
   * @maxItems 10000
   */
  readonly diagnostics: readonly MediaDiagnostic[];
  /**
   * @minItems 2
   * @maxItems 16
   */
  readonly inputRefs:
    | [FrozenArtifactInput, FrozenArtifactInput]
    | [FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput]
    | [FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput, FrozenArtifactInput]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ]
    | [
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
        FrozenArtifactInput,
      ];
  readonly configSnapshot: JsonObject;
  readonly createdAt: Timestamp;
}
export interface CaptionCue {
  readonly cueId: PortableId;
  readonly sourceIndex: number;
  readonly startMs: number;
  readonly endMs: number;
  readonly text: string;
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
export interface TimingMapping {
  readonly mappingId: PortableId;
  readonly level: "cue" | "sentence" | "word";
  readonly sourceCueId: PortableId;
  readonly startMs: number;
  readonly endMs: number;
  readonly text: string;
  readonly timingPrecision: "cue_exact" | "estimated";
  readonly timingBasis: "srt_cue" | "sentence_interpolation" | "word_interpolation";
}
export interface MediaDiagnostic {
  readonly code: PortableId;
  readonly severity: "info" | "warning" | "error";
  readonly message: string;
  readonly blocking: boolean;
  readonly cueId?: PortableId;
  readonly sceneId?: PortableId;
}
export interface ScenePlanDocument {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "scene_plan";
  readonly scenePlanId: PortableId;
  readonly projectId: PortableId;
  readonly runId: RunId;
  readonly supersedesArtifactId?: ArtifactId;
  /**
   * @minItems 2
   * @maxItems 32
   */
  readonly inputRefs: readonly [FrozenArtifactInput, FrozenArtifactInput, ...FrozenArtifactInput[]];
  readonly configSnapshot: JsonObject;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly scenes: readonly [ScenePlanScene, ...ScenePlanScene[]];
  /**
   * @maxItems 10000
   */
  readonly diagnostics: readonly MediaDiagnostic[];
  readonly changeSummary: ChangeSummary;
  readonly createdAt: Timestamp;
}
export interface ScenePlanScene {
  readonly sceneId: PortableId;
  readonly order: number;
  readonly title: string;
  readonly narrativeRole: string;
  readonly suggestedStartMs: number;
  readonly suggestedEndMs: number;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly cueIds: readonly [PortableId, ...PortableId[]];
  readonly claimIds: StringSet;
  readonly evidenceRefs: StringSet;
}
export interface ChangeSummary {
  readonly summary: string;
  /**
   * @maxItems 10000
   */
  readonly changedSceneIds: readonly PortableId[];
}
export interface TimelineDocument {
  readonly schemaVersion: SchemaVersion;
  readonly documentType: "timeline";
  readonly timelineId: PortableId;
  readonly projectId: PortableId;
  readonly runId: RunId;
  readonly supersedesArtifactId?: ArtifactId;
  readonly durationMs: number;
  readonly canvas: TimelineCanvas;
  readonly audioTrack: TimelineAudioTrack;
  /**
   * @minItems 1
   * @maxItems 10000
   */
  readonly sceneTrack: readonly [TimelineScene, ...TimelineScene[]];
  readonly captionTrack: TimelineCaptionReferenceTrack;
  readonly safeArea: TimelineSafeArea;
  /**
   * @minItems 3
   * @maxItems 32
   */
  readonly inputRefs: readonly [
    FrozenArtifactInput,
    FrozenArtifactInput,
    FrozenArtifactInput,
    ...FrozenArtifactInput[],
  ];
  readonly configSnapshot: JsonObject;
  readonly changeSummary: ChangeSummary;
  readonly createdAt: Timestamp;
}
export interface TimelineCanvas {
  readonly width: number;
  readonly height: number;
  readonly frameRateNumerator: number;
  readonly frameRateDenominator: number;
}
export interface TimelineAudioTrack {
  readonly audioArtifactId: ArtifactId;
  readonly startMs: 0;
  readonly endMs: number;
}
export interface TimelineScene {
  readonly sceneId: PortableId;
  readonly startMs: number;
  readonly endMs: number;
}
export interface TimelineCaptionReferenceTrack {
  readonly captionsArtifactId: ArtifactId;
  /**
   * @maxItems 10000
   */
  readonly cueIds: readonly PortableId[];
  readonly visible: boolean;
}
export interface TimelineSafeArea {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}
