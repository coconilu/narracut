import { invoke } from "@tauri-apps/api/core";
import {
  NARRACUT_MEDIA_COMMAND_API_VERSION,
  type EnqueueAudioImportRequest,
  type EnqueueCaptionsImportRequest,
  type GenerateScenePlanRequest,
  type GenerateTimelineRequest,
  type GetMediaDocumentRequest,
  type MediaDocumentResult,
  type MediaJobAcceptedResult,
  type MediaSaveResult,
  type SaveScenePlanRequest,
  type SaveTimelineRequest,
} from "@narracut/contracts";

export { isMediaCommandError } from "./media-commands-model.js";

export type EnqueueAudioImportInput = Omit<EnqueueAudioImportRequest, "apiVersion" | "command">;
export type EnqueueCaptionsImportInput = Omit<EnqueueCaptionsImportRequest, "apiVersion" | "command">;
export type GenerateScenePlanInput = Omit<GenerateScenePlanRequest, "apiVersion" | "command">;
export type GenerateTimelineInput = Omit<GenerateTimelineRequest, "apiVersion" | "command">;
export type GetMediaDocumentInput = Omit<GetMediaDocumentRequest, "apiVersion" | "command">;
export type SaveScenePlanInput = Omit<SaveScenePlanRequest, "apiVersion" | "command">;
export type SaveTimelineInput = Omit<SaveTimelineRequest, "apiVersion" | "command">;

export const mediaCommands = {
  enqueueAudioImport(input: EnqueueAudioImportInput): Promise<MediaJobAcceptedResult> {
    return invoke("enqueue_audio_import", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "enqueue_audio_import",
      ...input,
    } satisfies EnqueueAudioImportRequest });
  },
  enqueueCaptionsImport(input: EnqueueCaptionsImportInput): Promise<MediaJobAcceptedResult> {
    return invoke("enqueue_captions_import", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "enqueue_captions_import",
      ...input,
    } satisfies EnqueueCaptionsImportRequest });
  },
  generateScenePlan(input: GenerateScenePlanInput): Promise<MediaJobAcceptedResult> {
    return invoke("generate_scene_plan", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "generate_scene_plan",
      ...input,
    } satisfies GenerateScenePlanRequest });
  },
  generateTimeline(input: GenerateTimelineInput): Promise<MediaJobAcceptedResult> {
    return invoke("generate_timeline", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "generate_timeline",
      ...input,
    } satisfies GenerateTimelineRequest });
  },
  getDocument(input: GetMediaDocumentInput): Promise<MediaDocumentResult> {
    return invoke("get_media_document", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "get_media_document",
      ...input,
    } satisfies GetMediaDocumentRequest });
  },
  saveScenePlan(input: SaveScenePlanInput): Promise<MediaSaveResult> {
    return invoke("save_scene_plan", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "save_scene_plan",
      ...input,
    } satisfies SaveScenePlanRequest });
  },
  saveTimeline(input: SaveTimelineInput): Promise<MediaSaveResult> {
    return invoke("save_timeline", { request: {
      apiVersion: NARRACUT_MEDIA_COMMAND_API_VERSION,
      command: "save_timeline",
      ...input,
    } satisfies SaveTimelineRequest });
  },
} as const;
