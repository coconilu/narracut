use std::collections::HashMap;

use serde_json::Value;

use super::{
    provider_response_invalid, provider_unavailable, CodexCliCompletedTurn, ProviderError,
    ProviderUsageData, MAX_JSONL_EVENTS, MAX_JSONL_LINE_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolState {
    AwaitThread,
    AwaitTurn,
    InTurn,
    Completed,
}

#[derive(Debug)]
pub(crate) struct CodexJsonlMachine {
    state: ProtocolState,
    event_count: usize,
    thread_id: Option<String>,
    final_message: Option<String>,
    usage: Option<ProviderUsageData>,
    item_types: HashMap<String, String>,
}

impl Default for CodexJsonlMachine {
    fn default() -> Self {
        Self {
            state: ProtocolState::AwaitThread,
            event_count: 0,
            thread_id: None,
            final_message: None,
            usage: None,
            item_types: HashMap::new(),
        }
    }
}

impl CodexJsonlMachine {
    pub(crate) fn feed_line(&mut self, line: &[u8]) -> Result<(), ProviderError> {
        self.event_count = self.event_count.saturating_add(1);
        if self.event_count > MAX_JSONL_EVENTS {
            return Err(provider_response_invalid(
                "Codex CLI JSONL 事件数超过上限。",
            ));
        }
        if line.is_empty() || line.len() > MAX_JSONL_LINE_BYTES {
            return Err(provider_response_invalid(format!(
                "Codex CLI JSONL 第 {} 行为空或超过上限。",
                self.event_count
            )));
        }
        let event: Value = serde_json::from_slice(line).map_err(|_| {
            provider_response_invalid(format!(
                "Codex CLI JSONL 第 {} 行不是合法 JSON。",
                self.event_count
            ))
        })?;
        let event_type = event.get("type").and_then(Value::as_str).ok_or_else(|| {
            provider_response_invalid(format!(
                "Codex CLI JSONL 第 {} 行缺少事件类型。",
                self.event_count
            ))
        })?;

        match event_type {
            "thread.started" if self.state == ProtocolState::AwaitThread => {
                let id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 160)
                    .ok_or_else(|| {
                        provider_response_invalid("Codex thread.started 缺少有界 thread_id。")
                    })?;
                self.thread_id = Some(id.to_owned());
                self.state = ProtocolState::AwaitTurn;
            }
            "turn.started" if self.state == ProtocolState::AwaitTurn => {
                self.state = ProtocolState::InTurn;
            }
            "item.started" | "item.updated" | "item.completed"
                if self.state == ProtocolState::InTurn =>
            {
                self.feed_item(event_type, &event)?;
            }
            "turn.completed" if self.state == ProtocolState::InTurn => {
                let usage = parse_usage(&event)?;
                if self.final_message.is_none() {
                    return Err(provider_response_invalid(
                        "Codex turn.completed 前没有最终 agent_message。",
                    ));
                }
                self.usage = Some(usage);
                self.state = ProtocolState::Completed;
            }
            "turn.failed" if self.state == ProtocolState::InTurn => {
                return Err(provider_unavailable("Codex CLI turn 执行失败。", true));
            }
            "error" if matches!(self.state, ProtocolState::AwaitTurn | ProtocolState::InTurn) => {
                return Err(provider_unavailable("Codex CLI 返回错误事件。", true));
            }
            _ => {
                return Err(provider_response_invalid(format!(
                    "Codex CLI JSONL 事件越序或类型不受支持：{event_type}。"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<CodexCliCompletedTurn, ProviderError> {
        if self.state != ProtocolState::Completed {
            return Err(provider_response_invalid(
                "Codex CLI JSONL 未以 turn.completed 完成。",
            ));
        }
        Ok(CodexCliCompletedTurn {
            thread_id: self
                .thread_id
                .expect("completed protocol contains a thread id"),
            final_message: self
                .final_message
                .expect("completed protocol contains a final message"),
            usage: self.usage.expect("completed protocol contains usage"),
        })
    }

    fn feed_item(&mut self, event_type: &str, event: &Value) -> Result<(), ProviderError> {
        let item = event
            .get("item")
            .and_then(Value::as_object)
            .ok_or_else(|| provider_response_invalid("Codex item 事件缺少 item 对象。"))?;
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 160)
            .ok_or_else(|| provider_response_invalid("Codex item 缺少有界 id。"))?;
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| provider_response_invalid("Codex item 缺少 type。"))?;
        if is_forbidden_item(item_type) {
            return Err(provider_response_invalid(
                "Codex CLI 产生了被适配器策略禁止的工具事件。",
            ));
        }
        if !matches!(
            item_type,
            "agent_message" | "reasoning" | "plan" | "plan_update"
        ) {
            return Err(provider_response_invalid(
                "Codex CLI 产生了当前适配器未知的 item 类型。",
            ));
        }
        if let Some(previous) = self
            .item_types
            .insert(item_id.to_owned(), item_type.to_owned())
        {
            if previous != item_type {
                return Err(provider_response_invalid(
                    "Codex item id 在同一 turn 内改变了类型。",
                ));
            }
        }
        if event_type == "item.completed" && item_type == "agent_message" {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= 2 * 1024 * 1024)
                .ok_or_else(|| {
                    provider_response_invalid("Codex completed agent_message 缺少有界 text。")
                })?;
            self.final_message = Some(text.to_owned());
        }
        Ok(())
    }
}

fn parse_usage(event: &Value) -> Result<ProviderUsageData, ProviderError> {
    let usage = event
        .get("usage")
        .and_then(Value::as_object)
        .ok_or_else(|| provider_response_invalid("Codex turn.completed 缺少 usage。"))?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| provider_response_invalid("Codex usage 缺少 input_tokens。"))?;
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| provider_response_invalid("Codex usage 缺少 output_tokens。"))?;
    let total_tokens = input_tokens
        .checked_add(output_tokens)
        .ok_or_else(|| provider_response_invalid("Codex usage token 总量溢出。"))?;
    Ok(ProviderUsageData {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens: usage.get("cached_input_tokens").and_then(Value::as_u64),
        reasoning_tokens: usage.get("reasoning_output_tokens").and_then(Value::as_u64),
    })
}

fn is_forbidden_item(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution"
            | "file_change"
            | "mcp_tool_call"
            | "web_search"
            | "tool_call"
            | "image_generation"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ProviderErrorCode;

    fn line(value: Value) -> Vec<u8> {
        serde_json::to_vec(&value).expect("JSON line")
    }

    fn prefix(machine: &mut CodexJsonlMachine) {
        machine
            .feed_line(&line(
                json!({"type":"thread.started","thread_id":"thread_fixture"}),
            ))
            .expect("thread starts");
        machine
            .feed_line(&line(json!({"type":"turn.started"})))
            .expect("turn starts");
    }

    #[test]
    fn incremental_fsm_accepts_completed_turn_and_usage() {
        let mut machine = CodexJsonlMachine::default();
        prefix(&mut machine);
        machine
            .feed_line(&line(json!({
                "type":"item.completed",
                "item":{"id":"message","type":"agent_message","text":"{}"}
            })))
            .expect("message completes");
        machine
            .feed_line(&line(json!({
                "type":"turn.completed",
                "usage":{"input_tokens":3,"output_tokens":5,"cached_input_tokens":2}
            })))
            .expect("turn completes");
        let completed = machine.finish().expect("protocol finishes");
        assert_eq!(completed.thread_id, "thread_fixture");
        assert_eq!(completed.final_message, "{}");
        assert_eq!(completed.usage.total_tokens, 8);
        assert_eq!(completed.usage.cached_input_tokens, Some(2));
    }

    #[test]
    fn incremental_fsm_rejects_forbidden_event_immediately() {
        let mut machine = CodexJsonlMachine::default();
        prefix(&mut machine);
        let error = machine
            .feed_line(&line(json!({
                "type":"item.started",
                "item":{"id":"tool","type":"command_execution","command":"secret"}
            })))
            .expect_err("forbidden item fails on feed");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert!(!error.retryable);
        assert!(!error.message.contains("secret"));
    }

    #[test]
    fn incremental_fsm_rejects_unknown_out_of_order_and_post_terminal_events() {
        let mut out_of_order = CodexJsonlMachine::default();
        assert!(out_of_order
            .feed_line(&line(json!({"type":"turn.started"})))
            .is_err());

        let mut unknown = CodexJsonlMachine::default();
        prefix(&mut unknown);
        assert!(unknown
            .feed_line(&line(json!({
                "type":"item.completed",
                "item":{"id":"future","type":"future_tool"}
            })))
            .is_err());

        let mut completed = CodexJsonlMachine::default();
        prefix(&mut completed);
        completed
            .feed_line(&line(json!({
                "type":"item.completed",
                "item":{"id":"message","type":"agent_message","text":"{}"}
            })))
            .expect("message");
        completed
            .feed_line(&line(json!({
                "type":"turn.completed",
                "usage":{"input_tokens":1,"output_tokens":1}
            })))
            .expect("terminal");
        assert!(completed
            .feed_line(&line(json!({
                "type":"item.started",
                "item":{"id":"late","type":"command_execution"}
            })))
            .is_err());
    }

    #[test]
    fn incremental_fsm_rejects_invalid_json_and_incomplete_eof() {
        let mut invalid = CodexJsonlMachine::default();
        assert!(invalid.feed_line(b"not json").is_err());

        let mut incomplete = CodexJsonlMachine::default();
        prefix(&mut incomplete);
        let error = incomplete.finish().expect_err("truncated protocol fails");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
    }
}
