import {
  useId,
  useState,
  type CSSProperties,
  type FormEvent,
} from "react";
import type {
  TimelineCanvasInput,
  TimelineDocument,
  TimelineEdit,
  TimelineSafeAreaInput,
} from "@narracut/contracts";
import {
  buildTimelineTrackLayout,
  formatDuration,
  narrowMediaDocument,
  validateTimelineEdit,
} from "./media-stage-model.js";
import { MediaInputSelector } from "./media-input-selector";
import { MediaJobPanel } from "./media-job-panel";
import type {
  MediaInputOption,
  MediaInputOptionGroup,
  MediaStageStudioController,
} from "./use-media-stage";

export interface MediaTimelineViewProps {
  readonly controller: MediaStageStudioController;
}

interface GenerationSettings {
  readonly canvas: TimelineCanvasInput;
  readonly safeArea: TimelineSafeAreaInput;
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

function parseInteger(value: string): number | undefined {
  const parsed = Number(value);
  return Number.isInteger(parsed) ? parsed : undefined;
}

function parseGenerationSettings(values: {
  readonly width: string;
  readonly height: string;
  readonly frameRateNumerator: string;
  readonly frameRateDenominator: string;
  readonly safeX: string;
  readonly safeY: string;
  readonly safeWidth: string;
  readonly safeHeight: string;
}): GenerationSettings | undefined {
  const width = parseInteger(values.width);
  const height = parseInteger(values.height);
  const frameRateNumerator = parseInteger(values.frameRateNumerator);
  const frameRateDenominator = parseInteger(values.frameRateDenominator);
  const x = parseInteger(values.safeX);
  const y = parseInteger(values.safeY);
  const safeWidth = parseInteger(values.safeWidth);
  const safeHeight = parseInteger(values.safeHeight);
  if (
    width === undefined ||
    height === undefined ||
    frameRateNumerator === undefined ||
    frameRateDenominator === undefined ||
    x === undefined ||
    y === undefined ||
    safeWidth === undefined ||
    safeHeight === undefined ||
    width <= 0 ||
    height <= 0 ||
    frameRateNumerator <= 0 ||
    frameRateDenominator <= 0 ||
    x < 0 ||
    y < 0 ||
    safeWidth <= 0 ||
    safeHeight <= 0 ||
    x + safeWidth > width ||
    y + safeHeight > height
  ) {
    return undefined;
  }
  return {
    canvas: { width, height, frameRateNumerator, frameRateDenominator },
    safeArea: { x, y, width: safeWidth, height: safeHeight },
  };
}

function NumericField({
  id,
  label,
  value,
  min,
  onChange,
}: {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly min: number;
  readonly onChange: (value: string) => void;
}) {
  return (
    <label htmlFor={id}>
      <span>{label}</span>
      <input
        id={id}
        min={min}
        onChange={(event) => onChange(event.target.value)}
        required
        step={1}
        type="number"
        value={value}
      />
    </label>
  );
}

function TimelineGenerationPanel({ controller }: MediaTimelineViewProps) {
  const formId = useId();
  const [audioArtifactId, setAudioArtifactId] = useState<string>();
  const [captionsArtifactId, setCaptionsArtifactId] = useState<string>();
  const [scenePlanArtifactId, setScenePlanArtifactId] = useState<string>();
  const [width, setWidth] = useState("1920");
  const [height, setHeight] = useState("1080");
  const [frameRateNumerator, setFrameRateNumerator] = useState("30");
  const [frameRateDenominator, setFrameRateDenominator] = useState("1");
  const [safeX, setSafeX] = useState("96");
  const [safeY, setSafeY] = useState("54");
  const [safeWidth, setSafeWidth] = useState("1728");
  const [safeHeight, setSafeHeight] = useState("972");
  const [error, setError] = useState<string | null>(null);
  const audioGroup = groupForStage(controller.inputOptions, "audio");
  const captionsGroup = groupForStage(controller.inputOptions, "captions");
  const scenePlanGroup = groupForStage(controller.inputOptions, "scene_plan");
  const disabled = !controller.available || controller.busyLabel !== null;

  async function submitGeneration(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled) return;
    const audio = selectedOption(audioGroup, audioArtifactId);
    const captions = selectedOption(captionsGroup, captionsArtifactId);
    const scenePlan = selectedOption(scenePlanGroup, scenePlanArtifactId);
    if (!audio || !captions || !scenePlan) {
      setError("生成时间轴前必须显式确定 audio、captions 与 scene_plan 输入。");
      return;
    }
    const settings = parseGenerationSettings({
      width,
      height,
      frameRateNumerator,
      frameRateDenominator,
      safeX,
      safeY,
      safeWidth,
      safeHeight,
    });
    if (!settings) {
      setError("画布、帧率必须为正整数；安全区必须为画布内的正尺寸整数矩形。");
      return;
    }
    setError(null);
    await controller.generateTimeline(
      {
        audioInput: audio.reference,
        captionsInput: captions.reference,
        scenePlanInput: scenePlan.reference,
      },
      settings.canvas,
      settings.safeArea,
    );
  }

  return (
    <section className="media-timeline-generation" aria-labelledby={`${formId}-title`}>
      <h2 id={`${formId}-title`}>生成最小时间轴</h2>
      <p>组合三个已批准媒体输入，并冻结画布、帧率与安全区参数。</p>
      {!controller.available ? (
        <p className="media-readonly-notice" role="status">
          {controller.unavailableReason ?? "当前时间轴命令不可用。"}
        </p>
      ) : null}
      <form onSubmit={(event) => void submitGeneration(event)}>
        <MediaInputSelector
          disabled={disabled}
          group={audioGroup}
          id={`${formId}-audio`}
          onChange={(option) => setAudioArtifactId(option?.artifactId)}
          value={audioArtifactId}
        />
        <MediaInputSelector
          disabled={disabled}
          group={captionsGroup}
          id={`${formId}-captions`}
          onChange={(option) => setCaptionsArtifactId(option?.artifactId)}
          value={captionsArtifactId}
        />
        <MediaInputSelector
          disabled={disabled}
          group={scenePlanGroup}
          id={`${formId}-scene-plan`}
          onChange={(option) => setScenePlanArtifactId(option?.artifactId)}
          value={scenePlanArtifactId}
        />

        <fieldset disabled={disabled}>
          <legend>画布与帧率</legend>
          <NumericField id={`${formId}-width`} label="宽度" min={1} onChange={setWidth} value={width} />
          <NumericField id={`${formId}-height`} label="高度" min={1} onChange={setHeight} value={height} />
          <NumericField id={`${formId}-fps-numerator`} label="帧率分子" min={1} onChange={setFrameRateNumerator} value={frameRateNumerator} />
          <NumericField id={`${formId}-fps-denominator`} label="帧率分母" min={1} onChange={setFrameRateDenominator} value={frameRateDenominator} />
        </fieldset>

        <fieldset disabled={disabled}>
          <legend>安全区（默认四边各 5%）</legend>
          <NumericField id={`${formId}-safe-x`} label="X" min={0} onChange={setSafeX} value={safeX} />
          <NumericField id={`${formId}-safe-y`} label="Y" min={0} onChange={setSafeY} value={safeY} />
          <NumericField id={`${formId}-safe-width`} label="宽度" min={1} onChange={setSafeWidth} value={safeWidth} />
          <NumericField id={`${formId}-safe-height`} label="高度" min={1} onChange={setSafeHeight} value={safeHeight} />
        </fieldset>

        {error ? (
          <p className="media-input-error" role="alert">
            {error}
          </p>
        ) : null}
        <button className="button primary" disabled={disabled} type="submit">
          {controller.busyLabel ?? "生成新时间轴"}
        </button>
      </form>
    </section>
  );
}

interface PositionedTimelineItem {
  readonly id: string;
  readonly startMs: number;
  readonly endMs: number;
  readonly leftPercent: number;
  readonly widthPercent: number;
}

type TimelinePositionStyle = CSSProperties & {
  readonly "--timeline-left": string;
  readonly "--timeline-width": string;
};

type SafeAreaStyle = CSSProperties & {
  readonly "--safe-left": string;
  readonly "--safe-top": string;
  readonly "--safe-width": string;
  readonly "--safe-height": string;
};

function clampRatio(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

function timelinePositionStyle(item: PositionedTimelineItem): TimelinePositionStyle {
  const left = clampRatio(item.leftPercent / 100);
  const measuredWidth = clampRatio(item.widthPercent / 100);
  const remainingWidth = clampRatio(1 - left);
  const visibleWidth = measuredWidth > 0 ? Math.max(measuredWidth, 0.004) : 0;
  const width = Math.min(visibleWidth, remainingWidth);
  const leftValue = `${left * 100}%`;
  const widthValue = `${width * 100}%`;
  return {
    "--timeline-left": leftValue,
    "--timeline-width": widthValue,
    left: "var(--timeline-left)",
    position: "absolute",
    width: "var(--timeline-width)",
  };
}

function safeAreaStyle(document: TimelineDocument): SafeAreaStyle {
  const { canvas, safeArea } = document;
  const left = clampRatio(safeArea.x / canvas.width);
  const top = clampRatio(safeArea.y / canvas.height);
  const width = Math.min(clampRatio(safeArea.width / canvas.width), 1 - left);
  const height = Math.min(clampRatio(safeArea.height / canvas.height), 1 - top);
  return {
    "--safe-left": `${left * 100}%`,
    "--safe-top": `${top * 100}%`,
    "--safe-width": `${width * 100}%`,
    "--safe-height": `${height * 100}%`,
    border: "2px solid currentColor",
    boxSizing: "border-box",
    height: "var(--safe-height)",
    left: "var(--safe-left)",
    position: "absolute",
    top: "var(--safe-top)",
    width: "var(--safe-width)",
  };
}

function timelineContinuityWarnings(document: TimelineDocument): readonly string[] {
  const warnings: string[] = [];
  const scenes = document.sceneTrack;
  if (
    document.audioTrack.startMs < 0 ||
    document.audioTrack.endMs > document.durationMs
  ) {
    warnings.push("音频轨超出时间轴总时长边界。");
  }
  if (scenes[0].startMs > 0) {
    warnings.push(
      `开头存在空隙：${formatDuration(0)} – ${formatDuration(scenes[0].startMs)}。`,
    );
  }
  for (let index = 1; index < scenes.length; index += 1) {
    const previous = scenes[index - 1];
    const current = scenes[index];
    if (current.startMs > previous.endMs) {
      warnings.push(
        `${previous.sceneId} 与 ${current.sceneId} 之间存在空隙：${formatDuration(previous.endMs)} – ${formatDuration(current.startMs)}。`,
      );
    } else if (current.startMs < previous.endMs) {
      warnings.push(
        `${previous.sceneId} 与 ${current.sceneId} 存在重叠：${formatDuration(current.startMs)} – ${formatDuration(previous.endMs)}。`,
      );
    }
  }
  for (const scene of scenes) {
    if (scene.startMs < 0 || scene.endMs > document.durationMs) {
      warnings.push(`${scene.sceneId} 超出时间轴总时长边界。`);
    }
  }
  const lastScene = scenes[scenes.length - 1];
  if (lastScene.endMs < document.durationMs) {
    warnings.push(
      `结尾存在空隙：${formatDuration(lastScene.endMs)} – ${formatDuration(document.durationMs)}。`,
    );
  }
  return warnings;
}

function TimelineTrackItem({
  item,
  label,
}: {
  readonly item: PositionedTimelineItem;
  readonly label: string;
}) {
  return (
    <li
      aria-label={`${label}，${formatDuration(item.startMs)} 至 ${formatDuration(item.endMs)}`}
      style={{
        ...timelinePositionStyle(item),
        border: "1px solid currentColor",
        boxSizing: "border-box",
        minHeight: "2.75rem",
        overflow: "hidden",
        padding: "0.35rem",
      }}
      title={`${label} · ${formatDuration(item.startMs)} – ${formatDuration(item.endMs)}`}
    >
      <strong>{label}</strong>
      <br />
      <small>
        {formatDuration(item.startMs)} – {formatDuration(item.endMs)}
      </small>
    </li>
  );
}

function TimelineTrack({
  title,
  description,
  items,
}: {
  readonly title: string;
  readonly description: string;
  readonly items: readonly PositionedTimelineItem[];
}) {
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} className="media-timeline-track">
      <h3 id={titleId}>{title}</h3>
      <p>{description}</p>
      <ol
        aria-label={`${title}区段`}
        style={{
          listStyle: "none",
          margin: 0,
          minHeight: "3rem",
          overflow: "hidden",
          padding: 0,
          position: "relative",
        }}
      >
        {items.map((item) => (
          <TimelineTrackItem item={item} key={item.id} label={item.id} />
        ))}
      </ol>
    </section>
  );
}

function TimelineDocumentView({ document }: { readonly document: TimelineDocument }) {
  const layout = buildTimelineTrackLayout(document);
  const warnings = timelineContinuityWarnings(document);
  const safeArea = document.safeArea;
  const canvas = document.canvas;

  return (
    <article className="media-timeline-document" aria-labelledby="timeline-document-title">
      <h2 id="timeline-document-title">正式时间轴</h2>
      <dl>
        <dt>总时长</dt>
        <dd>
          {formatDuration(document.durationMs)}（{document.durationMs} ms）
        </dd>
        <dt>画布</dt>
        <dd>
          {canvas.width} × {canvas.height}
        </dd>
        <dt>帧率</dt>
        <dd>
          {canvas.frameRateNumerator}/{canvas.frameRateDenominator} fps
        </dd>
        <dt>安全区</dt>
        <dd>
          x {safeArea.x}、y {safeArea.y}、{safeArea.width} × {safeArea.height}
        </dd>
        <dt>变更摘要</dt>
        <dd>{document.changeSummary.summary || "未提供摘要"}</dd>
        <dt>变更场景</dt>
        <dd>
          {document.changeSummary.changedSceneIds.length > 0
            ? document.changeSummary.changedSceneIds.join("、")
            : "无"}
        </dd>
        <dt>替代产物</dt>
        <dd>{document.supersedesArtifactId ?? "无，这是首个正式版本"}</dd>
        <dt>字幕</dt>
        <dd>
          {document.captionTrack.visible ? "可见" : "隐藏"}，{document.captionTrack.cueIds.length} 条 cue
        </dd>
      </dl>

      <section aria-labelledby="timeline-safe-area-title">
        <h3 id="timeline-safe-area-title">画布安全区</h3>
        <div
          aria-label={`${canvas.width} × ${canvas.height} 画布，安全区从 ${safeArea.x}, ${safeArea.y} 开始，尺寸 ${safeArea.width} × ${safeArea.height}`}
          role="img"
          style={{
            aspectRatio: `${canvas.width} / ${canvas.height}`,
            border: "1px solid currentColor",
            maxWidth: "40rem",
            position: "relative",
            width: "100%",
          }}
        >
          <div aria-hidden="true" style={safeAreaStyle(document)} />
        </div>
      </section>

      {warnings.length > 0 ? (
        <section aria-labelledby="timeline-continuity-title" role="alert">
          <h3 id="timeline-continuity-title">连续性警告</h3>
          <ul>
            {warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        </section>
      ) : (
        <p role="status">场景轨从开头到结尾连续，无空隙或重叠。</p>
      )}

      {layout ? (
        <section aria-labelledby="timeline-tracks-title">
          <h3 id="timeline-tracks-title">三轨预览</h3>
          <TimelineTrack
            description={`完整音频，${formatDuration(layout.tracks[0].items[0].startMs)} – ${formatDuration(layout.tracks[0].items[0].endMs)}`}
            items={layout.tracks[0].items}
            title="音频轨"
          />
          <TimelineTrack
            description={`${layout.tracks[1].items.length} 个场景区段；每个区段显示 scene_id 与起止时间。`}
            items={layout.tracks[1].items}
            title="场景轨"
          />
          <TimelineTrack
            description={`${layout.tracks[2].visible ? "可见" : "隐藏"}，${layout.tracks[2].cueCount} 条 cue。`}
            items={layout.tracks[2].items}
            title="字幕轨"
          />
        </section>
      ) : (
        <p className="media-input-error" role="alert">
          时间轴布局不可用；原始文档未被静默修复。
        </p>
      )}
    </article>
  );
}

type TimelineEditType = TimelineEdit["editType"];

function parseFiniteNumber(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function TimelineBoundaryFields({
  document,
  formId,
  leftSceneId,
  boundaryMs,
  disabled,
  onLeftSceneChange,
  onBoundaryChange,
}: {
  readonly document: TimelineDocument;
  readonly formId: string;
  readonly leftSceneId: string;
  readonly boundaryMs: string;
  readonly disabled: boolean;
  readonly onLeftSceneChange: (sceneId: string, currentBoundaryMs: number) => void;
  readonly onBoundaryChange: (value: string) => void;
}) {
  const leftSceneIndex = document.sceneTrack.findIndex(
    (scene) => scene.sceneId === leftSceneId,
  );
  const rightScene =
    leftSceneIndex >= 0 ? document.sceneTrack[leftSceneIndex + 1] : undefined;
  const editableLeftScenes = document.sceneTrack.slice(0, -1);

  if (editableLeftScenes.length === 0) {
    return <p role="alert">至少需要两个相邻场景才能移动边界。</p>;
  }

  return (
    <fieldset disabled={disabled}>
      <legend>移动相邻场景边界</legend>
      <label htmlFor={`${formId}-left-scene`}>
        左侧场景
        <select
          id={`${formId}-left-scene`}
          onChange={(event) => {
            const index = document.sceneTrack.findIndex(
              (scene) => scene.sceneId === event.target.value,
            );
            const nextRightScene = document.sceneTrack[index + 1];
            onLeftSceneChange(event.target.value, nextRightScene?.startMs ?? 0);
          }}
          value={leftSceneId}
        >
          {editableLeftScenes.map((scene) => (
            <option key={scene.sceneId} value={scene.sceneId}>
              {scene.sceneId}
            </option>
          ))}
        </select>
      </label>
      <p>
        右侧相邻场景：<strong>{rightScene?.sceneId ?? "未找到"}</strong>
      </p>
      <label htmlFor={`${formId}-boundary-ms`}>
        新边界（ms）
        <input
          id={`${formId}-boundary-ms`}
          onChange={(event) => onBoundaryChange(event.target.value)}
          required
          step="any"
          type="number"
          value={boundaryMs}
        />
      </label>
      {rightScene ? (
        <p>
          必须位于 {document.sceneTrack[leftSceneIndex].startMs} 与 {rightScene.endMs} ms 之间，并为两侧保留正时长。
        </p>
      ) : null}
    </fieldset>
  );
}

function TimelineSafeAreaFields({
  formId,
  safeX,
  safeY,
  safeWidth,
  safeHeight,
  disabled,
  onSafeXChange,
  onSafeYChange,
  onSafeWidthChange,
  onSafeHeightChange,
}: {
  readonly formId: string;
  readonly safeX: string;
  readonly safeY: string;
  readonly safeWidth: string;
  readonly safeHeight: string;
  readonly disabled: boolean;
  readonly onSafeXChange: (value: string) => void;
  readonly onSafeYChange: (value: string) => void;
  readonly onSafeWidthChange: (value: string) => void;
  readonly onSafeHeightChange: (value: string) => void;
}) {
  return (
    <fieldset disabled={disabled}>
      <legend>设置安全区</legend>
      <NumericField id={`${formId}-edit-safe-x`} label="X" min={0} onChange={onSafeXChange} value={safeX} />
      <NumericField id={`${formId}-edit-safe-y`} label="Y" min={0} onChange={onSafeYChange} value={safeY} />
      <NumericField id={`${formId}-edit-safe-width`} label="宽度" min={1} onChange={onSafeWidthChange} value={safeWidth} />
      <NumericField id={`${formId}-edit-safe-height`} label="高度" min={1} onChange={onSafeHeightChange} value={safeHeight} />
    </fieldset>
  );
}

function TimelineCaptionVisibilityField({
  formId,
  visible,
  disabled,
  onChange,
}: {
  readonly formId: string;
  readonly visible: boolean;
  readonly disabled: boolean;
  readonly onChange: (visible: boolean) => void;
}) {
  return (
    <fieldset disabled={disabled}>
      <legend>设置字幕可见性</legend>
      <label htmlFor={`${formId}-caption-visible`}>
        <input
          checked={visible}
          id={`${formId}-caption-visible`}
          onChange={(event) => onChange(event.target.checked)}
          type="checkbox"
        />
        导出时显示字幕
      </label>
    </fieldset>
  );
}

function TimelineEditor({
  controller,
  document,
  baseArtifactId,
}: {
  readonly controller: MediaStageStudioController;
  readonly document: TimelineDocument;
  readonly baseArtifactId: string;
}) {
  const formId = useId();
  const [editType, setEditType] = useState<TimelineEditType>("move_boundary");
  const [leftSceneId, setLeftSceneId] = useState(
    document.sceneTrack[0]?.sceneId ?? "",
  );
  const [boundaryMs, setBoundaryMs] = useState("");
  const [safeX, setSafeX] = useState(String(document.safeArea.x));
  const [safeY, setSafeY] = useState(String(document.safeArea.y));
  const [safeWidth, setSafeWidth] = useState(String(document.safeArea.width));
  const [safeHeight, setSafeHeight] = useState(String(document.safeArea.height));
  const [captionVisible, setCaptionVisible] = useState(
    document.captionTrack.visible,
  );
  const [changeSummary, setChangeSummary] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const disabled =
    !controller.available || controller.busyLabel !== null || submitting;

  function buildEdit(): TimelineEdit | undefined {
    if (editType === "move_boundary") {
      const leftSceneIndex = document.sceneTrack.findIndex(
        (scene) => scene.sceneId === leftSceneId,
      );
      const rightScene = document.sceneTrack[leftSceneIndex + 1];
      const boundary = parseFiniteNumber(boundaryMs);
      if (leftSceneIndex < 0 || !rightScene || boundary === undefined) {
        return undefined;
      }
      return {
        editType,
        leftSceneId,
        rightSceneId: rightScene.sceneId,
        boundaryMs: boundary,
      };
    }
    if (editType === "set_safe_area") {
      const x = parseInteger(safeX);
      const y = parseInteger(safeY);
      const width = parseInteger(safeWidth);
      const height = parseInteger(safeHeight);
      if (
        x === undefined ||
        y === undefined ||
        width === undefined ||
        height === undefined
      ) {
        return undefined;
      }
      return {
        editType,
        safeArea: { x, y, width, height },
      };
    }
    return { editType, visible: captionVisible };
  }

  function clearDraft() {
    setEditType("move_boundary");
    setLeftSceneId(document.sceneTrack[0]?.sceneId ?? "");
    setBoundaryMs("");
    setSafeX("");
    setSafeY("");
    setSafeWidth("");
    setSafeHeight("");
    setCaptionVisible(document.captionTrack.visible);
    setChangeSummary("");
  }

  async function submitEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled) return;
    const summary = changeSummary.trim();
    if (!summary) {
      setError("保存新版本前必须填写变更摘要。");
      return;
    }
    const edit = buildEdit();
    if (!edit) {
      setError("请完整填写当前编辑所需的有效数值。");
      return;
    }
    const validation = validateTimelineEdit(document, edit);
    if (!validation.valid) {
      setError(validation.errors.join("；"));
      return;
    }
    setError(null);
    setStatus(null);
    setSubmitting(true);
    try {
      const saved = await controller.saveTimeline([validation.value], summary);
      if (saved) {
        clearDraft();
        setStatus("已保存一个时间轴编辑，当前草稿已清空。");
      } else {
        setError("时间轴新版本未保存，请查看任务错误后重试。");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <section className="media-timeline-editor" aria-labelledby={`${formId}-title`}>
      <h2 id={`${formId}-title`}>创建时间轴新版本</h2>
      <p id={`${formId}-draft-binding`} role="status">
        草稿仅绑定基础产物 <code>{baseArtifactId}</code>；加载新的 artifact 后，旧草稿会被清除且不会迁移。
      </p>
      <form
        aria-describedby={`${formId}-draft-binding`}
        onSubmit={(event) => void submitEdit(event)}
      >
        <label htmlFor={`${formId}-edit-type`}>
          本次唯一编辑
          <select
            disabled={disabled}
            id={`${formId}-edit-type`}
            onChange={(event) => {
              setEditType(event.target.value as TimelineEditType);
              setError(null);
              setStatus(null);
            }}
            value={editType}
          >
            <option value="move_boundary">移动相邻场景边界</option>
            <option value="set_safe_area">设置安全区</option>
            <option value="set_caption_visibility">设置字幕可见性</option>
          </select>
        </label>

        <div aria-live="polite">
          {editType === "move_boundary" ? (
            <TimelineBoundaryFields
              boundaryMs={boundaryMs}
              disabled={disabled}
              document={document}
              formId={formId}
              leftSceneId={leftSceneId}
              onBoundaryChange={setBoundaryMs}
              onLeftSceneChange={(sceneId, currentBoundaryMs) => {
                setLeftSceneId(sceneId);
                setBoundaryMs(String(currentBoundaryMs));
              }}
            />
          ) : editType === "set_safe_area" ? (
            <TimelineSafeAreaFields
              disabled={disabled}
              formId={formId}
              onSafeHeightChange={setSafeHeight}
              onSafeWidthChange={setSafeWidth}
              onSafeXChange={setSafeX}
              onSafeYChange={setSafeY}
              safeHeight={safeHeight}
              safeWidth={safeWidth}
              safeX={safeX}
              safeY={safeY}
            />
          ) : (
            <TimelineCaptionVisibilityField
              disabled={disabled}
              formId={formId}
              onChange={setCaptionVisible}
              visible={captionVisible}
            />
          )}
        </div>

        <label htmlFor={`${formId}-change-summary`}>
          变更摘要（必填）
          <textarea
            disabled={disabled}
            id={`${formId}-change-summary`}
            onChange={(event) => setChangeSummary(event.target.value)}
            required
            rows={3}
            value={changeSummary}
          />
        </label>
        {error ? (
          <p className="media-input-error" role="alert">
            {error}
          </p>
        ) : null}
        {status ? <p role="status">{status}</p> : null}
        <button className="button primary" disabled={disabled} type="submit">
          {controller.busyLabel ?? (submitting ? "正在保存…" : "保存一个编辑")}
        </button>
      </form>
    </section>
  );
}

function TimelineLastSaveResult({
  controller,
}: {
  readonly controller: MediaStageStudioController;
}) {
  const result = controller.lastSaveResult;
  if (!result) return null;
  return (
    <section aria-live="polite" className="media-timeline-save-result">
      <h2>最近保存结果</h2>
      <dl>
        <dt>新产物</dt>
        <dd>{result.artifactId}</dd>
        <dt>运行</dt>
        <dd>{result.runId}</dd>
        <dt>已变更场景</dt>
        <dd>
          {result.changedSceneIds.length > 0
            ? result.changedSceneIds.join("、")
            : "无场景 ID 变更"}
        </dd>
        <dt>已失效下游阶段</dt>
        <dd>
          {result.staleBecauseStageIds.length > 0
            ? result.staleBecauseStageIds.join("、")
            : "无"}
        </dd>
      </dl>
    </section>
  );
}

export function MediaTimelineView({ controller }: MediaTimelineViewProps) {
  const document = narrowMediaDocument(controller.document, "timeline");
  const rejectedTimeline =
    !document &&
    controller.document !== null &&
    controller.document.documentType === "timeline";

  return (
    <div className="media-timeline-view">
      <TimelineGenerationPanel controller={controller} />
      {document ? (
        <>
          <TimelineDocumentView document={document} />
          {controller.documentArtifactId ? (
            <TimelineEditor
              baseArtifactId={controller.documentArtifactId}
              controller={controller}
              document={document}
              key={controller.documentArtifactId}
            />
          ) : (
            <p className="media-input-error" role="alert">
              当前正式时间轴缺少基础 artifact ID，无法保存新版本。
            </p>
          )}
        </>
      ) : (
        <section className="media-document-empty" aria-label="时间轴说明">
          <h2>尚无正式时间轴</h2>
          <p>请选择三个已审核输入并生成；这里不会构造演示时间轴。</p>
          {rejectedTimeline ? (
            <p className="media-input-error" role="alert">
              当前 timeline 文档未通过结构防线，可能存在越界或重叠；已拒绝预览，未静默修正。
            </p>
          ) : null}
        </section>
      )}
      <TimelineLastSaveResult controller={controller} />
      <MediaJobPanel controller={controller} />
    </div>
  );
}
