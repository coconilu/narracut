# NarraCut（叙剪）

NarraCut 是一个本地优先、AI 原生的视频创作桌面工作台。它把“一个想法变成一条视频”拆成可观察、可编辑、可重跑的阶段，而不是把全部过程藏在一次黑盒生成里。

> 当前状态：Tauri 2 + React + TypeScript 基础工程已建立，产品界面与业务能力尚未实现。

## 产品目标

每个视频项目都保存完整工作过程：

| 阶段 | 主要输入 | 可检查的中间产物 |
| --- | --- | --- |
| 选题与目标 | 想法、受众、平台、时长 | 教学目标、内容边界、完成标准 |
| 资料与证据 | 本地文件、网页、笔记 | 证据条目、引用、事实主张 |
| 口播稿 | 目标、证据、表达风格 | 脚本版本、引用检查结果 |
| 音频与字幕 | 审核通过的脚本 | 音频、SRT、词级时间戳 |
| 场景与时间轴 | 字幕时间、脚本结构 | 场景拆分、镜头时间轴、画面需求 |
| 视觉设计 | 场景计划、视觉规范 | 静态草稿、素材清单、动效规格 |
| 内容层渲染 | 已确认的场景与素材 | 可预览的内容层视频、渲染清单 |
| 成片导出 | 内容层、字幕、音频、真人层 | 最终视频与完整 manifest |

任何阶段都可以修改配置并只重跑受影响的后续步骤；历史运行和产物不会被直接覆盖。

## 技术方向

- 桌面壳：Tauri 2
- 界面：React 19 + TypeScript + Vite
- 本地核心：Rust
- AI 接入：统一 Provider 接口，支持远程 API 与本地 Codex CLI
- 数据：SQLite 作为索引，项目目录保存可迁移的配置与产物
- 渲染：通过适配器接入 HTML 动效、FFmpeg、HyperFrames 等实现

详细边界见 [docs/architecture.md](docs/architecture.md)。

## 本地开发

前置条件：Node.js、Rust，以及 Windows 上构建 Tauri 所需的系统依赖。

```powershell
npm install
npm run tauri dev
```

常用检查：

```powershell
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

## 开源说明

仓库已按开源项目初始化，但许可证尚未确定。在选定许可证之前，请不要把本仓库视为已经授予再分发许可。

