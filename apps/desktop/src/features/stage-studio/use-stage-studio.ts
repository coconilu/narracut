import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  DecisionRecord,
  ProjectDescriptor,
  ReviewDecision,
  StageRun,
  WorkflowStageState,
} from "@narracut/contracts";
import {
  describeDesktopError,
  desktopGateway,
  type RegenerationPreview,
  type RunArtifactCollection,
  type StageRegenerationIntent,
  type StageStudioSnapshot,
} from "../../lib/desktop-gateway";
import type { WorkflowSnapshotView } from "../../lib/workflow-commands";
import { createRequestGate } from "../app/request-gate.js";
import {
  canReviewRun,
  chooseRunIds,
  parseConfigDraft,
  reuseStableIntent,
  sortRunsNewestFirst,
  uniqueArtifactIds,
} from "./stage-studio-model.js";

export type StageStudioTab =
  | "input"
  | "config"
  | "output"
  | "preview"
  | "history"
  | "compare"
  | "review";

interface StableReviewIntent {
  readonly signature: string;
  readonly reviewId: string;
  readonly createdAt: string;
}

interface StableConfigIntent {
  readonly signature: string;
  readonly decision: DecisionRecord;
}

interface StableRegenerationRequest extends StageRegenerationIntent {
  readonly signature: string;
}

interface UseStageStudioInput {
  readonly project: ProjectDescriptor;
  readonly workflow: WorkflowSnapshotView;
  readonly stageId: string;
  readonly supportsRegeneration: boolean;
  readonly onRefreshWorkspace: () => Promise<boolean>;
}

export interface StageStudioController {
  readonly activeTab: StageStudioTab;
  readonly snapshot: StageStudioSnapshot | null;
  readonly selectedRun?: StageRun;
  readonly compareRun?: StageRun;
  readonly selectedRunId?: string;
  readonly compareRunId?: string;
  readonly artifactsByRun: Readonly<Record<string, RunArtifactCollection>>;
  readonly artifactLoading: boolean;
  readonly configDraft: string;
  readonly configRationale: string;
  readonly reviewDecision: ReviewDecision;
  readonly reviewComments: string;
  readonly selectedArtifactIds: readonly string[];
  readonly regenerationImpact: RegenerationPreview | null;
  readonly busyLabel: string | null;
  readonly notice: string | null;
  readonly error: string | null;
  readonly setActiveTab: (tab: StageStudioTab) => void;
  readonly setSelectedRunId: (runId: string) => void;
  readonly setCompareRunId: (runId: string) => void;
  readonly setConfigDraft: (value: string) => void;
  readonly setConfigRationale: (value: string) => void;
  readonly setReviewDecision: (decision: ReviewDecision) => void;
  readonly setReviewComments: (value: string) => void;
  readonly toggleArtifact: (artifactId: string) => void;
  readonly saveConfig: () => Promise<boolean>;
  readonly submitReview: () => Promise<boolean>;
  readonly previewRegeneration: () => Promise<boolean>;
  readonly queueRegeneration: () => Promise<boolean>;
  readonly refreshStage: () => Promise<boolean>;
  readonly clearNotice: () => void;
  readonly clearError: () => void;
}

function portableId(prefix: string): string {
  return `${prefix}${crypto.randomUUID().replace(/-/g, "").slice(0, 20)}`;
}

function stateForStage(
  workflow: WorkflowSnapshotView,
  stageId: string,
): WorkflowStageState | undefined {
  return workflow.stageStates.find((state) => state.stageId === stageId);
}

export function useStageStudio({
  project,
  workflow,
  stageId,
  supportsRegeneration,
  onRefreshWorkspace,
}: UseStageStudioInput): StageStudioController {
  const [stageRequestGate] = useState(createRequestGate);
  const [artifactRequestGate] = useState(createRequestGate);
  const [actionRequestGate] = useState(createRequestGate);
  const [activeTab, setActiveTab] = useState<StageStudioTab>("preview");
  const [snapshot, setSnapshot] = useState<StageStudioSnapshot | null>(null);
  const [selectedRunId, setSelectedRunIdState] = useState<string>();
  const [compareRunId, setCompareRunIdState] = useState<string>();
  const [artifactsByRun, setArtifactsByRun] = useState<
    Readonly<Record<string, RunArtifactCollection>>
  >({});
  const [artifactLoading, setArtifactLoading] = useState(false);
  const [configDraft, setConfigDraft] = useState("{}");
  const [configRationale, setConfigRationale] = useState("");
  const [reviewDecision, setReviewDecision] =
    useState<ReviewDecision>("approved");
  const [reviewComments, setReviewComments] = useState("");
  const [selectedArtifactIds, setSelectedArtifactIds] = useState<readonly string[]>(
    [],
  );
  const [regenerationImpact, setRegenerationImpact] =
    useState<RegenerationPreview | null>(null);
  const [loadingLabel, setLoadingLabel] = useState<string | null>(null);
  const [actionLabel, setActionLabel] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reviewIntentRef = useRef<StableReviewIntent | null>(null);
  const configIntentRef = useRef<StableConfigIntent | null>(null);
  const regenerationIntentRef = useRef<StableRegenerationRequest | null>(null);
  const actionInFlightRef = useRef(false);
  const workflowRef = useRef(workflow);
  workflowRef.current = workflow;

  useEffect(
    () => () => {
      actionRequestGate.invalidate();
    },
    [actionRequestGate],
  );

  const commitSnapshot = useCallback(
    (nextSnapshot: StageStudioSnapshot) => {
      const runs = sortRunsNewestFirst(nextSnapshot.runs);
      const currentState = stateForStage(workflowRef.current, nextSnapshot.stageId);
      const selection = chooseRunIds(
        runs,
        currentState?.latestRunId,
        currentState?.approvedRunId,
      );
      const selectedRun = runs.find(
        (run) => run.runId === selection.selectedRunId,
      );
      setSnapshot({ ...nextSnapshot, runs });
      setSelectedRunIdState(selection.selectedRunId);
      setCompareRunIdState(selection.compareRunId);
      setConfigDraft(JSON.stringify(nextSnapshot.config.values, null, 2));
      setConfigRationale("");
      setSelectedArtifactIds(uniqueArtifactIds(selectedRun));
      setArtifactsByRun({});
      setRegenerationImpact(null);
    },
    [],
  );

  const loadStage = useCallback(
    async (requestedStageId: string): Promise<boolean> => {
      const request = stageRequestGate.begin();
      artifactRequestGate.invalidate();
      setLoadingLabel("正在读取阶段历史…");
      setError(null);
      try {
        const nextSnapshot = await desktopGateway.loadStageStudio(
          project,
          requestedStageId,
        );
        if (!request.isCurrent()) return false;
        commitSnapshot(nextSnapshot);
        return true;
      } catch (reason) {
        if (!request.isCurrent()) return false;
        setSnapshot(null);
        setError(describeDesktopError(reason));
        return false;
      } finally {
        if (request.isCurrent()) setLoadingLabel(null);
      }
    },
    [artifactRequestGate, commitSnapshot, project, stageRequestGate],
  );

  useEffect(() => {
    reviewIntentRef.current = null;
    configIntentRef.current = null;
    regenerationIntentRef.current = null;
    void loadStage(stageId);
    return () => {
      stageRequestGate.invalidate();
      artifactRequestGate.invalidate();
    };
  }, [artifactRequestGate, loadStage, stageId, stageRequestGate]);

  const selectedRun = useMemo(
    () => snapshot?.runs.find((run) => run.runId === selectedRunId),
    [selectedRunId, snapshot?.runs],
  );
  const compareRun = useMemo(
    () => snapshot?.runs.find((run) => run.runId === compareRunId),
    [compareRunId, snapshot?.runs],
  );

  useEffect(() => {
    const targets = [selectedRun, compareRun].filter(
      (run, index, values): run is StageRun =>
        Boolean(run) && values.findIndex((value) => value?.runId === run?.runId) === index,
    );
    const request = artifactRequestGate.begin();
    if (targets.length === 0) {
      setArtifactsByRun({});
      setArtifactLoading(false);
      return () => {
        artifactRequestGate.invalidate();
      };
    }
    setArtifactLoading(true);
    void Promise.all(
      targets.map((run) => desktopGateway.loadRunArtifacts(project, run)),
    )
      .then((collections) => {
        if (!request.isCurrent()) return;
        setArtifactsByRun(
          Object.fromEntries(
            collections.map((collection) => [collection.runId, collection]),
          ),
        );
      })
      .catch((reason: unknown) => {
        if (request.isCurrent()) setError(describeDesktopError(reason));
      })
      .finally(() => {
        if (request.isCurrent()) setArtifactLoading(false);
      });
    return () => artifactRequestGate.invalidate();
  }, [artifactRequestGate, compareRun, project, selectedRun]);

  const reconcileAfterMutation = useCallback(
    async (requestedStageId: string) => {
      const workspaceRefreshed = await onRefreshWorkspace();
      const stageRefreshed = await loadStage(requestedStageId);
      if (!workspaceRefreshed || !stageRefreshed) {
        throw new Error(
          "操作已提交，但未能完整重读工作区真相；请刷新后再确认下游状态。",
        );
      }
      return true;
    },
    [loadStage, onRefreshWorkspace],
  );

  const setSelectedRunId = useCallback(
    (runId: string) => {
      const run = snapshot?.runs.find((candidate) => candidate.runId === runId);
      if (!run) return;
      setSelectedRunIdState(runId);
      setSelectedArtifactIds(uniqueArtifactIds(run));
      setRegenerationImpact(null);
      reviewIntentRef.current = null;
      regenerationIntentRef.current = null;
    },
    [snapshot?.runs],
  );

  const setCompareRunId = useCallback(
    (runId: string) => {
      if (!snapshot?.runs.some((run) => run.runId === runId)) return;
      setCompareRunIdState(runId);
    },
    [snapshot?.runs],
  );

  const toggleArtifact = useCallback((artifactId: string) => {
    setSelectedArtifactIds((current) =>
      current.includes(artifactId)
        ? current.filter((id) => id !== artifactId)
        : [...current, artifactId],
    );
    reviewIntentRef.current = null;
  }, []);

  const saveConfig = useCallback(async (): Promise<boolean> => {
    if (!snapshot || actionInFlightRef.current || actionLabel) return false;
    const rationale = configRationale.trim();
    if (!rationale) {
      setError("保存配置前请填写本次变更理由，以便历史追溯。");
      return false;
    }
    let values;
    try {
      values = parseConfigDraft(configDraft);
    } catch (reason) {
      setError(describeDesktopError(reason));
      return false;
    }
    const signature = JSON.stringify({
      projectId: project.projectId,
      stageId: snapshot.stageId,
      revision: snapshot.config.revision,
      values,
      rationale,
    });
    configIntentRef.current = reuseStableIntent<{ decision: DecisionRecord }>(
      configIntentRef.current,
      signature,
      () => ({
        decision: {
          decisionId: portableId("decision_ui_"),
          key: "ui_config_update",
          value: values,
          rationale,
          madeBy: "local_user",
          madeAt: new Date().toISOString(),
        },
      }),
    );
    const intent = configIntentRef.current;
    actionInFlightRef.current = true;
    const request = actionRequestGate.begin();
    setActionLabel("正在保存配置快照…");
    setError(null);
    try {
      await desktopGateway.updateStageConfig(project, snapshot.config, {
        values,
        decision: intent.decision,
      });
      if (!request.isCurrent()) return false;
      await reconcileAfterMutation(snapshot.stageId);
      if (!request.isCurrent()) return false;
      configIntentRef.current = null;
      setNotice("配置已生成新修订；旧运行仍保留原始配置快照。");
      return true;
    } catch (reason) {
      if (!request.isCurrent()) return false;
      try {
        const reconciled = await desktopGateway.loadStageStudio(
          project,
          snapshot.stageId,
        );
        const applied = reconciled.config.decisions.some(
          (decision) => decision.decisionId === intent.decision.decisionId,
        );
        if (applied) {
          const workspaceRefreshed = await onRefreshWorkspace();
          if (!request.isCurrent()) return false;
          commitSnapshot(reconciled);
          if (!workspaceRefreshed) {
            setError("配置已写入，但工作区状态刷新失败；请手动刷新确认影响范围。");
            return false;
          }
          configIntentRef.current = null;
          setNotice("配置响应中断，但已通过工程真相重读确认写入成功。");
          return true;
        }
        commitSnapshot(reconciled);
      } catch {
        // 保留原错误；下一次手动刷新仍会从项目真相重读。
      }
      setError(describeDesktopError(reason));
      return false;
    } finally {
      actionInFlightRef.current = false;
      if (request.isCurrent()) setActionLabel(null);
    }
  }, [
    actionLabel,
    actionRequestGate,
    commitSnapshot,
    configDraft,
    configRationale,
    onRefreshWorkspace,
    project,
    reconcileAfterMutation,
    snapshot,
  ]);

  const submitReview = useCallback(async (): Promise<boolean> => {
    if (!snapshot || !selectedRun || actionInFlightRef.current || actionLabel) {
      return false;
    }
    if (!canReviewRun(selectedRun) && reviewDecision === "approved") {
      setError("只有 succeeded 的历史运行可以被采用。");
      return false;
    }
    const comments = reviewComments.trim();
    if (!comments) {
      setError("请填写审核意见，明确采用、修改或拒绝的依据。");
      return false;
    }
    const artifactIds = selectedRun.artifactIds.filter((artifactId) =>
      selectedArtifactIds.includes(artifactId),
    );
    if (reviewDecision === "approved" && artifactIds.length === 0) {
      setError("采用运行时必须明确选择至少一个产物。");
      return false;
    }
    const signature = JSON.stringify({
      projectId: project.projectId,
      stageId: selectedRun.stageId,
      runId: selectedRun.runId,
      decision: reviewDecision,
      comments,
      artifactIds,
    });
    reviewIntentRef.current = reuseStableIntent(
      reviewIntentRef.current,
      signature,
      () => ({
        reviewId: portableId("review_ui_"),
        createdAt: new Date().toISOString(),
      }),
    );
    const intent = reviewIntentRef.current;
    actionInFlightRef.current = true;
    const request = actionRequestGate.begin();
    setActionLabel("正在写入不可变审核记录…");
    setError(null);
    try {
      await desktopGateway.reviewStageRun(project, selectedRun, {
        reviewId: intent.reviewId,
        decision: reviewDecision,
        reviewer: {
          kind: "human",
          reviewerId: "local_user",
          displayName: "本机创作者",
        },
        comments,
        artifactIds,
      });
      if (!request.isCurrent()) return false;
      await reconcileAfterMutation(snapshot.stageId);
      if (!request.isCurrent()) return false;
      reviewIntentRef.current = null;
      setReviewComments("");
      setNotice(
        reviewDecision === "approved"
          ? "候选运行已采用；受影响下游阶段已按工程真相更新。"
          : "审核记录已保存；既有采用版本和历史运行没有被静默覆盖。",
      );
      return true;
    } catch (reason) {
      if (!request.isCurrent()) return false;
      await loadStage(snapshot.stageId);
      if (request.isCurrent()) setError(describeDesktopError(reason));
      return false;
    } finally {
      actionInFlightRef.current = false;
      if (request.isCurrent()) setActionLabel(null);
    }
  }, [
    actionLabel,
    actionRequestGate,
    loadStage,
    project,
    reconcileAfterMutation,
    reviewComments,
    reviewDecision,
    selectedArtifactIds,
    selectedRun,
    snapshot,
  ]);

  const previewRegeneration = useCallback(async (): Promise<boolean> => {
    if (!snapshot || !selectedRun || actionInFlightRef.current || actionLabel) {
      return false;
    }
    if (!supportsRegeneration) {
      setError("当前阶段契约未声明局部重生成能力。");
      return false;
    }
    actionInFlightRef.current = true;
    const request = actionRequestGate.begin();
    setActionLabel("正在计算重生成影响范围…");
    setError(null);
    try {
      const impact = await desktopGateway.previewStageRegeneration(
        project,
        snapshot.stageId,
      );
      if (!request.isCurrent()) return false;
      setRegenerationImpact(impact);
      setActiveTab("history");
      setNotice("影响范围仅为预览；确认前不会创建任务或修改历史。");
      return true;
    } catch (reason) {
      if (request.isCurrent()) setError(describeDesktopError(reason));
      return false;
    } finally {
      actionInFlightRef.current = false;
      if (request.isCurrent()) setActionLabel(null);
    }
  }, [
    actionLabel,
    actionRequestGate,
    project,
    selectedRun,
    snapshot,
    supportsRegeneration,
  ]);

  const queueRegeneration = useCallback(async (): Promise<boolean> => {
    if (
      !snapshot ||
      !selectedRun ||
      !regenerationImpact ||
      actionInFlightRef.current ||
      actionLabel
    ) {
      return false;
    }
    if (!supportsRegeneration) return false;
    const signature = JSON.stringify({
      projectId: project.projectId,
      stageId: selectedRun.stageId,
      sourceRunId: selectedRun.runId,
      configRevision: snapshot.config.revision,
    });
    regenerationIntentRef.current = reuseStableIntent(
      regenerationIntentRef.current,
      signature,
      () => ({
        runId: portableId(`run_${selectedRun.stageId}_ui_`),
        idempotencyKey: portableId("idem_ui_"),
      }),
    );
    const intent = regenerationIntentRef.current;
    actionInFlightRef.current = true;
    const request = actionRequestGate.begin();
    setActionLabel("正在冻结快照并创建任务…");
    setError(null);
    try {
      const job = await desktopGateway.regenerateStage(project, selectedRun, intent);
      if (!request.isCurrent()) return false;
      await reconcileAfterMutation(snapshot.stageId);
      if (!request.isCurrent()) return false;
      regenerationIntentRef.current = null;
      setRegenerationImpact(null);
      setNotice(
        `已创建任务 ${job.jobId}；新 runId 为 ${job.runId}，历史版本未被覆盖。`,
      );
      return true;
    } catch (reason) {
      if (!request.isCurrent()) return false;
      await loadStage(snapshot.stageId);
      if (request.isCurrent()) setError(describeDesktopError(reason));
      return false;
    } finally {
      actionInFlightRef.current = false;
      if (request.isCurrent()) setActionLabel(null);
    }
  }, [
    actionLabel,
    actionRequestGate,
    loadStage,
    project,
    reconcileAfterMutation,
    regenerationImpact,
    selectedRun,
    snapshot,
    supportsRegeneration,
  ]);

  return {
    activeTab,
    snapshot,
    selectedRun,
    compareRun,
    selectedRunId,
    compareRunId,
    artifactsByRun,
    artifactLoading,
    configDraft,
    configRationale,
    reviewDecision,
    reviewComments,
    selectedArtifactIds,
    regenerationImpact,
    busyLabel: actionLabel ?? loadingLabel,
    notice,
    error,
    setActiveTab,
    setSelectedRunId,
    setCompareRunId,
    setConfigDraft,
    setConfigRationale,
    setReviewDecision,
    setReviewComments,
    toggleArtifact,
    saveConfig,
    submitReview,
    previewRegeneration,
    queueRegeneration,
    refreshStage: () => loadStage(stageId),
    clearNotice: () => setNotice(null),
    clearError: () => setError(null),
  };
}
