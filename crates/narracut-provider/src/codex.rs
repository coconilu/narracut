use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use narracut_contracts::validate_provider_message;
use semver::{Version, VersionReq};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command as TokioCommand},
    sync::mpsc,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::script_contract::{
    script_output_schema, validate_reference_subset, SCRIPT_INSTRUCTIONS,
};
use crate::{
    AiProvider, ProviderCancellation, ProviderCapabilityData, ProviderCredentialStatusData,
    ProviderError, ProviderErrorCode, ProviderExecutionData, ProviderExecutionIdentityData,
    ProviderModelCapabilityData, ProviderOperation, ProviderUsageData, SecretString,
    StructuredProviderRequestData, StructuredProviderResultData, StructuredScriptOutputData,
    PROVIDER_API_VERSION,
};

pub const CODEX_PROVIDER_ID: &str = "local_codex";
pub const CODEX_ADAPTER_VERSION: &str = "narracut-codex-cli/1.0.0";
pub const CODEX_VERSION_WINDOW: &str = ">=0.144.0,<0.145.0";

const CODEX_MODEL: &str = "gpt-5.6-terra";
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_LIMIT: usize = 16 * 1024;
const MAX_JSONL_LINE_BYTES: usize = 256 * 1024;
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_JSONL_EVENTS: usize = 2_048;
const EXECUTION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const EXECUTION_TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
const TASKKILL_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

const REQUIRED_EXEC_HELP_FLAGS: &[&str] = &[
    "--json",
    "--ephemeral",
    "--ignore-user-config",
    "--ignore-rules",
    "--sandbox",
    "--color",
    "--skip-git-repo-check",
    "--model",
    "--output-schema",
    "--cd",
];

const FIXED_CONFIG_OVERRIDES: &[&str] = &[
    "features.shell_tool=false",
    "features.skill_mcp_dependency_install=false",
    "web_search=\"disabled\"",
    "shell_environment_policy.inherit=\"none\"",
    "tools.view_image=false",
];

const ENV_ALLOWLIST: &[&str] = &[
    "SystemRoot",
    "WINDIR",
    "PATH",
    "PATHEXT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "HOME",
    "LOCALAPPDATA",
    "APPDATA",
    "CODEX_HOME",
    "LANG",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCliProbeData {
    pub installed: bool,
    pub logged_in: bool,
    pub version_supported: bool,
    pub cli_version: Option<String>,
    pub executable_hash: Option<String>,
    pub diagnostic_code: String,
    pub diagnostic: String,
    executable: Option<PathBuf>,
}

impl CodexCliProbeData {
    pub fn ready_fixture(
        executable: impl Into<PathBuf>,
        cli_version: impl Into<String>,
        executable_hash: impl Into<String>,
    ) -> Self {
        Self {
            installed: true,
            logged_in: true,
            version_supported: true,
            cli_version: Some(cli_version.into()),
            executable_hash: Some(executable_hash.into()),
            diagnostic_code: "ready".to_owned(),
            diagnostic: "Codex CLI 已安装、已登录且版本位于受支持窗口。".to_owned(),
            executable: Some(executable.into()),
        }
    }

    fn unavailable(code: &str, diagnostic: &str) -> Self {
        Self {
            installed: false,
            logged_in: false,
            version_supported: false,
            cli_version: None,
            executable_hash: None,
            diagnostic_code: code.to_owned(),
            diagnostic: diagnostic.to_owned(),
            executable: None,
        }
    }

    fn configured(&self) -> bool {
        self.installed && self.logged_in && self.version_supported && self.identity().is_some()
    }

    fn identity(&self) -> Option<ProviderExecutionIdentityData> {
        Some(ProviderExecutionIdentityData {
            adapter_version: CODEX_ADAPTER_VERSION.to_owned(),
            cli_version: self.cli_version.clone()?,
            executable_hash: self.executable_hash.clone()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CodexCliRunSpec {
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: Vec<u8>,
    pub environment: BTreeMap<OsString, OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCliRunOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout_lines: Vec<Vec<u8>>,
    pub stderr_bytes: usize,
}

#[async_trait]
pub trait CodexCliRunner: Send + Sync {
    fn probe(&self) -> Result<CodexCliProbeData, ProviderError>;

    async fn run(
        &self,
        spec: CodexCliRunSpec,
        cancellation: ProviderCancellation,
    ) -> Result<CodexCliRunOutput, ProviderError>;
}

#[derive(Debug, Default)]
pub struct SystemCodexCliRunner;

#[async_trait]
impl CodexCliRunner for SystemCodexCliRunner {
    fn probe(&self) -> Result<CodexCliProbeData, ProviderError> {
        probe_codex_cli()
    }

    async fn run(
        &self,
        spec: CodexCliRunSpec,
        cancellation: ProviderCancellation,
    ) -> Result<CodexCliRunOutput, ProviderError> {
        run_codex_process(spec, cancellation).await
    }
}

pub struct CodexCliProvider {
    runner: Arc<dyn CodexCliRunner>,
}

impl CodexCliProvider {
    pub fn production() -> Self {
        Self::with_runner(SystemCodexCliRunner)
    }

    pub fn with_runner(runner: impl CodexCliRunner + 'static) -> Self {
        Self {
            runner: Arc::new(runner),
        }
    }

    fn require_ready_probe(&self) -> Result<CodexCliProbeData, ProviderError> {
        let probe = self.runner.probe()?;
        if probe.configured() {
            Ok(probe)
        } else {
            Err(provider_unavailable(
                "本机 Codex CLI 尚未完成安装、登录或兼容性检查。",
                false,
            ))
        }
    }

    fn status_data(&self, probe: CodexCliProbeData) -> ProviderCredentialStatusData {
        ProviderCredentialStatusData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_credential_status".to_owned(),
            provider_id: CODEX_PROVIDER_ID.to_owned(),
            configured: probe.configured(),
            storage: "none".to_owned(),
            installed: Some(probe.installed),
            logged_in: Some(probe.logged_in),
            version_supported: Some(probe.version_supported),
            cli_version: probe.cli_version,
            diagnostic_code: Some(probe.diagnostic_code),
            diagnostic: Some(probe.diagnostic),
        }
    }
}

#[async_trait]
impl AiProvider for CodexCliProvider {
    fn capability(&self) -> ProviderCapabilityData {
        ProviderCapabilityData {
            provider_id: CODEX_PROVIDER_ID.to_owned(),
            display_name: "本机 Codex CLI".to_owned(),
            transport: "local_cli".to_owned(),
            credential_storage: "none".to_owned(),
            supports_streaming: true,
            supports_cancellation: true,
            reports_usage: true,
            default_model: CODEX_MODEL.to_owned(),
            models: vec![ProviderModelCapabilityData {
                model_id: CODEX_MODEL.to_owned(),
                display_name: "GPT-5.6 Terra（Codex 登录态）".to_owned(),
                supported_tasks: vec!["script_generation".to_owned()],
                structured_outputs: true,
                max_output_tokens: 32768,
            }],
        }
    }

    fn local_status(&self) -> Result<Option<ProviderCredentialStatusData>, ProviderError> {
        Ok(Some(self.status_data(self.runner.probe()?)))
    }

    fn execution_identity(&self) -> Result<Option<ProviderExecutionIdentityData>, ProviderError> {
        Ok(Some(
            self.require_ready_probe()?
                .identity()
                .expect("configured probe contains identity"),
        ))
    }

    fn adapter_version(&self) -> Option<&'static str> {
        Some(CODEX_ADAPTER_VERSION)
    }

    async fn execute(
        &self,
        request: &StructuredProviderRequestData,
        credential: Option<&SecretString>,
        cancellation: ProviderCancellation,
    ) -> Result<ProviderExecutionData, ProviderError> {
        if credential.is_some() {
            return Err(provider_internal(
                "本机 Codex Provider 不接受 API Key 或其他注入凭据。",
            ));
        }
        if cancellation.is_canceled() {
            return Err(canceled_error());
        }
        let frozen_identity = request.execution_identity.as_ref().ok_or_else(|| {
            provider_invalid("本机 Codex 请求缺少冻结 adapter/CLI/executable 身份。")
        })?;
        let probe = self.require_ready_probe()?;
        let current_identity = probe
            .identity()
            .expect("configured probe contains execution identity");
        if frozen_identity != &current_identity {
            return Err(provider_unavailable(
                "Codex CLI 版本或可执行文件哈希已变化；请创建新任务以冻结新身份。",
                false,
            ));
        }
        let executable = probe
            .executable
            .clone()
            .ok_or_else(|| provider_internal("就绪探测缺少 canonical executable。"))?;

        let capsule =
            tempdir().map_err(|_| provider_internal("无法创建隔离的 Codex CLI 临时执行胶囊。"))?;
        let schema_path = capsule.path().join("narracut-script-v1.schema.json");
        write_json_file(&schema_path, &script_output_schema())?;
        let spec = build_run_spec(executable, capsule.path(), &schema_path, request)?;
        let output = self.runner.run(spec, cancellation).await?;
        if !output.success {
            return Err(provider_unavailable(
                format!(
                    "Codex CLI 运行失败（exit={}，stderrBytes={}）。",
                    output
                        .exit_code
                        .map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
                    output.stderr_bytes
                ),
                output.exit_code.is_none_or(|code| code != 2),
            ));
        }
        parse_jsonl_result(request, &output.stdout_lines)
    }
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), ProviderError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| provider_internal("无法序列化 Codex CLI 胶囊 JSON。"))?;
    std::fs::write(path, bytes).map_err(|_| provider_internal("无法写入 Codex CLI 胶囊文件。"))
}

fn build_run_spec(
    executable: PathBuf,
    capsule: &Path,
    schema_path: &Path,
    request: &StructuredProviderRequestData,
) -> Result<CodexCliRunSpec, ProviderError> {
    let capsule_text = capsule
        .to_str()
        .ok_or_else(|| provider_internal("Codex CLI 胶囊路径不是有效 Unicode。"))?;
    let schema_text = schema_path
        .to_str()
        .ok_or_else(|| provider_internal("Codex CLI 输出 Schema 路径不是有效 Unicode。"))?;
    let mut argv = vec!["exec".to_owned()];
    for config in FIXED_CONFIG_OVERRIDES {
        argv.push("-c".to_owned());
        argv.push((*config).to_owned());
    }
    argv.extend([
        "--json".to_owned(),
        "--ephemeral".to_owned(),
        "--ignore-user-config".to_owned(),
        "--ignore-rules".to_owned(),
        "--sandbox".to_owned(),
        "read-only".to_owned(),
        "--color".to_owned(),
        "never".to_owned(),
        "--skip-git-repo-check".to_owned(),
        "--model".to_owned(),
        request.model.clone(),
        "--output-schema".to_owned(),
        schema_text.to_owned(),
        "-C".to_owned(),
        capsule_text.to_owned(),
        "-".to_owned(),
    ]);
    let prompt = serde_json::to_vec(&json!({
        "instructions": SCRIPT_INSTRUCTIONS,
        "policy": {
            "tools": "forbidden",
            "network": "forbidden",
            "filesystemWrites": "forbidden",
            "source": "approved_frozen_inputs_only"
        },
        "request": {
            "providerRequestId": request.provider_request_id,
            "projectId": request.project_id,
            "stageId": request.stage_id,
            "runId": request.run_id,
            "inputs": request.inputs,
            "config": request.config,
            "outputSchemaVersion": request.output_schema_version,
        }
    }))
    .map_err(|_| provider_internal("无法构造 Codex CLI 固定 stdin。"))?;
    Ok(CodexCliRunSpec {
        executable,
        argv,
        cwd: capsule.to_owned(),
        stdin: prompt,
        environment: sanitized_environment(),
    })
}

fn parse_jsonl_result(
    request: &StructuredProviderRequestData,
    lines: &[Vec<u8>],
) -> Result<ProviderExecutionData, ProviderError> {
    if lines.is_empty() || lines.len() > MAX_JSONL_EVENTS {
        return Err(provider_response_invalid(
            "Codex CLI JSONL 事件数量为空或超过上限。",
        ));
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        AwaitThread,
        AwaitTurn,
        InTurn,
        Completed,
    }
    let mut state = State::AwaitThread;
    let mut thread_id: Option<String> = None;
    let mut final_message: Option<String> = None;
    let mut usage: Option<ProviderUsageData> = None;
    let mut item_types = HashMap::<String, String>::new();

    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() || line.len() > MAX_JSONL_LINE_BYTES {
            return Err(provider_response_invalid(format!(
                "Codex CLI JSONL 第 {} 行为空或超过上限。",
                index + 1
            )));
        }
        let event: Value = serde_json::from_slice(line).map_err(|_| {
            provider_response_invalid(format!(
                "Codex CLI JSONL 第 {} 行不是合法 JSON。",
                index + 1
            ))
        })?;
        let event_type = event.get("type").and_then(Value::as_str).ok_or_else(|| {
            provider_response_invalid(format!("Codex CLI JSONL 第 {} 行缺少事件类型。", index + 1))
        })?;
        match event_type {
            "thread.started" if state == State::AwaitThread => {
                let id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 160)
                    .ok_or_else(|| {
                        provider_response_invalid("Codex thread.started 缺少有界 thread_id。")
                    })?;
                thread_id = Some(id.to_owned());
                state = State::AwaitTurn;
            }
            "turn.started" if state == State::AwaitTurn => state = State::InTurn,
            "item.started" | "item.updated" | "item.completed" if state == State::InTurn => {
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
                if let Some(previous) = item_types.insert(item_id.to_owned(), item_type.to_owned())
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
                            provider_response_invalid(
                                "Codex completed agent_message 缺少有界 text。",
                            )
                        })?;
                    final_message = Some(text.to_owned());
                }
            }
            "turn.completed" if state == State::InTurn => {
                usage = Some(parse_usage(&event)?);
                if final_message.is_none() {
                    return Err(provider_response_invalid(
                        "Codex turn.completed 前没有最终 agent_message。",
                    ));
                }
                state = State::Completed;
            }
            "turn.failed" if state == State::InTurn => {
                return Err(provider_unavailable("Codex CLI turn 执行失败。", true));
            }
            "error" if matches!(state, State::AwaitTurn | State::InTurn) => {
                return Err(provider_unavailable("Codex CLI 返回错误事件。", true));
            }
            _ => {
                return Err(provider_response_invalid(format!(
                    "Codex CLI JSONL 事件越序或类型不受支持：{event_type}。"
                )));
            }
        }
    }
    if state != State::Completed {
        return Err(provider_response_invalid(
            "Codex CLI JSONL 未以 turn.completed 完成。",
        ));
    }
    let output: StructuredScriptOutputData = serde_json::from_str(
        final_message
            .as_deref()
            .expect("completed state contains final message"),
    )
    .map_err(|_| provider_response_invalid("Codex 最终 agent_message 不是脚本 JSON。"))?;
    validate_reference_subset(CODEX_PROVIDER_ID, request, &output)?;
    let result = StructuredProviderResultData {
        api_version: PROVIDER_API_VERSION.to_owned(),
        message_type: "provider_result".to_owned(),
        provider_request_id: request.provider_request_id.clone(),
        provider_id: CODEX_PROVIDER_ID.to_owned(),
        model: request.model.clone(),
        response_id: thread_id.expect("completed state contains thread id"),
        status: "completed".to_owned(),
        output,
        usage: usage.expect("completed state contains usage"),
        completed_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| provider_internal("无法格式化 Codex CLI 完成时间。"))?,
    };
    let value = serde_json::to_value(&result)
        .map_err(|_| provider_internal("无法序列化 Codex CLI Provider 结果。"))?;
    validate_provider_message(&value)
        .map_err(|_| provider_response_invalid("Codex CLI 结果不符合共享 Provider v1 契约。"))?;
    Ok(ProviderExecutionData { result })
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

fn probe_codex_cli() -> Result<CodexCliProbeData, ProviderError> {
    let Some(executable) = discover_codex_executable()? else {
        return Ok(CodexCliProbeData::unavailable(
            "not_installed",
            "未在当前 PATH 中发现可 canonicalize 的 Codex CLI。",
        ));
    };
    let hash = match executable_sha256(&executable) {
        Ok(hash) => hash,
        Err(_) => {
            return Ok(probe_failure(
                executable,
                None,
                None,
                "无法读取 Codex CLI 可执行文件并计算 SHA-256。",
            ));
        }
    };
    let version_output = match run_probe(&executable, &["--version"]) {
        Ok(output) if output.success => output,
        _ => {
            return Ok(probe_failure(
                executable,
                None,
                Some(hash),
                "Codex CLI 版本探测失败或超时。",
            ));
        }
    };
    let cli_version = match parse_cli_version(&version_output.stdout) {
        Some(version) => version,
        None => {
            return Ok(probe_failure(
                executable,
                None,
                Some(hash),
                "Codex CLI 版本输出格式不受支持。",
            ));
        }
    };
    let requirement = VersionReq::parse(CODEX_VERSION_WINDOW)
        .expect("checked-in Codex compatibility window is valid");
    let parsed =
        Version::parse(&cli_version).expect("parse_cli_version only returns semantic versions");
    if !requirement.matches(&parsed) {
        return Ok(CodexCliProbeData {
            installed: true,
            logged_in: false,
            version_supported: false,
            cli_version: Some(cli_version),
            executable_hash: Some(hash),
            diagnostic_code: "unsupported_version".to_owned(),
            diagnostic: format!("Codex CLI 版本不在当前适配器兼容窗口 {CODEX_VERSION_WINDOW}。"),
            executable: Some(executable),
        });
    }
    let help_output = match run_probe(&executable, &["exec", "--help"]) {
        Ok(output) if output.success => output,
        _ => {
            return Ok(probe_failure(
                executable,
                Some(cli_version),
                Some(hash),
                "Codex exec --help 探测失败或超时。",
            ));
        }
    };
    let help_text = String::from_utf8_lossy(&help_output.stdout);
    if REQUIRED_EXEC_HELP_FLAGS
        .iter()
        .any(|flag| !help_text.contains(flag))
    {
        return Ok(probe_failure(
            executable,
            Some(cli_version),
            Some(hash),
            "Codex exec 缺少适配器要求的固定安全参数。",
        ));
    }
    let login = run_probe(&executable, &["login", "status"]);
    let logged_in = login.is_ok_and(|output| output.success);
    if !logged_in {
        return Ok(CodexCliProbeData {
            installed: true,
            logged_in: false,
            version_supported: true,
            cli_version: Some(cli_version),
            executable_hash: Some(hash),
            diagnostic_code: "not_logged_in".to_owned(),
            diagnostic: "Codex CLI 已安装但未检测到可用登录态；请在终端运行 codex login。"
                .to_owned(),
            executable: Some(executable),
        });
    }
    Ok(CodexCliProbeData {
        installed: true,
        logged_in: true,
        version_supported: true,
        cli_version: Some(cli_version),
        executable_hash: Some(hash),
        diagnostic_code: "ready".to_owned(),
        diagnostic: "Codex CLI 已安装、已登录且版本位于受支持窗口。".to_owned(),
        executable: Some(executable),
    })
}

fn probe_failure(
    executable: PathBuf,
    cli_version: Option<String>,
    executable_hash: Option<String>,
    diagnostic: &str,
) -> CodexCliProbeData {
    CodexCliProbeData {
        installed: true,
        logged_in: false,
        version_supported: false,
        cli_version,
        executable_hash,
        diagnostic_code: "probe_failed".to_owned(),
        diagnostic: diagnostic.to_owned(),
        executable: Some(executable),
    }
}

fn discover_codex_executable() -> Result<Option<PathBuf>, ProviderError> {
    let Some(path_value) = std::env::var_os("PATH") else {
        return Ok(None);
    };
    #[cfg(windows)]
    let candidates = {
        let extensions = std::env::var_os("PATHEXT")
            .and_then(|value| value.into_string().ok())
            .map(|value| {
                value
                    .split(';')
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![".EXE".to_owned(), ".CMD".to_owned(), ".BAT".to_owned()]);
        extensions
            .into_iter()
            .map(|extension| format!("codex{extension}"))
            .collect::<Vec<_>>()
    };
    #[cfg(not(windows))]
    let candidates = vec!["codex".to_owned()];

    for directory in std::env::split_paths(&path_value) {
        for candidate in &candidates {
            let path = directory.join(candidate);
            if !path.is_file() {
                continue;
            }
            let canonical = match std::fs::canonicalize(&path) {
                Ok(path) if path.is_file() => path,
                _ => continue,
            };
            #[cfg(windows)]
            if canonical
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            {
                return Ok(Some(canonical));
            }
            #[cfg(not(windows))]
            return Ok(Some(canonical));
        }
    }
    Ok(None)
}

fn executable_sha256(path: &Path) -> Result<String, ProviderError> {
    let mut file = File::open(path)
        .map_err(|_| provider_unavailable("无法读取 Codex CLI 可执行文件。", false))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| provider_unavailable("读取 Codex CLI 可执行文件失败。", false))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{encoded}"))
}

#[derive(Debug)]
struct ProbeOutput {
    success: bool,
    stdout: Vec<u8>,
    _stderr_bytes: usize,
}

fn run_probe(executable: &Path, args: &[&str]) -> Result<ProbeOutput, ProviderError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .envs(sanitized_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|_| provider_unavailable("无法启动 Codex CLI 探测进程。", false))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| provider_internal("Codex CLI 探测缺少 stdout pipe。"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| provider_internal("Codex CLI 探测缺少 stderr pipe。"))?;
    let stdout_reader = thread::spawn(move || read_bounded_sync(stdout, PROBE_OUTPUT_LIMIT));
    let stderr_reader = thread::spawn(move || read_bounded_sync(stderr, PROBE_OUTPUT_LIMIT));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(provider_unavailable("Codex CLI 探测超时。", true));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(provider_unavailable("Codex CLI 探测状态读取失败。", false));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| provider_internal("Codex CLI stdout 探测线程异常。"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| provider_internal("Codex CLI stderr 探测线程异常。"))??;
    Ok(ProbeOutput {
        success: status.success(),
        stdout,
        _stderr_bytes: stderr.len(),
    })
}

fn read_bounded_sync(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| provider_unavailable("Codex CLI 探测输出读取失败。", false))?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(provider_response_invalid(
                "Codex CLI 探测输出超过有界上限。",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

fn parse_cli_version(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?.trim();
    let version = text.strip_prefix("codex-cli ")?.trim();
    Version::parse(version).ok()?;
    Some(version.to_owned())
}

fn sanitized_environment() -> BTreeMap<OsString, OsString> {
    ENV_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect()
}

async fn run_codex_process(
    spec: CodexCliRunSpec,
    cancellation: ProviderCancellation,
) -> Result<CodexCliRunOutput, ProviderError> {
    let mut command = TokioCommand::new(&spec.executable);
    command
        .args(&spec.argv)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.as_std_mut().creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|_| provider_unavailable("无法启动冻结的 Codex CLI 执行进程。", true))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| provider_internal("Codex CLI 执行缺少 stdin pipe。"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| provider_internal("Codex CLI 执行缺少 stdout pipe。"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| provider_internal("Codex CLI 执行缺少 stderr pipe。"))?;
    let stdin_bytes = spec.stdin;
    let stdin_task = tokio::spawn(async move {
        stdin
            .write_all(&stdin_bytes)
            .await
            .map_err(|_| provider_unavailable("Codex CLI stdin 写入失败。", true))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| provider_unavailable("Codex CLI stdin 关闭失败。", true))
    });
    let (line_tx, mut line_rx) = mpsc::channel(32);
    let stdout_task = tokio::spawn(read_stdout_lines(stdout, line_tx));
    let stderr_task = tokio::spawn(read_stderr_bounded(stderr));
    let total_deadline = tokio::time::Instant::now() + EXECUTION_TOTAL_TIMEOUT;
    let mut idle_deadline = tokio::time::Instant::now() + EXECUTION_IDLE_TIMEOUT;
    let mut poll = tokio::time::interval(PROCESS_POLL_INTERVAL);
    let mut stdout_lines = Vec::new();
    let status = loop {
        tokio::select! {
            _ = cancellation.cancelled() => {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {
                        terminate_process_tree(&mut child).await?;
                        finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                        return Err(canceled_error());
                    }
                    Err(_) => {
                        direct_kill_and_wait(&mut child).await;
                        finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                        return Err(cancellation_failed("取消时无法确认 Codex CLI 主进程状态。"));
                    }
                }
            }
            event = line_rx.recv() => {
                match event {
                    Some(Ok(line)) => {
                        idle_deadline = tokio::time::Instant::now() + EXECUTION_IDLE_TIMEOUT;
                        if stdout_lines.len() >= MAX_JSONL_EVENTS {
                            cleanup_after_protocol_failure(&mut child).await;
                            finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                            return Err(provider_response_invalid("Codex CLI JSONL 事件数超过上限。"));
                        }
                        stdout_lines.push(line);
                    }
                    Some(Err(error)) => {
                        cleanup_after_protocol_failure(&mut child).await;
                        finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                        return Err(error);
                    }
                    None => {}
                }
            }
            _ = poll.tick() => {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {}
                    Err(_) => {
                        cleanup_after_protocol_failure(&mut child).await;
                        finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                        return Err(provider_unavailable("无法读取 Codex CLI 进程状态。", true));
                    }
                }
            }
            _ = tokio::time::sleep_until(idle_deadline) => {
                cleanup_after_protocol_failure(&mut child).await;
                finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                return Err(provider_unavailable("Codex CLI JSONL 输出空闲超时。", true));
            }
            _ = tokio::time::sleep_until(total_deadline) => {
                cleanup_after_protocol_failure(&mut child).await;
                finish_io_tasks(stdin_task, stdout_task, stderr_task).await;
                return Err(provider_unavailable("Codex CLI 执行超过总时限。", true));
            }
        }
    };
    while let Some(event) = line_rx.recv().await {
        stdout_lines.push(event?);
        if stdout_lines.len() > MAX_JSONL_EVENTS {
            return Err(provider_response_invalid(
                "Codex CLI JSONL 事件数超过上限。",
            ));
        }
    }
    stdin_task
        .await
        .map_err(|_| provider_internal("Codex CLI stdin task 异常。"))??;
    stdout_task
        .await
        .map_err(|_| provider_internal("Codex CLI stdout task 异常。"))??;
    let stderr_bytes = stderr_task
        .await
        .map_err(|_| provider_internal("Codex CLI stderr task 异常。"))??;
    Ok(CodexCliRunOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout_lines,
        stderr_bytes,
    })
}

async fn read_stdout_lines(
    mut reader: impl AsyncRead + Unpin,
    sender: mpsc::Sender<Result<Vec<u8>, ProviderError>>,
) -> Result<(), ProviderError> {
    let mut total = 0_usize;
    let mut pending = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| provider_unavailable("Codex CLI stdout 读取失败。", true))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > MAX_STDOUT_BYTES {
            let error = provider_response_invalid("Codex CLI stdout 超过 4 MiB 上限。");
            let _ = sender.send(Err(error.clone())).await;
            return Err(error);
        }
        pending.extend_from_slice(&buffer[..read]);
        while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.len() > MAX_JSONL_LINE_BYTES {
                let error = provider_response_invalid("Codex CLI JSONL 单行超过上限。");
                let _ = sender.send(Err(error.clone())).await;
                return Err(error);
            }
            if sender.send(Ok(line)).await.is_err() {
                return Ok(());
            }
        }
        if pending.len() > MAX_JSONL_LINE_BYTES {
            let error = provider_response_invalid("Codex CLI JSONL 单行超过上限。");
            let _ = sender.send(Err(error.clone())).await;
            return Err(error);
        }
    }
    if !pending.is_empty() {
        sender
            .send(Ok(pending))
            .await
            .map_err(|_| provider_internal("Codex CLI JSONL channel 已关闭。"))?;
    }
    Ok(())
}

async fn read_stderr_bounded(mut reader: impl AsyncRead + Unpin) -> Result<usize, ProviderError> {
    let mut total = 0_usize;
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| provider_unavailable("Codex CLI stderr 读取失败。", true))?;
        if read == 0 {
            return Ok(total);
        }
        total = total.saturating_add(read);
        if total > MAX_STDERR_BYTES {
            return Err(provider_response_invalid(
                "Codex CLI stderr 超过 64 KiB 上限。",
            ));
        }
    }
}

async fn finish_io_tasks(
    stdin: tokio::task::JoinHandle<Result<(), ProviderError>>,
    stdout: tokio::task::JoinHandle<Result<(), ProviderError>>,
    stderr: tokio::task::JoinHandle<Result<usize, ProviderError>>,
) {
    let _ = tokio::time::timeout(PROCESS_WAIT_TIMEOUT, stdin).await;
    let _ = tokio::time::timeout(PROCESS_WAIT_TIMEOUT, stdout).await;
    let _ = tokio::time::timeout(PROCESS_WAIT_TIMEOUT, stderr).await;
}

async fn cleanup_after_protocol_failure(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() && terminate_process_tree(child).await.is_err() {
        direct_kill_and_wait(child).await;
    }
}

async fn direct_kill_and_wait(child: &mut Child) {
    let _ = child.kill().await;
    let _ = tokio::time::timeout(PROCESS_WAIT_TIMEOUT, child.wait()).await;
}

#[cfg(windows)]
async fn terminate_process_tree(child: &mut Child) -> Result<(), ProviderError> {
    terminate_process_tree_with_taskkill(child, canonical_taskkill(), TASKKILL_TIMEOUT).await
}

#[cfg(windows)]
async fn terminate_process_tree_with_taskkill(
    child: &mut Child,
    taskkill: Result<PathBuf, ProviderError>,
    taskkill_timeout: Duration,
) -> Result<(), ProviderError> {
    if child
        .try_wait()
        .map_err(|_| cancellation_failed("取消前无法确认 Codex CLI 主进程状态。"))?
        .is_some()
    {
        return Ok(());
    }
    let pid = child
        .id()
        .ok_or_else(|| cancellation_failed("Codex CLI 主进程缺少 PID。"))?;
    let taskkill = match taskkill {
        Ok(path) => path,
        Err(error) => {
            direct_kill_and_wait(child).await;
            return Err(error);
        }
    };
    let taskkill_environment = match taskkill_environment(&taskkill) {
        Ok(environment) => environment,
        Err(error) => {
            direct_kill_and_wait(child).await;
            return Err(error);
        }
    };
    let mut command = TokioCommand::new(taskkill);
    command
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (key, value) in taskkill_environment {
        command.env(key, value);
    }
    command.as_std_mut().creation_flags(0x0800_0000);
    let mut killer = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            direct_kill_and_wait(child).await;
            return Err(cancellation_failed(
                "无法启动 canonical System32 taskkill.exe。",
            ));
        }
    };
    let killer_status = match tokio::time::timeout(taskkill_timeout, killer.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            let _ = killer.kill().await;
            let _ = tokio::time::timeout(PROCESS_WAIT_TIMEOUT, killer.wait()).await;
            direct_kill_and_wait(child).await;
            return Err(cancellation_failed("taskkill 整树取消超时或状态读取失败。"));
        }
    };
    if !killer_status.success() {
        direct_kill_and_wait(child).await;
        return Err(cancellation_failed("taskkill 未能确认整棵进程树已终止。"));
    }
    match tokio::time::timeout(PROCESS_WAIT_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        _ => {
            direct_kill_and_wait(child).await;
            Err(cancellation_failed(
                "taskkill 成功后 Codex CLI 主进程仍未完成 wait。",
            ))
        }
    }
}

#[cfg(windows)]
fn taskkill_environment(taskkill: &Path) -> Result<BTreeMap<OsString, OsString>, ProviderError> {
    let system32 = taskkill
        .parent()
        .ok_or_else(|| cancellation_failed("taskkill.exe 缺少 System32 父目录。"))?;
    let root = system32
        .parent()
        .ok_or_else(|| cancellation_failed("taskkill.exe 缺少 Windows 根目录。"))?;
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|_| cancellation_failed("taskkill.exe 的 Windows 根目录无法 canonicalize。"))?;
    Ok([
        (
            OsString::from("SystemRoot"),
            canonical_root.clone().into_os_string(),
        ),
        (OsString::from("WINDIR"), canonical_root.into_os_string()),
    ]
    .into_iter()
    .collect())
}

#[cfg(windows)]
fn canonical_taskkill() -> Result<PathBuf, ProviderError> {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| cancellation_failed("SystemRoot 缺失或不是绝对路径。"))?;
    let canonical_root = std::fs::canonicalize(&root)
        .map_err(|_| cancellation_failed("SystemRoot 无法 canonicalize。"))?;
    let system32 = canonical_root.join("System32");
    let canonical_system32 = std::fs::canonicalize(&system32)
        .map_err(|_| cancellation_failed("System32 无法 canonicalize。"))?;
    let taskkill = canonical_system32.join("taskkill.exe");
    let canonical = std::fs::canonicalize(&taskkill)
        .map_err(|_| cancellation_failed("System32 taskkill.exe 不可用。"))?;
    let parent_ok = canonical
        .parent()
        .is_some_and(|parent| parent == canonical_system32);
    let name_ok = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("taskkill.exe"));
    if !canonical.is_file() || !parent_ok || !name_ok {
        return Err(cancellation_failed(
            "canonical taskkill.exe 不位于 canonical System32。",
        ));
    }
    Ok(canonical)
}

#[cfg(not(windows))]
async fn terminate_process_tree(child: &mut Child) -> Result<(), ProviderError> {
    direct_kill_and_wait(child).await;
    Err(cancellation_failed(
        "当前 Alpha 仅在 Windows 提供可确认的 Codex CLI 整棵进程树取消。",
    ))
}

fn provider_invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::InvalidRequest,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

fn provider_response_invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::ProviderResponseInvalid,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

fn provider_unavailable(message: impl Into<String>, retryable: bool) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::ProviderUnavailable,
        ProviderOperation::ExecuteProviderRequest,
        message,
        retryable,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

fn provider_internal(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Internal,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

fn canceled_error() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Canceled,
        ProviderOperation::ExecuteProviderRequest,
        "Codex CLI 执行已取消，主进程与 helper 进程树已完成清理。",
        false,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

fn cancellation_failed(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::CancellationFailed,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider(CODEX_PROVIDER_ID)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex},
    };

    #[cfg(windows)]
    use std::{os::windows::process::CommandExt as _, process::Command as StdCommand};

    use super::*;

    struct FixtureRunner {
        probe: CodexCliProbeData,
        output: CodexCliRunOutput,
        seen: Arc<Mutex<Option<CodexCliRunSpec>>>,
    }

    #[async_trait]
    impl CodexCliRunner for FixtureRunner {
        fn probe(&self) -> Result<CodexCliProbeData, ProviderError> {
            Ok(self.probe.clone())
        }

        async fn run(
            &self,
            spec: CodexCliRunSpec,
            _cancellation: ProviderCancellation,
        ) -> Result<CodexCliRunOutput, ProviderError> {
            let mut capsule_entries = fs::read_dir(&spec.cwd)
                .expect("read capsule")
                .map(|entry| {
                    entry
                        .expect("read capsule entry")
                        .file_name()
                        .into_string()
                        .expect("Unicode capsule entry")
                })
                .collect::<Vec<_>>();
            capsule_entries.sort();
            assert_eq!(capsule_entries, ["narracut-script-v1.schema.json"]);
            let schema_path = PathBuf::from(argument_after(&spec.argv, "--output-schema"));
            assert_eq!(schema_path.parent(), Some(spec.cwd.as_path()));
            assert!(schema_path.is_file());
            *self.seen.lock().expect("seen spec") = Some(spec);
            Ok(self.output.clone())
        }
    }

    fn local_request() -> StructuredProviderRequestData {
        let values = serde_json::from_str::<Vec<Value>>(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("provider fixtures");
        serde_json::from_value(
            values
                .into_iter()
                .find(|value| {
                    value["messageType"] == "provider_request"
                        && value["providerId"] == CODEX_PROVIDER_ID
                })
                .expect("local request fixture"),
        )
        .expect("local request DTO")
    }

    fn script_output() -> Value {
        let values = serde_json::from_str::<Vec<Value>>(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("provider fixtures");
        values
            .into_iter()
            .find(|value| value["messageType"] == "provider_result")
            .expect("provider result fixture")["output"]
            .clone()
    }

    fn completed_jsonl(output: &Value) -> Vec<Vec<u8>> {
        [
            json!({"type":"thread.started","thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53"}),
            json!({"type":"turn.started"}),
            json!({"type":"item.started","item":{"id":"item_reasoning","type":"reasoning"}}),
            json!({"type":"item.completed","item":{"id":"item_message","type":"agent_message","text":output.to_string()}}),
            json!({"type":"turn.completed","usage":{"input_tokens":1200,"cached_input_tokens":200,"output_tokens":520,"reasoning_output_tokens":80}}),
        ]
        .into_iter()
        .map(|line| serde_json::to_vec(&line).expect("jsonl line"))
        .collect()
    }

    #[tokio::test]
    async fn fixed_runner_uses_schema_only_capsule_frozen_stdin_and_minimal_environment() {
        let request = local_request();
        let identity = request
            .execution_identity
            .clone()
            .expect("fixture identity");
        let seen = Arc::new(Mutex::new(None));
        let provider = CodexCliProvider::with_runner(FixtureRunner {
            probe: CodexCliProbeData::ready_fixture(
                "C:\\fixture\\codex.exe",
                identity.cli_version,
                identity.executable_hash,
            ),
            output: CodexCliRunOutput {
                success: true,
                exit_code: Some(0),
                stdout_lines: completed_jsonl(&script_output()),
                stderr_bytes: 0,
            },
            seen: seen.clone(),
        });
        let execution = provider
            .execute(&request, None, ProviderCancellation::default())
            .await
            .expect("fixture run succeeds");
        assert_eq!(execution.result.provider_id, CODEX_PROVIDER_ID);
        assert_eq!(execution.result.usage.total_tokens, 1720);
        assert_eq!(execution.result.usage.cached_input_tokens, Some(200));
        assert_eq!(execution.result.usage.reasoning_tokens, Some(80));

        let spec = seen.lock().expect("seen spec").take().expect("run spec");
        assert_eq!(spec.argv.first().map(String::as_str), Some("exec"));
        for config in FIXED_CONFIG_OVERRIDES {
            assert!(spec.argv.windows(2).any(|pair| pair == ["-c", *config]));
        }
        for required in [
            "--json",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--sandbox",
            "read-only",
            "--color",
            "never",
            "--skip-git-repo-check",
            "--model",
            "--output-schema",
            "-C",
            "-",
        ] {
            assert!(spec.argv.iter().any(|argument| argument == required));
        }
        for forbidden in [
            "--add-dir",
            "resume",
            "--profile",
            "--image",
            "--dangerously-bypass-approvals-and-sandbox",
        ] {
            assert!(!spec.argv.iter().any(|argument| argument == forbidden));
        }
        assert!(spec
            .environment
            .keys()
            .all(|key| ENV_ALLOWLIST.iter().any(|allowed| key == allowed)));
        for secret in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"] {
            assert!(!spec.environment.contains_key(&OsString::from(secret)));
        }
        assert!(!spec.environment.contains_key(&OsString::from("ComSpec")));
        let stdin: Value = serde_json::from_slice(&spec.stdin).expect("fixed JSON stdin");
        assert_eq!(stdin["request"]["inputs"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            stdin["request"]["inputs"],
            serde_json::to_value(&request.inputs).expect("frozen inputs")
        );
        assert_eq!(
            stdin["request"]["config"],
            serde_json::to_value(&request.config).expect("frozen config")
        );
        assert!(stdin["request"].get("projectPath").is_none());
        assert_eq!(stdin["policy"]["tools"], "forbidden");
        assert!(!String::from_utf8_lossy(&spec.stdin).contains("auth.json"));
        let output_schema = argument_after(&spec.argv, "--output-schema");
        assert!(Path::new(output_schema).starts_with(&spec.cwd));
        assert_eq!(argument_after(&spec.argv, "-C"), spec.cwd.to_string_lossy());
        assert!(
            !spec.cwd.exists(),
            "temporary capsule must be removed after provider execution returns"
        );
    }

    #[test]
    fn jsonl_fsm_rejects_tool_events_and_out_of_order_events() {
        let request = local_request();
        let tool_lines = [
            json!({"type":"thread.started","thread_id":"thread_safe"}),
            json!({"type":"turn.started"}),
            json!({"type":"item.started","item":{"id":"item_tool","type":"command_execution","command":"whoami"}}),
        ]
        .into_iter()
        .map(|line| serde_json::to_vec(&line).expect("line"))
        .collect::<Vec<_>>();
        let tool_error = parse_jsonl_result(&request, &tool_lines).expect_err("tool event fails");
        assert_eq!(tool_error.code, ProviderErrorCode::ProviderResponseInvalid);
        assert!(!tool_error.message.contains("whoami"));

        let out_of_order = completed_jsonl(&script_output())
            .into_iter()
            .skip(1)
            .collect::<Vec<_>>();
        let order_error =
            parse_jsonl_result(&request, &out_of_order).expect_err("turn before thread fails");
        assert_eq!(order_error.code, ProviderErrorCode::ProviderResponseInvalid);
    }

    #[test]
    fn jsonl_fsm_rejects_unknown_items_and_cross_combined_provenance() {
        let request = local_request();
        let unknown = [
            json!({"type":"thread.started","thread_id":"thread_safe"}),
            json!({"type":"turn.started"}),
            json!({"type":"item.completed","item":{"id":"item_unknown","type":"future_tool"}}),
        ]
        .into_iter()
        .map(|line| serde_json::to_vec(&line).expect("line"))
        .collect::<Vec<_>>();
        assert!(parse_jsonl_result(&request, &unknown).is_err());

        let mut output = script_output();
        output["segments"][0]["provenance"][0]["evidenceRef"] =
            Value::String("evidence_not_reviewed".to_owned());
        let error = parse_jsonl_result(&request, &completed_jsonl(&output))
            .expect_err("unreviewed provenance fails");
        assert_eq!(error.code, ProviderErrorCode::ProviderResponseInvalid);
    }

    #[tokio::test]
    async fn execution_revalidates_frozen_cli_identity() {
        let request = local_request();
        let seen = Arc::new(Mutex::new(None));
        let provider = CodexCliProvider::with_runner(FixtureRunner {
            probe: CodexCliProbeData::ready_fixture(
                "C:\\fixture\\codex.exe",
                "0.144.1",
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            ),
            output: CodexCliRunOutput {
                success: true,
                exit_code: Some(0),
                stdout_lines: completed_jsonl(&script_output()),
                stderr_bytes: 0,
            },
            seen: seen.clone(),
        });
        let error = provider
            .execute(&request, None, ProviderCancellation::default())
            .await
            .expect_err("identity drift fails before spawn");
        assert_eq!(error.code, ProviderErrorCode::ProviderUnavailable);
        assert!(seen.lock().expect("seen").is_none());
    }

    #[test]
    fn version_window_is_bounded_and_version_parser_is_exact() {
        let requirement = VersionReq::parse(CODEX_VERSION_WINDOW).expect("window");
        assert!(requirement.matches(&Version::parse("0.144.0").expect("min")));
        assert!(requirement.matches(&Version::parse("0.144.99").expect("patch")));
        assert!(!requirement.matches(&Version::parse("0.143.99").expect("old")));
        assert!(!requirement.matches(&Version::parse("0.145.0").expect("upper")));
        assert_eq!(
            parse_cli_version(b"codex-cli 0.144.1\r\n").as_deref(),
            Some("0.144.1")
        );
        assert!(parse_cli_version(b"codex 0.144.1").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn taskkill_environment_is_exact_and_derived_from_canonical_path() {
        let taskkill = canonical_taskkill().expect("canonical taskkill");
        let environment = taskkill_environment(&taskkill).expect("taskkill environment");
        assert_eq!(
            environment.keys().collect::<Vec<_>>(),
            [&OsString::from("SystemRoot"), &OsString::from("WINDIR")]
        );
        let expected_root = std::fs::canonicalize(
            taskkill
                .parent()
                .and_then(Path::parent)
                .expect("Windows root"),
        )
        .expect("canonical Windows root")
        .into_os_string();
        assert!(environment.values().all(|value| value == &expected_root));
        assert!(!environment.contains_key(&OsString::from("PATH")));
        assert!(!environment.contains_key(&OsString::from("ComSpec")));
    }

    #[cfg(windows)]
    #[test]
    fn process_tree_fixture() {
        let Some(mode) = std::env::var_os("NARRACUT_CODEX_PROCESS_FIXTURE") else {
            return;
        };
        match mode.to_string_lossy().as_ref() {
            "exit" => {}
            "child" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "parent" => {
                let executable = std::env::current_exe().expect("current test executable");
                let mut command = StdCommand::new(executable);
                command
                    .args([
                        "--exact",
                        "codex::tests::process_tree_fixture",
                        "--nocapture",
                    ])
                    .env("NARRACUT_CODEX_PROCESS_FIXTURE", "child")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000);
                let child = command.spawn().expect("spawn child process fixture");
                let pid_file = std::env::var_os("NARRACUT_CODEX_PID_FILE")
                    .map(PathBuf::from)
                    .expect("parent fixture PID file");
                fs::write(pid_file, child.id().to_string()).expect("write child PID");
                let _child = child;
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            other => panic!("unknown process fixture mode: {other}"),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_taskkill_terminates_real_parent_and_child_processes() {
        let directory = tempdir().expect("PID fixture directory");
        let pid_file = directory.path().join("child.pid");
        let mut parent = spawn_process_fixture("parent", Some(&pid_file)).await;
        let parent_pid = parent.id().expect("parent PID");
        let child_pid = wait_for_pid_file(&pid_file).await;
        let parent_started = process_exists(parent_pid);
        let child_started = process_exists(child_pid);

        let cancellation = terminate_process_tree(&mut parent).await;
        let parent_stopped = wait_for_process_exit(parent_pid).await;
        let child_stopped = wait_for_process_exit(child_pid).await;
        if !child_stopped {
            best_effort_kill_pid(child_pid);
        }

        assert!(
            parent_started,
            "parent fixture must be observable before cancel"
        );
        assert!(
            child_started,
            "child fixture must be observable before cancel"
        );
        cancellation.expect("canonical taskkill cancels the process tree");
        assert!(parent_stopped, "parent PID must be gone after cancellation");
        assert!(child_stopped, "child PID must be gone after cancellation");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_missing_taskkill_kills_main_but_reports_cancellation_failed() {
        let mut child = spawn_process_fixture("child", None).await;
        let pid = child.id().expect("fixture PID");
        let error = terminate_process_tree_with_taskkill(
            &mut child,
            Err(cancellation_failed("fixture taskkill missing")),
            Duration::from_millis(100),
        )
        .await
        .expect_err("missing taskkill cannot acknowledge tree cancellation");

        assert_eq!(error.code, ProviderErrorCode::CancellationFailed);
        assert!(
            wait_for_process_exit(pid).await,
            "main process must be cleaned up"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_taskkill_failure_and_timeout_never_acknowledge_cancellation() {
        let directory = tempdir().expect("fake taskkill directory");
        let fake_taskkill = compile_fake_taskkill(directory.path());

        let mut failed_child = spawn_process_fixture("child", None).await;
        let failed_pid = failed_child.id().expect("failure fixture PID");
        let failure = terminate_process_tree_with_taskkill(
            &mut failed_child,
            Ok(fake_taskkill.clone()),
            Duration::from_secs(2),
        )
        .await
        .expect_err("non-zero taskkill must fail cancellation");
        assert_eq!(failure.code, ProviderErrorCode::CancellationFailed);
        assert!(
            wait_for_process_exit(failed_pid).await,
            "failed taskkill still cleans up the main process"
        );

        let timeout_taskkill = directory.path().join("taskkill-timeout.exe");
        fs::copy(&fake_taskkill, &timeout_taskkill).expect("copy timeout fixture");
        let mut timed_out_child = spawn_process_fixture("child", None).await;
        let timed_out_pid = timed_out_child.id().expect("timeout fixture PID");
        let timeout = terminate_process_tree_with_taskkill(
            &mut timed_out_child,
            Ok(timeout_taskkill),
            Duration::from_millis(100),
        )
        .await
        .expect_err("timed out taskkill must fail cancellation");
        assert_eq!(timeout.code, ProviderErrorCode::CancellationFailed);
        assert!(
            wait_for_process_exit(timed_out_pid).await,
            "timed out taskkill still cleans up the main process"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_main_exit_race_succeeds_without_taskkill() {
        let mut child = spawn_process_fixture("exit", None).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("poll exit fixture").is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "exit fixture must finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        terminate_process_tree_with_taskkill(
            &mut child,
            Err(cancellation_failed("must not be observed")),
            Duration::from_millis(1),
        )
        .await
        .expect("an already-exited main process wins the cancellation race");
    }

    #[cfg(windows)]
    async fn spawn_process_fixture(mode: &str, pid_file: Option<&Path>) -> Child {
        let executable = std::env::current_exe().expect("current test executable");
        let mut command = TokioCommand::new(executable);
        command
            .args([
                "--exact",
                "codex::tests::process_tree_fixture",
                "--nocapture",
            ])
            .env("NARRACUT_CODEX_PROCESS_FIXTURE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(pid_file) = pid_file {
            command.env("NARRACUT_CODEX_PID_FILE", pid_file);
        }
        command.as_std_mut().creation_flags(0x0800_0000);
        command.spawn().expect("spawn process fixture")
    }

    #[cfg(windows)]
    async fn wait_for_pid_file(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(value) = fs::read_to_string(path) {
                if let Ok(pid) = value.trim().parse() {
                    return pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "parent fixture must publish child PID"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(windows)]
    fn tasklist_path() -> PathBuf {
        let taskkill = canonical_taskkill().expect("canonical taskkill");
        std::fs::canonicalize(taskkill.with_file_name("tasklist.exe")).expect("canonical tasklist")
    }

    #[cfg(windows)]
    fn process_exists(pid: u32) -> bool {
        let output = StdCommand::new(tasklist_path())
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .stdin(Stdio::null())
            .output()
            .expect("query process list");
        let needle = format!("\"{pid}\"").into_bytes();
        output.status.success()
            && output
                .stdout
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[cfg(windows)]
    async fn wait_for_process_exit(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        !process_exists(pid)
    }

    #[cfg(windows)]
    fn best_effort_kill_pid(pid: u32) {
        let _ = StdCommand::new(canonical_taskkill().unwrap_or_else(|_| PathBuf::new()))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }

    #[cfg(windows)]
    fn compile_fake_taskkill(directory: &Path) -> PathBuf {
        let source = directory.join("taskkill-fixture.rs");
        let executable = directory.join("taskkill-failure.exe");
        fs::write(
            &source,
            r#"fn main() {
    let timeout = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .is_some_and(|name| name.contains("timeout"));
    if timeout {
        std::thread::sleep(std::time::Duration::from_secs(30));
    } else {
        std::process::exit(7);
    }
}
"#,
        )
        .expect("write fake taskkill source");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
        let output = StdCommand::new(rustc)
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .stdin(Stdio::null())
            .output()
            .expect("compile fake taskkill");
        assert!(
            output.status.success(),
            "fake taskkill compilation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable
    }

    fn argument_after<'a>(argv: &'a [String], argument: &str) -> &'a str {
        argv.windows(2)
            .find(|pair| pair[0] == argument)
            .map(|pair| pair[1].as_str())
            .unwrap_or_else(|| panic!("missing {argument}"))
    }
}
