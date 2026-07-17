# PR07 阶段审阅工作室 UI 规格

## 目标

PR07 把 PR06 的应用壳升级为可操作的阶段审阅工作室。用户需要在一个稳定界面中完成：

```text
查看阶段输入与当前配置
  → 选择历史运行
  → 查看结构化输出与产物元数据
  → 对比两个不可变版本
  → 提交审核并显式选择采用版本
  → 修改配置并预览 stale / 重生成影响
  → 基于既有运行快照受控重跑
```

任何操作都不能覆盖 `StageRun`、`ReviewRecord` 或 Artifact 历史。配置保存使用乐观修订；审核和重生成使用稳定请求身份，失败重试不得暗中创建重复历史。

## 范围

| 能力 | PR07 交付 |
|---|---|
| Input | 展示运行冻结的 `inputRefs`、claim/evidence 数量与上游采用关系 |
| Config | JSON 对象编辑、修订号、变更理由、乐观冲突错误 |
| Output | 运行状态、日志摘要、Artifact 清单与内容可用性 |
| Preview | 结构化产物元数据、来源、哈希、追溯和演示内容预览 |
| Run history | 最多 100 条不可变运行，默认选择 latest/approved，状态与时间可辨 |
| Compare | A/B 两个运行的配置、输入哈希、执行器、日志与产物差异 |
| Review | `approved` / `changes_requested` / `rejected`，评论与产物选择 |
| Regenerate | 无副作用影响预览；基于所选历史运行的冻结输入和 executor 入队新 run |

## 明确边界

- PR07 不实现新的 AI Provider；没有可复用历史运行时，重生成入口说明需等待 PR08 Provider 配置。
- 真实 Tauri 工程只通过既有 workflow/storage/job typed commands 操作；浏览器演示数据不进入真实路径。
- Artifact Store 当前只返回经过校验的元数据与项目相对 `contentUri`。PR07 的真实预览因此是结构化元数据、可用性与追溯预览；媒体字节渲染由后续 Renderer/媒体预览契约接管。
- 修改配置先展示影响范围，不自动重跑；重跑会创建新的 `runId`，绝不复活或覆盖终态任务。
- 审核只允许选择目标 `StageRun.artifactIds` 中的产物。

## 信息架构

```text
WorkbenchShell
├─ StageRail
├─ StageStudio
│  ├─ ViewTabs: 输入 / 配置 / 输出 / 预览 / 历史 / 比较 / 审核
│  ├─ 主内容视图
│  └─ RegenerationImpact
├─ RunHistoryPanel
│  ├─ 当前采用版本
│  ├─ 候选与历史运行
│  └─ 对比版本 B 选择
└─ ActivityPanel
```

## 视觉基准

| 项 | 规格 |
|---|---|
| 基准视口 | 1600 × 1000 |
| 顶栏 | 56px；保持 NarraCut、项目、阶段与全局动作 |
| 阶段栏 | 220px；阶段状态和运行引用 |
| 主区 | 暖灰纸张与深色工具面；比较视图为双纸张列 |
| 历史栏 | 360px；运行卡片、采用标记、审核动作 |
| 活动区 | 190–208px；任务、事件、日志和产物 |
| 状态颜色 | coral=候选/动作，green=已采用，amber=stale/警告，red=失败 |

## 响应式规则

- 1280px 以下：历史栏变为右侧抽屉；主区保留单列预览，对比视图可水平滚动。
- 900px 以下：阶段栏与历史栏均由顶栏按钮打开；主内容不低于 720px 页面宽度。
- busy 时禁用阶段切换、返回、刷新、配置保存、审核与重生成，避免请求乱序。

## 数据加载与一致性

1. 切换阶段只加载该阶段 history/config，不扫描所有阶段历史。
2. 选择运行后最多读取前 24 个 Artifact 元数据；其余明确显示为未展开数量。
3. 阶段、产物与 mutation 各自使用单调 request generation；过期响应不得提交 UI 状态。
4. 审核或配置保存成功后，重新读取 workflow 与当前阶段 history；不从本地乐观拼接最终真相。
5. 重生成复用所选运行的 `inputRefs` 与 `executor`，新建稳定 `runId + idempotencyKey`；失败后同一意图重试复用身份。

## 验收交互

| 场景 | 期望 |
|---|---|
| 快速切换两个阶段 | 较早响应不会覆盖最后选择 |
| 选择历史运行 | Input/Output/Preview/Review 同步切换 |
| 选择比较版本 B | 双列差异不改变当前采用版本 |
| 保存配置 | 修订递增，affected stages 可见，采用历史保留 |
| 批准候选 | 产生不可变 ReviewRecord，采用引用刷新，下游 stale 可见 |
| 请求修改/拒绝 | 不替换现有 approved run |
| 重生成预览 | 只读展示距离、直接原因和局部支持能力 |
| 确认重生成 | 新 run/job 入队；旧运行仍在历史中 |
| Artifact 缺失/损坏 | 单项显示不可用，不使全部历史空白 |
