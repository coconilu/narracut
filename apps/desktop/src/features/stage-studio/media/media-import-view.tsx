import { useId, useState, type FormEvent } from "react";
import { validateImportForm } from "./media-stage-model.js";
import type {
  MediaInputOption,
  MediaInputOptionGroup,
} from "./use-media-stage";
import type { MediaStageStudioController } from "./use-media-stage";
import { MediaInputSelector } from "./media-input-selector";
import { MediaJobPanel } from "./media-job-panel";

export interface MediaImportViewProps {
  readonly controller: MediaStageStudioController;
  readonly stageId: "audio" | "captions";
}

type Ownership = "self_recorded" | "licensed";

function groupForStage(
  groups: readonly MediaInputOptionGroup[],
  stageId: string,
): MediaInputOptionGroup | undefined {
  return groups.find((group) => group.requirement.stageId === stageId);
}

function localFileName(path: string): string {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? "尚未选择文件";
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

export function MediaImportView({
  controller,
  stageId,
}: MediaImportViewProps) {
  const formId = useId();
  const [sourcePath, setSourcePath] = useState("");
  const [ownership, setOwnership] = useState<Ownership>("self_recorded");
  const [author, setAuthor] = useState("");
  const [rightsStatement, setRightsStatement] = useState("");
  const [licenseId, setLicenseId] = useState("");
  const [attributionText, setAttributionText] = useState("");
  const [scriptArtifactId, setScriptArtifactId] = useState<string>();
  const [audioArtifactId, setAudioArtifactId] = useState<string>();
  const [formError, setFormError] = useState<string | null>(null);
  const [pickingFile, setPickingFile] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const scriptGroup = groupForStage(controller.inputOptions, "script");
  const audioGroup = groupForStage(controller.inputOptions, "audio");
  const busy = controller.busyLabel !== null;
  const upgrading = controller.rightsUpgradeRequired;
  const disabled = busy || pickingFile || submitting || !controller.available;
  const title = upgrading
    ? "重新授权为 1.2 媒体版本"
    : stageId === "audio"
      ? "导入口播音频"
      : "导入时间对齐字幕";
  const extension = stageId === "audio" ? "WAV" : "SRT";

  function changeOwnership(nextOwnership: Ownership) {
    setOwnership(nextOwnership);
    if (nextOwnership === "self_recorded") {
      setLicenseId("");
      setAttributionText("");
    }
  }

  async function chooseSourceFile() {
    if (!controller.available || pickingFile) return;
    setPickingFile(true);
    setFormError(null);
    try {
      const picker = await import("./media-file-picker");
      const selectedPath =
        stageId === "audio"
          ? await picker.pickAudioFile()
          : await picker.pickCaptionsFile();
      if (selectedPath) setSourcePath(selectedPath);
    } catch {
      setFormError("无法打开系统文件选择器，请确认当前运行在 NarraCut 桌面端。");
    } finally {
      setPickingFile(false);
    }
  }

  async function submitImport(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled || !controller.available) return;
    setSubmitting(true);
    setFormError(null);
    try {
      const validated = validateImportForm({
        sourcePath: upgrading ? "legacy-media-reauthorization" : sourcePath,
        ownership,
        author,
        rightsStatement,
        licenseId,
        attributionText,
      });
      if (!validated.valid) {
        setFormError(validated.errors.join("；"));
        return;
      }

      if (upgrading) {
        const accepted = await controller.reauthorizeMedia(validated.value.rights);
        if (accepted) setFormError(null);
        return;
      }

      const scriptOption = selectedOption(scriptGroup, scriptArtifactId);
      if (!scriptOption) {
        setFormError("请选择一个完整且已批准的脚本产物。");
        return;
      }

      let accepted = false;
      if (stageId === "audio") {
        accepted = await controller.enqueueAudio(
          validated.value,
          scriptOption.reference,
        );
      } else {
        const audioOption = selectedOption(audioGroup, audioArtifactId);
        if (!audioOption) {
          setFormError("请选择一个完整且已批准的音频产物。");
          return;
        }
        const audioDocument = await controller.readInputDocument(
          audioOption.reference,
          "audio_media",
        );
        if (!audioDocument || audioDocument.durationMs <= 0) {
          setFormError("无法从所选已批准音频读取有效时长。");
          return;
        }
        accepted = await controller.enqueueCaptions(
          validated.value,
          scriptOption.reference,
          audioOption.reference,
          audioDocument.durationMs,
        );
      }

      if (accepted) setSourcePath("");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="media-import-view">
      <header className="media-import-heading">
        <div>
          <h2>{title}</h2>
          <p>
            {upgrading
              ? "旧媒体保持只读；提交后创建新的不可变运行，并在审核采用后使下游过期。"
              : "源文件只用于有界媒体命令；界面不会显示本机完整路径。"}
          </p>
        </div>
      </header>

      {!controller.available ? (
        <p className="media-readonly-notice" role="status">
          {controller.unavailableReason ?? "当前媒体导入不可用。"}
        </p>
      ) : null}

      <form
        className="media-import-form"
        onSubmit={(event) => void submitImport(event)}
      >
        {!upgrading ? (
          <>
            <fieldset disabled={disabled}>
              <legend>{extension} 源文件</legend>
              <div className="media-file-row">
                <div>
                  <strong>{sourcePath ? localFileName(sourcePath) : "尚未选择文件"}</strong>
                  <span>仅允许单个 .{extension.toLowerCase()} 文件</span>
                </div>
                <button
                  className="button"
                  disabled={disabled}
                  onClick={() => void chooseSourceFile()}
                  type="button"
                >
                  {pickingFile ? "正在打开…" : "选择文件"}
                </button>
              </div>
            </fieldset>

            <MediaInputSelector
              disabled={disabled}
              group={scriptGroup}
              id={`${formId}-script-input`}
              onChange={(option) => setScriptArtifactId(option?.artifactId)}
              value={scriptArtifactId}
            />

            {stageId === "captions" ? (
              <MediaInputSelector
                disabled={disabled}
                group={audioGroup}
                id={`${formId}-audio-input`}
                onChange={(option) => setAudioArtifactId(option?.artifactId)}
                value={audioArtifactId}
              />
            ) : null}
          </>
        ) : (
          <p className="media-readonly-notice" role="status">
            当前采用产物使用旧版权利模型，Export 已 fail-closed。请补录真实授权后创建新运行；旧产物不会被覆盖。
          </p>
        )}

        <fieldset disabled={disabled}>
          <legend>权利与来源声明</legend>
          <div className="media-ownership-options">
            <label>
              <input
                checked={ownership === "self_recorded"}
                name={`${formId}-ownership`}
                onChange={() => changeOwnership("self_recorded")}
                type="radio"
              />
              自行录制或制作
            </label>
            <label>
              <input
                checked={ownership === "licensed"}
                name={`${formId}-ownership`}
                onChange={() => changeOwnership("licensed")}
                type="radio"
              />
              已取得许可
            </label>
          </div>

          <label htmlFor={`${formId}-author`}>
            <span>作者或录制者</span>
            <input
              id={`${formId}-author`}
              onChange={(event) => setAuthor(event.target.value)}
              required
              type="text"
              value={author}
            />
          </label>
          <label htmlFor={`${formId}-rights-statement`}>
            <span>权利声明</span>
            <textarea
              id={`${formId}-rights-statement`}
              onChange={(event) => setRightsStatement(event.target.value)}
              required
              value={rightsStatement}
            />
          </label>

          {ownership === "licensed" ? (
            <div className="media-license-fields">
              <label htmlFor={`${formId}-license-id`}>
                <span>许可编号</span>
                <input
                  id={`${formId}-license-id`}
                  onChange={(event) => setLicenseId(event.target.value)}
                  required
                  type="text"
                  value={licenseId}
                />
              </label>
              <label htmlFor={`${formId}-attribution`}>
                <span>署名文本</span>
                <input
                  id={`${formId}-attribution`}
                  onChange={(event) => setAttributionText(event.target.value)}
                  required
                  type="text"
                  value={attributionText}
                />
              </label>
            </div>
          ) : null}
        </fieldset>

        {formError ? (
          <p className="media-input-error" role="alert">
            {formError}
          </p>
        ) : null}

        <button className="button primary" disabled={disabled} type="submit">
          {busy
            ? controller.busyLabel
            : submitting
              ? "正在校验并提交…"
              : title}
        </button>
      </form>

      <MediaJobPanel controller={controller} />
    </div>
  );
}
