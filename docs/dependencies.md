# 依赖登记

新增依赖必须记录用途、许可证与可替代方案。这里不替代最终发行时的完整
第三方许可证清单。

| 依赖 | 范围 | 用途 | 许可证 | 可替代方案 |
| --- | --- | --- | --- | --- |
| `json-schema-to-typescript` 15.0.4 | `@narracut/contracts` 开发依赖 | 从权威 JSON Schema 生成 TypeScript 类型 | MIT | Quicktype；自建受限代码生成器 |
| `ajv` 8.20.0 | `@narracut/contracts` 开发依赖 | 使用 JSON Schema 2020-12 验证合法/非法夹具 | MIT | Rust `jsonschema`；其他兼容 Draft 2020-12 的验证器 |
| `typify` 0.7.0 | `narracut-contracts` 编译期依赖 | 从同一 JSON Schema 生成 Rust 类型 | Apache-2.0 | Quicktype；受测试约束的手写适配层 |
| `jsonschema` 0.48.0 | `narracut-contracts` 运行时依赖（关闭网络/文件解析 feature） | Rust 反序列化前执行 Draft 2020-12 完整约束校验 | MIT | 在 Rust 端为每个约束维护显式验证层；由可信边界预校验 |
| `regress` 0.11.1 | `narracut-contracts` 运行时依赖 | 支持 Typify 生成类型中的 ECMA 正则约束 | MIT OR Apache-2.0 | 在适配层重复实现目录名校验；更换生成器 |
| `atomic-write-file` 0.3.0 | `narracut-core` | 同目录临时文件、同步与原子替换项目 JSON | BSD-3-Clause | 手写平台适配；仅使用 SQLite 但会失去可迁移项目目录 |
| `trash` 5.2.6 | `narracut-core` | 将项目目录移入 Windows、macOS 或 FreeDesktop 回收站 | MIT | 平台 API 适配器；不提供删除能力 |
| `uuid` 1.24.0 | `narracut-core` | 生成项目 ID、临时目录名与迁移备份名 | MIT OR Apache-2.0 | ULID；系统随机源加自定义编码 |
| `time` 0.3.53 | `narracut-core` | 生成并解析任务租约、退避与契约要求的 RFC 3339 时间戳 | MIT OR Apache-2.0 | `chrono`；平台时间与手写格式化 |
| `rusqlite` 0.40.1（`bundled`） | `narracut-core` | 本机最近项目、Artifact 与任务摘要的可重建 SQLite 索引；随应用编译 SQLite | MIT；捆绑 SQLite 为 Public Domain | `sqlx` + SQLite；`redb`；手写文件索引 |
| `sha2` 0.11.0 | `narracut-core`；桌面集成测试开发依赖 | 流式计算和复核 Artifact/任务请求的 SHA-256 内容身份；测试端独立构造 `2addb7a` 过渡持久化格式 | MIT OR Apache-2.0 | `ring`；系统哈希工具（会扩大进程边界） |
| `tempfile` 3.27.0 | `narracut-core` | 以跨平台 `persist_noclobber` 原子占用内容地址，并隔离文件系统测试 | MIT OR Apache-2.0 | 平台无替换移动 API；同卷原子硬链接后移除临时名；测试内手写临时目录清理 |
| `async-trait` 0.1.89 | `narracut-provider` 与桌面集成测试 | 定义可替换的异步 Provider/HTTP 适配器，便于 Mock 与取消测试 | MIT OR Apache-2.0 | Rust 原生 async trait 返回显式 Future；为每个适配器手写装箱 Future |
| `keyring` 4.1.5 | `narracut-provider` | 通过操作系统凭据存储保存 Provider Secret，项目与 SQLite 仅保留是否已配置 | MIT OR Apache-2.0 | Windows Credential Manager/macOS Keychain/Secret Service 的平台专用适配器 |
| `reqwest` 0.13.4（`rustls`、`system-proxy`） | `narracut-provider` | 使用固定 endpoint 调用 OpenAI Responses API；仅开放 JSON、TLS 与系统代理能力 | MIT OR Apache-2.0 | `ureq`；`hyper` + `rustls` 的受限客户端封装 |
| `tokio` 1.52.3 | 桌面 Provider worker 与测试 | 异步任务、取消选择、退避计时和 Mock Provider 测试 | MIT | Tauri async runtime 的显式 Future 组合；`async-std`（会引入第二套运行时） |

选择这些依赖是为了让 JSON Schema 成为单一真相，并在生成、编译、测试和 Rust
运行时入口检测结构或约束漂移。`jsonschema` 已关闭远程 HTTP 与本地文件引用解析；
这些依赖不进入视频项目文件，也不获得 Provider 或 Renderer 权限。

`trash` 是当前唯一触发外部破坏性文件操作的适配器。调用前项目服务会验证规范路径、
当前 Project Schema 和调用方提供的 `expectedProjectId`；测试使用可替换的内存记录后端，
不会污染真实回收站。项目标识与迁移提交使用 `atomic-write-file`，新建和复制则使用
同一父目录中的临时目录完成后再重命名提交。

`rusqlite` 使用 `bundled`，避免依赖用户机器上不可控的 SQLite DLL；数据库只保存可重建
索引，不保存项目唯一真相。`sha2` 在 Rust 进程内流式处理字节，避免为哈希开放外部 shell。
内容对象通过 `tempfile::TempPath::persist_noclobber` 提交：目标地址若在竞态窗口中出现，
提交会失败并进入完整性复核，绝不使用允许替换目标的重命名语义。临时文件位于项目内
`artifacts/.tmp/`，因此不会跨文件系统提交。
任务定义与事件同样使用 `persist_noclobber`；临时文件放在项目内 `cache/job-writes/`，避免
并发事件扫描观察到半提交文件，同时保持同卷无覆盖提交。

Provider 网络依赖只位于 `narracut-provider` 适配器之后。`reqwest` 的 URL、授权头和结构化
输出 Schema 由实现固定，不向 Tauri command 暴露；`keyring` 中的 Secret 不实现序列化，
调试输出始终脱敏。测试通过内存凭据存储与 Mock HTTP 传输验证边界，不访问真实系统凭据或网络。
