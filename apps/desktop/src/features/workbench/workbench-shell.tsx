import { useMemo, useState } from "react";
import { Brand } from "../../components/brand";
import { Icon } from "../../components/icons";
import type { WorkspaceBundle } from "../../lib/desktop-gateway";
import {
  buildStageViews,
  chooseInitialStageId,
  stageStatusLabel,
} from "../../model/workbench";
import { RunHistoryPanel } from "../stage-studio/run-history-panel";
import { StageStudioPanel } from "../stage-studio/stage-studio-panel";
import { useStageStudio } from "../stage-studio/use-stage-studio";
import { ActivityPanel, type ActivityTab } from "./activity-panel";
import { StageRail } from "./stage-rail";

interface WorkbenchShellProps {
  readonly bundle: WorkspaceBundle;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly onBack: () => void;
  readonly onCancelJob: (jobId: string) => Promise<boolean>;
  readonly onRecover: () => Promise<boolean>;
  readonly onRefresh: () => Promise<boolean>;
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
  const [shellNotice, setShellNotice] = useState<string | null>(null);
  const selectedStage =
    stages.find((stage) => stage.definition.stageId === selectedStageId) ?? stages[0];
  const studio = useStageStudio({
    project: bundle.project,
    workflow: bundle.workflow,
    stageId: selectedStage?.definition.stageId ?? selectedStageId,
    supportsRegeneration:
      selectedStage?.definition.supportsPartialRegeneration ?? false,
    onRefreshWorkspace: onRefresh,
  });
  const combinedBusyLabel = busyLabel ?? studio.busyLabel;
  const disabled = combinedBusyLabel !== null;
  const selectedStageActiveJobs = bundle.jobs.filter(
    (job) =>
      job.stageId === selectedStage?.definition.stageId &&
      ["queued", "running", "retrying"].includes(job.status),
  );
  const activeJob =
    selectedStageActiveJobs.length === 1 ? selectedStageActiveJobs[0] : undefined;
  const visibleNotice = studio.notice ?? shellNotice;
  const visibleError = studio.error ?? error;

  if (!selectedStage) {
    return (
      <main className="workbench-empty">
        <Brand />
        <h1>工作流没有可用阶段</h1>
        <button className="button" onClick={onBack} type="button">返回项目</button>
      </main>
    );
  }

  async function stopActiveJob() {
    if (!activeJob) return;
    const succeeded = await onCancelJob(activeJob.jobId);
    if (!succeeded) return;
    setShellNotice("停止请求已记录。当前任务及其执行快照仍保留；继续执行必须明确发起新的重试或运行。");
    setActivityTab("events");
  }

  async function recoverJobs() {
    const succeeded = await onRecover();
    if (!succeeded) return;
    setShellNotice("任务恢复扫描已完成；恢复、终结与索引结果已写入活动区。");
    setActivityTab("events");
  }

  async function refreshAll() {
    const workspaceRefreshed = await onRefresh();
    if (!workspaceRefreshed) return;
    await studio.refreshStage();
  }

  function chooseStage(stageId: string) {
    if (disabled) return;
    setSelectedStageId(stageId);
    studio.setActiveTab("preview");
    studio.clearNotice();
    setShellNotice(null);
  }

  function clearVisibleNotice() {
    if (studio.notice) studio.clearNotice();
    else setShellNotice(null);
  }

  function clearVisibleError() {
    if (studio.error) studio.clearError();
    else onClearError();
  }

  return (
    <main className="workbench-shell" data-testid="workbench-shell">
      <header className="workbench-topbar">
        <button
          className="back-button"
          aria-label="返回项目列表"
          disabled={disabled}
          onClick={onBack}
          type="button"
        >
          <Icon name="chevron-left" size={17} />
        </button>
        <Brand compact />
        <span className="top-divider" aria-hidden="true" />
        <div className="project-lockup">
          <strong>{bundle.project.name}</strong>
          <span>本地工程 · 已保存 · {bundle.mode === "demo" ? "演示模式" : "真实工程"}</span>
        </div>
        <div className="stage-context">
          <i className={selectedStage.state.status} />
          {selectedStage.definition.title} · {stageStatusLabel(selectedStage.state.status)}
        </div>
        <div className="top-spacer" />
        <div className="executor">
          AI 执行器
          <strong>
            {studio.selectedRun
              ? `${studio.selectedRun.executor.providerId} · ${studio.selectedRun.executor.executionMode}`
              : "等待历史快照"}
          </strong>
        </div>
        <div className="workbench-actions">
          <button
            className="button"
            disabled={disabled}
            onClick={() => studio.setActiveTab("config")}
            type="button"
          >
            配置
          </button>
          <button
            className="button primary"
            disabled={disabled || !studio.selectedRun || !selectedStage.definition.supportsPartialRegeneration}
            onClick={() => studio.setActiveTab("history")}
            title={
              selectedStage.definition.supportsPartialRegeneration
                ? "先查看影响范围，再创建新的运行任务"
                : "当前阶段契约未声明局部重生成能力"
            }
            type="button"
          >
            重生成
          </button>
          <button
            className="button danger"
            disabled={!activeJob || disabled}
            onClick={() => void stopActiveJob()}
            title={
              selectedStageActiveJobs.length > 1
                ? "当前阶段存在多个活动任务，请从任务历史中选择后再停止"
                : activeJob
                  ? `停止任务 ${activeJob.jobId}`
                  : "当前阶段没有可停止的任务"
            }
            type="button"
          >
            停止
          </button>
          <button
            className="button"
            disabled={disabled}
            onClick={() => studio.setActiveTab("history")}
            type="button"
          >
            历史
          </button>
          <button
            className="button"
            disabled={disabled}
            onClick={() => {
              chooseStage("export");
              setShellNotice("已定位导出阶段；最终媒体必须与可追踪 manifest 一起生成。实际导出流程属于后续 PR。");
            }}
            type="button"
          >
            导出
          </button>
        </div>
      </header>

      <div className="workspace stage-studio-workspace">
        <StageRail
          disabled={disabled}
          onSelect={chooseStage}
          selectedStageId={selectedStage.definition.stageId}
          stages={stages}
        />
        <StageStudioPanel
          controller={studio}
          disabled={disabled}
          stage={selectedStage}
        />
        <RunHistoryPanel
          controller={studio}
          disabled={disabled}
          stage={selectedStage}
        />
      </div>

      <ActivityPanel
        activeTab={activityTab}
        busy={disabled}
        events={bundle.events}
        jobs={bundle.jobs}
        onRecover={recoverJobs}
        onTabChange={setActivityTab}
        recovery={bundle.recovery}
      />

      <div className="workbench-statusline">
        <span>
          {stageStatusLabel(selectedStage.state.status)} · {selectedStage.state.latestRunId ?? "尚未运行"}
        </span>
        <button disabled={disabled} onClick={() => void refreshAll()} type="button">
          <Icon name="refresh" size={13} />刷新
        </button>
      </div>

      {visibleNotice ? (
        <div className="workbench-notice" role="status">
          <span>{visibleNotice}</span>
          <button aria-label="关闭提示" onClick={clearVisibleNotice} type="button">
            <Icon name="x" size={14} />
          </button>
        </div>
      ) : null}

      {visibleError ? (
        <div className="workbench-error" role="alert">
          <Icon name="alert" size={15} />
          <span>{visibleError}</span>
          <button aria-label="关闭错误提示" onClick={clearVisibleError} type="button">
            <Icon name="x" size={14} />
          </button>
        </div>
      ) : null}

      {combinedBusyLabel ? (
        <div className="busy-status workbench-busy" role="status">
          <span className="busy-spinner" aria-hidden="true" />
          {combinedBusyLabel}
        </div>
      ) : null}
    </main>
  );
}
