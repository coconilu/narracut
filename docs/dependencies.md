# 依赖登记

新增依赖必须记录用途、许可证与可替代方案。这里不替代最终发行时的完整
第三方许可证清单。

| 依赖 | 范围 | 用途 | 许可证 | 可替代方案 |
| --- | --- | --- | --- | --- |
| `json-schema-to-typescript` 15.0.4 | `@narracut/contracts` 开发依赖 | 从权威 JSON Schema 生成 TypeScript 类型 | MIT | Quicktype；自建受限代码生成器 |
| `ajv` 8.20.0 | `@narracut/contracts` 开发依赖 | 使用 JSON Schema 2020-12 验证合法/非法夹具 | MIT | Rust `jsonschema`；其他兼容 Draft 2020-12 的验证器 |
| `typify` 0.7.0 | `narracut-contracts` 编译期依赖 | 从同一 JSON Schema 生成 Rust 类型 | Apache-2.0 | Quicktype；受测试约束的手写适配层 |
| `jsonschema` 0.48.0 | `narracut-contracts` 运行时依赖（关闭网络/文件解析 feature） | Rust 反序列化前执行 Draft 2020-12 完整约束校验 | MIT | 在 Rust 端为每个约束维护显式验证层；由可信边界预校验 |

选择这些依赖是为了让 JSON Schema 成为单一真相，并在生成、编译、测试和 Rust
运行时入口检测结构或约束漂移。`jsonschema` 已关闭远程 HTTP 与本地文件引用解析；
这些依赖不进入视频项目文件，也不获得 Provider 或 Renderer 权限。
