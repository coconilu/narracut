# NarraCut 架构草案

## 1. 核心模型

NarraCut 的核心不是“视频文件”，而是一个可版本化执行的项目图：

```text
Project
  ├─ Brief
  ├─ Sources -> Evidence -> Claims
  ├─ Script -> Voice -> Captions
  ├─ Scenes -> Timeline -> Visual Specs
  ├─ Assets -> Content Render
  └─ Final Export -> Manifest
```

每个阶段都由四部分构成：

| 对象 | 作用 |
| --- | --- |
| `StageDefinition` | 定义输入、输出、校验规则和依赖关系 |
| `StageConfig` | 用户可编辑的生成参数与人工决定 |
| `StageRun` | 一次不可变的执行记录，包含状态与配置快照 |
| `Artifact` | 带哈希、类型、来源和版本的中间产物 |

修改配置后创建新的 `StageRun`。旧运行继续保留，系统根据依赖图标记下游产物为“需要刷新”，由用户决定重跑范围。

## 2. 进程边界

```text
React UI
   │ typed commands / events
Tauri Core (Rust)
   ├─ Project Service
   ├─ Workflow Engine
   ├─ Job Queue
   ├─ Artifact Store
   ├─ AI Provider Adapters
   └─ Renderer Adapters
          │
          ├─ Remote APIs
          ├─ Local Codex CLI
          ├─ HTML renderer
          ├─ FFmpeg
          └─ HyperFrames
```

- React 负责编辑、预览和展示状态，不直接管理进程或文件系统。
- Rust 负责权限边界、项目读写、任务调度、进程生命周期和事件推送。
- Provider 与 Renderer 适配器把第三方实现隔离在稳定契约之后。

## 3. 项目存储

推荐采用“SQLite 索引 + 可迁移项目目录”的组合：

```text
my-video.narracut/
  project.json
  stages/
  runs/
  artifacts/
  assets/
  cache/
  exports/
  manifests/
```

- SQLite 保存最近项目、搜索索引、任务状态与 UI 偏好。
- 项目目录保存真实配置、运行记录和产物，可备份、迁移和版本升级。
- 大文件通过内容哈希去重；manifest 记录每项外部素材的来源与许可证。

## 4. AI Provider 契约

第一阶段提供两种实现，但共享同一个任务协议：

| Provider | 用途 | 安全边界 |
| --- | --- | --- |
| API Provider | 调用 OpenAI 或兼容服务 | 密钥进入系统安全存储，禁止写入项目文件 |
| Codex CLI Provider | 使用用户本机 Codex 环境 | 固定工作目录、结构化输出 schema、受限命令模板 |

Provider 接收结构化任务与允许读取的上下文清单，返回结构化结果、模型元数据、用量和诊断信息。UI 不依赖任一供应商的私有响应格式。

## 5. 任务与重生成

长任务立即返回 `job_id`：

```text
queued -> running -> succeeded
                  -> failed -> retrying
                  -> canceled
```

幂等键由 `stage_id + input_hash + config_hash + provider_version` 组成。相同输入可复用结果；局部配置变化只使依赖图中的相关节点失效。

## 6. 建议的首个可用版本

| 里程碑 | 可验收结果 |
| --- | --- |
| M0 工程底座 | Tauri 可启动，项目可新建、打开、删除 |
| M1 可观察工作流 | 阶段列表、配置编辑、运行历史、产物查看 |
| M2 AI 双通道 | API 与 Codex CLI 均能完成同一结构化脚本任务 |
| M3 时间轴 | 导入 SRT，生成并人工调整场景与画面规划 |
| M4 内容层 | 静态预览、HTML 动效、局部渲染与视频预览 |
| M5 导出 | 音频、字幕、内容层合成并输出可追溯 manifest |

## 7. Monorepo 边界

仓库采用 pnpm workspace 与 Cargo workspace 组合，不把所有功能都强制包装成 npm package：

```text
narracut/
  apps/
    desktop/            # @narracut/desktop + Tauri host
  packages/
    contracts/          # @narracut/contracts
  crates/               # 稳定后再提取的 Rust packages
  package.json          # pnpm 调度入口
  Cargo.toml            # Cargo virtual workspace
```

| 边界 | 规则 |
| --- | --- |
| `apps/*` | 可独立启动或交付的应用；Desktop 同时保留相邻的 React 与 `src-tauri` |
| `packages/*` | 可独立类型检查、构建和复用的 TypeScript 契约或实现 |
| `crates/*` | 权限、存储、工作流、Provider、Renderer 等稳定的 Rust 边界 |
| 工作流阶段 | 默认保留在核心内部；仅在出现独立构建、运行或复用需求后拆分 |

依赖方向必须从应用和适配器指向稳定契约与核心。内部 TypeScript 依赖使用 `workspace:*`，Cargo 依赖使用 workspace 或 path dependency；禁止形成环形 workspace 依赖。
