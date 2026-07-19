import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AudioMediaDocument,
  CaptionsMediaDocument,
  JobSnapshot,
  MediaJobAcceptedResult,
  MediaReviewedInputReference,
  MediaSaveResult,
  NarraCutMediaDocument,
  ProjectDescriptor,
  ScenePlanDocument,
  ScenePlanEdit,
  StageRun,
  TimelineCanvasInput,
  TimelineEdit,
  TimelineDocument,
  TimelineSafeAreaInput,
} from "@narracut/contracts";
import {
  desktopGateway,
  type WorkbenchArtifact,
} from "../../../lib/desktop-gateway";
import { isJobCommandError, jobCommands } from "../../../lib/job-commands";
import {
  isMediaCommandError,
  mediaCommands,
} from "../../../lib/media-commands";
import type { WorkflowSnapshotView } from "../../../lib/workflow-commands";
import { createRequestGate } from "../../app/request-gate.js";
import {
  buildReviewedInputReference,
  isMediaStageId,
  narrowMediaDocument,
  requirementsForStage,
  validateSceneEdit,
  validateTimelineEdit,
  type MediaStageId,
  type MediaStageRequirement,
  type ValidatedImportForm,
} from "./media-stage-model.js";

export interface MediaInputOption {
  readonly artifactId: string;
  readonly kind: string;
  readonly label: string;
  readonly reference: MediaReviewedInputReference;
}

export interface MediaInputOptionGroup {
  readonly requirement: MediaStageRequirement;
  readonly options: readonly MediaInputOption[];
  readonly autoSelectedReference?: MediaReviewedInputReference;
  readonly selectionRequired: boolean;
  readonly error?: string;
}

export interface ScenePlanGenerationReferences {
  readonly researchInput: MediaReviewedInputReference;
  readonly scriptInput: MediaReviewedInputReference;
  readonly captionsInput: MediaReviewedInputReference;
}

export interface TimelineGenerationReferences {
  readonly audioInput: MediaReviewedInputReference;
  readonly captionsInput: MediaReviewedInputReference;
  readonly scenePlanInput: MediaReviewedInputReference;
}

export interface MediaInputDocumentMap {
  readonly audio_media: AudioMediaDocument;
  readonly captions_media: CaptionsMediaDocument;
  readonly scene_plan: ScenePlanDocument;
  readonly timeline: TimelineDocument;
}

export type MediaInputDocumentType = keyof MediaInputDocumentMap;

export interface UseMediaStageStudioInput {
  readonly project: ProjectDescriptor;
  readonly workflow: WorkflowSnapshotView;
  readonly stageId: string;
  readonly selectedRun?: StageRun;
  readonly mode: "desktop" | "demo";
  readonly onRefreshWorkspace: () => Promise<boolean>;
  readonly onRefreshStage: () => Promise<boolean>;
}

export interface MediaStageStudioController {
  readonly available: boolean;
  readonly unavailableReason?: string;
  readonly inputOptions: readonly MediaInputOptionGroup[];
  readonly document: NarraCutMediaDocument | null;
  readonly documentArtifactId?: string;
  readonly acceptedJob: MediaJobAcceptedResult | null;
  readonly currentJobId?: string;
  readonly currentJob: JobSnapshot | null;
  readonly lastSaveResult: MediaSaveResult | null;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly notice: string | null;
  readonly refresh: () => Promise<boolean>;
  readonly readInputDocument: <T extends MediaInputDocumentType>(
    reference: MediaReviewedInputReference,
    expectedType: T,
  ) => Promise<MediaInputDocumentMap[T] | null>;
  readonly enqueueAudio: (
    form: ValidatedImportForm,
    scriptInput: MediaReviewedInputReference,
  ) => Promise<boolean>;
  readonly enqueueCaptions: (
    form: ValidatedImportForm,
    scriptInput: MediaReviewedInputReference,
    audioInput: MediaReviewedInputReference,
    audioDurationMs: number,
  ) => Promise<boolean>;
  readonly generateScenePlan: (
    references: ScenePlanGenerationReferences,
  ) => Promise<boolean>;
  readonly generateTimeline: (
    references: TimelineGenerationReferences,
    canvas: TimelineCanvasInput,
    safeArea: TimelineSafeAreaInput,
  ) => Promise<boolean>;
  readonly saveScenePlan: (
    edits: readonly ScenePlanEdit[],
    summary: string,
  ) => Promise<boolean>;
  readonly saveTimeline: (
    edits: readonly TimelineEdit[],
    summary: string,
  ) => Promise<boolean>;
  readonly cancel: () => Promise<boolean>;
  readonly retry: () => Promise<boolean>;
  readonly clearError: () => void;
  readonly clearNotice: () => void;
}

async function resolveInputRequirement(
  project: ProjectDescriptor,
  workflow: WorkflowSnapshotView,
  requirement: MediaStageRequirement,
): Promise<MediaInputOptionGroup> {
  const blocked = (error: string): MediaInputOptionGroup => ({
    requirement,
    options: [],
    selectionRequired: false,
    error,
  });
  const stageState = workflow.stageStates.find(
    (state) => state.stageId === requirement.stageId,
  );
  if (!stageState) return blocked("工作流缺少该上游阶段状态。");
  if (stageState.status !== "approved" || !stageState.approvedRunId) {
    return blocked("上游阶段尚未形成当前有效的 approved 版本。");
  }

  try {
    const snapshot = await desktopGateway.loadStageStudio(
      project,
      requirement.stageId,
    );
    if (snapshot.stageId !== requirement.stageId || snapshot.mode !== "desktop") {
      return blocked("上游阶段快照身份或运行模式不匹配。");
    }
    const approvedRun = snapshot.runs.find(
      (run) => run.runId === stageState.approvedRunId,
    );
    if (!approvedRun || approvedRun.status !== "succeeded") {
      return blocked("approvedRunId 未指向成功完成的运行。");
    }
    if (
      approvedRun.projectId !== project.projectId ||
      approvedRun.stageId !== requirement.stageId
    ) {
      return blocked("批准运行与当前工程或上游阶段不匹配。");
    }

    const approvedReview = [...snapshot.reviews]
      .filter(
        (review) =>
          review.projectId === project.projectId &&
          review.stageId === requirement.stageId &&
          review.runId === approvedRun.runId &&
          review.decision === "approved",
      )
      .sort(
        (left, right) =>
          right.createdAt.localeCompare(left.createdAt) ||
          right.reviewId.localeCompare(left.reviewId),
      )[0];
    if (!approvedReview) {
      return blocked("批准运行没有匹配的 approved ReviewRecord。");
    }

    const artifacts = await desktopGateway.loadRunArtifacts(project, approvedRun);
    if (artifacts.runId !== approvedRun.runId || artifacts.truncated) {
      return blocked(
        artifacts.truncated
          ? "上游产物元数据超过读取上限，已按 fail-closed 停止选择。"
          : "上游产物集合与批准运行不匹配。",
      );
    }
    const candidates = artifacts.items.filter((artifact) =>
      requirement.artifactKinds.includes(artifact.kind),
    );
    if (candidates.length === 0) {
      return blocked(
        `批准运行没有 ${requirement.artifactKinds.join(" / ")} 类型产物。`,
      );
    }

    const rejectedReasons = new Set<string>();
    const options: MediaInputOption[] = [];
    for (const artifact of candidates) {
      const result = buildReviewedInputReference({
        expectedProjectId: project.projectId,
        expectedStageId: requirement.stageId,
        expectedArtifactKinds: requirement.artifactKinds,
        stageState,
        run: approvedRun,
        review: approvedReview,
        artifact: artifactIdentity(project, approvedRun, artifact),
      });
      if (!result.valid) {
        result.errors.forEach((reason) => rejectedReasons.add(reason));
        continue;
      }
      options.push({
        artifactId: artifact.artifactId,
        kind: artifact.kind,
        label: `${artifact.kind} · ${artifact.artifactId}`,
        reference: result.value,
      });
    }
    if (options.length === 0) {
      const reasons = [...rejectedReasons].slice(0, 3).join("；");
      return blocked(reasons || "匹配产物未通过审核引用完整性校验。");
    }

    return {
      requirement,
      options,
      autoSelectedReference:
        options.length === 1 ? options[0].reference : undefined,
      selectionRequired: options.length > 1,
      error:
        options.length > 1
          ? `找到 ${options.length} 个有效产物，请显式选择。`
          : undefined,
    };
  } catch (reason) {
    return blocked(safeReasonMessage(reason, "读取上游审核输入失败。"));
  }
}

function artifactIdentity(
  project: ProjectDescriptor,
  run: StageRun,
  artifact: WorkbenchArtifact,
) {
  return {
    projectId: project.projectId,
    stageId: run.stageId,
    runId: run.runId,
    artifactId: artifact.artifactId,
    kind: artifact.kind,
    contentHash: artifact.contentHash,
    provenance: artifact.provenance,
  };
}

function safeReasonMessage(reason: unknown, fallback: string): string {
  if (reason instanceof Error && reason.message.trim()) {
    return redactLocalPaths(reason.message);
  }
  return fallback;
}

function redactLocalPaths(message: string): string {
  return message
    .replace(/[a-z]:[\\/](?:[^\\/\s]+[\\/])*[^\\/\s]*/gi, "所选文件")
    .replace(/(^|\s)\/(?:[^/\s]+\/)*[^/\s]*/g, "$1所选文件");
}

function describeActionError(
  reason: unknown,
  fallback: string,
  sourcePath?: string,
): string {
  const message =
    isMediaCommandError(reason) || isJobCommandError(reason)
      ? reason.message
      : reason instanceof Error && reason.message.trim()
        ? reason.message
        : fallback;
  const withoutKnownSource = sourcePath
    ? message.split(sourcePath).join("所选文件")
    : message;
  return redactLocalPaths(withoutKnownSource);
}

function isActiveJob(job: JobSnapshot): boolean {
  return ["queued", "running", "retrying"].includes(job.status);
}

function isRetryableTerminalJob(job: JobSnapshot): boolean {
  return job.status === "failed" || job.status === "canceled";
}

function jobIdFromSnapshot(snapshot: JobSnapshot): string | undefined {
  const jobId = snapshot.job.jobId;
  return typeof jobId === "string" && jobId.trim() ? jobId : undefined;
}

function portableId(prefix: string): string {
  return `${prefix}${crypto.randomUUID().replace(/-/g, "").slice(0, 20)}`;
}

function importFormError(form: ValidatedImportForm): string | undefined {
  if (!form.sourcePath.trim()) return "导入表单缺少本地源文件。";
  const rights = form.rights;
  if (
    !rights.author.trim() ||
    !rights.rightsStatement.trim() ||
    rights.authorizationRecords.length === 0 ||
    rights.voiceAuthorization.applicability !== "not_applicable" ||
    rights.voiceAuthorization.reason !== "not_voice_clone"
  ) {
    return "导入表单的作者、权利声明或声音授权无效。";
  }
  if (
    rights.ownership === "licensed" &&
    (!rights.licenseId.trim() || !rights.attributionText.trim())
  ) {
    return "许可素材缺少 licenseId 或署名文本。";
  }
  if (rights.ownership === "self_recorded" && rights.licenseId.trim()) {
    return "自行录制素材不能携带第三方 licenseId。";
  }
  return undefined;
}

function referenceMatchesStage(
  reference: MediaReviewedInputReference,
  stageId: string,
): boolean {
  return (
    reference.stageId === stageId &&
    Boolean(reference.runId) &&
    Boolean(reference.artifactId) &&
    /^sha256:[a-f\d]{64}$/i.test(reference.contentHash) &&
    Boolean(reference.reviewRecordId) &&
    reference.claimIds.length > 0 &&
    reference.evidenceRefs.length > 0
  );
}

function validCanvasAndSafeArea(
  canvas: TimelineCanvasInput,
  safeArea: TimelineSafeAreaInput,
): boolean {
  return (
    Number.isInteger(canvas.width) &&
    canvas.width > 0 &&
    Number.isInteger(canvas.height) &&
    canvas.height > 0 &&
    Number.isInteger(canvas.frameRateNumerator) &&
    canvas.frameRateNumerator > 0 &&
    Number.isInteger(canvas.frameRateDenominator) &&
    canvas.frameRateDenominator > 0 &&
    Number.isFinite(safeArea.x) &&
    safeArea.x >= 0 &&
    Number.isFinite(safeArea.y) &&
    safeArea.y >= 0 &&
    Number.isFinite(safeArea.width) &&
    safeArea.width > 0 &&
    Number.isFinite(safeArea.height) &&
    safeArea.height > 0 &&
    safeArea.x + safeArea.width <= canvas.width &&
    safeArea.y + safeArea.height <= canvas.height
  );
}

const AUDIO_IMPORT_LIMITS = Object.freeze({ maxBytes: 64 * 1024 * 1024 });
const CAPTIONS_IMPORT_LIMITS = Object.freeze({
  maxBytes: 4 * 1024 * 1024,
  maxCueCount: 10_000,
  maxCueTextBytes: 8_000,
});

const DOCUMENT_KIND_BY_STAGE = {
  audio: "voice_audio",
  captions: "captions",
  scene_plan: "scene_plan",
  timeline: "timeline",
} as const;

const DOCUMENT_TYPE_BY_STAGE = {
  audio: "audio_media",
  captions: "captions_media",
  scene_plan: "scene_plan",
  timeline: "timeline",
} as const;

const INPUT_STAGE_BY_DOCUMENT_TYPE: Record<MediaInputDocumentType, string> = {
  audio_media: "audio",
  captions_media: "captions",
  scene_plan: "scene_plan",
  timeline: "timeline",
};

async function readVerifiedDocument(
  project: ProjectDescriptor,
  artifactId: string,
  expectedContentHash: string | undefined,
  expectedRunId: string,
  stageId: MediaStageId,
  allowEnvelopeHash = false,
): Promise<NarraCutMediaDocument> {
  if (!expectedContentHash && !allowEnvelopeHash) {
    throw new Error("正式媒体产物缺少 contentHash。");
  }
  if (
    expectedContentHash &&
    !/^sha256:[a-f\d]{64}$/i.test(expectedContentHash)
  ) {
    throw new Error("正式媒体产物的 contentHash 格式无效。");
  }
  const result = await mediaCommands.getDocument({
    projectPath: project.projectPath,
    expectedProjectId: project.projectId,
    artifactId,
  });
  if (
    result.ownerProjectId !== project.projectId ||
    result.artifactId !== artifactId ||
    (expectedContentHash
      ? result.contentHash !== expectedContentHash
      : !/^sha256:[a-f\d]{64}$/i.test(result.contentHash))
  ) {
    throw new Error("媒体文档响应与产物元数据不匹配。");
  }
  const document = narrowMediaDocument(result.document);
  if (
    !document ||
    document.documentType !== DOCUMENT_TYPE_BY_STAGE[stageId] ||
    document.projectId !== project.projectId ||
    document.runId !== expectedRunId
  ) {
    throw new Error("媒体文档内容与当前工程、运行或阶段类型不匹配。");
  }
  return document;
}

export function useMediaStageStudio({
  project,
  workflow,
  stageId,
  selectedRun,
  mode,
  onRefreshWorkspace,
  onRefreshStage,
}: UseMediaStageStudioInput): MediaStageStudioController {
  const [inputRequestGate] = useState(createRequestGate);
  const [documentRequestGate] = useState(createRequestGate);
  const [inputDocumentRequestGate] = useState(createRequestGate);
  const [jobRequestGate] = useState(createRequestGate);
  const [terminalRequestGate] = useState(createRequestGate);
  const [actionRequestGate] = useState(createRequestGate);
  const [inputOptions, setInputOptions] = useState<
    readonly MediaInputOptionGroup[]
  >([]);
  const [inputLoading, setInputLoading] = useState(false);
  const [document, setDocument] = useState<NarraCutMediaDocument | null>(null);
  const [documentArtifactId, setDocumentArtifactId] = useState<string>();
  const [documentLoading, setDocumentLoading] = useState(false);
  const [inputDocumentLoading, setInputDocumentLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [acceptedJob, setAcceptedJob] =
    useState<MediaJobAcceptedResult | null>(null);
  const [currentJobId, setCurrentJobId] = useState<string>();
  const [currentJob, setCurrentJob] = useState<JobSnapshot | null>(null);
  const [lastSaveResult, setLastSaveResult] =
    useState<MediaSaveResult | null>(null);
  const [actionLabel, setActionLabel] = useState<string | null>(null);
  const actionInFlightRef = useRef(false);
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const pollFailureCountRef = useRef(0);
  const terminalHandledRef = useRef(new Set<string>());
  const inputDocumentCacheRef = useRef(
    new Map<string, NarraCutMediaDocument>(),
  );
  const [pollRevision, setPollRevision] = useState(0);
  const available = isMediaStageId(stageId) && mode === "desktop";
  const unavailableReason = !isMediaStageId(stageId)
    ? "当前阶段不属于媒体工作区。"
    : mode === "demo"
      ? "浏览器演示模式仅提供只读预览，媒体命令不可用。"
      : undefined;
  const loadInputOptions = useCallback(
    async (): Promise<boolean> => {
      const request = inputRequestGate.begin();
      const requirements = requirementsForStage(stageId) ?? [];
      if (!isMediaStageId(stageId)) {
        setInputOptions([]);
        setInputLoading(false);
        return false;
      }
      if (mode === "demo") {
        setInputOptions(
          requirements.map((requirement) => ({
            requirement,
            options: [],
            selectionRequired: false,
            error: "浏览器演示模式不会读取本地审核产物。",
          })),
        );
        setInputLoading(false);
        return false;
      }

      setInputLoading(true);
      setInputOptions(
        requirements.map((requirement) => ({
          requirement,
          options: [],
          selectionRequired: false,
          error: "正在核验批准运行与审核记录…",
        })),
      );
      const groups = await Promise.all(
        requirements.map((requirement) =>
          resolveInputRequirement(project, workflow, requirement),
        ),
      );
      if (!request.isCurrent()) return false;
      setInputOptions(groups);
      setInputLoading(false);
      return true;
    },
    [inputRequestGate, mode, project, stageId, workflow],
  );

  useEffect(() => {
    void loadInputOptions();
    return () => inputRequestGate.invalidate();
  }, [inputRequestGate, loadInputOptions]);

  const readInputDocument = useCallback(
    async <T extends MediaInputDocumentType>(
      reference: MediaReviewedInputReference,
      expectedType: T,
    ): Promise<MediaInputDocumentMap[T] | null> => {
      if (!available) return null;
      const knownOption = inputOptions.some((group) =>
        group.options.some(
          (option) =>
            option.reference.stageId === reference.stageId &&
            option.reference.runId === reference.runId &&
            option.reference.artifactId === reference.artifactId &&
            option.reference.contentHash === reference.contentHash &&
            option.reference.reviewRecordId === reference.reviewRecordId,
        ),
      );
      if (
        !knownOption ||
        reference.stageId !== INPUT_STAGE_BY_DOCUMENT_TYPE[expectedType]
      ) {
        setError("只能读取当前媒体阶段已核验的上游文档。");
        return null;
      }

      const cacheKey = [
        expectedType,
        reference.runId,
        reference.artifactId,
        reference.contentHash,
      ].join(":");
      const cached = inputDocumentCacheRef.current.get(cacheKey);
      if (cached?.documentType === expectedType) {
        return cached as MediaInputDocumentMap[T];
      }

      const request = inputDocumentRequestGate.begin();
      setInputDocumentLoading(true);
      setError(null);
      try {
        const result = await mediaCommands.getDocument({
          projectPath: project.projectPath,
          expectedProjectId: project.projectId,
          artifactId: reference.artifactId,
        });
        if (!request.isCurrent()) return null;
        const nextDocument = narrowMediaDocument(result.document);
        if (
          result.ownerProjectId !== project.projectId ||
          result.artifactId !== reference.artifactId ||
          result.contentHash !== reference.contentHash ||
          !nextDocument ||
          nextDocument.documentType !== expectedType ||
          nextDocument.projectId !== project.projectId ||
          nextDocument.runId !== reference.runId
        ) {
          throw new Error("上游媒体文档与审核引用的身份、哈希或类型不匹配。");
        }
        inputDocumentCacheRef.current.set(cacheKey, nextDocument);
        return nextDocument as MediaInputDocumentMap[T];
      } catch (reason) {
        if (request.isCurrent()) {
          setError(describeActionError(reason, "读取上游媒体文档失败。"));
        }
        return null;
      } finally {
        if (request.isCurrent()) setInputDocumentLoading(false);
      }
    },
    [available, inputDocumentRequestGate, inputOptions, project],
  );

  useEffect(
    () => () => inputDocumentRequestGate.invalidate(),
    [inputDocumentRequestGate],
  );

  const loadCurrentDocument = useCallback(async (): Promise<boolean> => {
    const request = documentRequestGate.begin();
    if (!isMediaStageId(stageId) || mode === "demo" || !selectedRun) {
      setDocument(null);
      setDocumentArtifactId(undefined);
      setDocumentLoading(false);
      return false;
    }
    if (
      selectedRun.projectId !== project.projectId ||
      selectedRun.stageId !== stageId ||
      selectedRun.status !== "succeeded"
    ) {
      setDocument(null);
      setDocumentArtifactId(undefined);
      setDocumentLoading(false);
      return false;
    }

    setDocumentLoading(true);
    setDocument(null);
    setDocumentArtifactId(undefined);
    setError(null);
    try {
      const artifacts = await desktopGateway.loadRunArtifacts(project, selectedRun);
      if (!request.isCurrent()) return false;
      if (artifacts.runId !== selectedRun.runId || artifacts.truncated) {
        throw new Error(
          artifacts.truncated
            ? "当前运行产物元数据超过读取上限，无法安全确定正式媒体文档。"
            : "当前运行产物集合与所选运行不匹配。",
        );
      }
      const expectedKind = DOCUMENT_KIND_BY_STAGE[stageId];
      const candidates = artifacts.items.filter(
        (artifact) =>
          artifact.kind === expectedKind &&
          artifact.contentAvailable &&
          selectedRun.artifactIds.includes(artifact.artifactId),
      );
      if (candidates.length !== 1) {
        throw new Error(
          candidates.length === 0
            ? `当前运行没有唯一可读取的 ${expectedKind} 正式产物。`
            : `当前运行包含 ${candidates.length} 个 ${expectedKind} 产物，已拒绝静默选择。`,
        );
      }
      const artifact = candidates[0];
      const nextDocument = await readVerifiedDocument(
        project,
        artifact.artifactId,
        artifact.contentHash,
        selectedRun.runId,
        stageId,
      );
      if (!request.isCurrent()) return false;
      setDocument(nextDocument);
      setDocumentArtifactId(artifact.artifactId);
      return true;
    } catch (reason) {
      if (!request.isCurrent()) return false;
      setDocument(null);
      setDocumentArtifactId(undefined);
      setError(safeReasonMessage(reason, "读取媒体文档失败。"));
      return false;
    } finally {
      if (request.isCurrent()) setDocumentLoading(false);
    }
  }, [documentRequestGate, mode, project, selectedRun, stageId]);

  useEffect(() => {
    void loadCurrentDocument();
    return () => documentRequestGate.invalidate();
  }, [documentRequestGate, loadCurrentDocument]);

  const clearPollTimer = useCallback(() => {
    if (pollTimerRef.current !== undefined) {
      clearTimeout(pollTimerRef.current);
      pollTimerRef.current = undefined;
    }
  }, []);

  const reloadMediaTruth = useCallback(async (): Promise<boolean> => {
    const parentRefreshes = await Promise.allSettled([
      onRefreshWorkspace(),
      onRefreshStage(),
    ]);
    const [inputsLoaded, documentLoaded] = await Promise.all([
      loadInputOptions(),
      loadCurrentDocument(),
    ]);
    return (
      parentRefreshes.every(
        (result) => result.status === "fulfilled" && result.value,
      ) &&
      (inputsLoaded || documentLoaded)
    );
  }, [loadCurrentDocument, loadInputOptions, onRefreshStage, onRefreshWorkspace]);

  const handleTerminalJob = useCallback(
    async (jobId: string, snapshot: JobSnapshot): Promise<void> => {
      if (terminalHandledRef.current.has(jobId)) return;
      terminalHandledRef.current.add(jobId);
      const request = terminalRequestGate.begin();
      clearPollTimer();
      await reloadMediaTruth();
      if (!request.isCurrent()) return;

      if (snapshot.status === "failed") {
        setError(
          redactLocalPaths(
            snapshot.lastError?.message ||
              snapshot.message ||
              `媒体任务 ${jobId} 执行失败。`,
          ),
        );
        setNotice("任务终态已同步；失败运行与诊断仍保留，可明确重试。");
      } else if (snapshot.status === "canceled") {
        setError(
          redactLocalPaths(snapshot.message || `媒体任务 ${jobId} 已取消。`),
        );
        setNotice("取消结果已同步；原运行历史未被覆盖。");
      } else {
        setError(null);
        setNotice(`媒体任务 ${jobId} 已完成，工作区与媒体文档已重读。`);
      }
    },
    [clearPollTimer, reloadMediaTruth, terminalRequestGate],
  );

  const pollJob = useCallback(
    async (jobId: string): Promise<void> => {
      const request = jobRequestGate.begin();
      try {
        const snapshot = await jobCommands.get({
          projectPath: project.projectPath,
          expectedProjectId: project.projectId,
          jobId,
        });
        if (!request.isCurrent()) return;
        pollFailureCountRef.current = 0;
        setCurrentJobId(jobId);
        setCurrentJob(snapshot);
        if (!isActiveJob(snapshot)) await handleTerminalJob(jobId, snapshot);
      } catch (reason) {
        if (!request.isCurrent()) return;
        pollFailureCountRef.current += 1;
        setError(describeActionError(reason, "读取媒体任务状态失败。"));
        if (pollFailureCountRef.current < 5) {
          setPollRevision((revision) => revision + 1);
        }
      }
    },
    [handleTerminalJob, jobRequestGate, project],
  );

  useEffect(() => {
    clearPollTimer();
    if (!currentJobId || !currentJob || !isActiveJob(currentJob)) return;
    pollTimerRef.current = setTimeout(() => {
      void pollJob(currentJobId);
    }, pollFailureCountRef.current > 0 ? 1_500 : 750);
    return clearPollTimer;
  }, [
    clearPollTimer,
    currentJob,
    currentJobId,
    pollJob,
    pollRevision,
  ]);

  useEffect(() => {
    clearPollTimer();
    jobRequestGate.invalidate();
    terminalRequestGate.invalidate();
    actionRequestGate.invalidate();
    inputDocumentRequestGate.invalidate();
    inputDocumentCacheRef.current.clear();
    terminalHandledRef.current.clear();
    pollFailureCountRef.current = 0;
    actionInFlightRef.current = false;
    setAcceptedJob(null);
    setCurrentJobId(undefined);
    setCurrentJob(null);
    setLastSaveResult(null);
    setActionLabel(null);
    setInputDocumentLoading(false);
    setNotice(null);
    setError(null);
    return () => {
      clearPollTimer();
      jobRequestGate.invalidate();
      terminalRequestGate.invalidate();
      actionRequestGate.invalidate();
      inputDocumentRequestGate.invalidate();
    };
  }, [
    actionRequestGate,
    clearPollTimer,
    inputDocumentRequestGate,
    jobRequestGate,
    project.projectId,
    stageId,
    terminalRequestGate,
  ]);

  const enqueueMediaJob = useCallback(
    async (
      expectedStageId: MediaStageId,
      label: string,
      sourcePath: string | undefined,
      createJob: (
        runId: string,
        idempotencyKey: string,
      ) => Promise<MediaJobAcceptedResult>,
    ): Promise<boolean> => {
      if (
        !available ||
        stageId !== expectedStageId ||
        actionInFlightRef.current ||
        (currentJob !== null && isActiveJob(currentJob))
      ) {
        return false;
      }
      const runId = portableId(`run_${expectedStageId}_ui_`);
      const idempotencyKey = portableId("idem_media_ui_");
      const request = actionRequestGate.begin();
      actionInFlightRef.current = true;
      setActionLabel(label);
      setError(null);
      setNotice(null);
      try {
        const accepted = await createJob(runId, idempotencyKey);
        if (!request.isCurrent()) return false;
        if (
          accepted.ownerProjectId !== project.projectId ||
          accepted.runId !== runId ||
          !accepted.jobId.trim()
        ) {
          throw new Error("媒体入队响应与本次运行意图不匹配。");
        }
        terminalHandledRef.current.delete(accepted.jobId);
        pollFailureCountRef.current = 0;
        setAcceptedJob(accepted);
        setCurrentJobId(accepted.jobId);
        setCurrentJob(null);
        setLastSaveResult(null);
        setNotice(
          `媒体任务 ${accepted.jobId} 已接受，新运行 ${accepted.runId} 不会覆盖历史。`,
        );
        await pollJob(accepted.jobId);
        return request.isCurrent();
      } catch (reason) {
        if (request.isCurrent()) {
          setError(
            describeActionError(reason, "创建媒体任务失败。", sourcePath),
          );
        }
        return false;
      } finally {
        if (request.isCurrent()) {
          actionInFlightRef.current = false;
          setActionLabel(null);
        }
      }
    },
    [actionRequestGate, available, currentJob, pollJob, project.projectId, stageId],
  );

  const enqueueAudio = useCallback(
    async (
      form: ValidatedImportForm,
      scriptInput: MediaReviewedInputReference,
    ): Promise<boolean> => {
      const formError = importFormError(form);
      if (formError || !referenceMatchesStage(scriptInput, "script")) {
        setError(formError || "音频导入需要完整的已批准脚本引用。");
        return false;
      }
      return enqueueMediaJob(
        "audio",
        "正在创建 WAV 导入任务…",
        form.sourcePath,
        (runId, idempotencyKey) =>
          mediaCommands.enqueueAudioImport({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            sourcePath: form.sourcePath,
            scriptInput,
            rights: form.rights,
            limits: AUDIO_IMPORT_LIMITS,
            idempotencyKey,
          }),
      );
    },
    [enqueueMediaJob, project],
  );

  const enqueueCaptions = useCallback(
    async (
      form: ValidatedImportForm,
      scriptInput: MediaReviewedInputReference,
      audioInput: MediaReviewedInputReference,
      audioDurationMs: number,
    ): Promise<boolean> => {
      const formError = importFormError(form);
      if (
        formError ||
        !referenceMatchesStage(scriptInput, "script") ||
        !referenceMatchesStage(audioInput, "audio") ||
        !Number.isFinite(audioDurationMs) ||
        audioDurationMs <= 0
      ) {
        setError(
          formError ||
            "字幕导入需要已批准脚本、已批准音频及有效音频时长。",
        );
        return false;
      }
      return enqueueMediaJob(
        "captions",
        "正在创建 SRT 导入任务…",
        form.sourcePath,
        (runId, idempotencyKey) =>
          mediaCommands.enqueueCaptionsImport({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            sourcePath: form.sourcePath,
            scriptInput,
            audioInput,
            audioDurationMs,
            rights: form.rights,
            limits: CAPTIONS_IMPORT_LIMITS,
            idempotencyKey,
          }),
      );
    },
    [enqueueMediaJob, project],
  );

  const generateScenePlan = useCallback(
    async (references: ScenePlanGenerationReferences): Promise<boolean> => {
      if (
        !referenceMatchesStage(references.researchInput, "research") ||
        !referenceMatchesStage(references.scriptInput, "script") ||
        !referenceMatchesStage(references.captionsInput, "captions")
      ) {
        setError("场景规划需要 research、script、captions 的完整批准引用。");
        return false;
      }
      return enqueueMediaJob(
        "scene_plan",
        "正在创建场景规划任务…",
        undefined,
        (runId, idempotencyKey) =>
          mediaCommands.generateScenePlan({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            ...references,
            idempotencyKey,
          }),
      );
    },
    [enqueueMediaJob, project],
  );

  const generateTimeline = useCallback(
    async (
      references: TimelineGenerationReferences,
      canvas: TimelineCanvasInput,
      safeArea: TimelineSafeAreaInput,
    ): Promise<boolean> => {
      if (
        !referenceMatchesStage(references.audioInput, "audio") ||
        !referenceMatchesStage(references.captionsInput, "captions") ||
        !referenceMatchesStage(references.scenePlanInput, "scene_plan") ||
        !validCanvasAndSafeArea(canvas, safeArea)
      ) {
        setError("时间轴需要三项完整批准引用及画布内的有效安全区。");
        return false;
      }
      return enqueueMediaJob(
        "timeline",
        "正在创建时间轴任务…",
        undefined,
        (runId, idempotencyKey) =>
          mediaCommands.generateTimeline({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            ...references,
            canvas,
            safeArea,
            idempotencyKey,
          }),
      );
    },
    [enqueueMediaJob, project],
  );

  const saveMediaDocument = useCallback(
    async (
      expectedStageId: "scene_plan" | "timeline",
      label: string,
      summary: string,
      save: (
        runId: string,
        idempotencyKey: string,
        baseArtifactId: string,
      ) => Promise<MediaSaveResult>,
    ): Promise<boolean> => {
      if (
        !available ||
        stageId !== expectedStageId ||
        !document ||
        !documentArtifactId ||
        actionInFlightRef.current ||
        (currentJob !== null && isActiveJob(currentJob))
      ) {
        return false;
      }
      const changeSummary = summary.trim();
      if (!changeSummary) {
        setError("保存新版本前必须填写变更摘要。");
        return false;
      }

      const runId = portableId(`run_${expectedStageId}_edit_ui_`);
      const idempotencyKey = portableId("idem_media_edit_ui_");
      const request = actionRequestGate.begin();
      actionInFlightRef.current = true;
      setActionLabel(label);
      setError(null);
      setNotice(null);
      try {
        const result = await save(runId, idempotencyKey, documentArtifactId);
        if (!request.isCurrent()) return false;
        if (
          result.ownerProjectId !== project.projectId ||
          result.runId !== runId ||
          !result.artifactId.trim()
        ) {
          throw new Error("媒体保存响应与本次新版本意图不匹配。");
        }
        setLastSaveResult(result);
        setAcceptedJob(null);

        const [parentRefreshes, nextDocument] = await Promise.all([
          Promise.allSettled([onRefreshWorkspace(), onRefreshStage()]),
          readVerifiedDocument(
            project,
            result.artifactId,
            undefined,
            result.runId,
            expectedStageId,
            true,
          ),
        ]);
        if (!request.isCurrent()) return false;
        setDocument(nextDocument);
        setDocumentArtifactId(result.artifactId);
        const refreshFailed = parentRefreshes.some(
          (refreshResult) =>
            refreshResult.status === "rejected" || !refreshResult.value,
        );
        const changed = result.changedSceneIds.length;
        const stale = result.staleBecauseStageIds.length
          ? `；下游过期：${result.staleBecauseStageIds.join("、")}`
          : "";
        setNotice(
          `已保存新产物 ${result.artifactId}，影响 ${changed} 个场景${stale}。`,
        );
        if (refreshFailed) {
          setError("新媒体版本已保存，但工作区或阶段刷新未完全成功，请手动刷新确认。");
        }
        return true;
      } catch (reason) {
        if (request.isCurrent()) {
          setError(describeActionError(reason, "保存媒体新版本失败。"));
        }
        return false;
      } finally {
        if (request.isCurrent()) {
          actionInFlightRef.current = false;
          setActionLabel(null);
        }
      }
    },
    [
      actionRequestGate,
      available,
      currentJob,
      document,
      documentArtifactId,
      onRefreshStage,
      onRefreshWorkspace,
      project,
      stageId,
    ],
  );

  const saveScenePlan = useCallback(
    async (
      edits: readonly ScenePlanEdit[],
      summary: string,
    ): Promise<boolean> => {
      if (
        document?.documentType !== "scene_plan" ||
        edits.length === 0 ||
        edits.length > 1_000 ||
        edits.some((edit) => !validateSceneEdit(document, edit).valid)
      ) {
        setError("场景编辑为空、超过上限或存在越界操作。");
        return false;
      }
      return saveMediaDocument(
        "scene_plan",
        "正在保存场景规划新版本…",
        summary,
        (runId, idempotencyKey, baseArtifactId) =>
          mediaCommands.saveScenePlan({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            baseArtifactId,
            edits: edits as readonly [ScenePlanEdit, ...ScenePlanEdit[]],
            changeSummary: summary.trim(),
            idempotencyKey,
          }),
      );
    },
    [document, project, saveMediaDocument],
  );

  const saveTimeline = useCallback(
    async (
      edits: readonly TimelineEdit[],
      summary: string,
    ): Promise<boolean> => {
      if (
        document?.documentType !== "timeline" ||
        edits.length === 0 ||
        edits.length > 1_000 ||
        edits.some((edit) => !validateTimelineEdit(document, edit).valid)
      ) {
        setError("时间轴编辑为空、超过上限或存在越界操作。");
        return false;
      }
      return saveMediaDocument(
        "timeline",
        "正在保存时间轴新版本…",
        summary,
        (runId, idempotencyKey, baseArtifactId) =>
          mediaCommands.saveTimeline({
            projectPath: project.projectPath,
            expectedProjectId: project.projectId,
            runId,
            baseArtifactId,
            edits: edits as readonly [TimelineEdit, ...TimelineEdit[]],
            changeSummary: summary.trim(),
            idempotencyKey,
          }),
      );
    },
    [document, project, saveMediaDocument],
  );

  const cancel = useCallback(async (): Promise<boolean> => {
    if (
      !available ||
      !currentJobId ||
      !currentJob ||
      !isActiveJob(currentJob) ||
      actionInFlightRef.current
    ) {
      return false;
    }
    const request = actionRequestGate.begin();
    actionInFlightRef.current = true;
    clearPollTimer();
    jobRequestGate.invalidate();
    setActionLabel("正在请求取消媒体任务…");
    setError(null);
    try {
      const snapshot = await jobCommands.cancel({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        jobId: currentJobId,
        message: "用户从媒体工作区请求停止当前任务。",
      });
      if (!request.isCurrent()) return false;
      const returnedJobId = jobIdFromSnapshot(snapshot);
      if (returnedJobId && returnedJobId !== currentJobId) {
        throw new Error("取消响应与当前媒体任务不匹配。");
      }
      setCurrentJob(snapshot);
      if (isActiveJob(snapshot)) {
        setNotice(`任务 ${currentJobId} 的取消请求已记录，正在等待安全终结。`);
      } else {
        await handleTerminalJob(currentJobId, snapshot);
      }
      return request.isCurrent();
    } catch (reason) {
      if (request.isCurrent()) {
        setError(describeActionError(reason, "取消媒体任务失败。"));
      }
      return false;
    } finally {
      if (request.isCurrent()) {
        actionInFlightRef.current = false;
        setActionLabel(null);
      }
    }
  }, [
    actionRequestGate,
    available,
    clearPollTimer,
    currentJob,
    currentJobId,
    handleTerminalJob,
    jobRequestGate,
    project,
  ]);

  const retry = useCallback(async (): Promise<boolean> => {
    if (
      !available ||
      !currentJobId ||
      !currentJob ||
      !isRetryableTerminalJob(currentJob) ||
      actionInFlightRef.current
    ) {
      return false;
    }
    const newRunId = portableId(`run_${stageId}_retry_ui_`);
    const idempotencyKey = portableId("idem_media_retry_ui_");
    const sourceJobId = currentJobId;
    const request = actionRequestGate.begin();
    actionInFlightRef.current = true;
    clearPollTimer();
    jobRequestGate.invalidate();
    setActionLabel("正在创建全新的媒体重试…");
    setError(null);
    setNotice(null);
    try {
      const snapshot = await jobCommands.retry({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        sourceJobId,
        newRunId,
        idempotencyKey,
      });
      if (!request.isCurrent()) return false;
      const newJobId = jobIdFromSnapshot(snapshot);
      const snapshotRunId = snapshot.job.stageRunId;
      if (
        !newJobId ||
        newJobId === sourceJobId ||
        (typeof snapshotRunId === "string" && snapshotRunId !== newRunId)
      ) {
        throw new Error("重试响应没有确认全新的 jobId 与 runId。");
      }
      terminalHandledRef.current.delete(newJobId);
      pollFailureCountRef.current = 0;
      setAcceptedJob(null);
      setCurrentJobId(newJobId);
      setCurrentJob(snapshot);
      setNotice(`已从 ${sourceJobId} 创建重试任务 ${newJobId}，新运行 ${newRunId}。`);
      if (!isActiveJob(snapshot)) await handleTerminalJob(newJobId, snapshot);
      return request.isCurrent();
    } catch (reason) {
      if (request.isCurrent()) {
        setError(describeActionError(reason, "创建媒体重试失败。"));
      }
      return false;
    } finally {
      if (request.isCurrent()) {
        actionInFlightRef.current = false;
        setActionLabel(null);
      }
    }
  }, [
    actionRequestGate,
    available,
    clearPollTimer,
    currentJob,
    currentJobId,
    handleTerminalJob,
    jobRequestGate,
    project,
    stageId,
  ]);

  const refresh = useCallback(async (): Promise<boolean> => {
    if (!available || actionInFlightRef.current) return false;
    const request = actionRequestGate.begin();
    actionInFlightRef.current = true;
    setActionLabel("正在重读媒体工作区真相…");
    setError(null);
    try {
      const refreshed = await reloadMediaTruth();
      if (request.isCurrent() && !refreshed) {
        setError("媒体工作区未能完整重读，请检查阶段输入或稍后重试。");
      }
      return request.isCurrent() && refreshed;
    } finally {
      if (request.isCurrent()) {
        actionInFlightRef.current = false;
        setActionLabel(null);
      }
    }
  }, [actionRequestGate, available, reloadMediaTruth]);
  const clearError = useCallback(() => setError(null), []);
  const clearNotice = useCallback(() => setNotice(null), []);

  return {
    available,
    unavailableReason,
    inputOptions,
    document,
    documentArtifactId,
    acceptedJob,
    currentJobId,
    currentJob,
    lastSaveResult,
    busyLabel:
      actionLabel ??
      (inputLoading
        ? "正在核验已批准的上游输入…"
        : documentLoading
          ? "正在读取当前媒体文档…"
          : inputDocumentLoading
            ? "正在读取已批准的上游媒体文档…"
            : null),
    error,
    notice,
    refresh,
    readInputDocument,
    enqueueAudio,
    enqueueCaptions,
    generateScenePlan,
    generateTimeline,
    saveScenePlan,
    saveTimeline,
    cancel,
    retry,
    clearError,
    clearNotice,
  };
}
