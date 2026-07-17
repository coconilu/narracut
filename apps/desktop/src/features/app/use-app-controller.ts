import { useCallback, useEffect, useState } from "react";
import type { ProjectDescriptor, RecentProject } from "@narracut/contracts";
import {
  describeDesktopError,
  desktopGateway,
  type WorkspaceBundle,
} from "../../lib/desktop-gateway";
import type { CreateProjectInput } from "../../lib/project-commands";

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
  const [recentProjects, setRecentProjects] = useState<readonly RecentProject[]>([]);
  const [workspace, setWorkspace] = useState<WorkspaceBundle | null>(null);
  const [drawerMode, setDrawerMode] = useState<ProjectDrawerMode>("create");
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadRecentProjects = useCallback(async () => {
    try {
      const projects = await desktopGateway.listRecentProjects();
      setRecentProjects(projects);
    } catch (reason) {
      setError(describeDesktopError(reason));
    }
  }, []);

  useEffect(() => {
    void loadRecentProjects();
  }, [loadRecentProjects]);

  const enterWorkspace = useCallback(async (project: ProjectDescriptor) => {
    const nextWorkspace = await desktopGateway.openWorkspace(project);
    setWorkspace(nextWorkspace);
    setDrawerMode(null);
  }, []);

  const runBusy = useCallback(
    async (label: string, operation: () => Promise<void>): Promise<boolean> => {
      setBusyLabel(label);
      setError(null);
      try {
        await operation();
        return true;
      } catch (reason) {
        setError(describeDesktopError(reason));
        return false;
      } finally {
        setBusyLabel(null);
      }
    },
    [],
  );

  const createProject = useCallback(
    async (input: CreateProjectInput) => {
      await runBusy("正在创建本地工程…", async () => {
        const project = await desktopGateway.createProject(input);
        await desktopGateway.initializeWorkflow(project);
        await enterWorkspace(project);
        await loadRecentProjects();
      });
    },
    [enterWorkspace, loadRecentProjects, runBusy],
  );

  const openProjectPath = useCallback(
    async (projectPath: string) => {
      await runBusy("正在检查并打开工程…", async () => {
        const project = await desktopGateway.openProject(projectPath.trim());
        await enterWorkspace(project);
        await loadRecentProjects();
      });
    },
    [enterWorkspace, loadRecentProjects, runBusy],
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
    setWorkspace(null);
    setDrawerMode(null);
    setError(null);
    void loadRecentProjects();
  }, [loadRecentProjects]);

  const refreshWorkspace = useCallback(async () => {
    if (!workspace) return;
    await runBusy("正在刷新工程状态…", async () => {
      setWorkspace(await desktopGateway.refreshWorkspace(workspace.project));
    });
  }, [runBusy, workspace]);

  const cancelJob = useCallback(
    async (jobId: string) => {
      if (!workspace) return false;
      return runBusy("正在记录停止请求…", async () => {
        setWorkspace(await desktopGateway.cancelJob(workspace.project, jobId));
      });
    },
    [runBusy, workspace],
  );

  const recoverWorkspace = useCallback(async () => {
    if (!workspace) return false;
    return runBusy("正在恢复可恢复任务…", async () => {
      setWorkspace(await desktopGateway.recoverWorkspace(workspace.project));
    });
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
