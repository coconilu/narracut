# @narracut/contracts

NarraCut 的版本化跨语言契约。当前有九个相互独立的权威 Schema：

| Schema | 边界 |
| --- | --- |
| [`narracut-contracts-v1.schema.json`](schema/narracut-contracts-v1.schema.json) | 可持久化项目、阶段、运行、产物、任务事件与 manifest |
| [`narracut-project-commands-v1.schema.json`](schema/narracut-project-commands-v1.schema.json) | 项目服务请求、响应与结构化错误 |
| [`narracut-storage-commands-v1.schema.json`](schema/narracut-storage-commands-v1.schema.json) | Artifact Store、SQLite 索引、校验与缓存维护命令 |
| [`narracut-workflow-commands-v1.schema.json`](schema/narracut-workflow-commands-v1.schema.json) | 阶段图、配置修订、不可变运行、审核采用、历史与 stale 影响预览 |
| [`narracut-job-commands-v1.schema.json`](schema/narracut-job-commands-v1.schema.json) | 持久化任务的入队、查询、取消、人工重试、恢复与结构化错误 |
| [`narracut-provider-v1.schema.json`](schema/narracut-provider-v1.schema.json) | AI Provider 能力、凭据命令、脚本入队、执行事件、结构化结果与错误 |
| [`narracut-media-v1.schema.json`](schema/narracut-media-v1.schema.json) | 音频、字幕、场景计划与时间轴文档 |
| [`narracut-media-commands-v1.schema.json`](schema/narracut-media-commands-v1.schema.json) | 媒体导入、生成、保存与读取命令 |
| [`narracut-renderer-v1.schema.json`](schema/narracut-renderer-v1.schema.json) | Renderer 能力、冻结输入、Scene Snapshot、受限渲染请求、事件、结果、产物清单与错误 |

这些契约共同遵循以下生成与校验规则：

- TypeScript 类型分别生成到 `src/generated/contracts-v1.ts`、`src/generated/project-commands-v1.ts`、`src/generated/storage-commands-v1.ts`、`src/generated/workflow-commands-v1.ts`、`src/generated/job-commands-v1.ts` 与 `src/generated/provider-v1.ts`；
- Rust 类型由 `crates/narracut-contracts` 在编译期从同一 Schema 导入；
- Rust 在反序列化前使用同一 Draft 2020-12 Schema 执行完整运行时校验；
- `fixtures/` 同时覆盖合法和非法文档，防止两端接受集合漂移。

当前 v1 契约包括 `Project`、`StageDefinition`、`StageConfig`、`StageExecutionSnapshot`、`StageRun`、
`Artifact`、`ReviewRecord`、`JobDefinition`、`JobEvent` 与 `RenderManifest`。阶段状态、运行状态和
后台任务状态彼此独立；事实性内容通过 `claimId` 与 `evidenceRef` 保持追溯。

`project-command v1` 包括检查、打开、新建、迁移、显示名修改、复制、归档与移入
回收站；所有消息带固定 `apiVersion`，错误使用稳定的 code 与 operation，而不是
要求 UI 解析错误文本。复制响应还明确返回源项目 ID 与不可变历史保留策略，防止调用方
把“新项目身份”误解为“改写历史运行归属”。

`storage-command v1` 包括文件 Artifact 导入、读取、哈希校验、项目索引重建、最近项目、
任务摘要、忘记项目与缓存清理。`ArtifactDraft` 从持久化 Artifact 联合派生；`artifactId`、
`projectId`、URI、SHA-256、字节数与创建时间只由 Rust Artifact Store 生成。命令 Schema
要求文件身份使用安全的 `artifact_` 前缀；对 Artifact 载荷保持通用对象边界，Rust 响应
适配器会额外用持久化 Schema 校验完整文档。

`workflow-command v1` 包括标准工作流初始化与读取、阶段配置乐观修订、执行快照冻结、
终态 StageRun 提交、ReviewRecord 审核采用、局部重生成影响预览与有界历史读取。命令响应中的
配置、执行快照、运行、审核和阶段定义仍会用持久化 Schema 二次校验；工作流命令只负责跨边界
封装，不复制这些文档的含义。

`job-command v1` 只向 UI 暴露入队、查询、取消、人工新运行重试与恢复。worker 领取、
租约续期、进度、Artifact 和终态提交属于 Rust 内部接口，防止前端伪造执行历史。命令响应
中的 JobDefinition 与 JobEvent 会再次通过持久化 Schema 校验。

`provider v1` 只暴露 Provider/模型目录、凭据配置状态、凭据变更和结构化脚本入队。凭据读取、
HTTP 执行、重试、取消、进度和 Artifact 写入保持在 Rust 内部；请求 Schema 禁止任意 endpoint、
header、prompt 与 shell 参数。Provider 输入引用已经审核的 Brief/Research Artifact，并携带内容
哈希、来源 StageRun、ReviewRecord、`claimId` 与 `evidenceRef`；结果引用必须是输入集合的子集。

`renderer v1` 只接受已审核 Timeline 引用和受限编码配置。UI 不能提供可执行路径、任意 FFmpeg
参数或 filter graph；Scene Snapshot 固定 CSP 与项目 URI 白名单，Renderer 结果冻结执行器身份、
Snapshot 哈希、影响场景、媒体元数据和不可变 Artifact 清单。

## 常用命令

```powershell
pnpm --filter @narracut/contracts generate
pnpm --filter @narracut/contracts check:generated
pnpm --filter @narracut/contracts test
pnpm --filter @narracut/contracts typecheck
```

不要手工修改生成文件。契约发生不兼容变化时，应新增 Schema 主版本与迁移逻辑，
不得直接改变旧版本含义。
