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
| `time` 0.3.53 | `narracut-core` | 生成契约要求的 RFC 3339 时间戳 | MIT OR Apache-2.0 | `chrono`；平台时间与手写格式化 |
| `tempfile` 3.27.0 | `narracut-core` 测试依赖 | 隔离项目服务文件系统测试 | MIT OR Apache-2.0 | 测试内手写临时目录清理 |

选择这些依赖是为了让 JSON Schema 成为单一真相，并在生成、编译、测试和 Rust
运行时入口检测结构或约束漂移。`jsonschema` 已关闭远程 HTTP 与本地文件引用解析；
这些依赖不进入视频项目文件，也不获得 Provider 或 Renderer 权限。

`trash` 是当前唯一触发外部破坏性文件操作的适配器。调用前项目服务会验证规范路径、
当前 Project Schema 和调用方提供的 `expectedProjectId`；测试使用可替换的内存记录后端，
不会污染真实回收站。项目标识与迁移提交使用 `atomic-write-file`，新建和复制则使用
同一父目录中的临时目录完成后再重命名提交。
