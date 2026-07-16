// 生成的错误契约同时保留 path、stageId 与 runId；保持未装箱可让 Tauri command
// 直接暴露同一个版本化类型，序列化边界不会出现第二套包装错误。
#![allow(clippy::result_large_err)]

use narracut_contracts::{
    validate_contract_document, validate_workflow_command_message, GetWorkflowRequest,
    InitializeWorkflowRequest, ListStageHistoryRequest, PrepareStageRunRequest,
    PreviewRegenerationRequest, RecordStageRunRequest, RegenerationImpactResult,
    ReviewStageRunRequest, StageConfigUpdateResult, StageHistoryResult, StageReviewResult,
    StageRunCommitResult, StageRunPreparationResult, UpdateStageConfigRequest,
    WorkflowCommandError, WorkflowSnapshot,
};
use narracut_core::{
    InitializeWorkflowOptions, PrepareStageRunOptions, RecordStageRunOptions, ReviewDecisionData,
    ReviewStageRunOptions, ReviewerReferenceData, TerminalRunStatusData, UpdateStageConfigOptions,
    WorkflowErrorCode, WorkflowOperation, WorkflowService, WorkflowServiceError,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

#[tauri::command]
pub async fn initialize_project_workflow(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<WorkflowSnapshot, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::InitializeWorkflow, move || {
        let request: InitializeWorkflowDto = decode_request::<InitializeWorkflowRequest, _>(
            request,
            WorkflowOperation::InitializeWorkflow,
        )?;
        let result = service
            .initialize_project_workflow(InitializeWorkflowOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
            })
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::InitializeWorkflow)
    })
    .await
}

#[tauri::command]
pub async fn get_project_workflow(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<WorkflowSnapshot, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::GetWorkflow, move || {
        let request: ProjectPathDto =
            decode_request::<GetWorkflowRequest, _>(request, WorkflowOperation::GetWorkflow)?;
        let result = service
            .get_project_workflow(request.project_path)
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::GetWorkflow)
    })
    .await
}

#[tauri::command]
pub async fn update_stage_config(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<StageConfigUpdateResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::UpdateStageConfig, move || {
        let request: UpdateStageConfigDto = decode_request::<UpdateStageConfigRequest, _>(
            request,
            WorkflowOperation::UpdateStageConfig,
        )?;
        let result = service
            .update_stage_config(UpdateStageConfigOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                stage_id: request.stage_id,
                expected_revision: request.expected_revision,
                values: request.values,
                decisions: request.decisions,
            })
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::UpdateStageConfig)
    })
    .await
}

#[tauri::command]
pub async fn prepare_stage_run(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<StageRunPreparationResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::PrepareStageRun, move || {
        let request: PrepareStageRunDto = decode_request::<PrepareStageRunRequest, _>(
            request,
            WorkflowOperation::PrepareStageRun,
        )?;
        let result = service
            .prepare_stage_run(PrepareStageRunOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                stage_id: request.stage_id,
                run_id: request.run_id,
                job_id: request.job_id,
                input_refs: request.input_refs,
                executor: request.executor,
            })
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::PrepareStageRun)
    })
    .await
}

#[tauri::command]
pub async fn record_stage_run(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<StageRunCommitResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::RecordStageRun, move || {
        let request: RecordStageRunDto =
            decode_request::<RecordStageRunRequest, _>(request, WorkflowOperation::RecordStageRun)?;
        let result = service
            .record_stage_run(RecordStageRunOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                stage_id: request.stage_id,
                run_id: request.run_id,
                status: request.status,
                job_id: request.job_id,
                artifact_ids: request.artifact_ids,
                log_summary: request.log_summary,
            })
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::RecordStageRun)
    })
    .await
}

#[tauri::command]
pub async fn review_stage_run(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<StageReviewResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::ReviewStageRun, move || {
        let request: ReviewStageRunDto =
            decode_request::<ReviewStageRunRequest, _>(request, WorkflowOperation::ReviewStageRun)?;
        let result = service
            .review_stage_run(ReviewStageRunOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                stage_id: request.stage_id,
                run_id: request.run_id,
                review_id: request.review_id,
                decision: request.decision,
                reviewer: request.reviewer,
                comments: request.comments,
                artifact_ids: request.artifact_ids,
            })
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::ReviewStageRun)
    })
    .await
}

#[tauri::command]
pub async fn preview_regeneration(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<RegenerationImpactResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::PreviewRegeneration, move || {
        let request: PreviewRegenerationDto = decode_request::<PreviewRegenerationRequest, _>(
            request,
            WorkflowOperation::PreviewRegeneration,
        )?;
        let result = service
            .preview_regeneration(request.project_path, request.changed_stage_ids)
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::PreviewRegeneration)
    })
    .await
}

#[tauri::command]
pub async fn list_stage_history(
    state: State<'_, WorkflowService>,
    request: Value,
) -> Result<StageHistoryResult, WorkflowCommandError> {
    let service = state.inner().clone();
    run_blocking(WorkflowOperation::ListStageHistory, move || {
        let request: ListStageHistoryDto = decode_request::<ListStageHistoryRequest, _>(
            request,
            WorkflowOperation::ListStageHistory,
        )?;
        let result = service
            .list_stage_history(request.project_path, &request.stage_id, request.limit)
            .map_err(workflow_error_to_contract)?;
        encode_response(result, WorkflowOperation::ListStageHistory)
    })
    .await
}

async fn run_blocking<T, F>(
    operation: WorkflowOperation,
    task: F,
) -> Result<T, WorkflowCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, WorkflowCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            internal_contract_error(operation, format!("后台工作流操作异常终止：{error}"))
        })?
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: WorkflowOperation,
) -> Result<TDto, WorkflowCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_workflow_command_message(&request).map_err(|error| {
        invalid_request_error(
            operation,
            format!("请求未通过 workflow-command v1：{error}"),
        )
    })?;
    let generated: TContract = serde_json::from_value(request).map_err(|error| {
        invalid_request_error(operation, format!("请求无法解析为生成契约：{error}"))
    })?;
    let value = serde_json::to_value(generated).map_err(|error| {
        internal_contract_error(operation, format!("生成请求类型无法重新序列化：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        invalid_request_error(operation, format!("请求字段无法转换为工作流输入：{error}"))
    })
}

fn encode_response<TInternal, TContract>(
    response: TInternal,
    operation: WorkflowOperation,
) -> Result<TContract, WorkflowCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response).map_err(|error| {
        internal_contract_error(operation, format!("序列化工作流响应失败：{error}"))
    })?;
    validate_embedded_documents(&value, operation)?;
    validate_workflow_command_message(&value).map_err(|error| {
        internal_contract_error(operation, format!("工作流响应违反 v1 契约：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        internal_contract_error(operation, format!("工作流响应无法转换为生成类型：{error}"))
    })
}

fn validate_embedded_documents(
    response: &Value,
    operation: WorkflowOperation,
) -> Result<(), WorkflowCommandError> {
    for field in ["config", "executionSnapshot", "run", "review"] {
        if let Some(document) = response.get(field) {
            validate_contract_document(document).map_err(|error| {
                internal_contract_error(
                    operation,
                    format!("响应字段 {field} 违反持久化契约：{error}"),
                )
            })?;
        }
    }
    for field in ["stageDefinitions", "configs", "runs", "reviews"] {
        if let Some(documents) = response.get(field).and_then(Value::as_array) {
            for document in documents {
                validate_contract_document(document).map_err(|error| {
                    internal_contract_error(
                        operation,
                        format!("响应数组 {field} 包含非法持久化文档：{error}"),
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn workflow_error_to_contract(error: WorkflowServiceError) -> WorkflowCommandError {
    let mut object = Map::from_iter([
        (
            "apiVersion".to_owned(),
            Value::String(narracut_contracts::NARRACUT_WORKFLOW_COMMAND_API_VERSION.to_owned()),
        ),
        (
            "code".to_owned(),
            Value::String(error.code.as_str().to_owned()),
        ),
        (
            "operation".to_owned(),
            Value::String(error.operation.as_str().to_owned()),
        ),
        ("message".to_owned(), Value::String(error.message)),
    ]);
    if let Some(path) = error.path {
        object.insert("path".to_owned(), Value::String(path));
    }
    if let Some(stage_id) = error.stage_id {
        object.insert("stageId".to_owned(), Value::String(stage_id));
    }
    if let Some(run_id) = error.run_id {
        object.insert("runId".to_owned(), Value::String(run_id));
    }
    contract_error_from_value(Value::Object(object), error.operation)
}

fn invalid_request_error(
    operation: WorkflowOperation,
    message: impl Into<String>,
) -> WorkflowCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_WORKFLOW_COMMAND_API_VERSION,
            "code": WorkflowErrorCode::InvalidRequest.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn internal_contract_error(
    operation: WorkflowOperation,
    message: impl Into<String>,
) -> WorkflowCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_WORKFLOW_COMMAND_API_VERSION,
            "code": WorkflowErrorCode::InternalContractError.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn contract_error_from_value(value: Value, operation: WorkflowOperation) -> WorkflowCommandError {
    validate_workflow_command_message(&value).unwrap_or_else(|error| {
        panic!(
            "workflow error adapter produced invalid schema for {}: {error}; value={value}",
            operation.as_str()
        )
    });
    serde_json::from_value(value).unwrap_or_else(|error| {
        panic!(
            "workflow error adapter failed to deserialize generated type for {}: {error}",
            operation.as_str()
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeWorkflowDto {
    project_path: String,
    expected_project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectPathDto {
    project_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStageConfigDto {
    project_path: String,
    expected_project_id: String,
    stage_id: String,
    expected_revision: u32,
    values: Map<String, Value>,
    decisions: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareStageRunDto {
    project_path: String,
    expected_project_id: String,
    stage_id: String,
    run_id: String,
    job_id: String,
    input_refs: Vec<Value>,
    executor: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordStageRunDto {
    project_path: String,
    expected_project_id: String,
    stage_id: String,
    run_id: String,
    status: TerminalRunStatusData,
    job_id: String,
    artifact_ids: Vec<String>,
    log_summary: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewStageRunDto {
    project_path: String,
    expected_project_id: String,
    stage_id: String,
    run_id: String,
    review_id: String,
    decision: ReviewDecisionData,
    reviewer: ReviewerReferenceData,
    comments: String,
    artifact_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRegenerationDto {
    project_path: String,
    changed_stage_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListStageHistoryDto {
    project_path: String,
    stage_id: String,
    limit: u32,
}

#[cfg(test)]
mod tests {
    use super::{decode_request, workflow_error_to_contract, ProjectPathDto};
    use narracut_contracts::{validate_workflow_command_message, GetWorkflowRequest};
    use narracut_core::{WorkflowErrorCode, WorkflowOperation, WorkflowServiceError};

    #[test]
    fn raw_requests_are_schema_checked_before_dto_conversion() {
        let error = decode_request::<GetWorkflowRequest, ProjectPathDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "get_project_workflow",
                "projectPath": "C:/Videos/demo",
                "unexpected": true
            }),
            WorkflowOperation::GetWorkflow,
        )
        .expect_err("additional field must fail");
        let value = serde_json::to_value(error).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_workflow_command_message(&value).expect("error follows schema");
    }

    #[test]
    fn core_errors_remain_structured_workflow_errors() {
        let contract = workflow_error_to_contract(
            WorkflowServiceError::new(
                WorkflowErrorCode::StageNotReady,
                WorkflowOperation::RecordStageRun,
                "上游尚未批准",
            )
            .for_stage("script")
            .for_run("run_script_001"),
        );
        let value = serde_json::to_value(contract).expect("serialize error");
        assert_eq!(value["code"], "stage_not_ready");
        assert_eq!(value["stageId"], "script");
        assert_eq!(value["runId"], "run_script_001");
        validate_workflow_command_message(&value).expect("error follows schema");
    }
}
