import assert from "node:assert/strict";
import test from "node:test";
import {
  buildReviewedInputReference,
  buildTimelineTrackLayout,
  formatDuration,
  isMediaStageId,
  narrowMediaDocument,
  requirementsForStage,
  validateImportForm,
  validateSceneEdit,
  validateTimelineEdit,
} from "./media-stage-model.js";

const HASH_A = `sha256:${"a".repeat(64)}`;
const HASH_B = `sha256:${"b".repeat(64)}`;

function frozenInput(overrides = {}) {
  return {
    projectId: "project_demo",
    stageId: "script",
    runId: "run_script_approved",
    artifactId: "artifact_script",
    contentHash: HASH_A,
    reviewRecordId: "review_script",
    claimIds: ["claim_intro"],
    evidenceRefs: ["evidence_source_1"],
    ...overrides,
  };
}

function importedSource() {
  return {
    sourceFileName: "voice.wav",
    sourceContentHash: HASH_B,
    byteLength: 16_000,
  };
}

function rights() {
  return {
    ownership: "self_recorded",
    author: "本机创作者",
    rightsStatement: "本人录制并授权用于本项目",
    licenseId: "",
    attributionText: "",
    voiceAuthorization: "not_voice_clone",
  };
}

function audioDocument(overrides = {}) {
  return {
    schemaVersion: "1.0.0",
    documentType: "audio_media",
    mediaId: "media_voice",
    projectId: "project_demo",
    runId: "run_audio_approved",
    artifactUri: "project://artifacts/audio.json",
    source: importedSource(),
    rights: rights(),
    durationMs: 5_000,
    sampleRateHz: 48_000,
    bitsPerSample: 16,
    channels: 2,
    blockAlign: 4,
    byteRate: 192_000,
    dataBytes: 960_000,
    inputRefs: [frozenInput()],
    configSnapshot: {},
    createdAt: "2026-07-18T08:00:00Z",
    ...overrides,
  };
}

function captionsDocument(overrides = {}) {
  const audioInput = frozenInput({
    stageId: "audio",
    runId: "run_audio_approved",
    artifactId: "artifact_audio",
  });
  return {
    schemaVersion: "1.0.0",
    documentType: "captions_media",
    captionsId: "captions_demo",
    projectId: "project_demo",
    runId: "run_captions_approved",
    rawArtifactId: "artifact_srt_raw",
    rawContentHash: HASH_B,
    source: { ...importedSource(), sourceFileName: "captions.srt" },
    audioInput,
    cues: [
      {
        cueId: "cue_1",
        sourceIndex: 1,
        startMs: 0,
        endMs: 2_000,
        text: "第一句",
        claimIds: ["claim_intro"],
        evidenceRefs: ["evidence_source_1"],
      },
    ],
    mappings: [
      {
        mappingId: "mapping_1",
        level: "cue",
        sourceCueId: "cue_1",
        startMs: 0,
        endMs: 2_000,
        text: "第一句",
        timingPrecision: "cue_exact",
        timingBasis: "srt_cue",
      },
    ],
    diagnostics: [],
    inputRefs: [frozenInput(), audioInput],
    configSnapshot: {},
    createdAt: "2026-07-18T08:05:00Z",
    ...overrides,
  };
}

function scenePlanDocument(overrides = {}) {
  return {
    schemaVersion: "1.0.0",
    documentType: "scene_plan",
    scenePlanId: "scene_plan_demo",
    projectId: "project_demo",
    runId: "run_scene_approved",
    inputRefs: [
      frozenInput({ stageId: "research", artifactId: "artifact_research" }),
      frozenInput(),
      frozenInput({ stageId: "captions", artifactId: "artifact_captions" }),
    ],
    configSnapshot: {},
    scenes: [
      {
        sceneId: "scene_1",
        order: 1,
        title: "开场",
        narrativeRole: "hook",
        suggestedStartMs: 0,
        suggestedEndMs: 2_000,
        cueIds: ["cue_1"],
        claimIds: ["claim_intro"],
        evidenceRefs: ["evidence_source_1"],
      },
      {
        sceneId: "scene_2",
        order: 2,
        title: "展开",
        narrativeRole: "explain",
        suggestedStartMs: 2_000,
        suggestedEndMs: 5_000,
        cueIds: ["cue_2"],
        claimIds: ["claim_body"],
        evidenceRefs: ["evidence_source_2"],
      },
    ],
    diagnostics: [],
    changeSummary: { summary: "初始场景", changedSceneIds: [] },
    createdAt: "2026-07-18T08:10:00Z",
    ...overrides,
  };
}

function timelineDocument(overrides = {}) {
  return {
    schemaVersion: "1.0.0",
    documentType: "timeline",
    timelineId: "timeline_demo",
    projectId: "project_demo",
    runId: "run_timeline_approved",
    durationMs: 5_000,
    canvas: {
      width: 1_920,
      height: 1_080,
      frameRateNumerator: 30,
      frameRateDenominator: 1,
    },
    audioTrack: {
      audioArtifactId: "artifact_audio",
      startMs: 0,
      endMs: 5_000,
    },
    sceneTrack: [
      { sceneId: "scene_1", startMs: 0, endMs: 2_000 },
      { sceneId: "scene_2", startMs: 2_000, endMs: 5_000 },
    ],
    captionTrack: {
      captionsArtifactId: "artifact_captions",
      cueIds: ["cue_1", "cue_2"],
      visible: true,
    },
    safeArea: { x: 96, y: 54, width: 1_728, height: 972 },
    inputRefs: [
      frozenInput({ stageId: "audio", artifactId: "artifact_audio" }),
      frozenInput({ stageId: "captions", artifactId: "artifact_captions" }),
      frozenInput({ stageId: "scene_plan", artifactId: "artifact_scene" }),
    ],
    configSnapshot: {},
    changeSummary: { summary: "初始时间轴", changedSceneIds: [] },
    createdAt: "2026-07-18T08:15:00Z",
    ...overrides,
  };
}

function reviewedInput(overrides = {}) {
  const base = {
    expectedProjectId: "project_demo",
    expectedStageId: "script",
    expectedArtifactKinds: ["script"],
    stageState: {
      stageId: "script",
      status: "approved",
      approvedRunId: "run_script_approved",
    },
    run: {
      projectId: "project_demo",
      stageId: "script",
      runId: "run_script_approved",
      status: "succeeded",
      artifactIds: ["artifact_script"],
    },
    review: {
      reviewId: "review_script",
      projectId: "project_demo",
      stageId: "script",
      runId: "run_script_approved",
      decision: "approved",
      artifactIds: ["artifact_script"],
    },
    artifact: {
      projectId: "project_demo",
      stageId: "script",
      runId: "run_script_approved",
      artifactId: "artifact_script",
      kind: "script",
      contentHash: HASH_A,
      provenance: [
        { claimId: "claim_intro", evidenceRef: "evidence_source_1" },
        { claimId: "claim_intro", evidenceRef: "evidence_source_2" },
      ],
    },
  };
  return {
    ...base,
    ...overrides,
    stageState: { ...base.stageState, ...overrides.stageState },
    run: { ...base.run, ...overrides.run },
    review: { ...base.review, ...overrides.review },
    artifact: { ...base.artifact, ...overrides.artifact },
  };
}

test("识别四个媒体阶段并给出明确的上游契约", () => {
  assert.equal(isMediaStageId("audio"), true);
  assert.equal(isMediaStageId("captions"), true);
  assert.equal(isMediaStageId("scene_plan"), true);
  assert.equal(isMediaStageId("timeline"), true);
  assert.equal(isMediaStageId("export"), false);
  assert.equal(requirementsForStage("export"), null);
  assert.deepEqual(requirementsForStage("timeline"), [
    { stageId: "audio", artifactKinds: ["voice_audio"] },
    { stageId: "captions", artifactKinds: ["captions"] },
    { stageId: "scene_plan", artifactKinds: ["scene_plan"] },
  ]);
  assert.deepEqual(requirementsForStage("scene_plan")?.[0], {
    stageId: "research",
    artifactKinds: ["claim_set"],
  });
});

test("运行时判型接受四类完整文档", () => {
  assert.equal(narrowMediaDocument(audioDocument())?.documentType, "audio_media");
  assert.equal(
    narrowMediaDocument(captionsDocument())?.documentType,
    "captions_media",
  );
  assert.equal(
    narrowMediaDocument(scenePlanDocument())?.documentType,
    "scene_plan",
  );
  assert.equal(narrowMediaDocument(timelineDocument())?.documentType, "timeline");
});

test("运行时判型拒绝字段缺失和 documentType 冒充", () => {
  assert.equal(narrowMediaDocument(audioDocument({ mediaId: undefined })), null);
  assert.equal(
    narrowMediaDocument({ ...audioDocument(), documentType: "timeline" }),
    null,
  );
  assert.equal(narrowMediaDocument(timelineDocument(), "scene_plan"), null);
  assert.equal(
    narrowMediaDocument(
      timelineDocument({ safeArea: { x: 1_900, y: 0, width: 100, height: 100 } }),
    ),
    null,
  );
});

test("批准运行、批准审核和完整追溯共同生成冻结输入引用", () => {
  const result = buildReviewedInputReference(reviewedInput());
  assert.deepEqual(result, {
    valid: true,
    value: {
      stageId: "script",
      runId: "run_script_approved",
      artifactId: "artifact_script",
      contentHash: HASH_A,
      reviewRecordId: "review_script",
      claimIds: ["claim_intro"],
      evidenceRefs: ["evidence_source_1", "evidence_source_2"],
    },
  });
});

test("冻结输入引用对身份、审核、哈希和追溯缺陷一律 fail-closed", () => {
  const cases = [
    reviewedInput({ run: { projectId: "project_other" } }),
    reviewedInput({ artifact: { stageId: "research" } }),
    reviewedInput({ review: { runId: "run_other" } }),
    reviewedInput({ stageState: { status: "stale" } }),
    reviewedInput({ review: { decision: "rejected" } }),
    reviewedInput({ review: { artifactIds: [] } }),
    reviewedInput({ artifact: { contentHash: undefined } }),
    reviewedInput({ artifact: { provenance: [] } }),
    reviewedInput({
      artifact: { provenance: [{ claimId: "", evidenceRef: "evidence_source_1" }] },
    }),
  ];

  for (const input of cases) {
    const result = buildReviewedInputReference(input);
    assert.equal(result.valid, false);
    assert.ok(result.errors.length > 0);
  }
});

test("导入表单固定非克隆授权，并区分自行录制与许可素材", () => {
  const selfRecorded = validateImportForm({
    sourcePath: "C:\\media\\voice.wav",
    ownership: "self_recorded",
    author: "本机创作者",
    rightsStatement: "本人录制并授权用于本项目",
    licenseId: "",
    attributionText: "",
  });
  assert.equal(selfRecorded.valid, true);
  assert.equal(selfRecorded.value.rights.voiceAuthorization, "not_voice_clone");

  const licensed = validateImportForm({
    sourcePath: "C:\\media\\licensed.wav",
    ownership: "licensed",
    author: "声音作者",
    rightsStatement: "已取得项目使用许可",
    licenseId: "license_2026_001",
    attributionText: "声音：声音作者",
  });
  assert.equal(licensed.valid, true);
});

test("导入表单拒绝缺失字段、不完整许可和 voiceAuthorization 覆盖", () => {
  const missing = validateImportForm({
    sourcePath: "",
    ownership: "licensed",
    author: "",
    rightsStatement: "",
    licenseId: "",
    attributionText: "",
  });
  assert.equal(missing.valid, false);
  assert.ok(missing.errors.length >= 5);

  const override = validateImportForm({
    sourcePath: "C:\\media\\voice.wav",
    ownership: "self_recorded",
    author: "本机创作者",
    rightsStatement: "本人录制",
    licenseId: "",
    attributionText: "",
    voiceAuthorization: "not_voice_clone",
  });
  assert.equal(override.valid, false);

  const contradictory = validateImportForm({
    sourcePath: "C:\\media\\voice.wav",
    ownership: "self_recorded",
    author: "本机创作者",
    rightsStatement: "本人录制",
    licenseId: "third_party_license",
    attributionText: "",
  });
  assert.equal(contradictory.valid, false);
});

test("场景编辑仅允许内部拆分、相邻合并和保留正时长的边界", () => {
  const document = scenePlanDocument();
  assert.equal(
    validateSceneEdit(document, {
      editType: "split",
      sceneId: "scene_1",
      splitAtMs: 1_000,
    }).valid,
    true,
  );
  assert.equal(
    validateSceneEdit(document, {
      editType: "split",
      sceneId: "scene_1",
      splitAtMs: 0,
    }).valid,
    false,
  );
  assert.equal(
    validateSceneEdit(document, {
      editType: "merge",
      firstSceneId: "scene_1",
      secondSceneId: "scene_2",
    }).valid,
    true,
  );
  assert.equal(
    validateSceneEdit(document, {
      editType: "merge",
      firstSceneId: "scene_2",
      secondSceneId: "scene_1",
    }).valid,
    false,
  );
  assert.equal(
    validateSceneEdit(document, {
      editType: "move_boundary",
      leftSceneId: "scene_1",
      rightSceneId: "scene_2",
      boundaryMs: 2_500,
    }).valid,
    true,
  );
  assert.equal(
    validateSceneEdit(document, {
      editType: "update",
      sceneId: "scene_1",
      title: "   ",
    }).valid,
    false,
  );
});

test("时间轴编辑校验相邻边界、安全区和字幕可见性", () => {
  const document = timelineDocument();
  assert.equal(
    validateTimelineEdit(document, {
      editType: "move_boundary",
      leftSceneId: "scene_1",
      rightSceneId: "scene_2",
      boundaryMs: 2_500,
    }).valid,
    true,
  );
  assert.equal(
    validateTimelineEdit(document, {
      editType: "set_safe_area",
      safeArea: { x: 100, y: 100, width: 1_000, height: 700 },
    }).valid,
    true,
  );
  assert.equal(
    validateTimelineEdit(document, {
      editType: "set_safe_area",
      safeArea: { x: 1_900, y: 0, width: 100, height: 100 },
    }).valid,
    false,
  );
  assert.equal(
    validateTimelineEdit(document, {
      editType: "set_caption_visibility",
      visible: "yes",
    }).valid,
    false,
  );
});

test("时长格式与三轨比例可直接供 UI 使用", () => {
  assert.equal(formatDuration(0), "00:00.000");
  assert.equal(formatDuration(62_345), "01:02.345");
  assert.equal(formatDuration(3_662_345), "01:01:02.345");
  assert.equal(formatDuration(-1), "—");

  const layout = buildTimelineTrackLayout(timelineDocument());
  assert.equal(layout.tracks.length, 3);
  assert.deepEqual(
    layout.tracks.map((track) => track.trackId),
    ["audio", "scenes", "captions"],
  );
  assert.equal(layout.tracks[0].items[0].widthPercent, 100);
  assert.equal(layout.tracks[1].items[0].widthPercent, 40);
  assert.equal(layout.tracks[1].items[1].leftPercent, 40);
  assert.equal(layout.tracks[1].items[1].widthPercent, 60);
  assert.equal(layout.tracks[2].cueCount, 2);
});
