// 任务队列只向前端暴露七个有界命令。worker 领取、续租、进度与终态提交保留在
// Rust 内部执行接口，避免前端获得可伪造执行历史的低层能力。
#![allow(clippy::result_large_err)]

use narracut_contracts::{
    validate_contract_document, validate_job_command_message, CancelJobRequest,
    EnqueueStageJobRequest, GetJobRequest, JobCommandError, JobEventsResult, JobListResult,
    JobRecoveryResult, JobSnapshot, ListJobEventsRequest, ListJobsRequest, RecoverJobsRequest,
    RetryStageJobRequest,
};
use narracut_core::{
    CancelJobOptions, EnqueueStageJobOptions, GetJobOptions, JobErrorCode, JobOperation,
    JobService, JobServiceError, JobStatusData, ListJobEventsOptions, ListJobsOptions,
    RecoverJobsOptions, RetryPolicyData, RetryStageJobOptions,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

use crate::media_runtime::{MediaRetryOptions, MediaRuntime, MediaRuntimeError};
use crate::provider_runtime::ProviderRuntime;
use crate::renderer_runtime::{RendererRuntime, RendererRuntimeError};

#[tauri::command]
pub async fn enqueue_stage_job(
    state: State<'_, JobService>,
    request: Value,
) -> Result<JobSnapshot, JobCommandError> {
    let service = state.inner().clone();
    run_blocking(JobOperation::EnqueueStageJob, move || {
        let request: EnqueueStageJobDto =
            decode_request::<EnqueueStageJobRequest, _>(request, JobOperation::EnqueueStageJob)?;
        let result = service
            .enqueue_stage_job(EnqueueStageJobOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                stage_id: request.stage_id,
                run_id: request.run_id,
                input_refs: request.input_refs,
                executor: request.executor,
                idempotency_key: request.idempotency_key,
                retry_policy: request.retry_policy,
            })
            .map_err(job_error_to_contract)?;
        encode_response(result, JobOperation::EnqueueStageJob)
    })
    .await
}

#[tauri::command]
pub async fn get_job(
    state: State<'_, JobService>,
    request: Value,
) -> Result<JobSnapshot, JobCommandError> {
    let service = state.inner().clone();
    run_blocking(JobOperation::GetJob, move || {
        let request: GetJobDto = decode_request::<GetJobRequest, _>(request, JobOperation::GetJob)?;
        let result = service
            .get_job(GetJobOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                job_id: request.job_id,
            })
            .map_err(job_error_to_contract)?;
        encode_response(result, JobOperation::GetJob)
    })
    .await
}

#[tauri::command]
pub async fn list_jobs(
    state: State<'_, JobService>,
    request: Value,
) -> Result<JobListResult, JobCommandError> {
    let service = state.inner().clone();
    run_blocking(JobOperation::ListJobs, move || {
        let request: ListJobsDto =
            decode_request::<ListJobsRequest, _>(request, JobOperation::ListJobs)?;
        let result = service
            .list_jobs(ListJobsOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                statuses: request.statuses,
                limit: request.limit,
            })
            .map_err(job_error_to_contract)?;
        encode_response(result, JobOperation::ListJobs)
    })
    .await
}

#[tauri::command]
pub async fn list_job_events(
    state: State<'_, JobService>,
    request: Value,
) -> Result<JobEventsResult, JobCommandError> {
    let service = state.inner().clone();
    run_blocking(JobOperation::ListJobEvents, move || {
        let request: ListJobEventsDto =
            decode_request::<ListJobEventsRequest, _>(request, JobOperation::ListJobEvents)?;
        let result = service
            .list_job_events(ListJobEventsOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                job_id: request.job_id,
                after_sequence: request.after_sequence,
                limit: request.limit,
            })
            .map_err(job_error_to_contract)?;
        encode_response(result, JobOperation::ListJobEvents)
    })
    .await
}

#[tauri::command]
pub async fn cancel_job(
    state: State<'_, JobService>,
    request: Value,
) -> Result<JobSnapshot, JobCommandError> {
    let service = state.inner().clone();
    run_blocking(JobOperation::CancelJob, move || {
        let request: CancelJobDto =
            decode_request::<CancelJobRequest, _>(request, JobOperation::CancelJob)?;
        let result = service
            .cancel_job(CancelJobOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                job_id: request.job_id,
                message: request.message,
            })
            .map_err(job_error_to_contract)?;
        encode_response(result, JobOperation::CancelJob)
    })
    .await
}

#[tauri::command]
pub async fn retry_stage_job(
    state: State<'_, JobService>,
    provider_runtime: State<'_, ProviderRuntime>,
    media_runtime: State<'_, MediaRuntime>,
    renderer_runtime: State<'_, RendererRuntime>,
    request: Value,
) -> Result<JobSnapshot, JobCommandError> {
    let service = state.inner().clone();
    let provider_runtime = provider_runtime.inner().clone();
    let media_runtime = media_runtime.inner().clone();
    let renderer_runtime = renderer_runtime.inner().clone();
    run_blocking(JobOperation::RetryStageJob, move || {
        let request: RetryStageJobDto =
            decode_request::<RetryStageJobRequest, _>(request, JobOperation::RetryStageJob)?;
        let project_path = request.project_path.clone();
        let expected_project_id = request.expected_project_id.clone();
        let source = service
            .get_job(GetJobOptions {
                project_path: request.project_path.clone(),
                expected_project_id: request.expected_project_id.clone(),
                job_id: request.source_job_id.clone(),
            })
            .map_err(job_error_to_contract)?;
        let result = if renderer_runtime.supports_renderer_job(&source) {
            renderer_runtime
                .retry_render_job(
                    request.project_path,
                    request.expected_project_id,
                    request.source_job_id,
                    request.new_run_id,
                    request.idempotency_key,
                )
                .map_err(renderer_retry_error_to_contract)?
        } else if media_runtime.supports_media_job(&source) {
            media_runtime
                .retry_media_job(MediaRetryOptions {
                    project_path: request.project_path,
                    expected_project_id: request.expected_project_id,
                    source_job_id: request.source_job_id,
                    new_run_id: request.new_run_id,
                    idempotency_key: request.idempotency_key,
                })
                .map_err(media_retry_error_to_contract)?
        } else {
            service
                .retry_stage_job(RetryStageJobOptions {
                    project_path: request.project_path,
                    expected_project_id: request.expected_project_id,
                    source_job_id: request.source_job_id,
                    new_run_id: request.new_run_id,
                    idempotency_key: request.idempotency_key,
                })
                .map_err(job_error_to_contract)?
        };
        let _schedule_result =
            provider_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        let _schedule_result =
            media_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        let _schedule_result =
            renderer_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        encode_response(result, JobOperation::RetryStageJob)
    })
    .await
}

#[tauri::command]
pub async fn recover_jobs(
    state: State<'_, JobService>,
    provider_runtime: State<'_, ProviderRuntime>,
    media_runtime: State<'_, MediaRuntime>,
    renderer_runtime: State<'_, RendererRuntime>,
    request: Value,
) -> Result<JobRecoveryResult, JobCommandError> {
    let service = state.inner().clone();
    let provider_runtime = provider_runtime.inner().clone();
    let media_runtime = media_runtime.inner().clone();
    let renderer_runtime = renderer_runtime.inner().clone();
    run_blocking(JobOperation::RecoverJobs, move || {
        let request: RecoverJobsDto =
            decode_request::<RecoverJobsRequest, _>(request, JobOperation::RecoverJobs)?;
        let project_path = request.project_path.clone();
        let expected_project_id = request.expected_project_id.clone();
        let result = service
            .recover_project_jobs(RecoverJobsOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
            })
            .map_err(job_error_to_contract)?;
        let _schedule_result =
            provider_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        let _schedule_result =
            media_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        let _schedule_result =
            renderer_runtime.schedule_project_jobs(&project_path, &expected_project_id);
        encode_response(result, JobOperation::RecoverJobs)
    })
    .await
}

fn renderer_retry_error_to_contract(error: RendererRuntimeError) -> JobCommandError {
    match error {
        RendererRuntimeError::Job(error) => job_error_to_contract(error),
        RendererRuntimeError::Service(error) => internal_contract_error(
            JobOperation::RetryStageJob,
            format!("Renderer retry was rejected: {}", error.message),
        ),
        RendererRuntimeError::InvalidReceipt => invalid_request_error(
            JobOperation::RetryStageJob,
            "The source Job is not a retryable Renderer Job.",
        ),
        RendererRuntimeError::UnsafePath => internal_contract_error(
            JobOperation::RetryStageJob,
            "The Renderer temporary path is unsafe.",
        ),
    }
}

async fn run_blocking<T, F>(operation: JobOperation, task: F) -> Result<T, JobCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, JobCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            internal_contract_error(operation, format!("后台任务队列操作异常终止：{error}"))
        })?
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: JobOperation,
) -> Result<TDto, JobCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_job_command_message(&request).map_err(|error| {
        invalid_request_error(operation, format!("请求未通过 job-command v1：{error}"))
    })?;
    let generated: TContract = serde_json::from_value(request).map_err(|error| {
        invalid_request_error(operation, format!("请求无法解析为生成契约：{error}"))
    })?;
    let value = serde_json::to_value(generated).map_err(|error| {
        internal_contract_error(operation, format!("生成请求类型无法重新序列化：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        invalid_request_error(
            operation,
            format!("请求字段无法转换为任务队列输入：{error}"),
        )
    })
}

fn encode_response<TInternal, TContract>(
    response: TInternal,
    operation: JobOperation,
) -> Result<TContract, JobCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response).map_err(|error| {
        internal_contract_error(operation, format!("序列化任务队列响应失败：{error}"))
    })?;
    validate_embedded_documents(&value, operation)?;
    validate_job_command_message(&value).map_err(|error| {
        internal_contract_error(operation, format!("任务队列响应违反 v1 契约：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        internal_contract_error(
            operation,
            format!("任务队列响应无法转换为生成类型：{error}"),
        )
    })
}

fn validate_embedded_documents(
    response: &Value,
    operation: JobOperation,
) -> Result<(), JobCommandError> {
    if let Some(document) = response.get("job") {
        validate_contract_document(document).map_err(|error| {
            internal_contract_error(operation, format!("响应 job 违反持久化契约：{error}"))
        })?;
    }
    if let Some(jobs) = response.get("jobs").and_then(Value::as_array) {
        for snapshot in jobs {
            if let Some(document) = snapshot.get("job") {
                validate_contract_document(document).map_err(|error| {
                    internal_contract_error(
                        operation,
                        format!("响应 jobs 包含非法 JobDefinition：{error}"),
                    )
                })?;
            }
        }
    }
    if let Some(events) = response.get("events").and_then(Value::as_array) {
        for event in events {
            validate_contract_document(event).map_err(|error| {
                internal_contract_error(
                    operation,
                    format!("响应 events 包含非法 JobEvent：{error}"),
                )
            })?;
        }
    }
    Ok(())
}

fn job_error_to_contract(error: JobServiceError) -> JobCommandError {
    let operation = error.operation;
    let mut object = Map::from_iter([
        (
            "apiVersion".to_owned(),
            Value::String(narracut_contracts::NARRACUT_JOB_COMMAND_API_VERSION.to_owned()),
        ),
        (
            "code".to_owned(),
            Value::String(error.code.as_str().to_owned()),
        ),
        (
            "operation".to_owned(),
            Value::String(operation.as_str().to_owned()),
        ),
        ("message".to_owned(), Value::String(error.message)),
    ]);
    if let Some(path) = error.path {
        object.insert("path".to_owned(), Value::String(path));
    }
    if let Some(job_id) = error.job_id {
        object.insert("jobId".to_owned(), Value::String(job_id.into()));
    }
    if let Some(stage_id) = error.stage_id {
        object.insert("stageId".to_owned(), Value::String(stage_id));
    }
    if let Some(run_id) = error.run_id {
        object.insert("runId".to_owned(), Value::String(run_id));
    }
    contract_error_from_value(Value::Object(object), operation)
}

fn media_retry_error_to_contract(error: MediaRuntimeError) -> JobCommandError {
    match error {
        MediaRuntimeError::Job(mut error) => {
            error.operation = JobOperation::RetryStageJob;
            job_error_to_contract(error)
        }
        MediaRuntimeError::Storage(_)
        | MediaRuntimeError::Media(_)
        | MediaRuntimeError::Serialization(_)
        | MediaRuntimeError::InvalidSnapshot(_) => internal_contract_error(
            JobOperation::RetryStageJob,
            "媒体重试请求或持久化快照未通过内部一致性校验。",
        ),
    }
}

fn invalid_request_error(operation: JobOperation, message: impl Into<String>) -> JobCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_JOB_COMMAND_API_VERSION,
            "code": JobErrorCode::InvalidRequest.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn internal_contract_error(operation: JobOperation, message: impl Into<String>) -> JobCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_JOB_COMMAND_API_VERSION,
            "code": JobErrorCode::InternalContractError.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn contract_error_from_value(value: Value, operation: JobOperation) -> JobCommandError {
    validate_job_command_message(&value).unwrap_or_else(|error| {
        panic!(
            "job error adapter produced invalid schema for {}: {error}; value={value}",
            operation.as_str()
        )
    });
    serde_json::from_value(value).unwrap_or_else(|error| {
        panic!(
            "job error adapter failed to deserialize generated type for {}: {error}",
            operation.as_str()
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueStageJobDto {
    project_path: String,
    expected_project_id: String,
    stage_id: String,
    run_id: String,
    input_refs: Vec<Value>,
    executor: Value,
    idempotency_key: String,
    retry_policy: RetryPolicyData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetJobDto {
    project_path: String,
    expected_project_id: String,
    job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListJobsDto {
    project_path: String,
    expected_project_id: String,
    statuses: Vec<JobStatusData>,
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListJobEventsDto {
    project_path: String,
    expected_project_id: String,
    job_id: String,
    after_sequence: Option<u32>,
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelJobDto {
    project_path: String,
    expected_project_id: String,
    job_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetryStageJobDto {
    project_path: String,
    expected_project_id: String,
    source_job_id: String,
    new_run_id: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoverJobsDto {
    project_path: String,
    expected_project_id: String,
}

#[cfg(test)]
mod tests {
    use super::{decode_request, job_error_to_contract, GetJobDto};
    use narracut_contracts::{validate_job_command_message, GetJobRequest};
    use narracut_core::{JobErrorCode, JobOperation, JobServiceError};

    #[test]
    fn raw_requests_are_schema_checked_before_dto_conversion() {
        let error = decode_request::<GetJobRequest, GetJobDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "get_job",
                "projectPath": "C:/Videos/demo",
                "expectedProjectId": "project_demo",
                "jobId": "job_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "unexpected": true
            }),
            JobOperation::GetJob,
        )
        .expect_err("additional field must fail");
        let value = serde_json::to_value(error).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_job_command_message(&value).expect("error follows schema");
    }

    #[test]
    fn core_errors_remain_structured_job_errors() {
        let contract = job_error_to_contract(
            JobServiceError::new(
                JobErrorCode::IdempotencyConflict,
                JobOperation::EnqueueStageJob,
                "幂等键已绑定其他请求",
            )
            .for_job("job_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .for_stage("brief")
            .for_run("run_brief_001"),
        );
        let value = serde_json::to_value(contract).expect("serialize error");
        assert_eq!(value["code"], "idempotency_conflict");
        assert_eq!(value["stageId"], "brief");
        assert_eq!(value["runId"], "run_brief_001");
        validate_job_command_message(&value).expect("error follows schema");
    }
}
