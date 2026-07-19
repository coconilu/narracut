use narracut_contracts::{
    EnqueueExportRequest, ExportCommandError, ExportJobAcceptedResult, ExportQaResult,
    ExportResult, ExportVerificationResult, GetExportResultRequest, RunExportQaRequest,
    VerifyExportRequest,
};
use narracut_core::{
    EnqueueExportOptions, ExportErrorCode, ExportOperation, ExportService, ExportServiceError,
    GetJobOptions, JobService, RunExportQaOptions,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::export_runtime::ExportRuntime;

#[tauri::command]
pub async fn run_export_qa(
    request: RunExportQaRequest,
    service: State<'_, ExportService>,
    runtime: State<'_, ExportRuntime>,
) -> Result<ExportQaResult, ExportCommandError> {
    let operation = ExportOperation::RunQa;
    let internal: RunExportQaOptions = map_request(&request, operation)?;
    let probe = runtime.adapter().probe().await;
    let identity = probe
        .identity
        .filter(|_| probe.available && probe.supported)
        .map(|identity| public_identity(&identity));
    let service = service.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || service.run_qa(internal, identity.as_ref()))
            .await
            .map_err(|_| {
                command_error(
                    operation,
                    ExportErrorCode::InternalContract,
                    "QA worker 异常结束。",
                    false,
                )
            })?
            .map_err(service_error)?;
    map_response(result, operation)
}

#[tauri::command]
pub async fn enqueue_export(
    request: EnqueueExportRequest,
    service: State<'_, ExportService>,
    runtime: State<'_, ExportRuntime>,
    jobs: State<'_, JobService>,
) -> Result<ExportJobAcceptedResult, ExportCommandError> {
    let operation = ExportOperation::Enqueue;
    let internal: EnqueueExportOptions = map_request(&request, operation)?;
    let project_path = internal.project_path.clone();
    let project_id = internal.expected_project_id.clone();
    let probe = runtime.adapter().probe().await;
    let identity = probe
        .identity
        .filter(|_| probe.available && probe.supported)
        .map(|identity| public_identity(&identity));
    let service = service.inner().clone();
    let accepted = tauri::async_runtime::spawn_blocking(move || {
        service.enqueue_export(internal, identity.as_ref())
    })
    .await
    .map_err(|_| {
        command_error(
            operation,
            ExportErrorCode::InternalContract,
            "Export enqueue worker 异常结束。",
            false,
        )
    })?
    .map_err(service_error)?;
    let snapshot = jobs
        .get_job(GetJobOptions {
            project_path: project_path.clone(),
            expected_project_id: project_id.clone(),
            job_id: accepted.job_id.clone(),
        })
        .map_err(|_| {
            command_error(
                operation,
                ExportErrorCode::InternalContract,
                "Export Job 创建后无法读取。",
                true,
            )
        })?;
    runtime.schedule_supported_job(project_path, project_id, &snapshot);
    map_response(
        serde_json::to_value(accepted).expect("internal ExportEnqueueResultData serializes"),
        operation,
    )
}

#[tauri::command]
pub async fn get_export_result(
    request: GetExportResultRequest,
    service: State<'_, ExportService>,
) -> Result<ExportResult, ExportCommandError> {
    let operation = ExportOperation::GetResult;
    let value = serde_json::to_value(&request).map_err(|_| {
        command_error(
            operation,
            ExportErrorCode::InvalidRequest,
            "请求无法解码。",
            false,
        )
    })?;
    let project_path = required_string(&value, "projectPath", operation)?;
    let project_id = required_string(&value, "expectedProjectId", operation)?;
    let job_id = required_string(&value, "jobId", operation)?;
    let service = service.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        service.get_result(&project_path, &project_id, &job_id)
    })
    .await
    .map_err(|_| {
        command_error(
            operation,
            ExportErrorCode::InternalContract,
            "ExportResult worker 异常结束。",
            false,
        )
    })?
    .map_err(service_error)?;
    map_response(result, operation)
}

#[tauri::command]
pub async fn verify_export(
    request: VerifyExportRequest,
    service: State<'_, ExportService>,
) -> Result<ExportVerificationResult, ExportCommandError> {
    let operation = ExportOperation::Verify;
    let value = serde_json::to_value(&request).map_err(|_| {
        command_error(
            operation,
            ExportErrorCode::InvalidRequest,
            "请求无法解码。",
            false,
        )
    })?;
    let export_directory = required_string(&value, "exportDirectory", operation)?;
    let service = service.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || service.verify_export(&export_directory))
            .await
            .map_err(|_| {
                command_error(
                    operation,
                    ExportErrorCode::InternalContract,
                    "完整性校验 worker 异常结束。",
                    false,
                )
            })?
            .map_err(service_error)?;
    map_response(result, operation)
}

fn map_request<T: DeserializeOwned>(
    request: &impl Serialize,
    operation: ExportOperation,
) -> Result<T, ExportCommandError> {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            command_error(
                operation,
                ExportErrorCode::InvalidRequest,
                "请求未通过内部 Export v1 映射。",
                false,
            )
        })
}

fn map_response<T: DeserializeOwned>(
    value: Value,
    operation: ExportOperation,
) -> Result<T, ExportCommandError> {
    serde_json::from_value(value).map_err(|_| {
        command_error(
            operation,
            ExportErrorCode::InternalContract,
            "响应未通过生成契约。",
            false,
        )
    })
}

fn service_error(error: ExportServiceError) -> ExportCommandError {
    command_error(error.operation, error.code, &error.message, error.retryable)
}

fn command_error(
    operation: ExportOperation,
    code: ExportErrorCode,
    message: &str,
    retryable: bool,
) -> ExportCommandError {
    serde_json::from_value(json!({ "apiVersion": "1.0.0", "operation": operation.as_str(), "code": code.as_str(), "message": redact(message), "retryable": retryable, "details": {} })).expect("ExportCommandError literal follows schema")
}

fn required_string(
    value: &Value,
    key: &str,
    operation: ExportOperation,
) -> Result<String, ExportCommandError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            command_error(
                operation,
                ExportErrorCode::InvalidRequest,
                "请求缺少必需字段。",
                false,
            )
        })
}
fn public_identity(identity: &narracut_renderer::RendererIdentity) -> Value {
    json!({ "adapterId": identity.adapter_id, "adapterVersion": identity.adapter_version, "executableFileName": identity.executable_file_name, "executableHash": identity.executable_hash, "ffmpegVersion": identity.ffmpeg_version, "ffprobeFileName": identity.ffprobe_file_name, "ffprobeHash": identity.ffprobe_hash, "ffprobeVersion": identity.ffprobe_version, "capabilityHash": identity.capability_hash })
}
fn redact(message: &str) -> String {
    message
        .replace(['\\', '/'], " ")
        .split_whitespace()
        .take(80)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_never_expose_paths() {
        let error = command_error(
            ExportOperation::Commit,
            ExportErrorCode::Io,
            "C:\\Users\\alice\\secret.mp4 failed",
            true,
        );
        assert!(!error.message.contains("C:\\"));
        assert!(!error.message.contains("/"));
    }
}
