import { useMemo, useState } from "react";
import type { RecentProject } from "@narracut/contracts";
import { Brand } from "../../components/brand";
import { Icon } from "../../components/icons";
import type { CreateProjectInput } from "../../lib/project-commands";
import type { ProjectDrawerMode } from "../app/use-app-controller";
import { ProjectDrawer } from "./project-drawer";

interface ProjectHomeProps {
  readonly projects: readonly RecentProject[];
  readonly drawerMode: ProjectDrawerMode;
  readonly busyLabel: string | null;
  readonly error: string | null;
  readonly gatewayMode: "desktop" | "demo";
  readonly onDrawerModeChange: (mode: ProjectDrawerMode) => void;
  readonly onClearError: () => void;
  readonly onCreate: (input: CreateProjectInput) => Promise<void>;
  readonly onOpenPath: (path: string) => Promise<void>;
  readonly onOpenRecent: (project: RecentProject) => Promise<void>;
}

type ProjectFilter = "recent" | "archived";

export function ProjectHome({
  projects,
  drawerMode,
  busyLabel,
  error,
  gatewayMode,
  onDrawerModeChange,
  onClearError,
  onCreate,
  onOpenPath,
  onOpenRecent,
}: ProjectHomeProps) {
  const [filter, setFilter] = useState<ProjectFilter>("recent");
  const counts = useMemo(
    () => ({
      recent: projects.filter((project) => !project.archived).length,
      archived: projects.filter((project) => project.archived).length,
    }),
    [projects],
  );
  const visibleProjects = useMemo(
    () => projects.filter((project) => project.archived === (filter === "archived")),
    [filter, projects],
  );

  return (
    <main className={`project-home ${drawerMode ? "has-drawer" : ""}`}>
      <header className="home-topbar">
        <Brand />
        <div className="home-topbar-context">
          <span>项目保存在本机 · 工程目录是唯一真相</span>
          <button className="icon-button" aria-label="设置" disabled title="设置将在后续里程碑接入" type="button">
            <Icon name="settings" size={17} />
          </button>
        </div>
      </header>

      <aside className="home-sidebar">
        <span className="sidebar-label">工作区</span>
        <nav aria-label="项目筛选">
          <button
            aria-current={filter === "recent" ? "page" : undefined}
            className={`sidebar-item ${filter === "recent" ? "active" : ""}`}
            onClick={() => setFilter("recent")}
            type="button"
          >
            <Icon name="folder" />
            <span>最近使用</span>
            <span className="sidebar-count">{counts.recent}</span>
          </button>
          <button
            aria-current={filter === "archived" ? "page" : undefined}
            className={`sidebar-item ${filter === "archived" ? "active" : ""}`}
            onClick={() => setFilter("archived")}
            type="button"
          >
            <Icon name="archive" />
            <span>已归档</span>
            <span className="sidebar-count">{counts.archived}</span>
          </button>
        </nav>
      </aside>

      <section className="project-main" aria-labelledby="projects-title">
        <div className="project-heading-row">
          <div>
            <h1 id="projects-title">{filter === "recent" ? "项目" : "已归档项目"}</h1>
            <p>
              {filter === "recent"
                ? "继续上一次创作，或从一个可迁移的本地工程开始。"
                : "归档不会删除本地工程；重新打开后可恢复到工作区。"}
            </p>
          </div>
          <div className="project-actions">
            <button className="button" disabled={busyLabel !== null} onClick={() => onDrawerModeChange("open")} type="button">
              打开项目
            </button>
            <button className="button primary" disabled={busyLabel !== null} onClick={() => onDrawerModeChange("create")} type="button">
              新建项目
            </button>
          </div>
        </div>

        {error ? (
          <div className="inline-alert" role="alert">
            <Icon name="alert" size={16} />
            <span>{error}</span>
            <button aria-label="关闭错误提示" onClick={onClearError} type="button">
              <Icon name="x" size={15} />
            </button>
          </div>
        ) : null}

        <ProjectTable
          busy={busyLabel !== null}
          gatewayMode={gatewayMode}
          onOpen={onOpenRecent}
          projects={visibleProjects}
        />
      </section>

      {drawerMode ? (
        <ProjectDrawer
          key={drawerMode}
          busy={busyLabel !== null}
          mode={drawerMode}
          onCancel={() => onDrawerModeChange(null)}
          onCreate={onCreate}
          onOpen={onOpenPath}
        />
      ) : null}

      {busyLabel ? (
        <div className="busy-status" role="status">
          <span className="busy-spinner" aria-hidden="true" />
          {busyLabel}
        </div>
      ) : null}
    </main>
  );
}

function ProjectTable({
  projects,
  busy,
  gatewayMode,
  onOpen,
}: {
  readonly projects: readonly RecentProject[];
  readonly busy: boolean;
  readonly gatewayMode: "desktop" | "demo";
  readonly onOpen: (project: RecentProject) => Promise<void>;
}) {
  if (projects.length === 0) {
    return (
      <div className="empty-projects">
        <Icon name="folder" size={24} />
        <strong>这里还没有项目</strong>
        <span>新建或打开一个本地 NarraCut 工程即可开始。</span>
      </div>
    );
  }

  return (
    <div className="project-table-wrap">
      <table className="project-table" aria-label="最近项目">
        <thead className="sr-only">
          <tr><th>项目</th><th>当前阶段</th><th>状态</th><th>最后打开</th><th>操作</th></tr>
        </thead>
        <tbody>
          {projects.map((project) => {
            const summary = projectSummary(project, gatewayMode);
            return (
              <tr key={project.projectId} className={!project.pathAvailable ? "unavailable" : ""}>
                <td className="project-identity-cell">
                  <button
                    className="project-name-button"
                    data-testid={`open-project-${project.projectId}`}
                    disabled={busy}
                    onClick={() => void onOpen(project)}
                    type="button"
                  >
                    <strong>{project.name}</strong>
                    <span>{project.projectPath}</span>
                  </button>
                </td>
                <td><small>当前阶段</small><span>{summary.stage}</span></td>
                <td><small>状态</small><span className={`project-status ${summary.tone}`}><i />{summary.status}</span></td>
                <td><small>最后打开</small><span>{formatLastOpened(project.lastOpenedAt)}</span></td>
                <td className="project-more"><Icon name="more" size={20} /></td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function projectSummary(project: RecentProject, gatewayMode: "desktop" | "demo") {
  if (!project.pathAvailable) return { stage: "时间轴", status: "路径不可用", tone: "muted" };
  if (project.archived) return { stage: "创作简报", status: "已归档", tone: "muted" };
  if (gatewayMode !== "demo") return { stage: "打开后读取", status: "本地工程", tone: "approved" };
  if (project.projectId === "project_moon_city") return { stage: "事实脚本", status: "待审核", tone: "approved" };
  if (project.projectId === "project_solar_storage") return { stage: "口播音频", status: "下游过期", tone: "stale" };
  return { stage: "创作简报", status: "可继续", tone: "approved" };
}

function formatLastOpened(value: string): string {
  const opened = new Date(value);
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const openedDay = new Date(opened.getFullYear(), opened.getMonth(), opened.getDate()).getTime();
  const time = opened.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  if (openedDay === startOfToday) return `今天 ${time}`;
  if (openedDay === startOfToday - 86_400_000) return `昨天 ${time}`;
  return opened.toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" });
}
