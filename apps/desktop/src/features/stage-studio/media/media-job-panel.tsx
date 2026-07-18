import type { JobStatus } from "@narracut/contracts";
import type { MediaStageStudioController } from "./use-media-stage";

export interface MediaJobPanelProps {
  readonly controller: Pick<
    MediaStageStudioController,
    "busyLabel" | "cancel" | "currentJob" | "currentJobId" | "retry"
  >;
}

const statusLabels: Record<JobStatus, string> = {
  queued: "排队中",
  running: "执行中",
  retrying: "重试中",
  succeeded: "已完成",
  failed: "失败",
  canceled: "已取消",
};

function redactLocalPaths(message: string): string {
  return message
    .replace(/[a-z]:[\\/](?:[^\\/\s]+[\\/])*[^\\/\s]*/gi, "本地文件")
    .replace(/(^|\s)\/(?:[^/\s]+\/)*[^/\s]*/g, "$1本地文件");
}

export function MediaJobPanel({ controller }: MediaJobPanelProps) {
  const job = controller.currentJob;
  if (!job) {
    return (
      <section className="media-job-panel" aria-label="媒体任务状态">
        <h3>任务状态</h3>
        <p className="media-input-meta">尚未创建媒体任务。</p>
      </section>
    );
  }

  const busy = controller.busyLabel !== null;
  const active = ["queued", "running", "retrying"].includes(job.status);
  const retryable = job.status === "failed" || job.status === "canceled";
  const progress = Math.min(Math.max(job.progress, 0), 1);
  const percent = Math.round(progress * 100);
  const message = job.lastError?.message || job.message;

  return (
    <section className="media-job-panel" aria-label="媒体任务状态">
      <div className="media-job-heading">
        <div>
          <h3>任务状态</h3>
          <p className="media-input-meta">
            {controller.currentJobId ?? "任务标识读取中"} · 第 {job.attempt} 次尝试
          </p>
        </div>
        <strong>{statusLabels[job.status]}</strong>
      </div>

      <label className="media-job-progress">
        <span>进度 {percent}%</span>
        <progress max={1} value={progress}>
          {percent}%
        </progress>
      </label>

      {message ? (
        <p className="media-job-message" role="status">
          {redactLocalPaths(message)}
        </p>
      ) : null}

      <div className="media-job-actions">
        {active ? (
          <button
            className="button danger"
            disabled={busy || job.cancellationRequested}
            onClick={() => void controller.cancel()}
            type="button"
          >
            {job.cancellationRequested ? "正在取消" : "取消任务"}
          </button>
        ) : null}
        {retryable ? (
          <button
            className="button"
            disabled={busy}
            onClick={() => void controller.retry()}
            type="button"
          >
            创建新重试
          </button>
        ) : null}
      </div>
    </section>
  );
}
