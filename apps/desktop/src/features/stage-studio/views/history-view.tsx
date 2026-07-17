import type { StageView } from "../../../model/workbench";
import type { StageStudioController } from "../use-stage-studio";
import {
  StudioHeading,
  formatDate,
  runStatusLabels,
} from "../stage-studio-primitives";

export function HistoryView({
  controller,
  disabled,
  stage,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
  readonly stage: StageView;
}) {
  const snapshot = controller.snapshot;
  if (!snapshot) return null;
  return (
    <div className="studio-scroll history-view">
      <StudioHeading
        eyebrow={`${snapshot.runs.length} 个不可变运行 · ${snapshot.reviews.length} 条审核`}
        title="历史与局部重生成"
        text="重生成会创建新 runId；执行前先展示受影响阶段，不会覆盖所选历史版本。"
      />
      <div className="history-layout">
        <div className="history-run-table">
          {snapshot.runs.length ? (
            snapshot.runs.map((run) => (
              <button
                className={run.runId === controller.selectedRunId ? "selected" : ""}
                disabled={disabled}
                key={run.runId}
                onClick={() => controller.setSelectedRunId(run.runId)}
                type="button"
              >
                <span>
                  <strong>{run.runId}</strong>
                  <small>{formatDate(run.completedAt ?? run.createdAt)}</small>
                </span>
                <span>{run.logSummary.message}</span>
                <em className={run.status}>{runStatusLabels[run.status]}</em>
              </button>
            ))
          ) : (
            <div className="studio-inline-empty">当前阶段还没有历史运行。</div>
          )}
        </div>
        <RegenerationCard
          controller={controller}
          disabled={disabled}
          stage={stage}
        />
      </div>
    </div>
  );
}

function RegenerationCard({
  controller,
  disabled,
  stage,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
  readonly stage: StageView;
}) {
  const snapshot = controller.snapshot;
  if (!snapshot) return null;
  return (
    <aside className="regeneration-card">
      <span>局部重生成</span>
      <strong>{controller.selectedRun?.runId ?? "请选择历史运行"}</strong>
      <p>
        复用所选运行的输入引用与执行器，冻结当前配置 rev {snapshot.config.revision}，并创建新的任务与运行预留。
      </p>
      {!stage.definition.supportsPartialRegeneration ? (
        <div className="studio-warning">
          阶段契约未声明 supportsPartialRegeneration。
        </div>
      ) : null}
      <button
        className="button"
        disabled={
          disabled ||
          !controller.selectedRun ||
          !stage.definition.supportsPartialRegeneration
        }
        onClick={() => void controller.previewRegeneration()}
        type="button"
      >
        预览影响范围
      </button>
      {controller.regenerationImpact ? (
        <div className="impact-list" data-testid="regeneration-impact">
          {controller.regenerationImpact.affectedStages.map((affected) => (
            <div key={affected.stageId}>
              <span>{affected.stageId}</span>
              <small>距离 {affected.distance} · {affected.currentStatus}</small>
            </div>
          ))}
          <button
            className="button primary"
            disabled={disabled}
            onClick={() => void controller.queueRegeneration()}
            type="button"
          >
            确认创建新任务
          </button>
        </div>
      ) : null}
    </aside>
  );
}
