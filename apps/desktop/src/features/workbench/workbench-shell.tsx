import { useMemo, useState } from "react";
import { Brand } from "../../components/brand";
import { Icon } from "../../components/icons";
import type { WorkspaceBundle } from "../../lib/desktop-gateway";
import {
  buildStageViews,
  chooseInitialStageId,
  stageStatusLabel,
} from "../../model/workbench";
import { ActivityPanel, type ActivityTab } from "./activity-panel";
import { InspectorPanel } from "./inspector-panel";
import { PreviewCanvas } from "./preview-canvas";
import { StageRail } from "./stage-rail";

interface WorkbenchShellProps {
  readonly bundle: WorkspaceBundle;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly onBack: () => void;
  readonly onCancelJob: (jobId: string) => Promise<void>;
  readonly onRecover: () => Promise<void>;
  readonly onRefresh: () => Promise<void>;
  readonly onClearError: () => void;
}

export function WorkbenchShell({
  bundle,
  busyLabel,
  error,
  onBack,
  onCancelJob,
  onRecover,
  onRefresh,
  onClearError,
}: WorkbenchShellProps) {
  const stages = useMemo(() => buildStageViews(bundle.workflow), [bundle.workflow]);
  const [selectedStageId, setSelectedStageId] = useState(() =>
    chooseInitialStageId(bundle.workflow),
  );
  const [activityTab, setActivityTab] = useState<ActivityTab>("events");
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const selectedStage =
    stages.find((stage) => stage.definition.stageId === selectedStageId) ?? stages[0];
  const activeJob = bundle.jobs.find((job) =>
    ["queued", "running", "retrying"].includes(job.status),
  );

  if (!selectedStage) {
    return (
      <main className="workbench-empty">
        <Brand />
        <h1>工作流没有可用阶段</h1>
        <button className="button" onClick={onBack} type="button">返回项目</button>
      </main>
    );
  }

  function showRunBoundary() {
    setNotice("PR06 已建立运行入口与任务观察区；阶段参数编辑和实际入队将在对应编辑器 PR 接入。历史运行不会被覆盖。");
    setActivityTab("events");
  }

  async function stopActiveJob() {
    if (!activeJob) return;
    await onCancelJob(activeJob.jobId);
    setNotice("停止请求已记录。当前运行保留在历史中，可从事件区恢复或重试。");
    setActivityTab("events");
  }

  async function recoverJobs() {
    await onRecover();
    setNotice("任务恢复扫描已完成，任务索引已同步。");
    setActivityTab("events");
  }

  return (
    <main className={`workbench-shell ${inspectorOpen ? "show-inspector" : ""}`} data-testid="workbench-shell">
      <header className="workbench-topbar">
        <button className="back-button" aria-label="返回项目列表" onClick={onBack} type="button">
          <Icon name="chevron-left" size={17} />
        </button>
        <Brand compact />
        <span className="top-divider" aria-hidden="true" />
        <div className="project-lockup">
          <strong>{bundle.project.name}</strong>
          <span>本地工程 · 已保存</span>
        </div>
        <div className="stage-context"><i />{selectedStage.definition.title}</div>
        <div className="top-spacer" />
        <div className="executor">AI 执行器 <strong>{bundle.mode === "demo" ? "Codex CLI" : "按阶段配置"}</strong></div>
        <button
          aria-expanded={inspectorOpen}
          className="button inspector-toggle"
          onClick={() => setInspectorOpen((current) => !current)}
          type="button"
        >
          配置
        </button>
        <div className="workbench-actions">
          <button className="button primary" onClick={showRunBoundary} type="button">
            运行
          </button>
          <button
            className="button danger"
            disabled={!activeJob || busyLabel !== null}
            onClick={() => void stopActiveJob()}
            type="button"
          >
            停止
          </button>
          <button
            className="button"
            onClick={() => {
              setActivityTab("events");
              setNotice("已定位当前工程的任务事件与历史运行入口。");
            }}
            type="button"
          >
            历史
          </button>
          <button
            className="button"
            onClick={() => {
              setSelectedStageId("export");
              setNotice("已定位导出阶段；最终媒体必须连同可追踪 manifest 一起生成。");
            }}
            type="button"
          >
            导出
          </button>
        </div>
      </header>

      <div className="workspace">
        <StageRail
          onSelect={(stageId) => {
            setSelectedStageId(stageId);
            setNotice(null);
          }}
          selectedStageId={selectedStage.definition.stageId}
          stages={stages}
        />
        <PreviewCanvas
          showDemoContent={bundle.mode === "demo"}
          stage={selectedStage}
        />
        <InspectorPanel
          onRunIntent={showRunBoundary}
          showDemoContent={bundle.mode === "demo"}
          stage={selectedStage}
          workflow={bundle.workflow}
        />
      </div>

      <ActivityPanel
        activeTab={activityTab}
        busy={busyLabel !== null}
        events={bundle.events}
        jobs={bundle.jobs}
        onRecover={recoverJobs}
        onTabChange={setActivityTab}
        recovery={bundle.recovery}
      />

      <div className="workbench-statusline">
        <span>{stageStatusLabel(selectedStage.state.status)} · {selectedStage.state.latestRunId ?? "尚未运行"}</span>
        <button onClick={() => void onRefresh()} type="button"><Icon name="refresh" size={13} />刷新</button>
      </div>

      {notice ? (
        <div className="workbench-notice" role="status">
          <span>{notice}</span>
          <button aria-label="关闭提示" onClick={() => setNotice(null)} type="button"><Icon name="x" size={14} /></button>
        </div>
      ) : null}

      {error ? (
        <div className="workbench-error" role="alert">
          <Icon name="alert" size={15} /><span>{error}</span>
          <button aria-label="关闭错误提示" onClick={onClearError} type="button"><Icon name="x" size={14} /></button>
        </div>
      ) : null}

      {busyLabel ? (
        <div className="busy-status workbench-busy" role="status">
          <span className="busy-spinner" aria-hidden="true" />{busyLabel}
        </div>
      ) : null}
    </main>
  );
}
