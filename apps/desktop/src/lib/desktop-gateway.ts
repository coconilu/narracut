import {
  NARRACUT_CONTRACT_VERSION,
  NARRACUT_PROJECT_COMMAND_API_VERSION,
  NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  type JobSnapshot,
  type JobStatus,
  type ProjectDescriptor,
  type RecentProject,
  type StageConfig,
  type StageDefinition,
  type WorkflowStageState,
} from "@narracut/contracts";
import {
  isJobCommandError,
  jobCommands,
  type ListJobsInput,
} from "./job-commands";
import {
  isProjectCommandError,
  projectCommands,
  type CreateProjectInput,
} from "./project-commands";
import {
  isStorageCommandError,
  storageCommands,
} from "./storage-commands";
import {
  isWorkflowCommandError,
  workflowCommands,
  type WorkflowSnapshotView,
} from "./workflow-commands";

export interface WorkbenchJob {
  readonly jobId: string;
  readonly runId: string;
  readonly stageId: string;
  readonly status: JobStatus;
  readonly attempt: number;
  readonly progress: number;
  readonly message?: string;
  readonly cancellationRequested: boolean;
  readonly artifactIds: readonly string[];
  readonly indexSynchronized: boolean;
  readonly updatedAt: string;
}

export interface WorkbenchEvent {
  readonly eventId: string;
  readonly sequence: number;
  readonly kind: string;
  readonly message: string;
  readonly createdAt: string;
  readonly progress?: number;
  readonly artifactId?: string;
  readonly tone: "active" | "approved" | "warning" | "muted";
}

export interface RecoverySummary {
  readonly recovered: number;
  readonly finalized: number;
  readonly skippedLive: number;
  readonly reindexed: number;
  readonly warnings: number;
}

export interface WorkspaceBundle {
  readonly project: ProjectDescriptor;
  readonly workflow: WorkflowSnapshotView;
  readonly jobs: readonly WorkbenchJob[];
  readonly events: readonly WorkbenchEvent[];
  readonly recovery: RecoverySummary;
  readonly mode: "desktop" | "demo";
}

export interface DesktopGateway {
  readonly mode: "desktop" | "demo";
  listRecentProjects(): Promise<readonly RecentProject[]>;
  createProject(input: CreateProjectInput): Promise<ProjectDescriptor>;
  initializeWorkflow(project: ProjectDescriptor): Promise<WorkflowSnapshotView>;
  openProject(projectPath: string): Promise<ProjectDescriptor>;
  openWorkspace(project: ProjectDescriptor): Promise<WorkspaceBundle>;
  refreshWorkspace(project: ProjectDescriptor): Promise<WorkspaceBundle>;
  cancelJob(project: ProjectDescriptor, jobId: string): Promise<WorkspaceBundle>;
  recoverWorkspace(project: ProjectDescriptor): Promise<WorkspaceBundle>;
}

const STANDARD_STAGE_SPECS = [
  ["brief", "创作简报", "明确受众、目标、边界与叙事方向。", []],
  ["research", "事实研究", "整理主张、证据、反证与来源追溯。", ["brief"]],
  ["script", "事实脚本", "把已审核的简报和证据组织为可追溯口播稿。", ["research"]],
  ["audio", "口播音频", "根据已审核脚本生成或导入口播音频。", ["script"]],
  ["captions", "字幕", "基于已审核脚本与音频生成时间对齐字幕。", ["script", "audio"]],
  ["scene_plan", "场景规划", "把事实脚本拆成镜头、素材需求与证据引用。", ["research", "script"]],
  ["timeline", "时间轴", "组合音频、字幕、场景与素材为可编辑时间轴。", ["audio", "captions", "scene_plan"]],
  ["render", "渲染", "通过 Renderer 接口把已审核时间轴渲染为候选视频。", ["timeline"]],
  ["export", "导出", "封装最终媒体与可追踪 manifest。", ["render"]],
] as const;

const EMPTY_RECOVERY: RecoverySummary = {
  recovered: 0,
  finalized: 0,
  skippedLive: 0,
  reindexed: 0,
  warnings: 0,
};

const demoNow = "2026-07-17T02:24:18.442Z";

let demoProjects: RecentProject[] = [
  {
    projectId: "project_moon_city",
    projectPath: "D:\\NarraCut\\moon-city",
    name: "月球城市为什么难",
    workflowDefinitionId: "workflow_standard_v1",
    projectFormatVersion: 1,
    archived: false,
    lastOpenedAt: "2026-07-17T01:42:00.000Z",
    markerUpdatedAt: "2026-07-17T01:42:00.000Z",
    pathAvailable: true,
  },
  {
    projectId: "project_solar_storage",
    projectPath: "D:\\NarraCut\\solar-storage",
    name: "三分钟读懂光伏储能",
    workflowDefinitionId: "workflow_standard_v1",
    projectFormatVersion: 1,
    archived: false,
    lastOpenedAt: "2026-07-16T10:06:00.000Z",
    markerUpdatedAt: "2026-07-16T10:06:00.000Z",
    pathAvailable: true,
  },
  {
    projectId: "project_urban_heat",
    projectPath: "E:\\Archive\\urban-heat",
    name: "城市热岛效应",
    workflowDefinitionId: "workflow_standard_v1",
    projectFormatVersion: 1,
    archived: false,
    lastOpenedAt: "2026-07-12T08:30:00.000Z",
    markerUpdatedAt: "2026-07-12T08:30:00.000Z",
    pathAvailable: false,
  },
  {
    projectId: "project_archived_ocean",
    projectPath: "D:\\NarraCut\\ocean-current",
    name: "海洋环流备忘",
    workflowDefinitionId: "workflow_standard_v1",
    projectFormatVersion: 1,
    archived: true,
    lastOpenedAt: "2026-06-28T04:12:00.000Z",
    markerUpdatedAt: "2026-06-28T04:12:00.000Z",
    pathAvailable: true,
  },
];

let demoCanceled = false;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function stringField(value: unknown, key: string): string | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const field = (value as Record<string, unknown>)[key];
  return typeof field === "string" ? field : undefined;
}

function numberField(value: unknown, key: string): number | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const field = (value as Record<string, unknown>)[key];
  return typeof field === "number" ? field : undefined;
}

function nestedErrorMessage(value: unknown): string | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const error = (value as Record<string, unknown>).error;
  return stringField(error, "message");
}

function mapJob(snapshot: JobSnapshot): WorkbenchJob {
  return {
    jobId: stringField(snapshot.job, "jobId") ?? "unknown_job",
    runId: stringField(snapshot.job, "stageRunId") ?? "unknown_run",
    stageId: stringField(snapshot.job, "stageId") ?? "unknown_stage",
    status: snapshot.status,
    attempt: snapshot.attempt,
    progress: snapshot.progress,
    message: snapshot.message,
    cancellationRequested: snapshot.cancellationRequested,
    artifactIds: snapshot.artifactIds,
    indexSynchronized: snapshot.indexSynchronized,
    updatedAt: snapshot.updatedAt,
  };
}

function eventTone(kind: string): WorkbenchEvent["tone"] {
  if (kind.includes("failed") || kind.includes("warning")) return "warning";
  if (kind.includes("completed") || kind.includes("artifact")) return "approved";
  if (kind.includes("progress") || kind.includes("started")) return "active";
  return "muted";
}

function mapEvent(value: unknown, index: number): WorkbenchEvent {
  const eventType = stringField(value, "eventType") ?? "event";
  const message =
    stringField(value, "message") ??
    nestedErrorMessage(value) ??
    eventType.replace(/_/g, " ");

  return {
    eventId: stringField(value, "eventId") ?? `event_${index}`,
    sequence: numberField(value, "sequence") ?? index,
    kind: `job.${eventType.replace(/_/g, ".")}`,
    message,
    createdAt: stringField(value, "createdAt") ?? new Date().toISOString(),
    progress: numberField(value, "progress"),
    artifactId: stringField(value, "artifactId"),
    tone: eventTone(eventType),
  };
}

function recoverySummary(result: {
  readonly recoveredJobIds: readonly string[];
  readonly finalizedJobIds: readonly string[];
  readonly skippedLiveJobIds: readonly string[];
  readonly reindexedJobs: number;
  readonly indexWarnings: number;
}): RecoverySummary {
  return {
    recovered: result.recoveredJobIds.length,
    finalized: result.finalizedJobIds.length,
    skippedLive: result.skippedLiveJobIds.length,
    reindexed: result.reindexedJobs,
    warnings: result.indexWarnings,
  };
}

async function readDesktopWorkspace(
  project: ProjectDescriptor,
  recovery: RecoverySummary,
  preloaded?: {
    readonly workflow: WorkflowSnapshotView;
    readonly listedJobs: Awaited<ReturnType<typeof jobCommands.list>>;
  },
): Promise<WorkspaceBundle> {
  const [workflow, listedJobs] = preloaded
    ? [preloaded.workflow, preloaded.listedJobs]
    : await Promise.all([
        workflowCommands.get(project.projectPath),
        jobCommands.list({
          projectPath: project.projectPath,
          expectedProjectId: project.projectId,
          statuses: [],
          limit: 100,
        }),
      ]);
  const jobs = listedJobs.jobs.map(mapJob);
  const eventJob = jobs.find((job) => ["running", "retrying"].includes(job.status)) ?? jobs[0];
  const events = eventJob
    ? await jobCommands.listEvents({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        jobId: eventJob.jobId,
        limit: 200,
      })
    : undefined;

  return {
    project,
    workflow,
    jobs,
    events: events?.events.map(mapEvent) ?? [],
    recovery,
    mode: "desktop",
  };
}

const realGateway: DesktopGateway = {
  mode: "desktop",
  async listRecentProjects() {
    const result = await storageCommands.listRecentProjects({
      limit: 50,
      includeMissing: true,
    });
    return result.projects;
  },
  createProject: (input) => projectCommands.create(input),
  initializeWorkflow: (project) =>
    workflowCommands.initialize({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
    }),
  openProject: (projectPath) => projectCommands.open(projectPath),
  async openWorkspace(project) {
    const jobListInput: ListJobsInput = {
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      statuses: [],
      limit: 100,
    };
    const recovery = await jobCommands.recover({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
    });
    const [workflow, listedJobs] = await Promise.all([
      workflowCommands.get(project.projectPath),
      jobCommands.list(jobListInput),
    ]);
    return readDesktopWorkspace(project, recoverySummary(recovery), {
      workflow,
      listedJobs,
    });
  },
  refreshWorkspace: (project) => readDesktopWorkspace(project, EMPTY_RECOVERY),
  async cancelJob(project, jobId) {
    await jobCommands.cancel({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      jobId,
      message: "用户从 NarraCut 工作台请求停止任务。",
    });
    return readDesktopWorkspace(project, EMPTY_RECOVERY);
  },
  async recoverWorkspace(project) {
    const recovery = await jobCommands.recover({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
    });
    return readDesktopWorkspace(project, recoverySummary(recovery));
  },
};

function toDescriptor(project: RecentProject): ProjectDescriptor {
  return {
    apiVersion: NARRACUT_PROJECT_COMMAND_API_VERSION,
    projectPath: project.projectPath,
    markerPath: `${project.projectPath}\\.narracut\\project.json`,
    projectId: project.projectId,
    name: project.name,
    workflowDefinitionId: project.workflowDefinitionId,
    projectFormatVersion: 1,
    defaultLocale: "zh-CN",
    archived: project.archived,
    createdAt: "2026-07-10T02:00:00.000Z",
    updatedAt: project.markerUpdatedAt,
  };
}

function demoDefinitions(): readonly StageDefinition[] {
  return STANDARD_STAGE_SPECS.map(([stageId, title, description, dependencies]) => ({
    schemaVersion: NARRACUT_CONTRACT_VERSION,
    documentType: "stage_definition",
    stageId,
    definitionVersion: "1.0.0",
    title,
    description,
    dependencies,
    inputKinds: [],
    outputKinds: [],
    configSchemaRef: `narracut://config/${stageId}/v1`,
    requiresApprovedInputs: dependencies.length > 0,
    supportsPartialRegeneration: ["research", "script", "captions", "scene_plan", "timeline", "render"].includes(stageId),
  }));
}

function demoWorkflow(project: ProjectDescriptor): WorkflowSnapshotView {
  const isMoonProject = project.projectId === "project_moon_city";
  const states: WorkflowStageState[] = STANDARD_STAGE_SPECS.map(([stageId], index) => {
    if (!isMoonProject) {
      return {
        stageId,
        status: index === 0 ? "ready" : "draft",
        staleBecauseStageIds: [],
      };
    }
    if (index < 2) {
      return {
        stageId,
        status: "approved",
        approvedRunId: index === 0 ? "run_brief_003" : "run_research_002",
        latestRunId: index === 0 ? "run_brief_003" : "run_research_002",
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "script") {
      return {
        stageId,
        status: demoCanceled ? "failed" : "running",
        latestRunId: "run_script_004",
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "audio") {
      return {
        stageId,
        status: "stale",
        approvedRunId: "run_audio_002",
        latestRunId: "run_audio_002",
        staleBecauseStageIds: ["script"],
      };
    }
    return {
      stageId,
      status: "draft",
      staleBecauseStageIds: [],
    };
  });
  const configs: StageConfig[] = STANDARD_STAGE_SPECS.map(([stageId]) => ({
    schemaVersion: NARRACUT_CONTRACT_VERSION,
    documentType: "stage_config",
    configId: `config_${stageId}_004`,
    projectId: project.projectId,
    stageId,
    revision: stageId === "script" ? 4 : 1,
    values:
      stageId === "script"
        ? {
            tone: "restrained",
            target_duration: "198s",
            citation_mode: "required",
            locale: "zh-CN",
          }
        : {},
    decisions: [],
    updatedAt: demoNow,
  }));

  return {
    apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
    ownerProjectId: project.projectId,
    workflowDefinitionId: project.workflowDefinitionId,
    stageDefinitions: demoDefinitions(),
    stageStates: states,
    configs,
  };
}

function demoJobs(project: ProjectDescriptor): readonly WorkbenchJob[] {
  if (project.projectId !== "project_moon_city") return [];
  return [
    {
      jobId: "job_script_004",
      runId: "run_script_004",
      stageId: "script",
      status: demoCanceled ? "canceled" : "running",
      attempt: 1,
      progress: demoCanceled ? 0.64 : 0.64,
      message: demoCanceled ? "用户已请求停止" : "正在核对第 4 段事实引用与时长",
      cancellationRequested: demoCanceled,
      artifactIds: ["artifact_74c9c91a"],
      indexSynchronized: true,
      updatedAt: demoNow,
    },
  ];
}

function demoEvents(): readonly WorkbenchEvent[] {
  return [
    {
      eventId: demoCanceled ? "event_cancel" : "event_progress",
      sequence: 18,
      kind: demoCanceled ? "job.cancel.requested" : "stage.running",
      message: demoCanceled ? "停止请求已记录；历史运行不会被覆盖" : "正在核对第 4 段事实引用与时长",
      createdAt: demoNow,
      progress: 0.64,
      tone: demoCanceled ? "warning" : "active",
    },
    {
      eventId: "event_artifact",
      sequence: 17,
      kind: "artifact.created",
      message: "script-outline.json 已保存到当前运行",
      createdAt: "2026-07-17T02:23:51.106Z",
      artifactId: "artifact_74c9c91a",
      tone: "approved",
    },
    {
      eventId: "event_warning",
      sequence: 16,
      kind: "job.warning",
      message: "1 条主张尚缺少直接证据，不会自动进入采用版本",
      createdAt: "2026-07-17T02:23:46.087Z",
      artifactId: "claim_urban_07",
      tone: "warning",
    },
  ];
}

function demoBundle(project: ProjectDescriptor, recovery = EMPTY_RECOVERY): WorkspaceBundle {
  return {
    project,
    workflow: demoWorkflow(project),
    jobs: demoJobs(project),
    events: project.projectId === "project_moon_city" ? demoEvents() : [],
    recovery,
    mode: "demo",
  };
}

const demoGateway: DesktopGateway = {
  mode: "demo",
  async listRecentProjects() {
    return [...demoProjects].sort((left, right) =>
      right.lastOpenedAt.localeCompare(left.lastOpenedAt),
    );
  },
  async createProject(input) {
    const separator = input.parentPath.includes("\\") ? "\\" : "/";
    const projectPath = `${input.parentPath.replace(/[\\/]$/, "")}${separator}${input.directoryName}`;
    const project: RecentProject = {
      projectId: `project_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`,
      projectPath,
      name: input.name,
      workflowDefinitionId: input.workflowDefinitionId,
      projectFormatVersion: 1,
      archived: false,
      lastOpenedAt: new Date().toISOString(),
      markerUpdatedAt: new Date().toISOString(),
      pathAvailable: true,
    };
    demoProjects = [project, ...demoProjects];
    return toDescriptor(project);
  },
  async initializeWorkflow(project) {
    return demoWorkflow(project);
  },
  async openProject(projectPath) {
    const recent = demoProjects.find((project) => project.projectPath === projectPath);
    if (recent) return toDescriptor(recent);
    const pathParts = projectPath.split(/[\\/]/).filter(Boolean);
    const fallbackName = pathParts[pathParts.length - 1] ?? "本地项目";
    return toDescriptor({
      projectId: `project_${fallbackName.replace(/[^a-zA-Z0-9_-]/g, "_")}`,
      projectPath,
      name: fallbackName,
      workflowDefinitionId: "workflow_standard_v1",
      projectFormatVersion: 1,
      archived: false,
      lastOpenedAt: new Date().toISOString(),
      markerUpdatedAt: new Date().toISOString(),
      pathAvailable: true,
    });
  },
  async openWorkspace(project) {
    return demoBundle(project, { ...EMPTY_RECOVERY, reindexed: 1 });
  },
  async refreshWorkspace(project) {
    return demoBundle(project);
  },
  async cancelJob(project) {
    demoCanceled = true;
    return demoBundle(project);
  },
  async recoverWorkspace(project) {
    return demoBundle(project, { ...EMPTY_RECOVERY, reindexed: 1 });
  },
};

export const desktopGateway: DesktopGateway = isTauriRuntime()
  ? realGateway
  : demoGateway;

export function describeDesktopError(error: unknown): string {
  if (
    isProjectCommandError(error) ||
    isStorageCommandError(error) ||
    isWorkflowCommandError(error) ||
    isJobCommandError(error)
  ) {
    return error.message;
  }
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "发生未知错误，请查看任务日志后重试。";
}
