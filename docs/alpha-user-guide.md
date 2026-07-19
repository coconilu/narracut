# NarraCut v0.1.0 Alpha 用户指南

## 安装前提

| 项目 | 要求 |
| --- | --- |
| 系统 | Windows 10/11 x64，WebView2 可用 |
| FFmpeg | 用户单独安装 FFmpeg/FFprobe 6–8；需 `libx264` 与 `aac` 编码能力 |
| Provider | 可选 OpenAI API Key（系统凭据库）或受支持的本机 Codex CLI |
| 空间 | 工程与导出卷至少保留“视频大小 × 2 + 64 MiB” |

安装包不包含 FFmpeg。先在 PowerShell 运行 `ffmpeg -version`、`ffprobe -version`；NarraCut 还会冻结可执行文件哈希和能力身份。

## 首个真实项目

| 顺序 | 操作 | 必须审核的真相 |
| --- | --- | --- |
| 1 | 新建项目，配置 Provider | Key 只进入系统凭据库，不写项目 |
| 2 | 完成 Brief、Research、Script | 事实性内容保留 claim/evidence |
| 3 | 导入 PCM WAV 与 UTF-8 SRT | 来源、作者、许可、署名；声音不是克隆或附授权记录 |
| 4 | 生成并编辑 Scene Plan、Timeline | 场景边界、字幕、安全区和采用版本 |
| 5 | 在 Render 预览 HTML Scene Snapshot，执行全片渲染 | 只批准同时含视频、快照和 render log 的成功运行 |
| 6 | 打开 Export，查看 11 项 QA，选择父目录并导出 | 阻塞项为 0；成功后点“复验哈希” |

导出包包含 `video.mp4`、`audio.wav`、`captions.json`、`timeline.json`、`LICENSES.txt`、`SHA256SUMS` 与 `manifest.json`。复制整个目录到另一台机器后可在 NarraCut 中执行完整性复验。

## 备份、迁移与恢复

- 备份整个项目目录；SQLite 只是可重建索引，不是唯一真相。
- 打开旧格式项目时先查看迁移预检，再显式迁移；失败会保留原项目与备份。
- 应用重启会扫描 recent projects，恢复 queued/retrying Job，并终结租约已失效的 running Job；不会把失败写成成功。
- 媒体源丢失时必须重新链接并重新审核，不能靠同名文件静默替代。
- 局部重生成会显示受影响 scene IDs，创建新运行；旧运行仍可查看和重新采用。

详见 [恢复矩阵](recovery-matrix.md)。

## 隐私与合规

项目和生成媒体默认留在本机；只有选择远程 Provider 时，冻结的审核输入才会发送到固定 API。不要导入无权使用的声音、肖像、字体或素材。生成图片/视频只能作为表达素材，不能充当事实证据。导出前检查 `LICENSES.txt` 与 Manifest 授权记录。

## 故障排查

| 提示 | 处理 |
| --- | --- |
| Renderer 不可用/身份漂移 | 检查 PATH 与版本；恢复原 FFmpeg build，或重新渲染并审核新运行 |
| QA provenance/rights blocked | 回到对应媒体或脚本运行补齐追溯/许可后重新审核 |
| 目标目录冲突 | 选择新的安全目录名；不要覆盖历史导出 |
| 磁盘不足 | 清理目标卷或改用其他父目录；残留 `.partial` 会由失败路径清理 |
| 输出损坏 | 保留失败目录用于诊断，从已批准 Render 创建新 Export Job |

Alpha 的范围与风险见 [已知限制](known-limitations.md)。
