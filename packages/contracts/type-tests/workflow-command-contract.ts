import {
  NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  type InitializeWorkflowRequest,
  type PrepareStageRunRequest,
  type RecordStageRunRequest,
  type ReviewStageRunRequest,
  type WorkflowCommandError,
  type WorkflowStageState,
} from "../src";

const initializeRequest = {
  apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  command: "initialize_project_workflow",
  projectPath: "C:/Videos/demo",
  expectedProjectId: "project_demo",
} satisfies InitializeWorkflowRequest;

const recordRequest = {
  apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  command: "record_stage_run",
  projectPath: "C:/Videos/demo",
  expectedProjectId: "project_demo",
  stageId: "script",
  runId: "run_script_001",
  status: "succeeded",
  jobId: "job_script_001",
  artifactIds: [],
  logSummary: {},
} satisfies RecordStageRunRequest;

const prepareRequest = {
  apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  command: "prepare_stage_run",
  projectPath: "C:/Videos/demo",
  expectedProjectId: "project_demo",
  stageId: "script",
  runId: "run_script_001",
  jobId: "job_script_001",
  inputRefs: [],
  executor: {},
} satisfies PrepareStageRunRequest;

const reviewRequest = {
  apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  command: "review_stage_run",
  projectPath: "C:/Videos/demo",
  expectedProjectId: "project_demo",
  stageId: "script",
  runId: "run_script_001",
  reviewId: "review_script_001",
  decision: "approved",
  reviewer: {
    kind: "human",
    reviewerId: "user_001",
    displayName: "审核人",
  },
  comments: "通过",
  artifactIds: [],
} satisfies ReviewStageRunRequest;

const staleState = {
  stageId: "script",
  status: "stale",
  approvedRunId: "run_script_001",
  latestRunId: "run_script_001",
  staleBecauseStageIds: ["research"],
} satisfies WorkflowStageState;

const structuredError = {
  apiVersion: NARRACUT_WORKFLOW_COMMAND_API_VERSION,
  code: "stage_not_ready",
  operation: "record_stage_run",
  message: "上游尚未批准",
  stageId: "script",
} satisfies WorkflowCommandError;

void initializeRequest;
void prepareRequest;
void recordRequest;
void reviewRequest;
void staleState;
void structuredError;
