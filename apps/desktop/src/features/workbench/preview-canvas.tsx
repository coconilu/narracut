import { useState } from "react";
import { stageStatusLabel, type StageView } from "../../model/workbench";

interface PreviewCanvasProps {
  readonly stage: StageView;
  readonly showDemoContent: boolean;
}

type CanvasView = "preview" | "structure";

export function PreviewCanvas({ stage, showDemoContent }: PreviewCanvasProps) {
  const [view, setView] = useState<CanvasView>("preview");
  const runIdParts = stage.state.latestRunId?.split("_") ?? [];
  const version = runIdParts[runIdParts.length - 1]?.replace(/^0+/, "") || "—";

  return (
    <section className="canvas-region" aria-label="阶段预览">
      <div className="canvas-toolbar">
        <div className="view-tabs" role="tablist" aria-label="预览模式">
          <button
            aria-selected={view === "preview"}
            className={`view-tab ${view === "preview" ? "active" : ""}`}
            onClick={() => setView("preview")}
            role="tab"
            type="button"
          >
            预览
          </button>
          <button
            aria-selected={view === "structure"}
            className={`view-tab ${view === "structure" ? "active" : ""}`}
            data-testid="canvas-structure-tab"
            onClick={() => setView("structure")}
            role="tab"
            type="button"
          >
            结构
          </button>
        </div>
        <span className="canvas-meta">
          {stage.definition.title}版本 {version} · {stage.definition.stageId === "script" ? "预计 03:18" : stageStatusLabel(stage.state.status)}
        </span>
      </div>

      <div className="document-stage">
        {view === "structure" ? (
          <StructureDocument stage={stage} />
        ) : stage.definition.stageId === "script" && showDemoContent ? (
          <ScriptDocument />
        ) : (
          <StageDocument stage={stage} />
        )}
      </div>
    </section>
  );
}

function ScriptDocument() {
  return (
    <article className="document script-document" data-testid="script-document">
      <div className="document-kicker">事实脚本 · 当前候选版本</div>
      <h1>为什么月球城市比想象中更难</h1>
      <p className="document-lead">受众：大众科普 · 目标时长：3 分钟 · 语气：克制、清晰、有证据</p>
      <p className="script-block">
        我们习惯把月球基地想象成一座等待施工的城市。但真正困难的，并不是把房子运上去，而是让一整套生命支持系统在极端环境里持续工作。
        <span className="evidence-ref">[C01 · E03]</span>
      </p>
      <p className="script-block selected">
        月壤细小、尖锐，而且带有静电。它会进入密封结构、磨损设备，也可能影响人的呼吸系统。一次看似普通的出舱，背后都需要复杂的隔离流程。
        <span className="evidence-ref">[C04 · E08]</span>
      </p>
      <p className="script-block">
        更关键的是能源。月球昼夜各持续约十四个地球日。只依赖太阳能，就必须为漫长月夜准备大规模储能，或者选择照明条件更稳定的极区。
      </p>
      <p className="script-block">
        所以，月球城市不是“把地球建筑搬过去”。它更像一艘不能返航的巨型飞船：每一个系统都必须可维修、可冗余，也必须知道自己依赖什么。
      </p>
      <div className="script-note">
        引用括号来自已审核的 Claims / Evidence；生成式视觉素材不能作为事实证据。
      </div>
    </article>
  );
}

function StageDocument({ stage }: { readonly stage: StageView }) {
  return (
    <article className="document stage-document" data-testid="stage-document">
      <div className="document-kicker">{stage.definition.title} · 阶段工作区</div>
      <h1>{stage.definition.title}</h1>
      <p className="stage-description">{stage.definition.description}</p>
      <div className="stage-document-rule" />
      <dl className="stage-document-facts">
        <div><dt>当前状态</dt><dd>{stageStatusLabel(stage.state.status)}</dd></div>
        <div><dt>最近运行</dt><dd>{stage.state.latestRunId ?? "尚未运行"}</dd></div>
        <div><dt>输入约束</dt><dd>{stage.definition.requiresApprovedInputs ? "仅消费已审核输入" : "允许项目资料输入"}</dd></div>
        <div><dt>局部重生成</dt><dd>{stage.definition.supportsPartialRegeneration ? "支持，执行前展示影响范围" : "不支持"}</dd></div>
      </dl>
    </article>
  );
}

function StructureDocument({ stage }: { readonly stage: StageView }) {
  return (
    <article className="document structure-document" data-testid="structure-document">
      <div className="document-kicker">结构视图 · {stage.definition.title}</div>
      <h1>输入、阶段与产物</h1>
      <div className="structure-flow">
        <div><span>上游依赖</span><strong>{stage.definition.dependencies.join(" + ") || "项目资料"}</strong></div>
        <i aria-hidden="true" />
        <div className="active"><span>当前阶段</span><strong>{stage.definition.stageId}</strong></div>
        <i aria-hidden="true" />
        <div><span>输出契约</span><strong>{stage.definition.outputKinds.join(" + ") || "待定义"}</strong></div>
      </div>
      <p className="structure-note">阶段输出会绑定输入引用、配置快照、产物清单和运行日志；重跑不会静默覆盖历史运行。</p>
    </article>
  );
}
