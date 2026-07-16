# NarraCut v1 契约与项目格式

## 1. 权威来源

NarraCut 跨 TypeScript、Rust、AI Provider 与 Renderer 的 v1 持久化数据边界，以
`packages/contracts/schema/narracut-contracts-v1.schema.json` 为唯一权威来源；项目服务
请求、响应与错误的权威来源是相互独立的
`packages/contracts/schema/narracut-project-commands-v1.schema.json`；阶段状态服务使用
`packages/contracts/schema/narracut-workflow-commands-v1.schema.json`。
TypeScript 与 Rust 类型必须由该 Schema 生成或导入，不得维护语义不同的同名结构。

契约版本为 `1.0.0`。所有可持久化顶层文档都必须同时包含：

| 字段 | 作用 |
| --- | --- |
| `schemaVersion` | 指明文档遵循的契约版本 |
| `documentType` | 区分项目、阶段、运行、产物、审核、任务事件和导出清单 |

项目文件额外包含 `projectFormatVersion: 1`，供项目服务在打开目录时检测迁移需求。

## 2. 项目目录

项目采用普通目录，根标识文件固定为 `narracut.project.json`：

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
  backups/
    migrations/
```

| 路径 | 是否可迁移真相 | 说明 |
| --- | --- | --- |
| `narracut.project.json` | 是 | `Project` 文档与当前采用版本引用 |
| `contracts/` | 是 | 项目使用的阶段定义与结构化输入输出契约 |
| `stages/` | 是 | 用户可编辑配置与人工决定 |
| `runs/` | 是 | 不可变 `StageExecutionSnapshot`、`StageRun` 与 `ReviewRecord` |
| `artifacts/`、`assets/` | 是 | 带内容哈希、来源、许可证和追溯信息的产物 |
| `exports/`、`manifests/` | 是 | 最终输出与 `RenderManifest` |
| `cache/` | 否 | 可安全重建，不得成为唯一真相 |
| `logs/` | 是 | 运行日志；`StageRun` 只保存摘要和日志产物引用 |
| `backups/migrations/` | 是 | 项目格式迁移前的原始标识文件备份 |

SQLite 仅保存最近项目、搜索索引、任务状态和 UI 偏好。复制整个项目目录后，
即使没有原 SQLite 数据，也必须能够重新建立索引并读取全部工程历史。

## 3. 状态边界

三套状态不得混用：

| 对象 | 状态 |
| --- | --- |
| Stage | `draft`、`ready`、`running`、`needs_review`、`approved`、`failed`、`stale` |
| StageRun | `queued`、`running`、`succeeded`、`failed`、`canceled` |
| Job | `queued`、`running`、`retrying`、`succeeded`、`failed`、`canceled` |

`stale` 描述阶段当前采用结果与上游不再一致，不代表历史 `StageRun` 被改写。
重试属于 Job 生命周期，不得通过修改已完成运行或复制副作用来表达。

`approved` 与 `stale` 阶段必须保存 `approvedRunId`；`stale` 还必须列出至少一个
`staleBecauseStageIds`。JobEvent 使用按事件类型判别的联合，终态、进度、错误和
产物载荷不能自由拼接。

## 4. 审核与追溯

- `StageExecutionSnapshot` 在执行开始时冻结输入引用、配置、执行器、jobId 与幂等键；终态 `StageRun` 只能从该快照构造。
- `StageRun` 保存实际执行快照、终态、产物清单和日志摘要；配置或上游在执行期间变化也不能抹掉历史。
- `InputReference` 是 `artifact` / `project_document` 判别联合；依赖输入必须绑定已批准 StageRun 与 ReviewRecord 的真实产物清单，项目文档只能通过受控 `project://` Resolver 读取并校验哈希。
- `ReviewRecord` 独立保存审核结论；`Project.stages[].approvedRunId` 明确指出当前采用版本。
- `Artifact.provenance` 与 `RenderManifest.claimEvidenceMap` 保留 `claimId` 和 `evidenceRef`。
- `ArtifactDraft` 只承载来源、证据角色和追溯输入；Artifact Store 负责生成身份、项目归属、内容 URI、SHA-256、字节数和创建时间，并为导入来源写入真实 `sourceContentHash`。
- 导入素材必须保存来源、作者、许可证、署名文本和源内容哈希。
- 生成素材只能标记为表达或非证据，Schema 禁止将其标记为事实证据。
- `RenderManifest` 分别保存时间轴、音频、字幕输入和最终视频输出，不依赖相同字段
  之间无法校验的 ID 对照。

Rust 侧必须通过 `validate_contract_document` 或 `parse_contract_document` 进入契约边界；
不能直接使用 `serde_json::from_value` 绕过数组长度、数值范围和判别联合约束。
项目 command 同理必须通过 `validate_project_command_message` 或
`parse_project_command_message`；具体调用边界见 [project-service.md](project-service.md)。
Artifact Store、SQLite 索引与缓存命令必须通过 `validate_storage_command_message` 或
`parse_storage_command_message`；其写入与恢复语义见 [storage-service.md](storage-service.md)。
工作流请求、响应与错误必须通过 `validate_workflow_command_message` 或
`parse_workflow_command_message`；阶段采用与 stale 语义见 [workflow-service.md](workflow-service.md)。

项目复制不得递归替换任意 JSON 中名为 `projectId` 的字段。v1 复制策略只重绑定当前
可编辑 StageConfig 的顶层项目身份；StageRun、Artifact、ReviewRecord 与
RenderManifest 必须原字节保留源身份，以维持配置 hash、内容 hash、幂等键和证据链；
新项目阶段状态投影清空并由 WorkflowService 根据版本化 DAG 幂等重建；根阶段恢复为
`ready`，其他阶段按依赖为 `draft`，不得把继承运行继续标记为当前采用结果。

## 5. 版本策略

- 增加可选字段或放宽约束可发布兼容的次版本。
- 删除字段、改变必填性、枚举含义或持久化语义必须发布新的主版本。
- 旧 Schema 与迁移夹具必须保留，项目升级由 Project Service 显式执行；打开项目不得
  隐式迁移，迁移前必须核对调用方确认过的源格式版本并保留原始标识文件备份。
- 任何重新生成都创建新的 `StageRun` 与 Artifact；不得覆盖历史文件。
