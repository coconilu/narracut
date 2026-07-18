import { MediaDocumentSummary } from "./media-document-summary";
import { MediaImportView } from "./media-import-view";
import { MediaScenePlanView } from "./media-scene-plan-view";
import {
  narrowMediaDocument,
  type MediaStageId,
} from "./media-stage-model.js";
import { MediaTimelineView } from "./media-timeline-view";
import type { MediaStageStudioController } from "./use-media-stage";

export interface MediaStageViewProps {
  readonly controller: MediaStageStudioController;
  readonly stageId: MediaStageId;
}

function AudioStage({
  controller,
}: {
  readonly controller: MediaStageStudioController;
}) {
  const document = narrowMediaDocument(controller.document, "audio_media");
  return (
    <div className="media-audio-view" data-testid="media-audio-view">
      <MediaDocumentSummary document={document} stageId="audio" />
      <MediaImportView controller={controller} stageId="audio" />
    </div>
  );
}

function CaptionsStage({
  controller,
}: {
  readonly controller: MediaStageStudioController;
}) {
  const document = narrowMediaDocument(controller.document, "captions_media");
  return (
    <div className="media-captions-view" data-testid="media-captions-view">
      <MediaDocumentSummary document={document} stageId="captions" />
      <MediaImportView controller={controller} stageId="captions" />
    </div>
  );
}

function ScenePlanStage({
  controller,
}: {
  readonly controller: MediaStageStudioController;
}) {
  return (
    <div className="media-scene-plan-view" data-testid="media-scene-plan-view">
      <MediaScenePlanView controller={controller} />
    </div>
  );
}

function TimelineStage({
  controller,
}: {
  readonly controller: MediaStageStudioController;
}) {
  return (
    <div className="media-timeline-view-shell" data-testid="media-timeline-view">
      <MediaTimelineView controller={controller} />
    </div>
  );
}

export function MediaStageView({
  controller,
  stageId,
}: MediaStageViewProps) {
  let content;
  switch (stageId) {
    case "audio":
      content = <AudioStage controller={controller} />;
      break;
    case "captions":
      content = <CaptionsStage controller={controller} />;
      break;
    case "scene_plan":
      content = <ScenePlanStage controller={controller} />;
      break;
    case "timeline":
      content = <TimelineStage controller={controller} />;
      break;
  }

  return (
    <div
      className="studio-scroll media-stage-view"
      data-testid="media-stage-view"
    >
      {content}
    </div>
  );
}
