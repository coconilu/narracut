# NarraCut Alpha 端到端夹具

此目录只保存可审计的小型文本输入；测试运行时生成 0.1 秒静音 PCM WAV、真实 FFmpeg 视频和临时工程，生成媒体不会提交 Git。

| 文件 | 用途 | 许可 |
| --- | --- | --- |
| `fixture.json` | 固定画布、追溯与授权声明 | CC0-1.0 |
| `captions.srt` | UTF-8 字幕输入 | CC0-1.0 |
| `LICENSE.md` | 夹具授权与边界 | — |

Windows 真实链：

```powershell
cargo test -p narracut-core alpha_fixture_real_render_qa_atomic_export_and_manifest_verification -- --nocapture
```

测试覆盖项目/工作流、结构化脚本、WAV/SRT、Scene Plan、Timeline、HTML Scene Snapshot、真实 FFmpeg、QA、原子导出、Manifest 校验与幂等重放。
