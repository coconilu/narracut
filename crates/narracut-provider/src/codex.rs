use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use narracut_contracts::validate_provider_message;
use semver::{Version, VersionReq};
use serde_json::{json, Value};
use tempfile::tempdir;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
mod process;
mod protocol;
#[cfg(windows)]
mod windows;

use process::run_codex_process;

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
    pub expected_executable_hash: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: Vec<u8>,
    pub environment: BTreeMap<OsString, OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCliCompletedTurn {
    pub thread_id: String,
    pub final_message: String,
    pub usage: ProviderUsageData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCliRunOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub completed_turn: Option<CodexCliCompletedTurn>,
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
            supports_streaming: false,
            supports_cancellation: cfg!(windows),
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
        let spec = build_run_spec(
            executable,
            frozen_identity.executable_hash.clone(),
            capsule.path(),
            &schema_path,
            request,
        )?;
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
        completed_turn_result(
            request,
            output.completed_turn.ok_or_else(|| {
                provider_response_invalid(
                    "Codex CLI 成功退出，但没有完整的 turn.completed 协议结果。",
                )
            })?,
        )
    }
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), ProviderError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| provider_internal("无法序列化 Codex CLI 胶囊 JSON。"))?;
    std::fs::write(path, bytes).map_err(|_| provider_internal("无法写入 Codex CLI 胶囊文件。"))
}

fn build_run_spec(
    executable: PathBuf,
    expected_executable_hash: String,
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
        expected_executable_hash,
        argv,
        cwd: capsule.to_owned(),
        stdin: prompt,
        environment: sanitized_environment(),
    })
}

fn completed_turn_result(
    request: &StructuredProviderRequestData,
    completed: CodexCliCompletedTurn,
) -> Result<ProviderExecutionData, ProviderError> {
    let output: StructuredScriptOutputData = serde_json::from_str(&completed.final_message)
        .map_err(|_| provider_response_invalid("Codex 最终 agent_message 不是脚本 JSON。"))?;
    validate_reference_subset(CODEX_PROVIDER_ID, request, &output)?;
    let result = StructuredProviderResultData {
        api_version: PROVIDER_API_VERSION.to_owned(),
        message_type: "provider_result".to_owned(),
        provider_request_id: request.provider_request_id.clone(),
        provider_id: CODEX_PROVIDER_ID.to_owned(),
        model: request.model.clone(),
        response_id: completed.thread_id,
        status: "completed".to_owned(),
        output,
        usage: completed.usage,
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

#[cfg(test)]
fn parse_jsonl_result(
    request: &StructuredProviderRequestData,
    lines: &[Vec<u8>],
) -> Result<ProviderExecutionData, ProviderError> {
    let mut machine = protocol::CodexJsonlMachine::default();
    for line in lines {
        machine.feed_line(line)?;
    }
    completed_turn_result(request, machine.finish()?)
}

#[cfg(not(windows))]
fn probe_codex_cli() -> Result<CodexCliProbeData, ProviderError> {
    Ok(CodexCliProbeData::unavailable(
        "probe_failed",
        "本机 Codex CLI Provider 当前仅支持 Windows Alpha；未执行 PATH、登录态或进程探测。",
    ))
}

#[cfg(windows)]
fn probe_codex_cli() -> Result<CodexCliProbeData, ProviderError> {
    let Some(executable) = discover_codex_executable()? else {
        return Ok(CodexCliProbeData::unavailable(
            "not_installed",
            "未在当前 PATH 中发现可 canonicalize 的 Codex CLI。",
        ));
    };
    let executable_guard = match windows::GuardedExecutable::open(&executable) {
        Ok(guard) => guard,
        Err(_) => {
            return Ok(probe_failure(
                executable,
                None,
                None,
                "无法读取 Codex CLI 可执行文件并计算 SHA-256。",
            ));
        }
    };
    let hash = executable_guard.hash().to_owned();
    let version_output = match run_probe(&executable, &hash, &["--version"]) {
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
    let help_output = match run_probe(&executable, &hash, &["exec", "--help"]) {
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
    let login = run_probe(&executable, &hash, &["login", "status"]);
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

#[cfg(windows)]
fn discover_codex_executable() -> Result<Option<PathBuf>, ProviderError> {
    let Some(path_value) = std::env::var_os("PATH") else {
        return Ok(None);
    };
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
            if canonical
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            {
                return Ok(Some(canonical));
            }
        }
    }
    Ok(None)
}

#[cfg(windows)]
#[derive(Debug)]
struct ProbeOutput {
    success: bool,
    stdout: Vec<u8>,
    _stderr_bytes: usize,
}

#[cfg(windows)]
fn run_probe(
    executable: &Path,
    expected_executable_hash: &str,
    args: &[&str],
) -> Result<ProbeOutput, ProviderError> {
    let capsule = tempdir().map_err(|_| provider_internal("无法创建 Codex CLI 探测临时目录。"))?;
    let spec = CodexCliRunSpec {
        executable: executable.to_owned(),
        expected_executable_hash: expected_executable_hash.to_owned(),
        argv: args.iter().map(|argument| (*argument).to_owned()).collect(),
        cwd: capsule.path().to_owned(),
        stdin: Vec::new(),
        environment: sanitized_environment(),
    };
    let output = process::run_probe_process(spec, PROBE_TIMEOUT, PROBE_OUTPUT_LIMIT)?;
    Ok(ProbeOutput {
        success: output.success,
        stdout: output.stdout,
        _stderr_bytes: output.stderr_bytes,
    })
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
                completed_turn: Some(CodexCliCompletedTurn {
                    thread_id: "0199a213-81c0-7800-8aa1-bbab2a035a53".to_owned(),
                    final_message: script_output().to_string(),
                    usage: ProviderUsageData {
                        input_tokens: 1200,
                        output_tokens: 520,
                        total_tokens: 1720,
                        cached_input_tokens: Some(200),
                        reasoning_tokens: Some(80),
                    },
                }),
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
                completed_turn: Some(CodexCliCompletedTurn {
                    thread_id: "thread_fixture".to_owned(),
                    final_message: script_output().to_string(),
                    usage: ProviderUsageData {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                        cached_input_tokens: None,
                        reasoning_tokens: None,
                    },
                }),
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

    fn argument_after<'a>(argv: &'a [String], argument: &str) -> &'a str {
        argv.windows(2)
            .find(|pair| pair[0] == argument)
            .map(|pair| pair[1].as_str())
            .unwrap_or_else(|| panic!("missing {argument}"))
    }
}
