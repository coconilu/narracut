# @narracut/contracts

NarraCut 的版本化跨语言契约。当前有两个相互独立的权威 Schema：

| Schema | 边界 |
| --- | --- |
| [`narracut-contracts-v1.schema.json`](schema/narracut-contracts-v1.schema.json) | 可持久化项目、阶段、运行、产物、任务事件与 manifest |
| [`narracut-project-commands-v1.schema.json`](schema/narracut-project-commands-v1.schema.json) | 项目服务请求、响应与结构化错误 |

两者共同遵循以下生成与校验规则：

- TypeScript 类型分别生成到 `src/generated/contracts-v1.ts` 与 `src/generated/project-commands-v1.ts`；
- Rust 类型由 `crates/narracut-contracts` 在编译期从同一 Schema 导入；
- Rust 在反序列化前使用同一 Draft 2020-12 Schema 执行完整运行时校验；
- `fixtures/` 同时覆盖合法和非法文档，防止两端接受集合漂移。

当前 v1 契约包括 `Project`、`StageDefinition`、`StageConfig`、`StageRun`、
`Artifact`、`ReviewRecord`、`JobEvent` 与 `RenderManifest`。阶段状态、运行状态和
后台任务状态彼此独立；事实性内容通过 `claimId` 与 `evidenceRef` 保持追溯。

`project-command v1` 包括检查、打开、新建、迁移、显示名修改、复制、归档与移入
回收站；所有消息带固定 `apiVersion`，错误使用稳定的 code 与 operation，而不是
要求 UI 解析错误文本。复制响应还明确返回源项目 ID 与不可变历史保留策略，防止调用方
把“新项目身份”误解为“改写历史运行归属”。

## 常用命令

```powershell
pnpm --filter @narracut/contracts generate
pnpm --filter @narracut/contracts check:generated
pnpm --filter @narracut/contracts test
pnpm --filter @narracut/contracts typecheck
```

不要手工修改生成文件。契约发生不兼容变化时，应新增 Schema 主版本与迁移逻辑，
不得直接改变旧版本含义。
