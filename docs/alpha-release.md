# NarraCut v0.1.0 Alpha 发布记录

发布日期候选：2026-07-19。此记录描述本仓库精确 PR12 候选的本机证据；安装包不提交 Git。

## 构建产物

```powershell
pnpm --filter @narracut/desktop tauri build -- --bundles nsis
```

| 项目 | 值 |
| --- | --- |
| 版本/架构 | 0.1.0 / Windows x64 / NSIS |
| 产物 | `target/release/bundle/nsis/NarraCut_0.1.0_x64-setup.exe` |
| 大小 | 7,878,173 bytes |
| SHA-256 | `FF5C58059AE77996F54FE2D010F2B2274494FB8D0B6E5C223693F436AC23ED6F` |
| FFmpeg | 不在安装包；用户单独安装 |

`tauri.conf.json` 没有 `externalBin`、FFmpeg resource 或下载 hook；bundle target 只包含 NSIS。

## Windows 安装生命周期

本机预检查确认没有 NarraCut 卸载注册项或安装目录，再执行：silent install → 首次启动 → 关闭 → 重开 → silent uninstall。本机已有 `AppData/Local/com.narracut.app` 可重建索引与 WebView 数据；本轮不删除或冒充全新用户数据环境。

| 检查 | 结果 |
| --- | --- |
| 安装 | exit 0；注册 NarraCut 0.1.0 到当前用户 `AppData\Local\NarraCut` |
| 首次启动 | 进程启动后 4 秒仍存活 |
| 关闭重开 | 新进程启动后 4 秒仍存活 |
| 卸载 | exit 0 |
| 残留 | 安装目录不存在；卸载注册项 0；Roaming app data 不存在；既有 Local app data 按卸载保留策略仍存在 |

证据边界：这是同一 Windows 用户、安装前无程序安装残留但保留既有 Local app data 的等价安装流程，不是全新 VM/全新 OS。未完成的“真正干净 Windows 环境”仍列为发布外部验证项，不能由本记录冒充通过。

## Alpha 真实链与资源基线

```powershell
cargo test -p narracut-renderer real_ffmpeg_smoke_produces_playable_h264_aac_mp4 -- --ignored --nocapture
cargo test -p narracut-core alpha_fixture_real_render_qa_atomic_export_and_manifest_verification -- --nocapture
```

固定 0.1 秒、1920×1080、30000/1001 fps Alpha 夹具在本机 FFmpeg 7.1.1 上的观测：

| 指标 | 结果 |
| --- | --- |
| 项目重新打开 | 0.354 ms |
| 真实 H.264/AAC 渲染 | 407 ms |
| QA 后原子导出 | 177 ms |
| E2E 总计 | 4,167 ms |
| 导出包/`.partial` 有效负载 | 31,255 bytes |
| 并发/磁盘边界 | 1 个 Export worker；视频×2 + 64 MiB 预留；请求临时上限 |

时间是单机小夹具基线，不是 SLA。测试验证 11 项 QA、6 个 Manifest 文件记录、SHA-256 完整性、真实授权记录解析、Job/StageRun 完成、提交点前取消回滚、提交点后成功优先及幂等重放。

## Browser QA

在 `http://127.0.0.1:1420/` 的真实生产 UI 上，从项目列表进入工作台并点击“导出”：

- Export fallback 可见，明确写出“浏览器演示不会伪造导出成功”；
- Audio 预览可见真实授权记录 ID；浏览器模式明确保持只读且不调用媒体命令；
- 不执行 FFmpeg、目录选择、Job 或成功产物伪造；
- 控制台 error 为 0；1578 px 桌面宽度与 375×812 窄屏均无水平溢出；
- 桌面端工作台文案说明会复验 FFmpeg、approved Render、SHA-256 并生成完整交付包。

## 发布门禁

最终精确 HEAD 必须通过：`pnpm test`、`pnpm build`、`pnpm typecheck`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、真实 FFmpeg smoke、`git diff --check`、NSIS build 和独立只读审查。
