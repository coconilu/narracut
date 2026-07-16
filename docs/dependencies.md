# 依赖登记

新增依赖必须记录用途、许可证与可替代方案。这里不替代最终发行时的完整
第三方许可证清单。

| 依赖 | 范围 | 用途 | 许可证 | 可替代方案 |
| --- | --- | --- | --- | --- |
| `json-schema-to-typescript` 15.0.4 | `@narracut/contracts` 开发依赖 | 从权威 JSON Schema 生成 TypeScript 类型 | MIT | Quicktype；自建受限代码生成器 |
| `ajv` 8.20.0 | `@narracut/contracts` 开发依赖 | 使用 JSON Schema 2020-12 验证合法/非法夹具 | MIT | Rust `jsonschema`；其他兼容 Draft 2020-12 的验证器 |
| `typify` 0.7.0 | `narracut-contracts` 编译期依赖 | 从同一 JSON Schema 生成 Rust 类型 | Apache-2.0 | Quicktype；受测试约束的手写适配层 |

选择这些依赖是为了让 JSON Schema 成为单一真相，并在编译或测试时检测
TypeScript/Rust 结构漂移。它们不进入视频项目文件，也不获得文件系统、网络、
Provider 或 Renderer 权限。
