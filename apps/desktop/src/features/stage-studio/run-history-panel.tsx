import type { ReviewDecision, StageRun } from "@narracut/contracts";
import type { StageView } from "../../model/workbench";
import { stageStatusLabel } from "../../model/workbench";
import type { StageStudioController } from "./use-stage-studio";

interface RunHistoryPanelProps {
  readonly controller: StageStudioController;
  readonly stage: StageView;
  readonly disabled: boolean;
}

const runStatusLabels: Record<StageRun["status"], string> = {
  queued: "排队",
  running: "运行中",
  succeeded: "完成",
  failed: "失败",
  canceled: "取消",
};

const reviewLabels: Record<ReviewDecision, string> = {
  approved: "采用",
  changes_requested: "请求修改",
  rejected: "拒绝",
};

export function RunHistoryPanel({
  controller,
  stage,
  disabled,
}: RunHistoryPanelProps) {
  const snapshot = controller.snapshot;
  const selectedReviews =
    snapshot?.reviews.filter(
      (review) => review.runId === controller.selectedRunId,
    ) ?? [];
  const latestReview = [...selectedReviews].sort((left, right) =>
    right.createdAt.localeCompare(left.createdAt),
  )[0];

  return (
    <aside className="run-history-panel" aria-label="运行历史与审核摘要">
      <div className="run-history-header">
        <div>
          <strong>运行历史</strong>
          <span>{snapshot?.runs.length ?? 0} 个不可变版本</span>
        </div>
        <em className={`workflow-state ${stage.state.status}`}>
          {stageStatusLabel(stage.state.status)}
        </em>
      </div>
      <div className="run-history-list">
        {snapshot?.runs.length ? (
          snapshot.runs.map((run) => {
            const selected = run.runId === controller.selectedRunId;
            const adopted = stage.state.approvedRunId === run.runId;
            return (
              <button
                aria-current={selected ? "true" : undefined}
                className={selected ? "selected" : ""}
                disabled={disabled}
                key={run.runId}
                onClick={() => controller.setSelectedRunId(run.runId)}
                type="button"
              >
                <span className="run-history-title">
                  <strong>{run.runId}</strong>
                  <em className={adopted ? "adopted" : run.status}>
                    {adopted ? "已采用" : runStatusLabels[run.status]}
                  </em>
                </span>
                <time>{formatDate(run.completedAt ?? run.createdAt)} · {run.executor.executionMode}</time>
                <p>{run.logSummary.message}</p>
                <small>
                  配置 rev {run.configSnapshot.revision} · {run.artifactIds.length} 个产物
                  {run.logSummary.warnings.length ? ` · ${run.logSummary.warnings.length} 个警告` : ""}
                </small>
              </button>
            );
          })
        ) : (
          <div className="run-history-empty">当前阶段还没有历史运行。</div>
        )}
      </div>
      <div className="review-summary-panel">
        <div className="review-summary-title">
          <strong>当前版本审核</strong>
          <span>{controller.selectedArtifactIds.length} 个产物已选择</span>
        </div>
        {latestReview ? (
          <article className="latest-review">
            <div><em className={latestReview.decision}>{reviewLabels[latestReview.decision]}</em><time>{formatDate(latestReview.createdAt)}</time></div>
            <p>{latestReview.comments}</p>
            <small>{latestReview.reviewer.displayName} · {latestReview.reviewId}</small>
          </article>
        ) : (
          <p className="no-review">该运行尚未审核；采用不会自动发生。</p>
        )}
        <button
          className="button primary"
          disabled={disabled || !controller.selectedRun}
          onClick={() => controller.setActiveTab("review")}
          type="button"
        >
          打开审核工作区
        </button>
        <button
          className="button"
          disabled={disabled || !controller.compareRun}
          onClick={() => controller.setActiveTab("compare")}
          type="button"
        >
          与采用版本比较
        </button>
      </div>
      <div className="run-history-contract">
        <span>{snapshot?.mode === "demo" ? "演示数据" : "真实工程"}</span>
        <code>narracut.workflow-command/v1</code>
      </div>
    </aside>
  );
}

function formatDate(value: string): string {
  return new Date(value).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}
