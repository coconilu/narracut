import type {
  CaptionsMediaDocument,
  ScenePlanDocument,
} from "./generated/media-v1";

type AssertFalse<Value extends false> = Value;
type LegacyCaptions = Extract<
  CaptionsMediaDocument,
  { readonly schemaVersion: "1.0.0" }
>;
type LegacyScenePlan = Extract<
  ScenePlanDocument,
  { readonly schemaVersion: "1.0.0" }
>;

type LegacyCaptionHasProvenance = "provenance" extends keyof LegacyCaptions["cues"][number]
  ? true
  : false;
type LegacyScenePlanHasCueTraceability = "cueTraceability" extends keyof LegacyScenePlan
  ? true
  : false;
type LegacySceneHasProvenance = "provenance" extends keyof LegacyScenePlan["scenes"][number]
  ? true
  : false;

export type MediaVersionContractAssertions = [
  AssertFalse<LegacyCaptionHasProvenance>,
  AssertFalse<LegacyScenePlanHasCueTraceability>,
  AssertFalse<LegacySceneHasProvenance>,
];
