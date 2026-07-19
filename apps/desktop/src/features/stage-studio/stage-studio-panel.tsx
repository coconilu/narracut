import type { StageView } from "../../model/workbench";
import { MediaStageView } from "./media/media-stage-view";
import { isMediaStageId } from "./media/media-stage-model.js";
import type { MediaStageStudioController } from "./media/use-media-stage";
import { RendererStageView } from "./renderer/renderer-stage-view";
import type { RendererStageController } from "./renderer/use-renderer-stage";
import { OutputView, PreviewView } from "./views/artifact-views";
import { CompareView } from "./views/compare-view";
import { ConfigView } from "./views/config-view";
import { HistoryView } from "./views/history-view";
import { InputView } from "./views/input-view";
import { ReviewView } from "./views/review-view";
import { StudioEmpty, runStatusLabels } from "./stage-studio-primitives";
import type {
  StageStudioController,
  StageStudioTab,
} from "./use-stage-studio";

interface StageStudioPanelProps {
  readonly controller: StageStudioController;
  readonly stage: StageView;
  readonly disabled: boolean;
  readonly mediaController?: MediaStageStudioController;
  readonly rendererController?: RendererStageController;
}

const tabLabels: Record<StageStudioTab, string> = {
  input: "输入",
  config: "配置",
  output: "输出",
  preview: "预览",
  history: "历史",
  compare: "比较",
  review: "审核",
};

export function StageStudioPanel({
  controller,
  stage,
  disabled,
  mediaController,
  rendererController,
}: StageStudioPanelProps) {
  const snapshot = controller.snapshot;
  const selectedArtifacts = controller.selectedRun
    ? controller.artifactsByRun[controller.selectedRun.runId]
    : undefined;
  const compareArtifacts = controller.compareRun
    ? controller.artifactsByRun[controller.compareRun.runId]
    : undefined;

  return (
    <section className="stage-studio" aria-label="阶段审阅工作室">
      <div className="studio-toolbar">
        <div className="studio-tabs" role="tablist" aria-label="阶段工作区">
          {(Object.keys(tabLabels) as StageStudioTab[]).map((tab) => (
            <button
              aria-selected={controller.activeTab === tab}
              className={`studio-tab ${controller.activeTab === tab ? "active" : ""}`}
              data-testid={`studio-tab-${tab}`}
              disabled={disabled}
              key={tab}
              onClick={() => controller.setActiveTab(tab)}
              role="tab"
              type="button"
            >
              {tabLabels[tab]}
            </button>
          ))}
        </div>
        <RunSelectors controller={controller} disabled={disabled} />
      </div>

      <div className="studio-body" role="tabpanel">
        {!snapshot ? (
          <StudioEmpty
            title="正在准备阶段工作区"
            text="阶段配置、不可变运行与审核记录将从工程真相中读取。"
          />
        ) : controller.activeTab === "input" ? (
          <InputView run={controller.selectedRun} />
        ) : controller.activeTab === "config" ? (
          <ConfigView controller={controller} disabled={disabled} />
        ) : controller.activeTab === "output" ? (
          <OutputView
            artifacts={selectedArtifacts}
            loading={controller.artifactLoading}
            run={controller.selectedRun}
          />
        ) : controller.activeTab === "preview" ? (
          rendererController && stage.definition.stageId === "render" ? (
            <RendererStageView controller={rendererController} disabled={disabled} mode={snapshot.mode} />
          ) : snapshot.mode === "desktop" &&
          mediaController &&
          isMediaStageId(stage.definition.stageId) ? (
            <MediaStageView
              controller={mediaController}
              stageId={stage.definition.stageId}
            />
          ) : (
            <>
              {snapshot.mode === "demo" &&
              isMediaStageId(stage.definition.stageId) ? (
                <div
                  className="media-demo-readonly"
                  data-testid="media-demo-readonly"
                  role="status"
                >
                  浏览器演示模式保留原只读预览；本机导入与媒体编辑只在 Tauri 桌面端可用，当前不会调用媒体命令。
                </div>
              ) : null}
              <PreviewView
                artifacts={selectedArtifacts}
                loading={controller.artifactLoading}
                mode={snapshot.mode}
                run={controller.selectedRun}
                stage={stage}
              />
            </>
          )
        ) : controller.activeTab === "history" ? (
          <HistoryView controller={controller} disabled={disabled} stage={stage} />
        ) : controller.activeTab === "compare" ? (
          <CompareView
            compareArtifacts={compareArtifacts}
            compareRun={controller.compareRun}
            selectedArtifacts={selectedArtifacts}
            selectedRun={controller.selectedRun}
          />
        ) : (
          <ReviewView controller={controller} disabled={disabled} />
        )}
      </div>
    </section>
  );
}

function RunSelectors({
  controller,
  disabled,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
}) {
  const snapshot = controller.snapshot;
  return (
    <div className="studio-run-selectors">
      <label>
        <span>版本 A</span>
        <select
          aria-label="主版本"
          disabled={disabled || !snapshot?.runs.length}
          onChange={(event) => controller.setSelectedRunId(event.target.value)}
          value={controller.selectedRunId ?? ""}
        >
          {!snapshot?.runs.length ? <option value="">暂无运行</option> : null}
          {snapshot?.runs.map((run) => (
            <option key={run.runId} value={run.runId}>
              {run.runId} · {runStatusLabels[run.status]}
              {controller.isRunReadOnly(run) ? " · 继承只读" : ""}
            </option>
          ))}
        </select>
      </label>
      <label className={controller.activeTab === "compare" ? "visible" : ""}>
        <span>版本 B</span>
        <select
          aria-label="比较版本"
          disabled={disabled || !controller.compareRunId}
          onChange={(event) => controller.setCompareRunId(event.target.value)}
          value={controller.compareRunId ?? ""}
        >
          {!controller.compareRunId ? <option value="">无可比较版本</option> : null}
          {snapshot?.runs
            .filter((run) => run.runId !== controller.selectedRunId)
            .map((run) => (
              <option key={run.runId} value={run.runId}>
                {run.runId} · {runStatusLabels[run.status]}
                {controller.isRunReadOnly(run) ? " · 继承只读" : ""}
              </option>
            ))}
        </select>
      </label>
    </div>
  );
}
