import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ExportJobAcceptedResult,
  ExportQaResult,
  ExportRenderInputReference,
  ExportResult,
  ExportVerificationResult,
  JobSnapshot,
  ProjectDescriptor,
} from "@narracut/contracts";
import { desktopGateway } from "../../../lib/desktop-gateway";
import { exportCommands, isExportCommandError } from "../../../lib/export-commands";
import { isJobCommandError, jobCommands } from "../../../lib/job-commands";
import type { WorkflowSnapshotView } from "../../../lib/workflow-commands";
import { resolveApprovedRenderCandidate, safeExportName } from "./export-stage-model.js";

export interface ExportStageController {
  readonly active: boolean;
  readonly available: boolean;
  readonly fallbackReason?: string;
  readonly renderInput: ExportRenderInputReference | null;
  readonly qaResult: ExportQaResult | null;
  readonly destinationDirectory: string | null;
  readonly exportName: string;
  readonly acceptedJob: ExportJobAcceptedResult | null;
  readonly currentJob: JobSnapshot | null;
  readonly result: ExportResult | null;
  readonly verification: ExportVerificationResult | null;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly notice: string | null;
  readonly refresh: () => Promise<boolean>;
  readonly chooseDestination: () => Promise<boolean>;
  readonly setExportName: (value: string) => void;
  readonly enqueue: () => Promise<boolean>;
  readonly cancel: () => Promise<boolean>;
  readonly retry: () => Promise<boolean>;
  readonly verify: () => Promise<boolean>;
  readonly clearError: () => void;
  readonly clearNotice: () => void;
}

interface UseExportStageInput {
  readonly project: ProjectDescriptor;
  readonly workflow: WorkflowSnapshotView;
  readonly stageId: string;
  readonly mode: "desktop" | "demo";
  readonly onRefreshWorkspace: () => Promise<boolean>;
  readonly onRefreshStage: () => Promise<boolean>;
}

const ACTIVE_STATUSES = new Set(["queued", "running", "retrying"]);

function activeJob(job: JobSnapshot | null): boolean {
  return Boolean(job && ACTIVE_STATUSES.has(job.status));
}

function snapshotJobId(job: JobSnapshot | null): string | undefined {
  const value = job?.job.jobId;
  return typeof value === "string" && value ? value : undefined;
}

function portableId(prefix: string): string {
  return `${prefix}${crypto.randomUUID().replace(/-/g, "").slice(0, 20)}`;
}

function reasonMessage(reason: unknown, fallback: string): string {
  const message = isExportCommandError(reason) || isJobCommandError(reason)
    ? reason.message
    : reason instanceof Error && reason.message.trim()
      ? reason.message
      : fallback;
  return message.replace(/[a-z]:[\\/](?:[^\\/\s]+[\\/])*[^\\/\s]*/gi, "本地路径");
}

export function useExportStage({
  project,
  workflow,
  stageId,
  mode,
  onRefreshWorkspace,
  onRefreshStage,
}: UseExportStageInput): ExportStageController {
  const active = stageId === "export";
  const [renderInput, setRenderInput] = useState<ExportRenderInputReference | null>(null);
  const [qaResult, setQaResult] = useState<ExportQaResult | null>(null);
  const [destinationDirectory, setDestinationDirectory] = useState<string | null>(null);
  const [exportName, setExportNameState] = useState(() => safeExportName(project.name));
  const [acceptedJob, setAcceptedJob] = useState<ExportJobAcceptedResult | null>(null);
  const [currentJob, setCurrentJob] = useState<JobSnapshot | null>(null);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [verification, setVerification] = useState<ExportVerificationResult | null>(null);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pollRevision, setPollRevision] = useState(0);
  const refreshGeneration = useRef(0);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const terminalHandled = useRef(new Set<string>());
  const available = active && mode === "desktop" && qaResult?.qa.passed === true && renderInput !== null;
  const fallbackReason = !active
    ? "当前不是导出阶段。"
    : mode === "demo"
      ? "浏览器演示模式不会读取本机工程、执行 QA 或创建导出 Job。"
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
    setResult(null);
    setVerification(null);
    if (mode === "demo") {
      setRenderInput(null);
      setQaResult(null);
      setBusyLabel(null);
      return false;
    }
    setBusyLabel("正在闭合 approved Render 并运行导出 QA…");
    try {
      const stageState = workflow.stageStates.find((state) => state.stageId === "render");
      const studio = await desktopGateway.loadStageStudio(project, "render");
      if (generation !== refreshGeneration.current) return false;
      const run = studio.runs.find((item) => item.runId === stageState?.approvedRunId);
      if (!stageState || !run) throw new Error("Render 没有当前有效的批准运行。");
      const artifacts = await desktopGateway.loadRunArtifacts(project, run);
      if (generation !== refreshGeneration.current) return false;
      if (artifacts.truncated) throw new Error("Render 产物列表超过安全读取上限。");
      const candidate = resolveApprovedRenderCandidate({
        projectId: project.projectId,
        stageState,
        runs: studio.runs,
        reviews: studio.reviews,
        artifacts: artifacts.items,
      });
      if (!candidate.valid) throw new Error(candidate.error);
      const qa = await exportCommands.runQa({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        renderInput: candidate.value,
      });
      if (generation !== refreshGeneration.current) return false;
      setRenderInput(candidate.value);
      setQaResult(qa);
      setBusyLabel(null);
      if (!qa.qa.passed) setNotice(`QA 阻塞导出：${qa.qa.blockingCount} 项必须处理。`);
      return qa.qa.passed;
    } catch (reason) {
      if (generation !== refreshGeneration.current) return false;
      setRenderInput(null);
      setQaResult(null);
      setBusyLabel(null);
      setError(reasonMessage(reason, "无法准备导出工作台。"));
      return false;
    }
  }, [active, clearPoll, mode, project, workflow]);

  useEffect(() => {
    void refresh();
    return () => { refreshGeneration.current += 1; clearPoll(); };
  }, [clearPoll, refresh]);

  const handleTerminal = useCallback(async (snapshot: JobSnapshot) => {
    const jobId = snapshotJobId(snapshot);
    if (!jobId || terminalHandled.current.has(jobId)) return;
    terminalHandled.current.add(jobId);
    clearPoll();
    if (snapshot.status === "succeeded") {
      try {
        const exportResult = await exportCommands.getResult({
          projectPath: project.projectPath,
          expectedProjectId: project.projectId,
          jobId,
        });
        const integrity = await exportCommands.verify({ exportDirectory: exportResult.exportPath });
        setResult(exportResult);
        setVerification(integrity);
        setNotice(integrity.status === "verified" ? "导出已原子提交，Manifest 与所有文件哈希复验通过。" : "导出完成，但完整性复验发现损坏。" );
      } catch (reason) {
        setError(reasonMessage(reason, "导出已完成，但结果或完整性报告读取失败。"));
      }
    } else {
      setError(reasonMessage(snapshot.lastError, snapshot.status === "canceled" ? "导出已取消。" : "导出失败。"));
      setNotice("失败或取消记录已保留；显式重试会创建新的运行。" );
    }
    await Promise.allSettled([onRefreshWorkspace(), onRefreshStage()]);
  }, [clearPoll, onRefreshStage, onRefreshWorkspace, project]);

  const poll = useCallback(async (jobId: string) => {
    try {
      const next = await jobCommands.get({ projectPath: project.projectPath, expectedProjectId: project.projectId, jobId });
      setCurrentJob(next);
      if (!activeJob(next)) await handleTerminal(next);
    } catch (reason) {
      setError(reasonMessage(reason, "读取 Export Job 状态失败。"));
      setPollRevision((value) => value + 1);
    }
  }, [handleTerminal, project]);

  useEffect(() => {
    clearPoll();
    const jobId = snapshotJobId(currentJob) ?? acceptedJob?.jobId;
    if (!jobId || (currentJob && !activeJob(currentJob))) return;
    pollTimer.current = setTimeout(() => void poll(jobId), 750);
    return clearPoll;
  }, [acceptedJob?.jobId, clearPoll, currentJob, poll, pollRevision]);

  const chooseDestination = useCallback(async (): Promise<boolean> => {
    if (mode !== "desktop" || activeJob(currentJob)) return false;
    try {
      const selected = await open({ directory: true, multiple: false, title: "选择 NarraCut 导出父目录" });
      if (typeof selected !== "string") return false;
      setDestinationDirectory(selected);
      setNotice("目标父目录已选择；最终结果会写入新的原子子目录。" );
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "无法选择导出目录。"));
      return false;
    }
  }, [currentJob, mode]);

  const enqueue = useCallback(async (): Promise<boolean> => {
    if (!available || !renderInput || !qaResult || !destinationDirectory || activeJob(currentJob) || busyLabel) return false;
    setBusyLabel("正在创建 Export Job…");
    setError(null);
    setNotice(null);
    setResult(null);
    setVerification(null);
    try {
      const accepted = await exportCommands.enqueue({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        runId: portableId("run_export_ui_"),
        renderInput,
        qaHash: qaResult.qa.qaHash,
        destinationDirectory,
        exportName: safeExportName(exportName),
        idempotencyKey: portableId("idem_export_ui_"),
        maxTemporaryBytes: 10 * 1024 * 1024 * 1024,
      });
      terminalHandled.current.delete(accepted.jobId);
      setAcceptedJob(accepted);
      setCurrentJob(null);
      setNotice(`导出请求已接受：${accepted.jobId}`);
      await poll(accepted.jobId);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "创建 Export Job 失败。"));
      return false;
    } finally {
      setBusyLabel(null);
    }
  }, [available, busyLabel, currentJob, destinationDirectory, exportName, poll, project, qaResult, renderInput]);

  const cancel = useCallback(async (): Promise<boolean> => {
    const jobId = snapshotJobId(currentJob) ?? acceptedJob?.jobId;
    if (!jobId || !currentJob || !activeJob(currentJob)) return false;
    try {
      const next = await jobCommands.cancel({ projectPath: project.projectPath, expectedProjectId: project.projectId, jobId, message: "用户从导出工作台请求取消。" });
      setCurrentJob(next);
      if (!activeJob(next)) await handleTerminal(next);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "取消 Export Job 失败。"));
      return false;
    }
  }, [acceptedJob?.jobId, currentJob, handleTerminal, project]);

  const retry = useCallback(async (): Promise<boolean> => {
    const sourceJobId = snapshotJobId(currentJob);
    if (!sourceJobId || !currentJob || !["failed", "canceled"].includes(currentJob.status)) return false;
    try {
      const next = await jobCommands.retry({ projectPath: project.projectPath, expectedProjectId: project.projectId, sourceJobId, newRunId: portableId("run_export_retry_"), idempotencyKey: portableId("idem_export_retry_") });
      const nextJobId = snapshotJobId(next);
      if (!nextJobId) throw new Error("重试响应缺少 jobId。");
      terminalHandled.current.delete(nextJobId);
      setAcceptedJob(null);
      setCurrentJob(next);
      setResult(null);
      setVerification(null);
      setNotice(`已创建新的导出重试运行：${nextJobId}`);
      return true;
    } catch (reason) {
      setError(reasonMessage(reason, "重试 Export Job 失败。"));
      return false;
    }
  }, [currentJob, project]);

  const verify = useCallback(async (): Promise<boolean> => {
    if (!result) return false;
    try {
      const integrity = await exportCommands.verify({ exportDirectory: result.exportPath });
      setVerification(integrity);
      setNotice(integrity.status === "verified" ? "导出完整性复验通过。" : "导出完整性复验发现损坏。" );
      return integrity.status === "verified";
    } catch (reason) {
      setError(reasonMessage(reason, "完整性复验失败。"));
      return false;
    }
  }, [result]);

  return {
    active, available, fallbackReason, renderInput, qaResult, destinationDirectory, exportName,
    acceptedJob, currentJob, result, verification, busyLabel, error, notice, refresh,
    chooseDestination, setExportName: (value) => setExportNameState(safeExportName(value)), enqueue,
    cancel, retry, verify, clearError: () => setError(null), clearNotice: () => setNotice(null),
  };
}
