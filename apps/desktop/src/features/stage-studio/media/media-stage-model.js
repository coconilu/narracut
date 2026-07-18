const MEDIA_STAGE_REQUIREMENTS = Object.freeze({
  audio: Object.freeze([
    Object.freeze({ stageId: "script", artifactKinds: Object.freeze(["script"]) }),
  ]),
  captions: Object.freeze([
    Object.freeze({ stageId: "script", artifactKinds: Object.freeze(["script"]) }),
    Object.freeze({ stageId: "audio", artifactKinds: Object.freeze(["voice_audio"]) }),
  ]),
  scene_plan: Object.freeze([
    Object.freeze({
      stageId: "research",
      artifactKinds: Object.freeze(["claim_set"]),
    }),
    Object.freeze({ stageId: "script", artifactKinds: Object.freeze(["script"]) }),
    Object.freeze({ stageId: "captions", artifactKinds: Object.freeze(["captions"]) }),
  ]),
  timeline: Object.freeze([
    Object.freeze({ stageId: "audio", artifactKinds: Object.freeze(["voice_audio"]) }),
    Object.freeze({ stageId: "captions", artifactKinds: Object.freeze(["captions"]) }),
    Object.freeze({ stageId: "scene_plan", artifactKinds: Object.freeze(["scene_plan"]) }),
  ]),
});

const SHA256_PATTERN = /^sha256:[a-f\d]{64}$/i;
const DOCUMENT_TYPES = new Set([
  "audio_media",
  "captions_media",
  "scene_plan",
  "timeline",
]);

export function isMediaStageId(value) {
  return (
    typeof value === "string" &&
    Object.prototype.hasOwnProperty.call(MEDIA_STAGE_REQUIREMENTS, value)
  );
}

export function requirementsForStage(stageId) {
  return isMediaStageId(stageId) ? MEDIA_STAGE_REQUIREMENTS[stageId] : null;
}

export function narrowMediaDocument(value, expectedDocumentType) {
  if (!isRecord(value) || value.schemaVersion !== "1.0.0") return null;
  if (!DOCUMENT_TYPES.has(value.documentType)) return null;
  if (
    expectedDocumentType !== undefined &&
    value.documentType !== expectedDocumentType
  ) {
    return null;
  }
  if (!isId(value.projectId) || !isId(value.runId) || !isId(value.createdAt)) {
    return null;
  }

  switch (value.documentType) {
    case "audio_media":
      return isAudioDocument(value) ? value : null;
    case "captions_media":
      return isCaptionsDocument(value) ? value : null;
    case "scene_plan":
      return isScenePlanDocument(value) ? value : null;
    case "timeline":
      return isTimelineDocument(value) ? value : null;
    default:
      return null;
  }
}

export function buildReviewedInputReference(input) {
  if (!isRecord(input)) {
    return invalidResult(["审核输入不是对象。"]);
  }

  const errors = [];
  const expectedProjectId = cleanText(input.expectedProjectId);
  const expectedStageId = cleanText(input.expectedStageId);
  const stageState = input.stageState;
  const run = input.run;
  const review = input.review;
  const artifact = input.artifact;

  if (!expectedProjectId) errors.push("缺少目标 projectId。");
  if (!expectedStageId) errors.push("缺少目标 stageId。");
  if (!isRecord(stageState)) errors.push("缺少阶段状态。");
  if (!isRecord(run)) errors.push("缺少已批准运行。");
  if (!isRecord(review)) errors.push("缺少审核记录。");
  if (!isRecord(artifact)) errors.push("缺少产物元数据。");
  if (errors.length > 0) return invalidResult(errors);

  if (stageState.stageId !== expectedStageId) {
    errors.push("阶段状态与目标 stageId 不匹配。");
  }
  if (stageState.status !== "approved") {
    errors.push("阶段尚未处于 approved 状态。");
  }
  if (!isId(stageState.approvedRunId)) {
    errors.push("阶段没有 approvedRunId。");
  }

  if (run.projectId !== expectedProjectId) {
    errors.push("运行不属于目标工程。");
  }
  if (run.stageId !== expectedStageId) {
    errors.push("运行不属于目标阶段。");
  }
  if (run.runId !== stageState.approvedRunId) {
    errors.push("运行不是阶段当前批准版本。");
  }
  if (run.status !== "succeeded") {
    errors.push("批准运行尚未成功完成。");
  }

  if (review.projectId !== expectedProjectId) {
    errors.push("审核记录不属于目标工程。");
  }
  if (review.stageId !== expectedStageId || review.runId !== run.runId) {
    errors.push("审核记录与目标阶段运行不匹配。");
  }
  if (review.decision !== "approved") {
    errors.push("审核决定不是 approved。");
  }

  if (
    artifact.projectId !== expectedProjectId ||
    artifact.stageId !== expectedStageId ||
    artifact.runId !== run.runId
  ) {
    errors.push("产物身份与目标工程、阶段或运行不匹配。");
  }
  if (!isId(artifact.artifactId)) {
    errors.push("产物缺少 artifactId。");
  }
  if (
    !Array.isArray(run.artifactIds) ||
    !run.artifactIds.includes(artifact.artifactId)
  ) {
    errors.push("运行产物清单未包含该产物。");
  }
  if (
    !Array.isArray(review.artifactIds) ||
    !review.artifactIds.includes(artifact.artifactId)
  ) {
    errors.push("审核记录未包含该产物。");
  }
  if (!isSha256(artifact.contentHash)) {
    errors.push("产物缺少有效 contentHash。");
  }
  if (
    Array.isArray(input.expectedArtifactKinds) &&
    input.expectedArtifactKinds.length > 0 &&
    !input.expectedArtifactKinds.includes(artifact.kind)
  ) {
    errors.push("产物 kind 不符合阶段输入契约。");
  }

  const provenance = Array.isArray(artifact.provenance)
    ? artifact.provenance
    : [];
  if (provenance.length === 0) {
    errors.push("产物没有 claim/evidence 追溯信息。");
  } else if (
    provenance.some(
      (item) =>
        !isRecord(item) || !isId(item.claimId) || !isId(item.evidenceRef),
    )
  ) {
    errors.push("产物追溯信息存在空 claimId 或 evidenceRef。");
  }

  if (!isId(review.reviewId)) errors.push("审核记录缺少 reviewId。");
  if (errors.length > 0) return invalidResult(errors);

  return {
    valid: true,
    value: {
      stageId: expectedStageId,
      runId: run.runId,
      artifactId: artifact.artifactId,
      contentHash: artifact.contentHash,
      reviewRecordId: review.reviewId,
      claimIds: uniqueStrings(provenance.map((item) => item.claimId)),
      evidenceRefs: uniqueStrings(provenance.map((item) => item.evidenceRef)),
    },
  };
}

export function validateImportForm(input) {
  if (!isRecord(input)) return invalidResult(["导入表单不是对象。"]);

  const errors = [];
  const sourcePath = cleanText(input.sourcePath);
  const ownership = input.ownership;
  const author = cleanText(input.author);
  const rightsStatement = cleanText(input.rightsStatement);
  const licenseId = cleanText(input.licenseId);
  const attributionText = cleanText(input.attributionText);

  if (!sourcePath) errors.push("请选择本地源文件。");
  if (ownership !== "self_recorded" && ownership !== "licensed") {
    errors.push("权利类型必须是 self_recorded 或 licensed。");
  }
  if (!author) errors.push("请填写作者或录制者。");
  if (!rightsStatement) errors.push("请填写权利声明。");
  if (Object.prototype.hasOwnProperty.call(input, "voiceAuthorization")) {
    errors.push("voiceAuthorization 由系统固定，调用方不能覆盖。");
  }
  if (ownership === "licensed") {
    if (!licenseId) errors.push("许可素材必须填写 licenseId。");
    if (!attributionText) errors.push("许可素材必须填写署名文本。");
  }
  if (ownership === "self_recorded" && licenseId) {
    errors.push("自行录制素材不应填写第三方 licenseId。");
  }

  if (errors.length > 0) return invalidResult(errors);
  return {
    valid: true,
    value: {
      sourcePath,
      rights: {
        ownership,
        author,
        rightsStatement,
        licenseId,
        attributionText,
        voiceAuthorization: "not_voice_clone",
      },
    },
  };
}

export function validateSceneEdit(document, edit) {
  const scenePlan = narrowMediaDocument(document, "scene_plan");
  if (!scenePlan) return invalidResult(["场景文档结构无效。"]);
  if (!isRecord(edit)) return invalidResult(["场景编辑不是对象。"]);

  const scenes = [...scenePlan.scenes].sort((left, right) => left.order - right.order);
  if (edit.editType === "split") {
    const scene = scenes.find((item) => item.sceneId === edit.sceneId);
    if (!scene) return invalidResult(["拆分目标场景不存在。"]);
    if (!isFiniteNumber(edit.splitAtMs)) {
      return invalidResult(["拆分时间必须是有限数字。"]);
    }
    if (
      edit.splitAtMs <= scene.suggestedStartMs ||
      edit.splitAtMs >= scene.suggestedEndMs
    ) {
      return invalidResult(["拆分时间必须严格位于场景内部。"]);
    }
    return validEditResult(edit);
  }

  if (edit.editType === "merge") {
    const firstIndex = scenes.findIndex(
      (item) => item.sceneId === edit.firstSceneId,
    );
    const secondIndex = scenes.findIndex(
      (item) => item.sceneId === edit.secondSceneId,
    );
    if (firstIndex < 0 || secondIndex < 0) {
      return invalidResult(["合并目标场景不存在。"]);
    }
    if (secondIndex !== firstIndex + 1) {
      return invalidResult(["只能按时间顺序合并相邻场景。"]);
    }
    return validEditResult(edit);
  }

  if (edit.editType === "update") {
    if (!scenes.some((item) => item.sceneId === edit.sceneId)) {
      return invalidResult(["更新目标场景不存在。"]);
    }
    const hasTitle = Object.prototype.hasOwnProperty.call(edit, "title");
    const hasRole = Object.prototype.hasOwnProperty.call(edit, "narrativeRole");
    if (!hasTitle && !hasRole) {
      return invalidResult(["更新场景至少需要 title 或 narrativeRole。"]);
    }
    if (hasTitle && !cleanText(edit.title)) {
      return invalidResult(["场景标题不能为空。"]);
    }
    if (hasRole && !cleanText(edit.narrativeRole)) {
      return invalidResult(["叙事角色不能为空。"]);
    }
    return validEditResult(edit);
  }

  if (edit.editType === "move_boundary") {
    return validateBoundaryEdit(
      scenes.map((scene) => ({
        sceneId: scene.sceneId,
        startMs: scene.suggestedStartMs,
        endMs: scene.suggestedEndMs,
      })),
      edit,
    );
  }

  return invalidResult(["不支持的场景编辑类型。"]);
}

export function validateTimelineEdit(document, edit) {
  const timeline = narrowMediaDocument(document, "timeline");
  if (!timeline) return invalidResult(["时间轴文档结构无效。"]);
  if (!isRecord(edit)) return invalidResult(["时间轴编辑不是对象。"]);

  if (edit.editType === "move_boundary") {
    return validateBoundaryEdit(timeline.sceneTrack, edit);
  }
  if (edit.editType === "set_safe_area") {
    if (!isSafeArea(edit.safeArea, timeline.canvas)) {
      return invalidResult(["安全区必须为画布内的正尺寸矩形。"]);
    }
    return validEditResult(edit);
  }
  if (edit.editType === "set_caption_visibility") {
    return typeof edit.visible === "boolean"
      ? validEditResult(edit)
      : invalidResult(["字幕可见性必须是布尔值。"]);
  }
  return invalidResult(["不支持的时间轴编辑类型。"]);
}

export function formatDuration(milliseconds) {
  if (!isFiniteNumber(milliseconds) || milliseconds < 0) return "—";
  const total = Math.round(milliseconds);
  const hours = Math.floor(total / 3_600_000);
  const minutes = Math.floor((total % 3_600_000) / 60_000);
  const seconds = Math.floor((total % 60_000) / 1_000);
  const millis = total % 1_000;
  const body = `${pad(minutes, hours > 0 ? 2 : 2)}:${pad(seconds, 2)}.${pad(millis, 3)}`;
  return hours > 0 ? `${pad(hours, 2)}:${body}` : body;
}

export function buildTimelineTrackLayout(document) {
  const timeline = narrowMediaDocument(document, "timeline");
  if (!timeline || timeline.durationMs <= 0) return null;
  const durationMs = timeline.durationMs;

  return {
    durationMs,
    tracks: [
      {
        trackId: "audio",
        items: [
          layoutItem(
            timeline.audioTrack.audioArtifactId,
            timeline.audioTrack.startMs,
            timeline.audioTrack.endMs,
            durationMs,
          ),
        ],
      },
      {
        trackId: "scenes",
        items: timeline.sceneTrack.map((scene) =>
          layoutItem(scene.sceneId, scene.startMs, scene.endMs, durationMs),
        ),
      },
      {
        trackId: "captions",
        visible: timeline.captionTrack.visible,
        cueCount: timeline.captionTrack.cueIds.length,
        items: [
          layoutItem(
            timeline.captionTrack.captionsArtifactId,
            0,
            durationMs,
            durationMs,
          ),
        ],
      },
    ],
  };
}

function isAudioDocument(value) {
  return (
    isId(value.mediaId) &&
    isId(value.artifactUri) &&
    isImportedSource(value.source) &&
    isRights(value.rights) &&
    isPositiveNumber(value.durationMs) &&
    isPositiveInteger(value.sampleRateHz) &&
    [8, 16, 24, 32].includes(value.bitsPerSample) &&
    isPositiveInteger(value.channels) &&
    isPositiveInteger(value.blockAlign) &&
    isPositiveInteger(value.byteRate) &&
    isPositiveInteger(value.dataBytes) &&
    isFrozenInputs(value.inputRefs, 1, 8) &&
    isRecord(value.configSnapshot)
  );
}

function isCaptionsDocument(value) {
  return (
    isId(value.captionsId) &&
    isId(value.rawArtifactId) &&
    isSha256(value.rawContentHash) &&
    isImportedSource(value.source) &&
    isFrozenInput(value.audioInput) &&
    isNonEmptyArray(value.cues) &&
    value.cues.every(isCaptionCue) &&
    isNonEmptyArray(value.mappings) &&
    value.mappings.every(isTimingMapping) &&
    Array.isArray(value.diagnostics) &&
    value.diagnostics.every(isDiagnostic) &&
    isFrozenInputs(value.inputRefs, 2, 16) &&
    isRecord(value.configSnapshot)
  );
}

function isScenePlanDocument(value) {
  return (
    isId(value.scenePlanId) &&
    isFrozenInputs(value.inputRefs, 2, 32) &&
    isRecord(value.configSnapshot) &&
    isNonEmptyArray(value.scenes) &&
    value.scenes.every(isScene) &&
    Array.isArray(value.diagnostics) &&
    value.diagnostics.every(isDiagnostic) &&
    isChangeSummary(value.changeSummary)
  );
}

function isTimelineDocument(value) {
  if (
    !isId(value.timelineId) ||
    !isPositiveNumber(value.durationMs) ||
    !isCanvas(value.canvas) ||
    !isAudioTrack(value.audioTrack, value.durationMs) ||
    !isNonEmptyArray(value.sceneTrack) ||
    !value.sceneTrack.every((scene) => isTimelineScene(scene, value.durationMs)) ||
    !isCaptionTrack(value.captionTrack) ||
    !isSafeArea(value.safeArea, value.canvas) ||
    !isFrozenInputs(value.inputRefs, 3, 32) ||
    !isRecord(value.configSnapshot) ||
    !isChangeSummary(value.changeSummary)
  ) {
    return false;
  }
  return value.sceneTrack.every(
    (scene, index, scenes) =>
      index === 0 || scenes[index - 1].endMs <= scene.startMs,
  );
}

function isImportedSource(value) {
  return (
    isRecord(value) &&
    isId(value.sourceFileName) &&
    isSha256(value.sourceContentHash) &&
    isPositiveInteger(value.byteLength)
  );
}

function isRights(value) {
  return (
    isRecord(value) &&
    (value.ownership === "self_recorded" || value.ownership === "licensed") &&
    isId(value.author) &&
    isId(value.rightsStatement) &&
    typeof value.licenseId === "string" &&
    typeof value.attributionText === "string" &&
    value.voiceAuthorization === "not_voice_clone" &&
    (value.ownership !== "licensed" ||
      (isId(value.licenseId) && isId(value.attributionText)))
  );
}

function isFrozenInputs(value, minimum, maximum) {
  return (
    Array.isArray(value) &&
    value.length >= minimum &&
    value.length <= maximum &&
    value.every(isFrozenInput)
  );
}

function isFrozenInput(value) {
  return (
    isRecord(value) &&
    isId(value.projectId) &&
    isId(value.stageId) &&
    isId(value.runId) &&
    isId(value.artifactId) &&
    isSha256(value.contentHash) &&
    isId(value.reviewRecordId) &&
    isNonEmptyStringArray(value.claimIds) &&
    isNonEmptyStringArray(value.evidenceRefs)
  );
}

function isCaptionCue(value) {
  return (
    isRecord(value) &&
    isId(value.cueId) &&
    isNonNegativeInteger(value.sourceIndex) &&
    isNonNegativeNumber(value.startMs) &&
    isPositiveNumber(value.endMs) &&
    value.endMs > value.startMs &&
    isId(value.text) &&
    isNonEmptyStringArray(value.claimIds) &&
    isNonEmptyStringArray(value.evidenceRefs)
  );
}

function isTimingMapping(value) {
  return (
    isRecord(value) &&
    isId(value.mappingId) &&
    ["cue", "sentence", "word"].includes(value.level) &&
    isId(value.sourceCueId) &&
    isNonNegativeNumber(value.startMs) &&
    isPositiveNumber(value.endMs) &&
    value.endMs > value.startMs &&
    isId(value.text) &&
    ["cue_exact", "estimated"].includes(value.timingPrecision) &&
    ["srt_cue", "sentence_interpolation", "word_interpolation"].includes(
      value.timingBasis,
    )
  );
}

function isDiagnostic(value) {
  return (
    isRecord(value) &&
    isId(value.code) &&
    ["info", "warning", "error"].includes(value.severity) &&
    isId(value.message) &&
    typeof value.blocking === "boolean"
  );
}

function isScene(value) {
  return (
    isRecord(value) &&
    isId(value.sceneId) &&
    isNonNegativeInteger(value.order) &&
    isId(value.title) &&
    isId(value.narrativeRole) &&
    isNonNegativeNumber(value.suggestedStartMs) &&
    isPositiveNumber(value.suggestedEndMs) &&
    value.suggestedEndMs > value.suggestedStartMs &&
    isNonEmptyStringArray(value.cueIds) &&
    isNonEmptyStringArray(value.claimIds) &&
    isNonEmptyStringArray(value.evidenceRefs)
  );
}

function isChangeSummary(value) {
  return (
    isRecord(value) &&
    typeof value.summary === "string" &&
    Array.isArray(value.changedSceneIds) &&
    value.changedSceneIds.every(isId)
  );
}

function isCanvas(value) {
  return (
    isRecord(value) &&
    isPositiveInteger(value.width) &&
    isPositiveInteger(value.height) &&
    isPositiveInteger(value.frameRateNumerator) &&
    isPositiveInteger(value.frameRateDenominator)
  );
}

function isAudioTrack(value, durationMs) {
  return (
    isRecord(value) &&
    isId(value.audioArtifactId) &&
    value.startMs === 0 &&
    isPositiveNumber(value.endMs) &&
    value.endMs <= durationMs
  );
}

function isTimelineScene(value, durationMs) {
  return (
    isRecord(value) &&
    isId(value.sceneId) &&
    isNonNegativeNumber(value.startMs) &&
    isPositiveNumber(value.endMs) &&
    value.endMs > value.startMs &&
    value.endMs <= durationMs
  );
}

function isCaptionTrack(value) {
  return (
    isRecord(value) &&
    isId(value.captionsArtifactId) &&
    Array.isArray(value.cueIds) &&
    value.cueIds.every(isId) &&
    typeof value.visible === "boolean"
  );
}

function isSafeArea(value, canvas) {
  return (
    isRecord(value) &&
    isRecord(canvas) &&
    isNonNegativeNumber(value.x) &&
    isNonNegativeNumber(value.y) &&
    isPositiveNumber(value.width) &&
    isPositiveNumber(value.height) &&
    value.x + value.width <= canvas.width &&
    value.y + value.height <= canvas.height
  );
}

function validateBoundaryEdit(scenes, edit) {
  const leftIndex = scenes.findIndex(
    (scene) => scene.sceneId === edit.leftSceneId,
  );
  const rightIndex = scenes.findIndex(
    (scene) => scene.sceneId === edit.rightSceneId,
  );
  if (leftIndex < 0 || rightIndex < 0) {
    return invalidResult(["边界两侧场景不存在。"]);
  }
  if (rightIndex !== leftIndex + 1) {
    return invalidResult(["只能移动相邻场景之间的边界。"]);
  }
  if (!isFiniteNumber(edit.boundaryMs)) {
    return invalidResult(["边界时间必须是有限数字。"]);
  }
  if (
    edit.boundaryMs <= scenes[leftIndex].startMs ||
    edit.boundaryMs >= scenes[rightIndex].endMs
  ) {
    return invalidResult(["新边界必须为两侧场景各保留正时长。"]);
  }
  return validEditResult(edit);
}

function layoutItem(id, startMs, endMs, durationMs) {
  const start = clamp(startMs, 0, durationMs);
  const end = clamp(endMs, start, durationMs);
  return {
    id,
    startMs: start,
    endMs: end,
    leftPercent: percentage(start, durationMs),
    widthPercent: percentage(end - start, durationMs),
  };
}

function percentage(value, total) {
  return Number(((value / total) * 100).toFixed(4));
}

function clamp(value, minimum, maximum) {
  return Math.min(Math.max(value, minimum), maximum);
}

function invalidResult(errors) {
  return { valid: false, errors };
}

function validEditResult(value) {
  return { valid: true, value };
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isId(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function cleanText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function isSha256(value) {
  return typeof value === "string" && SHA256_PATTERN.test(value);
}

function isFiniteNumber(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function isPositiveNumber(value) {
  return isFiniteNumber(value) && value > 0;
}

function isNonNegativeNumber(value) {
  return isFiniteNumber(value) && value >= 0;
}

function isPositiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function isNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function isNonEmptyArray(value) {
  return Array.isArray(value) && value.length > 0;
}

function isNonEmptyStringArray(value) {
  return isNonEmptyArray(value) && value.every(isId);
}

function uniqueStrings(values) {
  return [...new Set(values)];
}

function pad(value, width) {
  return String(value).padStart(width, "0");
}
