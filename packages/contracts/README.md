# @narracut/contracts

NarraCut 的版本化跨语言契约。唯一权威来源是
[`schema/narracut-contracts-v1.schema.json`](schema/narracut-contracts-v1.schema.json)：

- TypeScript 类型生成到 `src/generated/contracts-v1.ts`；
- Rust 类型由 `crates/narracut-contracts` 在编译期从同一 Schema 导入；
- `fixtures/` 同时覆盖合法和非法文档，防止两端序列化语义漂移。

当前 v1 契约包括 `Project`、`StageDefinition`、`StageConfig`、`StageRun`、
`Artifact`、`ReviewRecord`、`JobEvent` 与 `RenderManifest`。阶段状态、运行状态和
后台任务状态彼此独立；事实性内容通过 `claimId` 与 `evidenceRef` 保持追溯。

## 常用命令

```powershell
pnpm --filter @narracut/contracts generate
pnpm --filter @narracut/contracts check:generated
pnpm --filter @narracut/contracts test
pnpm --filter @narracut/contracts typecheck
```

不要手工修改生成文件。契约发生不兼容变化时，应新增 Schema 主版本与迁移逻辑，
不得直接改变旧版本含义。
