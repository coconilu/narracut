import type { StageStudioController } from "../use-stage-studio";
import { StudioHeading, formatDate } from "../stage-studio-primitives";

export function ConfigView({
  controller,
  disabled,
}: {
  readonly controller: StageStudioController;
  readonly disabled: boolean;
}) {
  const config = controller.snapshot?.config;
  if (!config) return null;
  return (
    <div className="studio-scroll config-view">
      <StudioHeading
        eyebrow={`配置快照 · rev ${config.revision}`}
        title="编辑阶段配置"
        text="保存会生成新修订并记录决策原因；历史运行仍引用各自的旧快照。"
      />
      <div className="config-editor-grid">
        <label className="studio-field code-field">
          <span>JSON 配置对象</span>
          <textarea
            aria-label="JSON 配置对象"
            disabled={disabled}
            onChange={(event) => controller.setConfigDraft(event.target.value)}
            spellCheck={false}
            value={controller.configDraft}
          />
        </label>
        <div className="config-sidecar">
          <label className="studio-field">
            <span>变更理由（写入 DecisionRecord）</span>
            <textarea
              aria-label="配置变更理由"
              disabled={disabled}
              onChange={(event) => controller.setConfigRationale(event.target.value)}
              placeholder="例如：将目标时长调整到 3 分钟，并要求全部事实陈述带引用。"
              value={controller.configRationale}
            />
          </label>
          <button
            className="button primary studio-primary-action"
            disabled={disabled}
            onClick={() => void controller.saveConfig()}
            type="button"
          >
            保存为 rev {config.revision + 1}
          </button>
          <div className="decision-history">
            <strong>最近决策记录</strong>
            {config.decisions.length > 0 ? (
              [...config.decisions].reverse().slice(0, 4).map((decision) => (
                <div key={decision.decisionId}>
                  <code>{decision.key}</code>
                  <span>{decision.rationale ?? "未填写理由"}</span>
                  <time>{formatDate(decision.madeAt)}</time>
                </div>
              ))
            ) : (
              <p>当前配置还没有人工决策记录。</p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
