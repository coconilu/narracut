import { useCallback, useEffect, useState } from "react";
import type { RecentProject } from "@narracut/contracts";
import {
  describeDesktopError,
  desktopGateway,
  type WorkspaceBundle,
} from "../../lib/desktop-gateway";
import type { CreateProjectInput } from "../../lib/project-commands";
import { createRequestGate } from "./request-gate.js";

export type ProjectDrawerMode = "create" | "open" | null;

export interface AppController {
  readonly recentProjects: readonly RecentProject[];
  readonly workspace: WorkspaceBundle | null;
  readonly drawerMode: ProjectDrawerMode;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly gatewayMode: "desktop" | "demo";
  readonly setDrawerMode: (mode: ProjectDrawerMode) => void;
  readonly clearError: () => void;
  readonly createProject: (input: CreateProjectInput) => Promise<void>;
  readonly openProjectPath: (projectPath: string) => Promise<void>;
  readonly openRecentProject: (project: RecentProject) => Promise<void>;
  readonly closeWorkspace: () => void;
  readonly refreshWorkspace: () => Promise<void>;
  readonly cancelJob: (jobId: string) => Promise<boolean>;
  readonly recoverWorkspace: () => Promise<boolean>;
}

export function useAppController(): AppController {
  const [requestGate] = useState(createRequestGate);
  const [recentProjects, setRecentProjects] = useState<readonly RecentProject[]>([]);
  const [workspace, setWorkspace] = useState<WorkspaceBundle | null>(null);
  const [drawerMode, setDrawerMode] = useState<ProjectDrawerMode>("create");
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadRecentProjects = useCallback(async () => {
    const request = requestGate.begin();
    try {
      const projects = await desktopGateway.listRecentProjects();
      if (!request.isCurrent()) return;
      setRecentProjects(projects);
    } catch (reason) {
      if (!request.isCurrent()) return;
      setError(describeDesktopError(reason));
    }
  }, [requestGate]);

  useEffect(() => {
    void loadRecentProjects();
  }, [loadRecentProjects]);

  const runBusy = useCallback(
    async <Result>(
      label: string,
      operation: () => Promise<Result>,
      commit: (result: Result) => void,
    ): Promise<boolean> => {
      const request = requestGate.begin();
      setBusyLabel(label);
      setError(null);
      try {
        const result = await operation();
        if (!request.isCurrent()) return false;
        commit(result);
        return true;
      } catch (reason) {
        if (!request.isCurrent()) return false;
        setError(describeDesktopError(reason));
        return false;
      } finally {
        if (request.isCurrent()) setBusyLabel(null);
      }
    },
    [requestGate],
  );

  const createProject = useCallback(
    async (input: CreateProjectInput) => {
      await runBusy("正在创建本地工程…", async () => {
        const project = await desktopGateway.createProject(input);
        await desktopGateway.initializeWorkflow(project);
        return desktopGateway.openWorkspace(project);
      }, (nextWorkspace) => {
        setWorkspace(nextWorkspace);
        setDrawerMode(null);
      });
    },
    [runBusy],
  );

  const openProjectPath = useCallback(
    async (projectPath: string) => {
      await runBusy("正在检查并打开工程…", async () => {
        const project = await desktopGateway.openProject(projectPath.trim());
        return desktopGateway.openWorkspace(project);
      }, (nextWorkspace) => {
        setWorkspace(nextWorkspace);
        setDrawerMode(null);
      });
    },
    [runBusy],
  );

  const openRecentProject = useCallback(
    async (project: RecentProject) => {
      if (!project.pathAvailable) {
        setError(`项目路径不可用：${project.projectPath}`);
        setDrawerMode("open");
        return;
      }
      await openProjectPath(project.projectPath);
    },
    [openProjectPath],
  );

  const closeWorkspace = useCallback(() => {
    requestGate.invalidate();
    setBusyLabel(null);
    setWorkspace(null);
    setDrawerMode(null);
    setError(null);
    void loadRecentProjects();
  }, [loadRecentProjects, requestGate]);

  const refreshWorkspace = useCallback(async () => {
    if (!workspace) return;
    await runBusy(
      "正在刷新工程状态…",
      () => desktopGateway.refreshWorkspace(workspace.project),
      setWorkspace,
    );
  }, [runBusy, workspace]);

  const cancelJob = useCallback(
    async (jobId: string) => {
      if (!workspace) return false;
      return runBusy(
        "正在记录停止请求…",
        () => desktopGateway.cancelJob(workspace.project, jobId),
        setWorkspace,
      );
    },
    [runBusy, workspace],
  );

  const recoverWorkspace = useCallback(async () => {
    if (!workspace) return false;
    return runBusy(
      "正在恢复可恢复任务…",
      () => desktopGateway.recoverWorkspace(workspace.project),
      setWorkspace,
    );
  }, [runBusy, workspace]);

  return {
    recentProjects,
    workspace,
    drawerMode,
    busyLabel,
    error,
    gatewayMode: desktopGateway.mode,
    setDrawerMode,
    clearError: () => setError(null),
    createProject,
    openProjectPath,
    openRecentProject,
    closeWorkspace,
    refreshWorkspace,
    cancelJob,
    recoverWorkspace,
  };
}
