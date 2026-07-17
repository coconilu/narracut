import type { WorkflowSnapshotView } from "../../lib/workflow-commands";
import { stageStatusLabel, type StageView } from "../../model/workbench";

interface InspectorPanelProps {
  readonly stage: StageView;
  readonly workflow: WorkflowSnapshotView;
  readonly showDemoContent: boolean;
  readonly onRunIntent: () => void;
}

export function InspectorPanel({ stage, workflow, showDemoContent, onRunIntent }: InspectorPanelProps) {
  const config = workflow.configs.find((item) => item.stageId === stage.definition.stageId);
  const definitions = new Map(workflow.stageDefinitions.map((item) => [item.stageId, item]));
  const states = new Map(workflow.stageStates.map((item) => [item.stageId, item]));
  const values = Object.entries(config?.values ?? {}).slice(0, 6);
  const executor = showDemoContent
    ? { provider: "local-codex", mode: "Codex CLI", model: "gpt-5-codex" }
    : { provider: "随运行快照读取", mode: "统一 Provider 接口", model: "未固定" };

  return (
    <aside className="inspector" aria-label="配置检查器">
      <div className="inspector-header">
        <strong>配置检查器</strong>
        <span>只读快照</span>
      </div>
      <div className="inspector-body">
        <section className="inspector-section">
          <div className="section-title"><span>执行器</span><span>{stageStatusLabel(stage.state.status)}</span></div>
          <div className="property"><span>Provider</span><strong>{executor.provider}</strong></div>
          <div className="property"><span>模式</span><strong>{executor.mode}</strong></div>
          <div className="property"><span>模型</span><strong>{executor.model}</strong></div>
        </section>

        <section className="inspector-section">
          <div className="section-title"><span>输入与依赖</span><span>{stage.definition.dependencies.length} 项</span></div>
          {stage.definition.dependencies.length > 0 ? stage.definition.dependencies.map((dependencyId) => {
            const dependency = definitions.get(dependencyId);
            const dependencyState = states.get(dependencyId);
            return (
              <div className="dependency" key={dependencyId}>
                <span className={`state-dot ${dependencyState?.status === "approved" ? "approved" : ""}`} />
                {dependency?.title ?? dependencyId} · {dependencyState ? stageStatusLabel(dependencyState.status) : "未知"}
              </div>
            );
          }) : <div className="dependency muted">直接使用已授权的项目资料</div>}
        </section>

        <section className="inspector-section">
          <div className="section-title"><span>配置快照</span><span>rev {config?.revision ?? "—"}</span></div>
          <div className="snapshot">
            {values.length > 0
              ? values.map(([key, value]) => <span key={key}>{key}: {formatConfigValue(value)}</span>)
              : <span>当前阶段尚未写入配置值</span>}
          </div>
        </section>

        <button className="button inspector-action" onClick={onRunIntent} type="button">
          {stage.state.latestRunId ? "重新运行" : "准备运行"}
        </button>
      </div>
    </aside>
  );
}

function formatConfigValue(value: unknown): string {
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return JSON.stringify(value);
}
