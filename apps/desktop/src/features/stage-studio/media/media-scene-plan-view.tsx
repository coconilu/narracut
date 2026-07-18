import { useId, useState, type FormEvent } from "react";
import type {
  ScenePlanDocument,
  ScenePlanEdit,
  ScenePlanScene,
} from "@narracut/contracts";
import {
  formatDuration,
  narrowMediaDocument,
  validateSceneEdit,
} from "./media-stage-model.js";
import { MediaInputSelector } from "./media-input-selector";
import { MediaJobPanel } from "./media-job-panel";
import type {
  MediaInputOption,
  MediaInputOptionGroup,
  MediaStageStudioController,
} from "./use-media-stage";

export interface MediaScenePlanViewProps {
  readonly controller: MediaStageStudioController;
}

function groupForStage(
  groups: readonly MediaInputOptionGroup[],
  stageId: string,
): MediaInputOptionGroup | undefined {
  return groups.find((group) => group.requirement.stageId === stageId);
}

function selectedOption(
  group: MediaInputOptionGroup | undefined,
  artifactId: string | undefined,
): MediaInputOption | undefined {
  if (!group) return undefined;
  return (
    group.options.find((option) => option.artifactId === artifactId) ??
    (group.options.length === 1 ? group.options[0] : undefined)
  );
}

type SceneEditType = ScenePlanEdit["editType"];

interface SceneEditDraft {
  readonly editType: SceneEditType;
  readonly sceneId: string;
  readonly splitAtMs: string;
  readonly leftSceneId: string;
  readonly title: string;
  readonly narrativeRole: string;
  readonly boundaryMs: string;
}

function rightSceneAfter(
  scenes: readonly ScenePlanScene[],
  leftSceneId: string,
): ScenePlanScene | undefined {
  const index = scenes.findIndex((scene) => scene.sceneId === leftSceneId);
  return index >= 0 ? scenes[index + 1] : undefined;
}

function editFromDraft(
  scenes: readonly ScenePlanScene[],
  draft: SceneEditDraft,
): ScenePlanEdit | undefined {
  if (draft.editType === "split") {
    if (!draft.sceneId || !draft.splitAtMs.trim()) return undefined;
    const splitAtMs = Number(draft.splitAtMs);
    return Number.isFinite(splitAtMs)
      ? { editType: "split", sceneId: draft.sceneId, splitAtMs }
      : undefined;
  }
  if (draft.editType === "merge") {
    const right = rightSceneAfter(scenes, draft.leftSceneId);
    return right
      ? {
          editType: "merge",
          firstSceneId: draft.leftSceneId,
          secondSceneId: right.sceneId,
        }
      : undefined;
  }
  if (draft.editType === "update") {
    const title = draft.title.trim();
    const narrativeRole = draft.narrativeRole.trim();
    return draft.sceneId && (title || narrativeRole)
      ? {
          editType: "update",
          sceneId: draft.sceneId,
          ...(title ? { title } : {}),
          ...(narrativeRole ? { narrativeRole } : {}),
        }
      : undefined;
  }
  const right = rightSceneAfter(scenes, draft.leftSceneId);
  if (!right || !draft.boundaryMs.trim()) return undefined;
  const boundaryMs = Number(draft.boundaryMs);
  return Number.isFinite(boundaryMs)
    ? {
        editType: "move_boundary",
        leftSceneId: draft.leftSceneId,
        rightSceneId: right.sceneId,
        boundaryMs,
      }
    : undefined;
}

function ScenePlanGenerationPanel({
  controller,
}: MediaScenePlanViewProps) {
  const formId = useId();
  const [researchArtifactId, setResearchArtifactId] = useState<string>();
  const [scriptArtifactId, setScriptArtifactId] = useState<string>();
  const [captionsArtifactId, setCaptionsArtifactId] = useState<string>();
  const [error, setError] = useState<string | null>(null);
  const researchGroup = groupForStage(controller.inputOptions, "research");
  const scriptGroup = groupForStage(controller.inputOptions, "script");
  const captionsGroup = groupForStage(controller.inputOptions, "captions");
  const disabled = !controller.available || controller.busyLabel !== null;

  async function submitGeneration(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled) return;
    const research = selectedOption(researchGroup, researchArtifactId);
    const script = selectedOption(scriptGroup, scriptArtifactId);
    const captions = selectedOption(captionsGroup, captionsArtifactId);
    if (!research || !script || !captions) {
      setError("生成场景规划前必须显式确定 research、script 与 captions 输入。");
      return;
    }
    setError(null);
    await controller.generateScenePlan({
      researchInput: research.reference,
      scriptInput: script.reference,
      captionsInput: captions.reference,
    });
  }

  return (
    <section className="media-scene-generation" aria-labelledby={`${formId}-title`}>
      <h2 id={`${formId}-title`}>生成场景规划</h2>
      <p>只消费已批准且追溯完整的 claim_set、script 与 captions。</p>
      {!controller.available ? (
        <p className="media-readonly-notice" role="status">
          {controller.unavailableReason ?? "当前场景规划命令不可用。"}
        </p>
      ) : null}
      <form onSubmit={(event) => void submitGeneration(event)}>
        <MediaInputSelector
          disabled={disabled}
          group={researchGroup}
          id={`${formId}-research`}
          onChange={(option) => setResearchArtifactId(option?.artifactId)}
          value={researchArtifactId}
        />
        <MediaInputSelector
          disabled={disabled}
          group={scriptGroup}
          id={`${formId}-script`}
          onChange={(option) => setScriptArtifactId(option?.artifactId)}
          value={scriptArtifactId}
        />
        <MediaInputSelector
          disabled={disabled}
          group={captionsGroup}
          id={`${formId}-captions`}
          onChange={(option) => setCaptionsArtifactId(option?.artifactId)}
          value={captionsArtifactId}
        />
        {error ? (
          <p className="media-input-error" role="alert">
            {error}
          </p>
        ) : null}
        <button className="button primary" disabled={disabled} type="submit">
          {controller.busyLabel ?? "生成新场景规划"}
        </button>
      </form>
    </section>
  );
}

function ScenePlanDocumentView({
  document,
}: {
  readonly document: ScenePlanDocument;
}) {
  const scenes = [...document.scenes].sort(
    (left, right) => left.order - right.order,
  );
  const blockingDiagnostics = document.diagnostics.filter(
    (diagnostic) => diagnostic.blocking,
  ).length;

  return (
    <section className="media-scene-document" aria-labelledby="scene-document-title">
      <div className="media-document-heading">
        <div>
          <h2 id="scene-document-title">正式场景规划</h2>
          <p>
            {document.scenePlanId} · run {document.runId}
          </p>
        </div>
        <strong>{scenes.length} 个场景</strong>
      </div>

      <dl className="media-document-summary">
        <div>
          <dt>诊断</dt>
          <dd>
            {document.diagnostics.length} 项
            {blockingDiagnostics ? `，其中 ${blockingDiagnostics} 项阻断` : ""}
          </dd>
        </div>
        <div>
          <dt>变更摘要</dt>
          <dd>{document.changeSummary.summary || "未提供摘要"}</dd>
        </div>
        <div>
          <dt>变更场景</dt>
          <dd>
            {document.changeSummary.changedSceneIds.length
              ? document.changeSummary.changedSceneIds.join("、")
              : "初始生成，无局部变更"}
          </dd>
        </div>
        <div>
          <dt>替代产物</dt>
          <dd>{document.supersedesArtifactId ?? "无，这是首个正式版本"}</dd>
        </div>
      </dl>

      <section className="media-diagnostics" aria-labelledby="scene-diagnostics-title">
        <h3 id="scene-diagnostics-title">
          诊断记录（{document.diagnostics.length}）
        </h3>
        {document.diagnostics.length ? (
          <ul>
            {document.diagnostics.map((diagnostic, index) => (
              <li
                className={diagnostic.blocking ? "blocking" : diagnostic.severity}
                key={`${diagnostic.code}-${diagnostic.sceneId ?? diagnostic.cueId ?? index}`}
                role={diagnostic.blocking ? "alert" : undefined}
              >
                <strong>
                  {diagnostic.blocking ? "阻断" : diagnostic.severity} · {diagnostic.code}
                </strong>
                <span>{diagnostic.message}</span>
                {diagnostic.sceneId ? <code>scene {diagnostic.sceneId}</code> : null}
                {diagnostic.cueId ? <code>cue {diagnostic.cueId}</code> : null}
              </li>
            ))}
          </ul>
        ) : (
          <p role="status">当前文档没有诊断。</p>
        )}
      </section>

      <div className="media-scene-table-wrap">
        <table className="media-scene-table">
          <caption>按叙事顺序排列的场景、时间与追溯引用</caption>
          <thead>
            <tr>
              <th scope="col">顺序 / 场景</th>
              <th scope="col">时间</th>
              <th scope="col">标题 / 角色</th>
              <th scope="col">字幕线索</th>
              <th scope="col">事实追溯</th>
            </tr>
          </thead>
          <tbody>
            {scenes.map((scene) => (
              <tr key={scene.sceneId}>
                <td>
                  <strong>#{scene.order}</strong>
                  <code>{scene.sceneId}</code>
                </td>
                <td>
                  <span>
                    {formatDuration(scene.suggestedStartMs)} –{" "}
                    {formatDuration(scene.suggestedEndMs)}
                  </span>
                  <small>
                    {scene.suggestedStartMs}–{scene.suggestedEndMs} ms
                  </small>
                </td>
                <td>
                  <strong>{scene.title}</strong>
                  <span>{scene.narrativeRole}</span>
                </td>
                <td>{scene.cueIds.join("、")}</td>
                <td>
                  <span>claims：{scene.claimIds.join("、") || "无"}</span>
                  <span>evidence：{scene.evidenceRefs.join("、") || "无"}</span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function ScenePlanEditor({
  controller,
  document,
  baseArtifactId,
}: {
  readonly controller: MediaStageStudioController;
  readonly document: ScenePlanDocument;
  readonly baseArtifactId: string;
}) {
  const formId = useId();
  const scenes = [...document.scenes].sort(
    (left, right) => left.order - right.order,
  );
  const initialSceneId = scenes[0]?.sceneId ?? "";
  const [editType, setEditType] = useState<SceneEditType>("split");
  const [sceneId, setSceneId] = useState(initialSceneId);
  const [leftSceneId, setLeftSceneId] = useState(initialSceneId);
  const [splitAtMs, setSplitAtMs] = useState("");
  const [boundaryMs, setBoundaryMs] = useState("");
  const [title, setTitle] = useState("");
  const [narrativeRole, setNarrativeRole] = useState("");
  const [changeSummary, setChangeSummary] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const selectedScene = scenes.find((scene) => scene.sceneId === sceneId);
  const leftScene = scenes.find((scene) => scene.sceneId === leftSceneId);
  const rightScene = rightSceneAfter(scenes, leftSceneId);
  const disabled =
    !controller.available || controller.busyLabel !== null || submitting;

  function resetDraft() {
    setEditType("split");
    setSceneId(initialSceneId);
    setLeftSceneId(initialSceneId);
    setSplitAtMs("");
    setBoundaryMs("");
    setTitle("");
    setNarrativeRole("");
    setChangeSummary("");
    setError(null);
  }

  async function submitEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled) return;
    const summary = changeSummary.trim();
    if (!summary) {
      setError("保存新版本前必须填写变更摘要。");
      return;
    }
    const edit = editFromDraft(scenes, {
      editType,
      sceneId,
      splitAtMs,
      leftSceneId,
      title,
      narrativeRole,
      boundaryMs,
    });
    if (!edit) {
      setError("请完整填写当前编辑所需的场景与时间字段。");
      return;
    }
    const validated = validateSceneEdit(document, edit);
    if (!validated.valid) {
      setError(validated.errors.join("；"));
      return;
    }

    setSubmitting(true);
    setError(null);
    setStatus(null);
    try {
      const saved = await controller.saveScenePlan([validated.value], summary);
      if (saved) {
        resetDraft();
        setStatus("新版本已保存；编辑草稿已清空，正在使用返回的新产物。");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <section className="media-scene-editor" aria-labelledby={`${formId}-title`}>
      <h2 id={`${formId}-title`}>编辑场景规划</h2>
      <p className="media-draft-binding" role="status">
        草稿仅绑定到产物 <code>{baseArtifactId}</code>。切换正式产物时，本表单会重建并清空旧编辑，绝不迁移到新 base。
      </p>

      <form onSubmit={(event) => void submitEdit(event)}>
        <fieldset disabled={disabled}>
          <legend>单次编辑类型</legend>
          <label htmlFor={`${formId}-edit-type`}>
            <span>操作</span>
            <select
              id={`${formId}-edit-type`}
              onChange={(event) =>
                setEditType(event.target.value as SceneEditType)
              }
              value={editType}
            >
              <option value="split">拆分场景</option>
              <option value="merge">合并相邻场景</option>
              <option value="update">更新标题或叙事角色</option>
              <option value="move_boundary">移动相邻场景边界</option>
            </select>
          </label>
        </fieldset>

        {editType === "split" ? (
          <fieldset disabled={disabled}>
            <legend>拆分一个场景</legend>
            <label htmlFor={`${formId}-split-scene`}>
              <span>场景</span>
              <select
                id={`${formId}-split-scene`}
                onChange={(event) => setSceneId(event.target.value)}
                value={sceneId}
              >
                {scenes.map((scene) => (
                  <option key={scene.sceneId} value={scene.sceneId}>
                    #{scene.order} · {scene.sceneId} · {scene.title}
                  </option>
                ))}
              </select>
            </label>
            <label htmlFor={`${formId}-split-ms`}>
              <span>拆分位置（毫秒，必须严格位于场景内部）</span>
              <input
                id={`${formId}-split-ms`}
                max={
                  selectedScene
                    ? selectedScene.suggestedEndMs - 1
                    : undefined
                }
                min={
                  selectedScene
                    ? selectedScene.suggestedStartMs + 1
                    : undefined
                }
                onChange={(event) => setSplitAtMs(event.target.value)}
                required
                step={1}
                type="number"
                value={splitAtMs}
              />
            </label>
          </fieldset>
        ) : null}

        {editType === "merge" ? (
          <fieldset disabled={disabled}>
            <legend>合并相邻场景</legend>
            <label htmlFor={`${formId}-merge-left`}>
              <span>左侧场景</span>
              <select
                id={`${formId}-merge-left`}
                onChange={(event) => setLeftSceneId(event.target.value)}
                value={leftSceneId}
              >
                {scenes.slice(0, -1).map((scene) => (
                  <option key={scene.sceneId} value={scene.sceneId}>
                    #{scene.order} · {scene.sceneId} · {scene.title}
                  </option>
                ))}
              </select>
            </label>
            <p>
              右侧相邻场景：<strong>{rightScene?.sceneId ?? "没有可合并的右侧场景"}</strong>
            </p>
          </fieldset>
        ) : null}

        {editType === "update" ? (
          <fieldset disabled={disabled}>
            <legend>更新场景文本</legend>
            <label htmlFor={`${formId}-update-scene`}>
              <span>场景</span>
              <select
                id={`${formId}-update-scene`}
                onChange={(event) => setSceneId(event.target.value)}
                value={sceneId}
              >
                {scenes.map((scene) => (
                  <option key={scene.sceneId} value={scene.sceneId}>
                    #{scene.order} · {scene.sceneId}
                  </option>
                ))}
              </select>
            </label>
            <label htmlFor={`${formId}-title-input`}>
              <span>新标题（与叙事角色至少填写一项）</span>
              <input
                id={`${formId}-title-input`}
                onChange={(event) => setTitle(event.target.value)}
                placeholder={selectedScene?.title}
                type="text"
                value={title}
              />
            </label>
            <label htmlFor={`${formId}-role-input`}>
              <span>新叙事角色</span>
              <input
                id={`${formId}-role-input`}
                onChange={(event) => setNarrativeRole(event.target.value)}
                placeholder={selectedScene?.narrativeRole}
                type="text"
                value={narrativeRole}
              />
            </label>
          </fieldset>
        ) : null}

        {editType === "move_boundary" ? (
          <fieldset disabled={disabled}>
            <legend>移动相邻场景边界</legend>
            <label htmlFor={`${formId}-boundary-left`}>
              <span>左侧场景</span>
              <select
                id={`${formId}-boundary-left`}
                onChange={(event) => setLeftSceneId(event.target.value)}
                value={leftSceneId}
              >
                {scenes.slice(0, -1).map((scene) => (
                  <option key={scene.sceneId} value={scene.sceneId}>
                    #{scene.order} · {scene.sceneId}
                  </option>
                ))}
              </select>
            </label>
            <p>
              右侧相邻场景：<strong>{rightScene?.sceneId ?? "无"}</strong>
            </p>
            <label htmlFor={`${formId}-boundary-ms`}>
              <span>新边界（毫秒）</span>
              <input
                id={`${formId}-boundary-ms`}
                max={rightScene ? rightScene.suggestedEndMs - 1 : undefined}
                min={leftScene ? leftScene.suggestedStartMs + 1 : undefined}
                onChange={(event) => setBoundaryMs(event.target.value)}
                required
                step={1}
                type="number"
                value={boundaryMs}
              />
            </label>
          </fieldset>
        ) : null}

        <fieldset disabled={disabled}>
          <legend>新版本说明</legend>
          <label htmlFor={`${formId}-summary`}>
            <span>变更摘要</span>
            <textarea
              id={`${formId}-summary`}
              onChange={(event) => setChangeSummary(event.target.value)}
              required
              value={changeSummary}
            />
          </label>
        </fieldset>

        {error ? (
          <p className="media-input-error" role="alert">
            {error}
          </p>
        ) : null}
        {status ? <p role="status">{status}</p> : null}
        <button className="button primary" disabled={disabled} type="submit">
          {controller.busyLabel ?? (submitting ? "正在保存…" : "保存为新版本")}
        </button>
      </form>
    </section>
  );
}

function ScenePlanSaveResult({
  controller,
}: MediaScenePlanViewProps) {
  const result = controller.lastSaveResult;
  if (!result) return null;
  return (
    <section className="media-save-result" aria-label="最近场景保存结果" role="status">
      <h2>最近保存结果</h2>
      <p>
        新产物 <code>{result.artifactId}</code> · run <code>{result.runId}</code>
      </p>
      <p>
        变更场景：
        {result.changedSceneIds.length
          ? result.changedSceneIds.join("、")
          : "服务未报告具体场景"}
      </p>
      <p>
        下游过期：
        {result.staleBecauseStageIds.length
          ? result.staleBecauseStageIds.join("、")
          : "无"}
      </p>
    </section>
  );
}

export function MediaScenePlanView({
  controller,
}: MediaScenePlanViewProps) {
  const document = narrowMediaDocument(controller.document, "scene_plan");

  return (
    <div className="media-scene-plan-view">
      <ScenePlanGenerationPanel controller={controller} />
      {document ? (
        <>
          <ScenePlanDocumentView document={document} />
          <ScenePlanEditor
            baseArtifactId={
              controller.documentArtifactId ?? document.scenePlanId
            }
            controller={controller}
            document={document}
            key={controller.documentArtifactId ?? document.scenePlanId}
          />
        </>
      ) : (
        <section className="media-document-empty" aria-label="场景规划说明">
          <h2>尚无正式场景规划</h2>
          <p>请选择三个已审核输入并生成；这里不会构造演示文档或覆盖历史。</p>
        </section>
      )}
      <ScenePlanSaveResult controller={controller} />
      <MediaJobPanel controller={controller} />
    </div>
  );
}
