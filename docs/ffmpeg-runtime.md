# FFmpeg Alpha 运行时与分发策略

NarraCut v0.1.0 安装包不捆绑、不下载 FFmpeg，也不提交任何 FFmpeg 二进制。用户须单独安装受支持 build；应用只通过 Renderer v1 固定 argv 调用 `ffmpeg`/`ffprobe`，不开放 shell 或任意参数。

| 冻结项 | 运行规则 |
| --- | --- |
| 版本 | FFmpeg/FFprobe major 6–8 |
| 能力 | `libx264` 视频编码、原生 `aac` 音频编码、MP4 probe |
| 身份 | 文件名、SHA-256、版本文本、能力哈希 |
| 漂移 | 入队与执行 identity 必须一致；变化后旧 Job fail-closed |
| 进程 | 隐藏窗口、固定参数、绝对超时、输出上限、Windows Job Object 整树取消 |

本地验收机检测到的 build 启用了 GPL 与 libx264，因此该二进制不能在项目许可证尚未确定时被 NarraCut 安装包再分发。本仓库许可证也不能替代 FFmpeg、x264 或用户选择 build 的许可证。需要“开箱即用”捆绑时，必须另行选定兼容发行方式、保存对应源码获取说明和完整许可证，并重新做法律/安全审查。

复现能力探测与真实媒体测试：

```powershell
cargo test -p narracut-renderer real_ffmpeg_smoke_produces_playable_h264_aac_mp4 -- --ignored --nocapture
cargo test -p narracut-core alpha_fixture_real_render_qa_atomic_export_and_manifest_verification -- --nocapture
```
