import type {
  StageDefinition,
  WorkflowStageState,
  WorkflowStageStatus,
} from "@narracut/contracts";
import type { WorkflowSnapshotView } from "../lib/workflow-commands";

export const STANDARD_STAGE_IDS = [
  "brief",
  "research",
  "script",
  "audio",
  "captions",
  "scene_plan",
  "timeline",
  "render",
  "export",
] as const;

export type StandardStageId = (typeof STANDARD_STAGE_IDS)[number];

export interface StageView {
  readonly definition: StageDefinition;
  readonly state: WorkflowStageState;
  readonly index: number;
}

const statusLabels: Record<WorkflowStageStatus, string> = {
  draft: "草稿",
  ready: "可运行",
  running: "运行中",
  needs_review: "待审核",
  approved: "已采用",
  failed: "失败",
  stale: "已过期",
};

export function buildStageViews(workflow: WorkflowSnapshotView): readonly StageView[] {
  const states = new Map(workflow.stageStates.map((state) => [state.stageId, state]));

  return workflow.stageDefinitions.flatMap((definition, index) => {
    const state = states.get(definition.stageId);
    return state ? [{ definition, state, index }] : [];
  });
}

export function chooseInitialStageId(workflow: WorkflowSnapshotView): string {
  const preferred = workflow.stageStates.find((state) =>
    ["running", "needs_review", "stale", "ready"].includes(state.status),
  );

  return preferred?.stageId ?? workflow.stageDefinitions[0]?.stageId ?? "brief";
}

export function stageStatusLabel(status: WorkflowStageStatus): string {
  return statusLabels[status];
}

export function stageStatusTone(
  status: WorkflowStageStatus,
): "approved" | "active" | "stale" | "failed" | "muted" {
  if (status === "approved") return "approved";
  if (status === "running" || status === "needs_review" || status === "ready") {
    return "active";
  }
  if (status === "stale") return "stale";
  if (status === "failed") return "failed";
  return "muted";
}

export function stageRunLabel(state: WorkflowStageState): string {
  if (state.staleBecauseStageIds.length > 0) {
    return `依赖 ${state.staleBecauseStageIds.map(stageShortTitle).join(" / ")}`;
  }
  if (state.latestRunId) return state.latestRunId;
  return "等待输入";
}

function stageShortTitle(stageId: string): string {
  const labels: Record<string, string> = {
    brief: "简报",
    research: "研究",
    script: "脚本",
    audio: "音频",
    captions: "字幕",
    scene_plan: "场景",
    timeline: "时间轴",
    render: "渲染",
    export: "导出",
  };
  return labels[stageId] ?? stageId;
}

export function stageNodeLabel(state: WorkflowStageState, index: number): string {
  if (state.status === "approved") return "check";
  if (state.status === "stale" || state.status === "failed") return "alert";
  return String(index + 1);
}
