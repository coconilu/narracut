import { useCallback, useEffect, useRef, useState } from "react";
import type {
  JobSnapshot,
  ProjectDescriptor,
  RenderJobAcceptedResult,
  RenderResult,
  RendererCapabilitiesResult,
  RendererConfig,
  RendererTimelineInputReference,
  SceneSnapshot,
  TimelineDocument,
} from "@narracut/contracts";
import { desktopGateway } from "../../../lib/desktop-gateway";
import { isJobCommandError, jobCommands } from "../../../lib/job-commands";
import { mediaCommands } from "../../../lib/media-commands";
import { isRendererCommandError, rendererCommands } from "../../../lib/renderer-commands";
import type { WorkflowSnapshotView } from "../../../lib/workflow-commands";
import { narrowMediaDocument } from "../media/media-stage-model.js";
import { defaultRenderConfig, resolveApprovedTimelineCandidate } from "./renderer-stage-model.js";

export interface UseRendererStageInput {
  readonly project: ProjectDescriptor;
  readonly workflow: WorkflowSnapshotView;
  readonly stageId: string;
  readonly mode: "desktop" | "demo";
  readonly onRefreshWorkspace: () => Promise<boolean>;
  readonly onRefreshStage: () => Promise<boolean>;
}

export interface RendererStageController {
  readonly active: boolean;
  readonly available: boolean;
  readonly fallbackReason?: string;
  readonly capabilities: RendererCapabilitiesResult | null;
  readonly timelineInput: RendererTimelineInputReference | null;
  readonly timeline: TimelineDocument | null;
  readonly config: RendererConfig | null;
  readonly selectedSceneId?: string;
  readonly snapshot: SceneSnapshot | null;
  readonly acceptedJob: RenderJobAcceptedResult | null;
  readonly currentJob: JobSnapshot | null;
  readonly result: RenderResult | null;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly notice: string | null;
  readonly refresh: () => Promise<boolean>;
  readonly selectScene: (sceneId: string) => void;
  readonly setPreset: (preset: RendererConfig["preset"]) => void;
  readonly setCrf: (crf: number) => void;
  readonly enqueueScene: () => Promise<boolean>;
  readonly enqueueTimeline: () => Promise<boolean>;
  readonly cancel: () => Promise<boolean>;
  readonly retry: () => Promise<boolean>;
  readonly clearError: () => void;
  readonly clearNotice: () => void;
}

const ACTIVE_JOB_STATUSES = new Set(["queued", "running", "retrying"]);

function activeJob(job: JobSnapshot | null): boolean {
  return Boolean(job && ACTIVE_JOB_STATUSES.has(job.status));
}

function jobId(snapshot: JobSnapshot): string | undefined {
  const value = snapshot.job.jobId;
  return typeof value === "string" && value ? value : undefined;
}

function portableId(prefix: string): string {
  return `${prefix}${crypto.randomUUID().replace(/-/g, "").slice(0, 20)}`;
}

function reasonMessage(reason: unknown, fallback: string): string {
  const message = isRendererCommandError(reason) || isJobCommandError(reason)
    ? reason.message
    : typeof reason === "object" && reason !== null && "message" in reason && typeof reason.message === "string" && reason.message.trim()
      ? reason.message
    : reason instanceof Error && reason.message.trim()
      ? reason.message
      : fallback;
  return message.replace(/[a-z]:[\\/](?:[^\\/\s]+[\\/])*[^\\/\s]*/gi, "本地路径");
}

export function useRendererStage({
  project,
  workflow,
  stageId,
  mode,
  onRefreshWorkspace,
  onRefreshStage,
}: UseRendererStageInput): RendererStageController {
  const [capabilities, setCapabilities] = useState<RendererCapabilitiesResult | null>(null);
  const [timelineInput, setTimelineInput] = useState<RendererTimelineInputReference | null>(null);
  const [timeline, setTimeline] = useState<TimelineDocument | null>(null);
  const [config, setConfig] = useState<RendererConfig | null>(null);
  const [selectedSceneId, setSelectedSceneId] = useState<string>();
  const [snapshot, setSnapshot] = useState<SceneSnapshot | null>(null);
  const [acceptedJob, setAcceptedJob] = useState<RenderJobAcceptedResult | null>(null);
  const [currentJob, setCurrentJob] = useState<JobSnapshot | null>(null);
  const [result, setResult] = useState<RenderResult | null>(null);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pollRevision, setPollRevision] = useState(0);
  const refreshGeneration = useRef(0);
  const snapshotGeneration = useRef(0);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const terminalHandled = useRef(new Set<string>());
  const active = stageId === "render";
  const available = active && mode === "desktop" && capabilities?.available === true && capabilities.supported;
  const fallbackReason = !active
    ? "当前不是渲染阶段。"
    : mode === "demo"
      ? "浏览器演示模式不会探测或执行本机 FFmpeg；请在 Tauri 桌面端打开真实工程。"
      : capabilities === null
        ? "正在核验本机 Renderer。"
        : undefined;

  const clearPoll = useCallback(() => {
    if (pollTimer.current !== undefined) clearTimeout(pollTimer.current);
    pollTimer.current = undefined;
  }, []);

  const refresh = useCallback(async (): Promise<boolean> => {
    const generation = ++refreshGeneration.current;
    clearPoll();
    if (!active) return false;
    setError(null);
    setNotice(null);
    setSnapshot(null);
    if (mode === "demo") {
      setCapabilities(null);
      setTimelineInput(null);
      setTimeline(null);
      setConfig(null);
      setBusyLabel(null);
      return false;
    }
    setBusyLabel("正在核验 Renderer 与已批准 Timeline…");
    try {
      const stageState = workflow.stageStates.find((state) => state.stageId === "timeline");
      const [probe, timelineStudio] = await Promise.all([
        rendererCommands.probe(),
        desktopGateway.loadStageStudio(project, "timeline"),
      ]);
      if (generation !== refreshGeneration.current) return false;
      const approvedRun = timelineStudio.runs.find((run) => run.runId === stageState?.approvedRunId);
      if (!stageState || !approvedRun) throw new Error("Timeline 没有当前有效的批准运行。");
      const artifacts = await desktopGateway.loadRunArtifacts(project, approvedRun);
      if (generation !== refreshGeneration.current) return false;
      if (artifacts.truncated) throw new Error("Timeline 产物列表超过安全读取上限。");
      const candidate = resolveApprovedTimelineCandidate({
        projectId: project.projectId,
        stageState,
        runs: timelineStudio.runs,
        reviews: timelineStudio.reviews,
        artifacts: artifacts.items,
      });
      if (!candidate.valid) throw new Error(candidate.error);
      const documentResult = await mediaCommands.getDocument({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        artifactId: candidate.value.artifactId,
      });
      if (generation !== refreshGeneration.current) return false;
      const document = narrowMediaDocument(documentResult.document, "timeline");
      if (!document || document.projectId !== project.projectId || document.runId !== candidate.value.runId || documentResult.contentHash !== candidate.value.contentHash) {
        throw new Error("Timeline 文档与批准运行、工程或内容哈希不一致。");
      }
      const nextConfig = defaultRenderConfig(document);
      if (!nextConfig) throw new Error("Timeline 无法生成受限渲染配置。");
      const firstSceneId = document.sceneTrack[0].sceneId;
      setCapabilities(probe);
      setTimelineInput(candidate.value);
      setTimeline(document);
      setConfig(nextConfig);
      setSelectedSceneId(firstSceneId);
      setBusyLabel(null);
      return true;
    } catch (reason) {
      if (generation !== refreshGeneration.current) return false;
      setCapabilities(null);
      setTimelineInput(null);
      setTimeline(null);
      setConfig(null);
      setBusyLabel(null);
      setError(reasonMessage(reason, "无法准备 Renderer 工作台。"));
      return false;
    }
  }, [active, clearPoll, mode, project, workflow]);

  useEffect(() => {
    void refresh();
    return () => { refreshGeneration.current += 1; snapshotGeneration.current += 1; clearPoll(); };
  }, [clearPoll, refresh]);

  useEffect(() => {
    const generation = ++snapshotGeneration.current;
    if (!available || !timelineInput || !selectedSceneId) {
      setSnapshot(null);
      return;
    }
    void rendererCommands.createSnapshot({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      timelineInput,
      sceneId: selectedSceneId,
    }).then((response) => {
      if (generation === snapshotGeneration.current && response.ownerProjectId === project.projectId) setSnapshot(response.snapshot);
    }).catch((reason) => {
      if (generation === snapshotGeneration.current) setError(reasonMessage(reason, "场景快照创建失败。"));
    });
  }, [available, project, selectedSceneId, timelineInput]);

  const handleTerminal = useCallback(async (snapshotValue: JobSnapshot) => {
    const currentJobId = jobId(snapshotValue);
    if (!currentJobId || terminalHandled.current.has(currentJobId)) return;
    terminalHandled.current.add(currentJobId);
    clearPoll();
    if (snapshotValue.status === "succeeded") {
      try {
        const renderResult = await rendererCommands.getResult({
          projectPath: project.projectPath,
          expectedProjectId: project.projectId,
          jobId: currentJobId,
        });
        setResult(renderResult);
        setNotice(`Render Job ${currentJobId} 已原子提交；历史运行未被覆盖。`);
      } catch (reason) {
        setError(reasonMessage(reason, "渲染已完成，但结果 manifest 读取失败。"));
      }
    } else {
      setError(reasonMessage(snapshotValue.lastError, snapshotValue.status === "canceled" ? "渲染已取消。" : "渲染失败。"));
      setNotice("失败/取消运行及诊断已保留，可显式重试为新运行。");
    }
    await Promise.allSettled([onRefreshWorkspace(), onRefreshStage()]);
  }, [clearPoll, onRefreshStage, onRefreshWorkspace, project]);

  const poll = useCallback(async (currentJobId: string) => {
    try {
      const next = await jobCommands.get({ projectPath: project.projectPath, expectedProjectId: project.projectId, jobId: currentJobId });
      setCurrentJob(next);
      if (!activeJob(next)) await handleTerminal(next);
    } catch (reason) {
      setError(reasonMessage(reason, "读取 Render Job 状态失败。"));
      setPollRevision((revision) => revision + 1);
    }
  }, [handleTerminal, project]);

  useEffect(() => {
    clearPoll();
    const currentJobId = currentJob ? jobId(currentJob) : acceptedJob?.jobId;
    if (!currentJobId || (currentJob && !activeJob(currentJob))) return;
    pollTimer.current = setTimeout(() => void poll(currentJobId), 750);
    return clearPoll;
  }, [acceptedJob?.jobId, clearPoll, currentJob, poll, pollRevision]);

  const enqueue = useCallback(async (target: "scene" | "timeline"): Promise<boolean> => {
    if (!available || !timelineInput || !config || !selectedSceneId || activeJob(currentJob) || busyLabel) return false;
    const runId = portableId("run_render_ui_");
    const idempotencyKey = portableId("idem_renderer_ui_");
    setBusyLabel(target === "scene" ? "正在创建场景 Render Job…" : "正在创建全片 Render Job…");
    setError(null);
    setNotice(null);
    setResult(null);
    try {
      const common = { projectPath: project.projectPath, expectedProjectId: project.projectId, runId, timelineInput, config, idempotencyKey };
      const accepted = target === "scene"
        ? await rendererCommands.enqueueScene({ ...common, sceneId: selectedSceneId })
        : await rendererCommands.enqueueTimeline(common);
      terminalHandled.current.delete(accepted.jobId);
      setAcceptedJob(accepted);
      setCurrentJob(null);
      setNotice(`已接受 ${target === "scene" ? "场景" : "全片"}渲染：${accepted.jobId}`);
      await poll(accepted.jobId);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "创建 Render Job 失败。"));
      return false;
    } finally {
      setBusyLabel(null);
    }
  }, [available, busyLabel, config, currentJob, poll, project, selectedSceneId, timelineInput]);

  const cancel = useCallback(async (): Promise<boolean> => {
    const currentJobId = currentJob ? jobId(currentJob) : acceptedJob?.jobId;
    if (!currentJobId || !currentJob || !activeJob(currentJob)) return false;
    try {
      const next = await jobCommands.cancel({ projectPath: project.projectPath, expectedProjectId: project.projectId, jobId: currentJobId, message: "用户从 Renderer 工作台请求取消。" });
      setCurrentJob(next);
      if (!activeJob(next)) await handleTerminal(next);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "取消 Render Job 失败。"));
      return false;
    }
  }, [acceptedJob?.jobId, currentJob, handleTerminal, project]);

  const retry = useCallback(async (): Promise<boolean> => {
    const sourceJobId = currentJob ? jobId(currentJob) : undefined;
    if (!sourceJobId || !currentJob || !["failed", "canceled"].includes(currentJob.status)) return false;
    try {
      const next = await jobCommands.retry({ projectPath: project.projectPath, expectedProjectId: project.projectId, sourceJobId, newRunId: portableId("run_render_retry_"), idempotencyKey: portableId("idem_renderer_retry_") });
      const nextJobId = jobId(next);
      if (!nextJobId) throw new Error("重试响应缺少 jobId。");
      terminalHandled.current.delete(nextJobId);
      setAcceptedJob(null);
      setCurrentJob(next);
      setResult(null);
      setNotice(`已创建新重试运行：${nextJobId}`);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "重试 Render Job 失败。"));
      return false;
    }
  }, [currentJob, project]);

  return {
    active, available, fallbackReason, capabilities, timelineInput, timeline, config,
    selectedSceneId, snapshot, acceptedJob, currentJob, result, busyLabel, error, notice,
    refresh, selectScene: setSelectedSceneId,
    setPreset: (preset) => setConfig((value) => value ? { ...value, preset } : value),
    setCrf: (crf) => setConfig((value) => value ? { ...value, crf } : value),
    enqueueScene: () => enqueue("scene"), enqueueTimeline: () => enqueue("timeline"), cancel, retry,
    clearError: () => setError(null), clearNotice: () => setNotice(null),
  };
}
