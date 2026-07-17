use async_trait::async_trait;
use narracut_contracts::validate_provider_message;
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::script_contract::{
    script_output_schema, validate_reference_subset, SCRIPT_INSTRUCTIONS,
};
use crate::{
    AiProvider, ProviderCancellation, ProviderCapabilityData, ProviderError, ProviderErrorCode,
    ProviderExecutionData, ProviderModelCapabilityData, ProviderOperation, ProviderUsageData,
    SecretString, StructuredProviderRequestData, StructuredProviderResultData,
    StructuredScriptOutputData, PROVIDER_API_VERSION,
};

const OPENAI_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";
const MAX_OPENAI_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponseData {
    pub status: u16,
    pub body: Value,
}

#[async_trait]
pub trait ProviderHttpTransport: Send + Sync {
    async fn post_json(
        &self,
        url: &str,
        credential: &SecretString,
        body: Value,
    ) -> Result<HttpResponseData, ProviderError>;
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .user_agent("NarraCut/0.1")
            .build()
            .map_err(|error| {
                ProviderError::new(
                    ProviderErrorCode::Internal,
                    ProviderOperation::ExecuteProviderRequest,
                    format!("无法初始化 OpenAI HTTP 客户端：{error}"),
                    false,
                )
                .for_provider("openai_api")
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ProviderHttpTransport for ReqwestTransport {
    async fn post_json(
        &self,
        url: &str,
        credential: &SecretString,
        body: Value,
    ) -> Result<HttpResponseData, ProviderError> {
        let mut response = self
            .client
            .post(url)
            .bearer_auth(credential.expose())
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                ProviderError::new(
                    ProviderErrorCode::ProviderUnavailable,
                    ProviderOperation::ExecuteProviderRequest,
                    format!("OpenAI Responses 请求失败：{error}"),
                    true,
                )
                .for_provider("openai_api")
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(http_status_error(status));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_OPENAI_RESPONSE_BYTES as u64)
        {
            return Err(invalid_response(
                "OpenAI Responses 成功响应超过有界读取上限。",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ProviderError::new(
                ProviderErrorCode::ProviderUnavailable,
                ProviderOperation::ExecuteProviderRequest,
                "读取 OpenAI Responses 成功响应失败。",
                true,
            )
            .for_provider("openai_api")
        })? {
            if bytes.len().saturating_add(chunk.len()) > MAX_OPENAI_RESPONSE_BYTES {
                return Err(invalid_response(
                    "OpenAI Responses 成功响应超过有界读取上限。",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::ProviderResponseInvalid,
                ProviderOperation::ExecuteProviderRequest,
                format!("OpenAI Responses 返回了无法解析的 JSON：{error}"),
                false,
            )
            .for_provider("openai_api")
        })?;
        Ok(HttpResponseData { status, body })
    }
}

pub struct OpenAiProvider<T: ProviderHttpTransport> {
    transport: T,
}

impl OpenAiProvider<ReqwestTransport> {
    pub fn production() -> Result<Self, ProviderError> {
        Ok(Self::with_transport(ReqwestTransport::new()?))
    }
}

impl<T: ProviderHttpTransport> OpenAiProvider<T> {
    pub fn with_transport(transport: T) -> Self {
        Self { transport }
    }

    fn request_body(request: &StructuredProviderRequestData) -> Result<Value, ProviderError> {
        let input = serde_json::to_string(&json!({
            "projectId": request.project_id,
            "stageId": request.stage_id,
            "runId": request.run_id,
            "inputs": request.inputs,
            "config": request.config,
        }))
        .map_err(|error| invalid_response(format!("结构化输入无法编码：{error}")))?;
        Ok(json!({
            "model": request.model,
            "instructions": SCRIPT_INSTRUCTIONS,
            "input": [{
                "role": "user",
                "content": [{"type": "input_text", "text": input}]
            }],
            "max_output_tokens": request.config.max_output_tokens,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "narracut_script_v1",
                    "strict": true,
                    "schema": script_output_schema()
                }
            }
        }))
    }

    fn parse_response(
        request: &StructuredProviderRequestData,
        response: HttpResponseData,
    ) -> Result<ProviderExecutionData, ProviderError> {
        if !(200..300).contains(&response.status) {
            return Err(http_status_error(response.status));
        }
        if response.body.get("status").and_then(Value::as_str) != Some("completed") {
            return Err(invalid_response("OpenAI Responses 未返回 completed 状态。"));
        }
        let response_id = required_string(&response.body, "id")?;
        let output_text = response
            .body
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .find(|content| content.get("type").and_then(Value::as_str) == Some("output_text"))
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_response("OpenAI Responses 缺少结构化 output_text。"))?;
        let output: StructuredScriptOutputData = serde_json::from_str(output_text)
            .map_err(|error| invalid_response(format!("结构化脚本无法解析：{error}")))?;
        validate_reference_subset("openai_api", request, &output)?;
        let usage = response
            .body
            .get("usage")
            .ok_or_else(|| invalid_response("OpenAI Responses 缺少 usage。"))?;
        let result = StructuredProviderResultData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_result".to_owned(),
            provider_request_id: request.provider_request_id.clone(),
            provider_id: "openai_api".to_owned(),
            model: request.model.clone(),
            response_id,
            status: "completed".to_owned(),
            output,
            usage: ProviderUsageData {
                input_tokens: required_u64(usage, "input_tokens")?,
                output_tokens: required_u64(usage, "output_tokens")?,
                total_tokens: required_u64(usage, "total_tokens")?,
                cached_input_tokens: usage
                    .pointer("/input_tokens_details/cached_tokens")
                    .and_then(Value::as_u64),
                reasoning_tokens: usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64),
            },
            completed_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|error| invalid_response(format!("完成时间无法格式化：{error}")))?,
        };
        let value = serde_json::to_value(&result)
            .map_err(|error| invalid_response(format!("Provider 结果无法序列化：{error}")))?;
        validate_provider_message(&value)
            .map_err(|error| invalid_response(format!("Provider 结果违反 v1 契约：{error}")))?;
        Ok(ProviderExecutionData { result })
    }
}

#[async_trait]
impl<T: ProviderHttpTransport> AiProvider for OpenAiProvider<T> {
    fn capability(&self) -> ProviderCapabilityData {
        ProviderCapabilityData {
            provider_id: "openai_api".to_owned(),
            display_name: "OpenAI API".to_owned(),
            transport: "remote_api".to_owned(),
            credential_storage: "system_keyring".to_owned(),
            supports_streaming: false,
            supports_cancellation: true,
            reports_usage: true,
            default_model: "gpt-5.6-terra".to_owned(),
            models: vec![ProviderModelCapabilityData {
                model_id: "gpt-5.6-terra".to_owned(),
                display_name: "GPT-5.6 Terra".to_owned(),
                supported_tasks: vec!["script_generation".to_owned()],
                structured_outputs: true,
                max_output_tokens: 32768,
            }],
        }
    }

    async fn execute(
        &self,
        request: &StructuredProviderRequestData,
        credential: Option<&SecretString>,
        cancellation: ProviderCancellation,
    ) -> Result<ProviderExecutionData, ProviderError> {
        let credential = credential.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::CredentialMissing,
                ProviderOperation::ExecuteProviderRequest,
                "OpenAI API Provider 缺少系统凭据。",
                false,
            )
            .for_provider("openai_api")
        })?;
        let body = Self::request_body(request)?;
        let response = tokio::select! {
            response = self.transport.post_json(OPENAI_RESPONSES_ENDPOINT, credential, body) => response?,
            _ = cancellation.cancelled() => {
                return Err(ProviderError::new(
                    ProviderErrorCode::Canceled,
                    ProviderOperation::ExecuteProviderRequest,
                    "OpenAI API 调用已取消。",
                    false,
                ).for_provider("openai_api"));
            }
        };
        Self::parse_response(request, response)
    }
}

fn required_string(value: &Value, field: &str) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_response(format!("OpenAI Responses 缺少字符串字段 {field}。")))
}

fn required_u64(value: &Value, field: &str) -> Result<u64, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response(format!("OpenAI Responses 缺少整数 usage.{field}。")))
}

fn invalid_response(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::ProviderResponseInvalid,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider("openai_api")
}

fn http_status_error(status: u16) -> ProviderError {
    ProviderError::new(
        if status == 429 {
            ProviderErrorCode::RateLimited
        } else {
            ProviderErrorCode::ProviderUnavailable
        },
        ProviderOperation::ExecuteProviderRequest,
        format!("OpenAI Responses 返回 HTTP {status}。"),
        status == 429 || status >= 500,
    )
    .for_provider("openai_api")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use super::{HttpResponseData, OpenAiProvider, ProviderHttpTransport, ReqwestTransport};
    use crate::{
        AiProvider, ProvenanceReferenceData, ProviderError, ProviderErrorCode, SecretString,
        StructuredProviderRequestData,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct MockTransport {
        response: HttpResponseData,
        seen_body: Arc<Mutex<Option<Value>>>,
    }

    #[async_trait]
    impl ProviderHttpTransport for MockTransport {
        async fn post_json(
            &self,
            url: &str,
            credential: &SecretString,
            body: Value,
        ) -> Result<HttpResponseData, ProviderError> {
            assert_eq!(url, "https://api.openai.com/v1/responses");
            assert_eq!(credential.expose(), "sk-test-secret-not-real-123456");
            *self.seen_body.lock().expect("body lock") = Some(body);
            Ok(self.response.clone())
        }
    }

    fn fixture_request() -> StructuredProviderRequestData {
        let values = serde_json::from_str::<Vec<Value>>(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("provider fixtures");
        serde_json::from_value(
            values
                .into_iter()
                .find(|value| value["messageType"] == "provider_request")
                .expect("provider request fixture"),
        )
        .expect("request DTO")
    }

    fn completed_response(output: Value) -> HttpResponseData {
        HttpResponseData {
            status: 200,
            body: json!({
                "id": "resp_narracut_001",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": output.to_string()}]
                }],
                "usage": {
                    "input_tokens": 120,
                    "output_tokens": 80,
                    "total_tokens": 200,
                    "input_tokens_details": {"cached_tokens": 10},
                    "output_tokens_details": {"reasoning_tokens": 5}
                }
            }),
        }
    }

    #[tokio::test]
    async fn sends_strict_json_schema_and_parses_usage_with_mock_http() {
        let request = fixture_request();
        let output = json!({
            "schemaVersion": "narracut.script/v1",
            "title": "月面城市",
            "language": "zh-CN",
            "summary": "可追溯脚本",
            "estimatedDurationSeconds": 60.0,
            "segments": [{
                "segmentId": "segment_001",
                "order": 0,
                "title": "开场",
                "narration": "已审核事实。",
                "provenance": [request.inputs[1].provenance[0].clone()]
            }]
        });
        let seen_body = Arc::new(Mutex::new(None));
        let provider = OpenAiProvider::with_transport(MockTransport {
            response: completed_response(output),
            seen_body: seen_body.clone(),
        });
        let execution = provider
            .execute(
                &request,
                Some(&SecretString::new("sk-test-secret-not-real-123456")),
                crate::ProviderCancellation::default(),
            )
            .await
            .expect("mock response succeeds");
        assert_eq!(execution.result.usage.total_tokens, 200);
        assert_eq!(execution.result.usage.cached_input_tokens, Some(10));
        let body = seen_body.lock().expect("body lock").clone().expect("body");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["max_output_tokens"], request.config.max_output_tokens);
        assert!(!contains_key_recursive(
            &body["text"]["format"]["schema"],
            "uniqueItems"
        ));
        assert!(!body.to_string().contains("sk-test-secret"));
    }

    #[tokio::test]
    async fn accepts_multiple_reviewed_provenance_pairs() {
        let mut request = fixture_request();
        request.inputs[1].provenance.push(ProvenanceReferenceData {
            claim_id: "claim_lunar_power".to_owned(),
            evidence_ref: "evidence_lunar_power".to_owned(),
        });
        let output = json!({
            "schemaVersion": "narracut.script/v1",
            "title": "多证据脚本",
            "language": "zh-CN",
            "summary": "保留精确溯源对",
            "estimatedDurationSeconds": 60.0,
            "segments": [{
                "segmentId": "segment_001",
                "order": 0,
                "title": "事实",
                "narration": "采用两组已审核事实。",
                "provenance": request.inputs[1].provenance.clone()
            }]
        });
        let provider = OpenAiProvider::with_transport(MockTransport {
            response: completed_response(output),
            seen_body: Arc::new(Mutex::new(None)),
        });
        let execution = provider
            .execute(
                &request,
                Some(&SecretString::new("sk-test-secret-not-real-123456")),
                crate::ProviderCancellation::default(),
            )
            .await
            .expect("reviewed pairs succeed");
        assert_eq!(execution.result.output.segments[0].provenance.len(), 2);
    }

    #[tokio::test]
    async fn rejects_cross_combined_provenance_pair() {
        let mut request = fixture_request();
        request.inputs[1].provenance.push(ProvenanceReferenceData {
            claim_id: "claim_lunar_power".to_owned(),
            evidence_ref: "evidence_lunar_power".to_owned(),
        });
        let output = json!({
            "schemaVersion": "narracut.script/v1",
            "title": "越界脚本",
            "language": "zh-CN",
            "summary": "应被拒绝",
            "estimatedDurationSeconds": 60.0,
            "segments": [{
                "segmentId": "segment_001",
                "order": 0,
                "title": "越界",
                "narration": "错误交叉组合。",
                "provenance": [{
                    "claimId": request.inputs[1].provenance[0].claim_id.clone(),
                    "evidenceRef": request.inputs[1].provenance[1].evidence_ref.clone()
                }]
            }]
        });
        let provider = OpenAiProvider::with_transport(MockTransport {
            response: completed_response(output),
            seen_body: Arc::new(Mutex::new(None)),
        });
        let error = provider
            .execute(
                &request,
                Some(&SecretString::new("sk-test-secret-not-real-123456")),
                crate::ProviderCancellation::default(),
            )
            .await
            .expect_err("cross-combined pair must fail");
        assert_eq!(error.code.as_str(), "provider_response_invalid");
    }

    #[tokio::test]
    async fn non_json_429_is_classified_before_body_parsing() {
        let (url, server) = serve_once(
            429,
            "Too Many Requests",
            "<html>untrusted-rate-limit-body</html>",
        );
        let error = local_transport()
            .post_json(
                &url,
                &SecretString::new("sk-local-test-secret-123456"),
                json!({"model": "fixture"}),
            )
            .await
            .expect_err("429 must fail");
        server.join().expect("local server joins");
        assert_eq!(error.code, ProviderErrorCode::RateLimited);
        assert!(error.retryable);
        assert!(!error.message.contains("untrusted-rate-limit-body"));
        assert!(!error.message.contains("sk-local-test-secret"));
    }

    #[tokio::test]
    async fn non_json_503_is_classified_before_body_parsing() {
        let (url, server) = serve_once(
            503,
            "Service Unavailable",
            "provider temporarily unavailable: untrusted-body",
        );
        let error = local_transport()
            .post_json(
                &url,
                &SecretString::new("sk-local-test-secret-123456"),
                json!({"model": "fixture"}),
            )
            .await
            .expect_err("503 must fail");
        server.join().expect("local server joins");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        assert!(error.retryable);
        assert!(!error.message.contains("untrusted-body"));
        assert!(!error.message.contains("sk-local-test-secret"));
    }

    fn local_transport() -> ReqwestTransport {
        ReqwestTransport {
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("local reqwest client"),
        }
    }

    fn serve_once(
        status: u16,
        reason: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().expect("local server address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).expect("read request");
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        (format!("http://{address}/v1/responses"), server)
    }

    fn contains_key_recursive(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key(key)
                    || object
                        .values()
                        .any(|child| contains_key_recursive(child, key))
            }
            Value::Array(items) => items.iter().any(|child| contains_key_recursive(child, key)),
            _ => false,
        }
    }
}
