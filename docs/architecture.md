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
Tauri Adapter (Rust)
   ├─ typed project commands
   │        │
Rust Core  ├─ Project Service
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
- Tauri 仅把版本化 command 转接到核心，不在 command 中散落文件操作。
- Rust 核心负责权限边界、项目读写、任务调度、进程生命周期和事件推送。
- Provider 与 Renderer 适配器把第三方实现隔离在稳定契约之后。

项目操作已经通过独立的 `project-command v1` Schema 固定请求、响应和结构化错误；
当前实现与文件安全边界见 [project-service.md](project-service.md)。
阶段图、配置、不可变运行、审核采用和 stale 预览通过独立的 `workflow-command v1`
固定边界；当前状态服务见 [workflow-service.md](workflow-service.md)。
持久化任务通过独立的 `job-command v1` 暴露入队、查询、取消、人工重试与恢复；worker
租约和执行事件保持在 Rust 内部，见 [job-service.md](job-service.md)。

## 3. 项目存储

推荐采用“SQLite 索引 + 可迁移项目目录”的组合：

```text
my-video/
  narracut.project.json
  sources/
  contracts/
  stages/
  runs/
  artifacts/
  assets/
  cache/
  exports/
  manifests/
  logs/
  jobs/
  backups/
    migrations/
```

- SQLite 保存最近项目、搜索索引、任务状态与 UI 偏好。
- 项目目录保存真实配置、运行记录和产物，可备份、迁移和版本升级。
- 大文件通过内容哈希去重；manifest 记录每项外部素材的来源与许可证。
- Artifact Store 使用项目内 SHA-256 内容寻址对象和原子元数据；SQLite 删除后可从项目目录重建，详见 [storage-service.md](storage-service.md)。
- `narracut.project.json` 遵循版本化 `Project` Schema；完整 v1 契约见
  [contracts-v1.md](contracts-v1.md)。
- 项目迁移必须先检查、再由用户确认显式执行；迁移不会在打开时静默发生。
- 标准工作流把阶段定义写入 `contracts/stages/`、当前配置写入 `stages/`、不可变运行与
  审核写入 `runs/`；marker 只保存当前采用引用和直接 stale 原因。
- `jobs/` 保存不可变 JobDefinition 与连续 JobEvent；SQLite 任务摘要可从这里重建。

## 4. AI Provider 契约

第一阶段提供两种实现，但共享同一个任务协议：

| Provider | 用途 | 安全边界 |
| --- | --- | --- |
| API Provider | 调用 OpenAI 或兼容服务 | 密钥进入系统安全存储，禁止写入项目文件 |
| Codex CLI Provider | 使用用户本机 Codex 环境 | 固定工作目录、结构化输出 schema、受限命令模板 |

Provider 接收结构化任务与允许读取的上下文清单，返回结构化结果、模型元数据、用量和诊断信息。UI 不依赖任一供应商的私有响应格式。

## 5. 任务与重生成

长任务立即返回 `job_id`，任务真相持久化在项目目录：

```text
queued -> running -> succeeded
         running -> attempt_failed -> retrying -> running
         running -> failed
                  -> canceled
```

工作流在入队前用 `prepare_stage_run` 冻结 `stage_id + input_hash + config_hash + executor`，
并由此计算幂等键。终态只能消费该不可变快照；相同输入可复用结果，执行期间的局部配置
变化只会把候选标为过期，不会伪造或丢弃真实执行历史。
领取使用有期限的 worker lease；过期租约由恢复流程记录为中断并按快照中的退避策略继续。
自动 retry 保持在同一未终结运行中，用户重试则创建新的 run 与 job。

PR02 中的项目复制仅处理不超过 64 MiB、2048 个文件、4096 个文件系统条目和 64 层
目录深度的有界操作；扫描过程中即时限流，超限时返回 `copy_too_large`。持久化队列基础
已经落地，但大型复制的专用 job type 尚未适配；完成适配前不会伪装成已排队。

复制会给新项目分配新身份，但不会改写 StageRun、Artifact、ReviewRecord 或 manifest
等不可变历史；它们保留源项目身份、hash 与幂等键。只有当前可编辑 StageConfig 的
已知顶层身份字段被重绑定；新项目清空阶段投影后由 DAG 重建根阶段 `ready` 与依赖阶段
`draft`，不会把源运行冒充当前采用结果。
复制来源和历史策略由 marker 与 command 响应显式记录。

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
  crates/
    narracut-contracts/ # Rust 生成类型与运行时 Schema 校验
    narracut-core/      # 项目服务等不依赖 UI 的核心能力
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
