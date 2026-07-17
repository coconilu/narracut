import type { ReviewDecision, StageRun } from "@narracut/contracts";

export const runStatusLabels: Record<StageRun["status"], string> = {
  queued: "已排队",
  running: "运行中",
  succeeded: "已完成",
  failed: "失败",
  canceled: "已取消",
};

export const reviewLabels: Record<ReviewDecision, string> = {
  approved: "采用",
  changes_requested: "请求修改",
  rejected: "拒绝",
};

export function StudioEmpty({
  title,
  text,
}: {
  readonly title: string;
  readonly text: string;
}) {
  return (
    <div className="studio-empty">
      <strong>{title}</strong>
      <span>{text}</span>
    </div>
  );
}

export function StudioHeading({
  eyebrow,
  title,
  text,
}: {
  readonly eyebrow: string;
  readonly title: string;
  readonly text: string;
}) {
  return (
    <header className="studio-heading">
      <span>{eyebrow}</span>
      <h2>{title}</h2>
      <p>{text}</p>
    </header>
  );
}

export function Metric({
  label,
  value,
  mono = false,
}: {
  readonly label: string;
  readonly value: string;
  readonly mono?: boolean;
}) {
  return (
    <div className="studio-metric">
      <span>{label}</span>
      <strong className={mono ? "mono" : ""}>{value}</strong>
    </div>
  );
}

export function formatDate(value: string): string {
  return new Date(value).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function formatBytes(value?: number): string {
  if (value === undefined) return "未知";
  if (value < 1_024) return `${value} B`;
  if (value < 1_048_576) return `${(value / 1_024).toFixed(1)} KB`;
  return `${(value / 1_048_576).toFixed(1)} MB`;
}

export function shortHash(value?: string): string {
  if (!value) return "未返回";
  return value.length > 22 ? `${value.slice(0, 14)}…${value.slice(-6)}` : value;
}

export function executorLabel(run: StageRun): string {
  return [run.executor.providerId, run.executor.model ?? run.executor.executionMode]
    .filter(Boolean)
    .join(" · ");
}

export function configChangedKeys(
  left: StageRun,
  right: StageRun,
): readonly string[] {
  const keys = new Set([
    ...Object.keys(left.configSnapshot.values),
    ...Object.keys(right.configSnapshot.values),
  ]);
  return [...keys].filter(
    (key) =>
      JSON.stringify(left.configSnapshot.values[key]) !==
      JSON.stringify(right.configSnapshot.values[key]),
  );
}

export function formatUnknown(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (value === undefined) return "未设置";
  return JSON.stringify(value);
}
