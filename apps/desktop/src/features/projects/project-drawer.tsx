import { useState, type FormEvent } from "react";
import type { CreateProjectInput } from "../../lib/project-commands";
import type { ProjectDrawerMode } from "../app/use-app-controller";

interface ProjectDrawerProps {
  readonly mode: Exclude<ProjectDrawerMode, null>;
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onCreate: (input: CreateProjectInput) => Promise<void>;
  readonly onOpen: (projectPath: string) => Promise<void>;
}

export function ProjectDrawer({
  mode,
  busy,
  onCancel,
  onCreate,
  onOpen,
}: ProjectDrawerProps) {
  if (mode === "open") {
    return (
      <OpenProjectDrawer
        busy={busy}
        onCancel={onCancel}
        onOpen={onOpen}
      />
    );
  }

  return (
    <CreateProjectDrawer
      busy={busy}
      onCancel={onCancel}
      onCreate={onCreate}
    />
  );
}

interface DrawerActionProps {
  readonly busy: boolean;
  readonly onCancel: () => void;
}

function CreateProjectDrawer({
  busy,
  onCancel,
  onCreate,
}: DrawerActionProps & {
  readonly onCreate: (input: CreateProjectInput) => Promise<void>;
}) {
  const [name, setName] = useState("月球城市为什么难");
  const [parentPath, setParentPath] = useState("D:\\NarraCut");
  const [directoryName, setDirectoryName] = useState("moon-city");
  const [directoryTouched, setDirectoryTouched] = useState(false);
  const [locale, setLocale] = useState("zh-CN");

  function updateName(value: string) {
    setName(value);
    if (!directoryTouched) setDirectoryName(suggestDirectoryName(value));
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!name.trim() || !parentPath.trim() || !directoryName.trim()) return;
    await onCreate({
      parentPath: parentPath.trim(),
      directoryName: directoryName.trim(),
      name: name.trim(),
      workflowDefinitionId: "workflow_standard_v1",
      defaultLocale: locale,
    });
  }

  return (
    <aside className="project-drawer" aria-label="新建项目" data-testid="project-drawer">
      <div className="drawer-header">
        <h2>新建项目</h2>
        <p>创建一个普通本地目录。运行、产物和审核历史都会保存在工程内。</p>
      </div>
      <form className="drawer-form" id="create-project-form" onSubmit={handleSubmit}>
        <label className="field">
          <span>项目名称</span>
          <input
            autoComplete="off"
            name="project-name"
            onChange={(event) => updateName(event.target.value)}
            value={name}
          />
        </label>
        <label className="field">
          <span>保存位置</span>
          <input
            className="path-input"
            autoComplete="off"
            name="parent-path"
            onChange={(event) => setParentPath(event.target.value)}
            value={parentPath}
          />
          <small>仅授权此目录；不会授予任意文件系统访问。</small>
        </label>
        <label className="field">
          <span>目录名</span>
          <input
            autoComplete="off"
            name="directory-name"
            onChange={(event) => {
              setDirectoryTouched(true);
              setDirectoryName(event.target.value);
            }}
            value={directoryName}
          />
        </label>
        <label className="field">
          <span>工作流</span>
          <select disabled name="workflow" value="workflow_standard_v1">
            <option value="workflow_standard_v1">标准解说视频 · 9 阶段</option>
          </select>
        </label>
        <label className="field">
          <span>默认语言</span>
          <select
            name="locale"
            onChange={(event) => setLocale(event.target.value)}
            value={locale}
          >
            <option value="zh-CN">简体中文（zh-CN）</option>
            <option value="en-US">English（en-US）</option>
          </select>
        </label>
      </form>
      <div className="drawer-footer">
        <button className="button quiet" disabled={busy} onClick={onCancel} type="button">
          取消
        </button>
        <button className="button primary" disabled={busy} form="create-project-form" type="submit">
          {busy ? "创建中…" : "创建并打开"}
        </button>
      </div>
    </aside>
  );
}

function OpenProjectDrawer({
  busy,
  onCancel,
  onOpen,
}: DrawerActionProps & {
  readonly onOpen: (projectPath: string) => Promise<void>;
}) {
  const [projectPath, setProjectPath] = useState("D:\\NarraCut\\moon-city");

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!projectPath.trim()) return;
    await onOpen(projectPath);
  }

  return (
    <aside className="project-drawer" aria-label="打开项目" data-testid="project-drawer">
      <div className="drawer-header">
        <h2>打开项目</h2>
        <p>输入 NarraCut 本地工程目录。打开时会验证工程标记与迁移状态。</p>
      </div>
      <form className="drawer-form" id="open-project-form" onSubmit={handleSubmit}>
        <label className="field">
          <span>工程目录</span>
          <input
            autoFocus
            className="path-input"
            autoComplete="off"
            name="project-path"
            onChange={(event) => setProjectPath(event.target.value)}
            value={projectPath}
          />
          <small>仅打开具备有效 NarraCut 工程标记的目录。</small>
        </label>
      </form>
      <div className="drawer-footer">
        <button className="button quiet" disabled={busy} onClick={onCancel} type="button">
          取消
        </button>
        <button className="button primary" disabled={busy} form="open-project-form" type="submit">
          {busy ? "打开中…" : "验证并打开"}
        </button>
      </div>
    </aside>
  );
}

function suggestDirectoryName(name: string): string {
  const ascii = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return ascii || "narracut-project";
}
