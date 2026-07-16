# @narracut/contracts

NarraCut 的版本化 TypeScript 契约包，定义桌面界面、Tauri commands、Provider 与 Renderer 之间可追踪的数据形状。

当前契约覆盖阶段运行状态、输入引用、配置快照、产物清单、日志摘要，以及 `claim_id` 与 `evidence_ref` 的追溯关系。跨 Rust 与 TypeScript 的行为应先在这里确定版本，再通过生成或适配层接入 Rust；不要在两端手工维护语义不同的同名结构。
