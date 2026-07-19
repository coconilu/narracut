# v0.1.0 Alpha 第三方许可证边界

依赖用途与替代方案见 [依赖登记](dependencies.md)。发行前以 Cargo/pnpm lockfile 生成机器清单，并人工复核以下非代码资产：

| 类别 | Alpha 处理 |
| --- | --- |
| Rust/TypeScript/Tauri 依赖 | 按各自 MIT、Apache-2.0、BSD 等许可证保留 notices |
| SQLite bundled | Public Domain |
| FFmpeg/FFprobe/x264 | 不随包分发；许可证取决于用户 build |
| 字体、模型、权重 | Alpha 不随包分发；代码许可证不覆盖它们 |
| Alpha 文本夹具 | CC0-1.0，见 `fixtures/alpha/LICENSE.md` |
| 用户声音、肖像、内容素材 | 用户负责授权；Manifest/`LICENSES.txt` 必须记录 |

仓库当前尚未选定代码许可证，因此不能把源码或安装包视为已获得公开再分发授权。发布给测试人员前应先确定分发授权范围。
