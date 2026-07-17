import {
  NARRACUT_CONTRACT_VERSION,
  NARRACUT_PROJECT_COMMAND_API_VERSION,
  NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  type AffectedStage,
  type Artifact,
  type DecisionRecord,
  type JsonObject,
  type JobSnapshot,
  type JobStatus,
  type ProjectDescriptor,
  type RecentProject,
  type RegenerationImpactResult,
  type ReviewDecision,
  type ReviewerReference,
  type ReviewRecord,
  type StageConfig,
  type StageDefinition,
  type StageRun,
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
  type StageConfigUpdate,
  type StageReview,
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

export interface StageStudioSnapshot {
  readonly stageId: string;
  readonly config: StageConfig;
  readonly runs: readonly StageRun[];
  readonly reviews: readonly ReviewRecord[];
  readonly mode: "desktop" | "demo";
}

export interface WorkbenchArtifact {
  readonly artifactId: string;
  readonly kind: string;
  readonly mediaType?: string;
  readonly byteLength?: number;
  readonly contentHash?: string;
  readonly evidenceRole?: Artifact["evidenceRole"];
  readonly sourceOrigin?: Artifact["source"]["origin"];
  readonly sourceLabel?: string;
  readonly provenance: Artifact["provenance"];
  readonly metadataUri?: string;
  readonly contentUri?: string;
  readonly contentAvailable: boolean;
  readonly loadError?: string;
  readonly demoPreview?: string;
}

export interface RunArtifactCollection {
  readonly runId: string;
  readonly total: number;
  readonly truncated: boolean;
  readonly items: readonly WorkbenchArtifact[];
}

export interface StageConfigChange {
  readonly values: JsonObject;
  readonly decision: DecisionRecord;
}

export interface StageReviewIntent {
  readonly reviewId: string;
  readonly decision: ReviewDecision;
  readonly reviewer: ReviewerReference;
  readonly comments: string;
  readonly artifactIds: readonly string[];
}

export interface StageRegenerationIntent {
  readonly runId: string;
  readonly idempotencyKey: string;
}

export interface RegenerationPreview {
  readonly changedStageIds: readonly string[];
  readonly affectedStages: readonly AffectedStage[];
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
  loadStageStudio(
    project: ProjectDescriptor,
    stageId: string,
  ): Promise<StageStudioSnapshot>;
  loadRunArtifacts(
    project: ProjectDescriptor,
    run: StageRun,
  ): Promise<RunArtifactCollection>;
  updateStageConfig(
    project: ProjectDescriptor,
    config: StageConfig,
    change: StageConfigChange,
  ): Promise<StageConfigUpdate>;
  reviewStageRun(
    project: ProjectDescriptor,
    run: StageRun,
    intent: StageReviewIntent,
  ): Promise<StageReview>;
  previewStageRegeneration(
    project: ProjectDescriptor,
    stageId: string,
  ): Promise<RegenerationPreview>;
  regenerateStage(
    project: ProjectDescriptor,
    sourceRun: StageRun,
    intent: StageRegenerationIntent,
  ): Promise<WorkbenchJob>;
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

const demoConfigOverrides = new Map<string, StageConfig>();
let demoAdditionalReviews: ReviewRecord[] = [];
let demoJobRecords: Array<{
  readonly intent: StageRegenerationIntent;
  readonly job: WorkbenchJob;
}> = [];

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

function assertStageOwnership(
  project: ProjectDescriptor,
  stageId: string,
  ownerProjectId: string,
  ownerStageId: string,
): void {
  if (ownerProjectId !== project.projectId || ownerStageId !== stageId) {
    throw new Error("阶段数据与当前工程不匹配，已拒绝跨工程操作。");
  }
}

function artifactSourceLabel(artifact: Artifact): string {
  if (artifact.source.origin === "generated") {
    return [artifact.source.providerId, artifact.source.model]
      .filter(Boolean)
      .join(" · ");
  }
  if (artifact.source.origin === "imported") {
    return `${artifact.source.author} · ${artifact.source.license}`;
  }
  return `派生自 ${artifact.source.sourceArtifactIds.length} 个产物`;
}

function mapArtifactRead(
  artifact: Artifact,
  metadataUri: string,
  contentUri: string,
  contentAvailable: boolean,
): WorkbenchArtifact {
  return {
    artifactId: artifact.artifactId,
    kind: artifact.kind,
    mediaType: artifact.mediaType,
    byteLength: artifact.byteLength,
    contentHash: artifact.contentHash,
    evidenceRole: artifact.evidenceRole,
    sourceOrigin: artifact.source.origin,
    sourceLabel: artifactSourceLabel(artifact),
    provenance: artifact.provenance,
    metadataUri,
    contentUri,
    contentAvailable,
  };
}

async function loadDesktopStageStudio(
  project: ProjectDescriptor,
  stageId: string,
): Promise<StageStudioSnapshot> {
  const [workflow, history] = await Promise.all([
    workflowCommands.get(project.projectPath),
    workflowCommands.listHistory({
      projectPath: project.projectPath,
      stageId,
      limit: 100,
    }),
  ]);
  if (
    workflow.ownerProjectId !== project.projectId ||
    history.ownerProjectId !== project.projectId ||
    history.stageId !== stageId
  ) {
    throw new Error("阶段历史返回了不匹配的工程身份，已停止读取。");
  }
  const config = workflow.configs.find((item) => item.stageId === stageId);
  if (!config) throw new Error(`阶段 ${stageId} 缺少配置快照。`);

  return {
    stageId,
    config,
    runs: history.runs,
    reviews: history.reviews,
    mode: "desktop",
  };
}

async function loadDesktopRunArtifacts(
  project: ProjectDescriptor,
  run: StageRun,
): Promise<RunArtifactCollection> {
  assertStageOwnership(project, run.stageId, run.projectId, run.stageId);
  const artifactIds = [...new Set(run.artifactIds)];
  const limitedIds = artifactIds.slice(0, 24);
  const items = await Promise.all(
    limitedIds.map(async (artifactId): Promise<WorkbenchArtifact> => {
      try {
        const result = await storageCommands.getArtifact(project.projectPath, artifactId);
        assertStageOwnership(
          project,
          run.stageId,
          result.artifact.projectId,
          result.artifact.stageId,
        );
        if (result.artifact.runId !== run.runId) {
          throw new Error("产物所属运行与当前历史运行不匹配。");
        }
        return mapArtifactRead(
          result.artifact,
          result.metadataUri,
          result.contentUri,
          result.contentAvailable,
        );
      } catch (error) {
        return {
          artifactId,
          kind: "unknown",
          provenance: [],
          contentAvailable: false,
          loadError: describeDesktopError(error),
        };
      }
    }),
  );

  return {
    runId: run.runId,
    total: artifactIds.length,
    truncated: artifactIds.length > limitedIds.length,
    items,
  };
}

function validateReviewIntent(run: StageRun, intent: StageReviewIntent): void {
  const uniqueIds = new Set(intent.artifactIds);
  if (uniqueIds.size !== intent.artifactIds.length) {
    throw new Error("审核产物不能重复选择。");
  }
  if (intent.decision === "approved" && run.status !== "succeeded") {
    throw new Error("只有 succeeded 的历史运行可以被采用。");
  }
  if (intent.decision === "approved" && intent.artifactIds.length === 0) {
    throw new Error("采用运行时必须明确选择至少一个产物。");
  }
  const allowed = new Set(run.artifactIds);
  if (intent.artifactIds.some((artifactId) => !allowed.has(artifactId))) {
    throw new Error("审核引用了不属于当前运行的产物。");
  }
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
  loadStageStudio: loadDesktopStageStudio,
  loadRunArtifacts: loadDesktopRunArtifacts,
  async updateStageConfig(project, config, change) {
    assertStageOwnership(project, config.stageId, config.projectId, config.stageId);
    const decisions = [...config.decisions, change.decision];
    if (decisions.length > 256) {
      throw new Error("阶段配置决策记录已达到 256 条上限。");
    }
    return workflowCommands.updateConfig({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      stageId: config.stageId,
      expectedRevision: config.revision,
      values: change.values,
      decisions: decisions.map((decision) => ({
        decisionId: decision.decisionId,
        key: decision.key,
        value: decision.value,
        rationale: decision.rationale,
        madeBy: decision.madeBy,
        madeAt: decision.madeAt,
      })),
    });
  },
  async reviewStageRun(project, run, intent) {
    assertStageOwnership(project, run.stageId, run.projectId, run.stageId);
    validateReviewIntent(run, intent);
    return workflowCommands.reviewRun({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      stageId: run.stageId,
      runId: run.runId,
      ...intent,
    });
  },
  async previewStageRegeneration(project, stageId) {
    const result: RegenerationImpactResult = await workflowCommands.preview(
      project.projectPath,
      [stageId],
    );
    if (result.ownerProjectId !== project.projectId) {
      throw new Error("重生成影响范围返回了不匹配的工程身份。");
    }
    return result;
  },
  async regenerateStage(project, sourceRun, intent) {
    assertStageOwnership(
      project,
      sourceRun.stageId,
      sourceRun.projectId,
      sourceRun.stageId,
    );
    const snapshot = await jobCommands.enqueue({
      projectPath: project.projectPath,
      expectedProjectId: project.projectId,
      stageId: sourceRun.stageId,
      runId: intent.runId,
      inputRefs: sourceRun.inputRefs,
      executor: sourceRun.executor,
      idempotencyKey: intent.idempotencyKey,
      retryPolicy: {
        maxAttempts: 3,
        initialBackoffMs: 1_000,
        backoffMultiplier: 2,
        maxBackoffMs: 30_000,
      },
    });
    return mapJob(snapshot);
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

const DEMO_EXECUTOR = {
  providerId: "local-codex",
  providerVersion: "0.1.0",
  executionMode: "codex_cli",
  model: "gpt-5-codex",
} as const;

let demoApprovedScriptRunId: string | undefined = "run_script_003";

function demoConfigKey(projectId: string, stageId: string): string {
  return `${projectId}:${stageId}`;
}

function createDemoConfig(
  projectId: string,
  stageId: string,
  revision = 1,
  values: JsonObject = {},
  decisions: readonly DecisionRecord[] = [],
  updatedAt = demoNow,
): StageConfig {
  return {
    schemaVersion: NARRACUT_CONTRACT_VERSION,
    documentType: "stage_config",
    configId: `config_${stageId}_001`,
    projectId,
    stageId,
    revision,
    values,
    decisions,
    updatedAt,
  };
}

function demoConfigFor(project: ProjectDescriptor, stageId: string): StageConfig {
  const overridden = demoConfigOverrides.get(demoConfigKey(project.projectId, stageId));
  if (overridden) return overridden;
  if (project.projectId === "project_moon_city" && stageId === "script") {
    return createDemoConfig(project.projectId, stageId, 4, {
      tone: "restrained",
      target_duration: "198s",
      citation_mode: "required",
      locale: "zh-CN",
    });
  }
  return createDemoConfig(project.projectId, stageId);
}

function demoInputRef(
  artifactId: string,
  sourceRunId: string,
  reviewRecordId: string,
  kind: string,
  claimIds: readonly string[] = [],
  evidenceRefs: readonly string[] = [],
): StageRun["inputRefs"][number] {
  return {
    refId: `ref_${artifactId.replace(/^artifact_/, "")}`,
    referenceType: "artifact",
    kind,
    contentHash: `sha256:${"a".repeat(64)}`,
    artifactId,
    sourceRunId,
    reviewRecordId,
    claimIds,
    evidenceRefs,
  };
}

function createDemoRun(input: {
  readonly project: ProjectDescriptor;
  readonly stageId: string;
  readonly runId: string;
  readonly status: StageRun["status"];
  readonly config: StageConfig;
  readonly artifactIds: readonly string[];
  readonly createdAt: string;
  readonly completedAt?: string;
  readonly message: string;
  readonly warnings?: readonly string[];
  readonly inputRefs?: StageRun["inputRefs"];
  readonly supersedesRunId?: string;
}): StageRun {
  return {
    schemaVersion: NARRACUT_CONTRACT_VERSION,
    documentType: "stage_run",
    runId: input.runId,
    projectId: input.project.projectId,
    stageId: input.stageId,
    stageDefinitionVersion: "1.0.0",
    status: input.status,
    jobId: `job_${input.runId.replace(/^run_/, "")}`,
    inputHash: `sha256:${"1".repeat(64)}`,
    configHash: `sha256:${"2".repeat(64)}`,
    idempotencyKey: `idem_${input.runId}`,
    inputRefs: input.inputRefs ?? [],
    configSnapshot: input.config,
    executor: DEMO_EXECUTOR,
    artifactIds: input.artifactIds,
    logSummary: {
      message: input.message,
      warnings: input.warnings ?? [],
      errors: input.status === "failed" ? ["结构化输出缺少 evidence_ref。"] : [],
    },
    supersedesRunId: input.supersedesRunId,
    createdAt: input.createdAt,
    startedAt: input.createdAt,
    completedAt: input.completedAt ?? input.createdAt,
  };
}

function demoRunsFor(
  project: ProjectDescriptor,
  stageId: string,
): readonly StageRun[] {
  if (project.projectId !== "project_moon_city") return [];
  const briefConfig = createDemoConfig(project.projectId, "brief");
  const researchConfig = createDemoConfig(project.projectId, "research");
  const scriptConfigV3 = createDemoConfig(project.projectId, "script", 3, {
    tone: "concise",
    target_duration: "174s",
    citation_mode: "required",
    locale: "zh-CN",
  });
  const scriptConfigV4 = createDemoConfig(project.projectId, "script", 4, {
    tone: "restrained",
    target_duration: "198s",
    citation_mode: "required",
    locale: "zh-CN",
  });
  const audioConfig = createDemoConfig(project.projectId, "audio");
  const runs: Record<string, readonly StageRun[]> = {
    brief: [
      createDemoRun({
        project,
        stageId: "brief",
        runId: "run_brief_003",
        status: "succeeded",
        config: briefConfig,
        artifactIds: ["artifact_brief_003"],
        createdAt: "2026-07-15T01:12:00.000Z",
        message: "创作边界、受众与叙事目标已冻结。",
      }),
    ],
    research: [
      createDemoRun({
        project,
        stageId: "research",
        runId: "run_research_002",
        status: "succeeded",
        config: researchConfig,
        artifactIds: ["artifact_research_002"],
        createdAt: "2026-07-16T02:30:00.000Z",
        message: "7 个主张与 11 个证据引用已结构化。",
        inputRefs: [
          demoInputRef(
            "artifact_brief_003",
            "run_brief_003",
            "review_brief_003",
            "creative_brief",
          ),
        ],
      }),
    ],
    script: [
      createDemoRun({
        project,
        stageId: "script",
        runId: "run_script_004",
        status: "succeeded",
        config: scriptConfigV4,
        artifactIds: ["artifact_script_004", "artifact_citations_004"],
        createdAt: "2026-07-17T02:24:18.442Z",
        message: "事实引用更完整，新增能源与月壤风险段落。",
        warnings: ["C07 当前只有间接证据。"],
        inputRefs: [
          demoInputRef(
            "artifact_research_002",
            "run_research_002",
            "review_research_002",
            "claim_evidence_set",
            ["C01", "C04", "C07"],
            ["E03", "E08", "E11"],
          ),
        ],
        supersedesRunId: "run_script_003",
      }),
      createDemoRun({
        project,
        stageId: "script",
        runId: "run_script_003",
        status: "succeeded",
        config: scriptConfigV3,
        artifactIds: ["artifact_script_003"],
        createdAt: "2026-07-16T10:42:00.000Z",
        message: "当前下游使用的采用版本。",
        inputRefs: [
          demoInputRef(
            "artifact_research_002",
            "run_research_002",
            "review_research_002",
            "claim_evidence_set",
            ["C01", "C04"],
            ["E03", "E06"],
          ),
        ],
        supersedesRunId: "run_script_002",
      }),
      createDemoRun({
        project,
        stageId: "script",
        runId: "run_script_002",
        status: "failed",
        config: scriptConfigV3,
        artifactIds: [],
        createdAt: "2026-07-15T08:08:00.000Z",
        message: "结构化输出校验失败，历史运行已保留。",
      }),
    ],
    audio: [
      createDemoRun({
        project,
        stageId: "audio",
        runId: "run_audio_002",
        status: "succeeded",
        config: audioConfig,
        artifactIds: ["artifact_audio_002"],
        createdAt: "2026-07-16T12:06:00.000Z",
        message: "采用脚本版本 3 的口播音频。",
        inputRefs: [
          demoInputRef(
            "artifact_script_003",
            "run_script_003",
            "review_script_003",
            "approved_script",
            ["C01", "C04"],
            ["E03", "E06"],
          ),
        ],
      }),
    ],
  };
  return runs[stageId] ?? [];
}

function createDemoReview(
  projectId: string,
  stageId: string,
  runId: string,
  reviewId: string,
  artifactIds: readonly string[],
  createdAt: string,
): ReviewRecord {
  return {
    schemaVersion: NARRACUT_CONTRACT_VERSION,
    documentType: "review_record",
    reviewId,
    projectId,
    stageId,
    runId,
    decision: "approved",
    reviewer: {
      kind: "human",
      reviewerId: "local_user",
      displayName: "本机创作者",
    },
    comments: "确认输入与引用链，可以作为下游采用版本。",
    artifactIds,
    createdAt,
  };
}

function demoReviewsFor(
  project: ProjectDescriptor,
  stageId: string,
): readonly ReviewRecord[] {
  if (project.projectId !== "project_moon_city") return [];
  const base: Record<string, readonly ReviewRecord[]> = {
    brief: [
      createDemoReview(
        project.projectId,
        "brief",
        "run_brief_003",
        "review_brief_003",
        ["artifact_brief_003"],
        "2026-07-15T01:20:00.000Z",
      ),
    ],
    research: [
      createDemoReview(
        project.projectId,
        "research",
        "run_research_002",
        "review_research_002",
        ["artifact_research_002"],
        "2026-07-16T02:42:00.000Z",
      ),
    ],
    script: [
      createDemoReview(
        project.projectId,
        "script",
        "run_script_003",
        "review_script_003",
        ["artifact_script_003"],
        "2026-07-16T10:50:00.000Z",
      ),
      ...demoAdditionalReviews,
    ],
    audio: [
      createDemoReview(
        project.projectId,
        "audio",
        "run_audio_002",
        "review_audio_002",
        ["artifact_audio_002"],
        "2026-07-16T12:15:00.000Z",
      ),
    ],
  };
  return base[stageId] ?? [];
}

function demoWorkflow(project: ProjectDescriptor): WorkflowSnapshotView {
  const isMoonProject = project.projectId === "project_moon_city";
  const scriptConfigChanged = demoConfigFor(project, "script").revision > 4;
  const states: WorkflowStageState[] = STANDARD_STAGE_SPECS.map(([stageId], index) => {
    if (!isMoonProject) {
      return {
        stageId,
        status: index === 0 ? "ready" : "draft",
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "brief" || stageId === "research") {
      const runId = stageId === "brief" ? "run_brief_003" : "run_research_002";
      return {
        stageId,
        status: "approved",
        approvedRunId: runId,
        latestRunId: runId,
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "script") {
      if (scriptConfigChanged && demoApprovedScriptRunId) {
        return {
          stageId,
          status: "stale",
          approvedRunId: demoApprovedScriptRunId,
          latestRunId: "run_script_004",
          staleBecauseStageIds: ["script"],
        };
      }
      if (demoApprovedScriptRunId === "run_script_004") {
        return {
          stageId,
          status: "approved",
          approvedRunId: "run_script_004",
          latestRunId: "run_script_004",
          staleBecauseStageIds: [],
        };
      }
      return {
        stageId,
        status: "needs_review",
        approvedRunId: demoApprovedScriptRunId,
        latestRunId: "run_script_004",
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "audio") {
      if (demoApprovedScriptRunId !== "run_script_003" || scriptConfigChanged) {
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
        status: "approved",
        approvedRunId: "run_audio_002",
        latestRunId: "run_audio_002",
        staleBecauseStageIds: [],
      };
    }
    if (stageId === "captions" && demoApprovedScriptRunId === "run_script_003") {
      return { stageId, status: "ready", staleBecauseStageIds: [] };
    }
    if (stageId === "scene_plan" && demoApprovedScriptRunId && !scriptConfigChanged) {
      return { stageId, status: "ready", staleBecauseStageIds: [] };
    }
    return { stageId, status: "draft", staleBecauseStageIds: [] };
  });

  return {
    apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
    ownerProjectId: project.projectId,
    workflowDefinitionId: project.workflowDefinitionId,
    stageDefinitions: demoDefinitions(),
    stageStates: states,
    configs: STANDARD_STAGE_SPECS.map(([stageId]) => demoConfigFor(project, stageId)),
  };
}

function demoJobs(project: ProjectDescriptor): readonly WorkbenchJob[] {
  if (project.projectId !== "project_moon_city") return [];
  return demoJobRecords.map((record) => record.job);
}

function demoEvents(): readonly WorkbenchEvent[] {
  const latestJob = demoJobRecords[0]?.job;
  const pendingEvent: WorkbenchEvent[] = latestJob
    ? [
        {
          eventId: `event_${latestJob.jobId}`,
          sequence: 19,
          kind:
            latestJob.status === "canceled"
              ? "job.cancel.requested"
              : "job.queued",
          message:
            latestJob.status === "canceled"
              ? "停止请求已记录；历史运行和执行快照均被保留。"
              : `${latestJob.runId} 已排队，使用选中历史版本的输入与执行器快照。`,
          createdAt: latestJob.updatedAt,
          progress: latestJob.progress,
          tone: latestJob.status === "canceled" ? "warning" : "active",
        },
      ]
    : [];
  return [
    ...pendingEvent,
    {
      eventId: "event_completed",
      sequence: 18,
      kind: "stage.completed",
      message: "run_script_004 已提交，等待人工审核。",
      createdAt: demoNow,
      tone: "approved",
    },
    {
      eventId: "event_artifact",
      sequence: 17,
      kind: "artifact.created",
      message: "script.json 与 citation-report.json 已进入 Artifact Store。",
      createdAt: "2026-07-17T02:24:17.806Z",
      artifactId: "artifact_script_004",
      tone: "approved",
    },
    {
      eventId: "event_warning",
      sequence: 16,
      kind: "job.warning",
      message: "1 条主张只有间接证据，已保留在日志摘要。",
      createdAt: "2026-07-17T02:23:46.087Z",
      artifactId: "claim_C07",
      tone: "warning",
    },
    {
      eventId: "event_snapshot",
      sequence: 15,
      kind: "config.snapshot",
      message: "配置 rev 4 与已采用上游输入已冻结。",
      createdAt: "2026-07-17T02:22:09.102Z",
      tone: "muted",
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

function demoArtifactFor(artifactId: string): WorkbenchArtifact {
  const definitions: Record<
    string,
    Omit<WorkbenchArtifact, "artifactId" | "metadataUri" | "contentUri">
  > = {
    artifact_brief_003: {
      kind: "creative_brief",
      mediaType: "application/json",
      byteLength: 2_418,
      contentHash: `sha256:${"3".repeat(64)}`,
      evidenceRole: "non_evidence",
      sourceOrigin: "generated",
      sourceLabel: "local-codex · gpt-5-codex",
      provenance: [],
      contentAvailable: true,
      demoPreview: "受众：大众科普\n目标：解释月球城市的系统性困难\n边界：不把生成式素材作为事实证据",
    },
    artifact_research_002: {
      kind: "claim_evidence_set",
      mediaType: "application/json",
      byteLength: 18_902,
      contentHash: `sha256:${"4".repeat(64)}`,
      evidenceRole: "factual_evidence",
      sourceOrigin: "derived",
      sourceLabel: "派生自已授权研究资料",
      provenance: [
        { claimId: "C01", evidenceRef: "E03" },
        { claimId: "C04", evidenceRef: "E08" },
        { claimId: "C07", evidenceRef: "E11" },
      ],
      contentAvailable: true,
      demoPreview: "C01 月尘会磨损设备 · E03\nC04 月球昼夜周期增加储能压力 · E08\nC07 封闭系统需要冗余 · E11",
    },
    artifact_script_004: {
      kind: "narration_script",
      mediaType: "application/json",
      byteLength: 8_742,
      contentHash: `sha256:${"5".repeat(64)}`,
      evidenceRole: "non_evidence",
      sourceOrigin: "generated",
      sourceLabel: "local-codex · gpt-5-codex",
      provenance: [
        { claimId: "C01", evidenceRef: "E03" },
        { claimId: "C04", evidenceRef: "E08" },
        { claimId: "C07", evidenceRef: "E11" },
      ],
      contentAvailable: true,
      demoPreview:
        "为什么月球城市比想象中更难\n\n我们习惯把月球基地想象成一座等待施工的城市。但真正困难的，并不是把房子运上去。\n\n月壤细小、尖锐，而且带有静电。它会进入密封结构、磨损设备，也可能影响人的呼吸系统。\n\n更关键的是能源。月球昼夜各持续约十四个地球日，因此需要长期储能或极区选址。\n\n所以，月球城市更像一艘不能返航的巨型飞船：每个系统都必须可维修、可冗余。",
    },
    artifact_citations_004: {
      kind: "citation_report",
      mediaType: "application/json",
      byteLength: 3_168,
      contentHash: `sha256:${"6".repeat(64)}`,
      evidenceRole: "non_evidence",
      sourceOrigin: "derived",
      sourceLabel: "派生自事实脚本与研究产物",
      provenance: [
        { claimId: "C01", evidenceRef: "E03" },
        { claimId: "C04", evidenceRef: "E08" },
        { claimId: "C07", evidenceRef: "E11" },
      ],
      contentAvailable: true,
      demoPreview: "引用覆盖率：4 / 4\n直接证据：3\n间接证据：1（C07）\n无引用事实陈述：0",
    },
    artifact_script_003: {
      kind: "narration_script",
      mediaType: "application/json",
      byteLength: 6_924,
      contentHash: `sha256:${"7".repeat(64)}`,
      evidenceRole: "non_evidence",
      sourceOrigin: "generated",
      sourceLabel: "openai-api · gpt-5",
      provenance: [
        { claimId: "C01", evidenceRef: "E03" },
        { claimId: "C04", evidenceRef: "E06" },
      ],
      contentAvailable: true,
      demoPreview:
        "月球城市，为什么不只是盖房子\n\n人类已经能把设备送上月球，但建造一座长期城市，远不只是运输和施工问题。\n\n月尘会磨损设备，也会给宇航员带来麻烦。\n\n月球上漫长的白昼与黑夜，让能源系统必须承受远超过地球的连续周期。",
    },
    artifact_audio_002: {
      kind: "narration_audio",
      mediaType: "audio/wav",
      byteLength: 31_482_112,
      contentHash: `sha256:${"8".repeat(64)}`,
      evidenceRole: "non_evidence",
      sourceOrigin: "generated",
      sourceLabel: "local-tts · voice-authorized-01",
      provenance: [
        { claimId: "C01", evidenceRef: "E03" },
        { claimId: "C04", evidenceRef: "E06" },
      ],
      contentAvailable: true,
      demoPreview: "WAV · 02:54 · 48 kHz · 授权记录 voice_consent_001",
    },
  };
  const definition = definitions[artifactId];
  if (!definition) {
    return {
      artifactId,
      kind: "unknown",
      provenance: [],
      contentAvailable: false,
      loadError: "演示数据中没有该产物元数据。",
    };
  }
  return {
    artifactId,
    ...definition,
    metadataUri: `demo://artifacts/${artifactId}/artifact.json`,
    contentUri: `demo://artifacts/${artifactId}/content`,
  };
}

function demoAffectedStages(
  project: ProjectDescriptor,
  changedStageId: string,
): readonly AffectedStage[] {
  const definitions = demoDefinitions();
  const distances = new Map<string, number>([[changedStageId, 0]]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const definition of definitions) {
      if (distances.has(definition.stageId)) continue;
      const upstreamDistances = definition.dependencies
        .map((dependency) => distances.get(dependency))
        .filter((distance): distance is number => distance !== undefined);
      if (upstreamDistances.length === 0) continue;
      distances.set(definition.stageId, Math.min(...upstreamDistances) + 1);
      changed = true;
    }
  }
  const states = new Map(
    demoWorkflow(project).stageStates.map((state) => [state.stageId, state]),
  );
  return definitions
    .filter((definition) => distances.has(definition.stageId))
    .map((definition) => {
      const distance = distances.get(definition.stageId) ?? 0;
      const state = states.get(definition.stageId);
      return {
        stageId: definition.stageId,
        distance,
        directCauseStageIds:
          distance === 0
            ? [changedStageId]
            : definition.dependencies.filter(
                (dependency) => distances.get(dependency) === distance - 1,
              ),
        currentStatus: state?.status ?? "draft",
        hasApprovedRun: Boolean(state?.approvedRunId),
        supportsPartialRegeneration: definition.supportsPartialRegeneration,
      } satisfies AffectedStage;
    })
    .sort((left, right) => left.distance - right.distance);
}

function demoReviewReplayMatches(
  review: ReviewRecord,
  run: StageRun,
  intent: StageReviewIntent,
): boolean {
  return (
    review.projectId === run.projectId &&
    review.stageId === run.stageId &&
    review.runId === run.runId &&
    review.decision === intent.decision &&
    JSON.stringify(review.reviewer) === JSON.stringify(intent.reviewer) &&
    review.comments === intent.comments &&
    JSON.stringify(review.artifactIds) === JSON.stringify(intent.artifactIds)
  );
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
  async cancelJob(project, jobId) {
    const index = demoJobRecords.findIndex((record) => record.job.jobId === jobId);
    if (index < 0) throw new Error(`找不到任务 ${jobId}。`);
    const record = demoJobRecords[index];
    if (["queued", "running", "retrying"].includes(record.job.status)) {
      demoJobRecords[index] = {
        intent: record.intent,
        job: {
          ...record.job,
          status: "canceled",
          message: "用户已请求停止；运行预留与历史不会被覆盖。",
          cancellationRequested: true,
          updatedAt: new Date().toISOString(),
        },
      };
    }
    return demoBundle(project);
  },
  async recoverWorkspace(project) {
    return demoBundle(project, { ...EMPTY_RECOVERY, reindexed: 1 });
  },
  async loadStageStudio(project, stageId) {
    const config = demoConfigFor(project, stageId);
    assertStageOwnership(project, stageId, config.projectId, config.stageId);
    return {
      stageId,
      config,
      runs: demoRunsFor(project, stageId),
      reviews: demoReviewsFor(project, stageId),
      mode: "demo",
    };
  },
  async loadRunArtifacts(project, run) {
    assertStageOwnership(project, run.stageId, run.projectId, run.stageId);
    const knownRun = demoRunsFor(project, run.stageId).find(
      (candidate) => candidate.runId === run.runId,
    );
    if (!knownRun) throw new Error("演示历史中找不到该运行。");
    const artifactIds = [...new Set(knownRun.artifactIds)];
    return {
      runId: run.runId,
      total: artifactIds.length,
      truncated: artifactIds.length > 24,
      items: artifactIds.slice(0, 24).map(demoArtifactFor),
    };
  },
  async updateStageConfig(project, config, change) {
    assertStageOwnership(project, config.stageId, config.projectId, config.stageId);
    const current = demoConfigFor(project, config.stageId);
    if (current.revision !== config.revision) {
      throw new Error(
        `阶段配置修订冲突：期望 ${config.revision}，实际 ${current.revision}。`,
      );
    }
    const decisions = [...config.decisions, change.decision];
    if (decisions.length > 256) {
      throw new Error("阶段配置决策记录已达到 256 条上限。");
    }
    const nextConfig = createDemoConfig(
      project.projectId,
      config.stageId,
      config.revision + 1,
      change.values,
      decisions,
      new Date().toISOString(),
    );
    demoConfigOverrides.set(
      demoConfigKey(project.projectId, config.stageId),
      nextConfig,
    );
    return {
      apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
      ownerProjectId: project.projectId,
      config: nextConfig,
      configUri: `stages/${config.stageId}/config.json`,
      affectedStages: demoAffectedStages(project, config.stageId),
    };
  },
  async reviewStageRun(project, run, intent) {
    assertStageOwnership(project, run.stageId, run.projectId, run.stageId);
    const knownRun = demoRunsFor(project, run.stageId).find(
      (candidate) => candidate.runId === run.runId,
    );
    if (!knownRun) throw new Error("演示历史中找不到该运行。");
    validateReviewIntent(knownRun, intent);
    const existing = demoReviewsFor(project, run.stageId).find(
      (review) => review.reviewId === intent.reviewId,
    );
    if (existing && !demoReviewReplayMatches(existing, knownRun, intent)) {
      throw new Error("reviewId 已被不同的审核内容占用。");
    }
    if (existing) {
      return {
        apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
        ownerProjectId: project.projectId,
        review: existing,
        reviewUri: `runs/${run.stageId}/${run.runId}/reviews/${intent.reviewId}.json`,
        stageStates: demoWorkflow(project).stageStates,
        invalidatedStageIds: [],
        applied: false,
        idempotentReplay: true,
      };
    }
    const review: ReviewRecord = {
      schemaVersion: NARRACUT_CONTRACT_VERSION,
      documentType: "review_record",
      reviewId: intent.reviewId,
      projectId: project.projectId,
      stageId: run.stageId,
      runId: run.runId,
      decision: intent.decision,
      reviewer: intent.reviewer,
      comments: intent.comments,
      artifactIds: intent.artifactIds,
      createdAt: new Date().toISOString(),
    };
    demoAdditionalReviews = [...demoAdditionalReviews, review];
    const approvalBefore = demoApprovedScriptRunId;
    if (run.stageId === "script") {
      if (intent.decision === "approved") {
        demoApprovedScriptRunId = run.runId;
      } else if (demoApprovedScriptRunId === run.runId) {
        demoApprovedScriptRunId = undefined;
      }
    }
    const invalidatedStageIds =
      run.stageId === "script" && approvalBefore !== demoApprovedScriptRunId
        ? ["audio"]
        : [];
    return {
      apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
      ownerProjectId: project.projectId,
      review,
      reviewUri: `runs/${run.stageId}/${run.runId}/reviews/${intent.reviewId}.json`,
      stageStates: demoWorkflow(project).stageStates,
      invalidatedStageIds,
      applied: true,
      idempotentReplay: false,
    };
  },
  async previewStageRegeneration(project, stageId) {
    return {
      changedStageIds: [stageId],
      affectedStages: demoAffectedStages(project, stageId),
    };
  },
  async regenerateStage(project, sourceRun, intent) {
    assertStageOwnership(
      project,
      sourceRun.stageId,
      sourceRun.projectId,
      sourceRun.stageId,
    );
    const sameIntent = demoJobRecords.find(
      (record) =>
        record.intent.runId === intent.runId &&
        record.intent.idempotencyKey === intent.idempotencyKey,
    );
    if (sameIntent) return sameIntent.job;
    if (
      demoJobRecords.some(
        (record) =>
          record.intent.runId === intent.runId ||
          record.intent.idempotencyKey === intent.idempotencyKey,
      )
    ) {
      throw new Error("重生成 runId 或 idempotencyKey 已被不同请求占用。");
    }
    const knownRun = demoRunsFor(project, sourceRun.stageId).find(
      (candidate) => candidate.runId === sourceRun.runId,
    );
    if (!knownRun) throw new Error("不能从未知历史运行发起重生成。");
    const job: WorkbenchJob = {
      jobId: `job_${intent.runId.replace(/^run_/, "")}`,
      runId: intent.runId,
      stageId: sourceRun.stageId,
      status: "queued",
      attempt: 1,
      progress: 0,
      message: `已冻结 ${sourceRun.runId} 的输入与执行器快照，等待 worker。`,
      cancellationRequested: false,
      artifactIds: [],
      indexSynchronized: true,
      updatedAt: new Date().toISOString(),
    };
    demoJobRecords = [{ intent, job }, ...demoJobRecords];
    return job;
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
