use narracut_contracts::{
    validate_contract_document, validate_storage_command_message, ArtifactCommitResult,
    ArtifactDraft, ArtifactReadResult, ArtifactVerificationResult, CacheCleanupResult,
    CleanProjectCacheRequest, ForgetProjectRequest, ForgetProjectResult, GetArtifactRequest,
    ImportArtifactFileRequest, IndexedJobsResult, ListIndexedJobsRequest,
    ListRecentProjectsRequest, ProjectIndexRebuildResult, RebuildProjectIndexRequest,
    RecentProjectsResult, StorageCommandError, VerifyArtifactRequest,
};
use narracut_core::{
    IndexedJobStatusData, ListIndexedJobsOptions, StorageErrorCode, StorageOperation,
    StorageService, StorageServiceError, StoreArtifactFileOptions,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

#[tauri::command]
pub async fn import_artifact_file(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<ArtifactCommitResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::ImportArtifact, move || {
        let request: ImportArtifactFileDto = decode_request::<ImportArtifactFileRequest, _>(
            request,
            StorageOperation::ImportArtifact,
        )?;
        let result = service
            .import_artifact_file(StoreArtifactFileOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                source_path: request.source_path,
                artifact: request.artifact,
            })
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::ImportArtifact)
    })
    .await
}

#[tauri::command]
pub async fn get_artifact(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<ArtifactReadResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::GetArtifact, move || {
        let request: ArtifactRequestDto =
            decode_request::<GetArtifactRequest, _>(request, StorageOperation::GetArtifact)?;
        let result = service
            .get_artifact(request.project_path, &request.artifact_id)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::GetArtifact)
    })
    .await
}

#[tauri::command]
pub async fn verify_artifact(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<ArtifactVerificationResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::VerifyArtifact, move || {
        let request: ArtifactRequestDto =
            decode_request::<VerifyArtifactRequest, _>(request, StorageOperation::VerifyArtifact)?;
        let result = service
            .verify_artifact(request.project_path, &request.artifact_id)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::VerifyArtifact)
    })
    .await
}

#[tauri::command]
pub async fn rebuild_project_index(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectIndexRebuildResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::RebuildProjectIndex, move || {
        let request: ProjectIdentityDto = decode_request::<RebuildProjectIndexRequest, _>(
            request,
            StorageOperation::RebuildProjectIndex,
        )?;
        let result = service
            .rebuild_project_index(request.project_path, &request.expected_project_id)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::RebuildProjectIndex)
    })
    .await
}

#[tauri::command]
pub async fn list_recent_projects(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<RecentProjectsResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::ListRecentProjects, move || {
        let request: ListRecentProjectsDto = decode_request::<ListRecentProjectsRequest, _>(
            request,
            StorageOperation::ListRecentProjects,
        )?;
        let result = service
            .list_recent_projects(request.limit, request.include_missing)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::ListRecentProjects)
    })
    .await
}

#[tauri::command]
pub async fn list_indexed_jobs(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<IndexedJobsResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::ListIndexedJobs, move || {
        let request: ListIndexedJobsDto = decode_request::<ListIndexedJobsRequest, _>(
            request,
            StorageOperation::ListIndexedJobs,
        )?;
        let result = service
            .list_indexed_jobs(ListIndexedJobsOptions {
                owner_project_id: request.owner_project_id,
                statuses: request.statuses,
                limit: request.limit,
            })
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::ListIndexedJobs)
    })
    .await
}

#[tauri::command]
pub async fn forget_project(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<ForgetProjectResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::ForgetProject, move || {
        let request: ForgetProjectDto =
            decode_request::<ForgetProjectRequest, _>(request, StorageOperation::ForgetProject)?;
        let result = service
            .forget_project(&request.owner_project_id)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::ForgetProject)
    })
    .await
}

#[tauri::command]
pub async fn clean_project_cache(
    state: State<'_, StorageService>,
    request: Value,
) -> Result<CacheCleanupResult, StorageCommandError> {
    let service = state.inner().clone();
    run_blocking(StorageOperation::CleanProjectCache, move || {
        let request: ProjectIdentityDto = decode_request::<CleanProjectCacheRequest, _>(
            request,
            StorageOperation::CleanProjectCache,
        )?;
        let result = service
            .clean_project_cache(request.project_path, &request.expected_project_id)
            .map_err(storage_error_to_contract)?;
        encode_response(result, StorageOperation::CleanProjectCache)
    })
    .await
}

async fn run_blocking<T, F>(operation: StorageOperation, task: F) -> Result<T, StorageCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, StorageCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            internal_contract_error(operation, format!("后台存储操作异常终止：{error}"))
        })?
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: StorageOperation,
) -> Result<TDto, StorageCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_storage_command_message(&request).map_err(|error| {
        invalid_request_error(operation, format!("请求未通过 storage-command v1：{error}"))
    })?;
    let generated: TContract = serde_json::from_value(request).map_err(|error| {
        invalid_request_error(operation, format!("请求无法解析为当前生成契约：{error}"))
    })?;
    let value = serde_json::to_value(generated).map_err(|error| {
        internal_contract_error(operation, format!("生成请求类型无法重新序列化：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        invalid_request_error(
            operation,
            format!("请求字段无法转换为存储服务输入：{error}"),
        )
    })
}

fn encode_response<TInternal, TContract>(
    response: TInternal,
    operation: StorageOperation,
) -> Result<TContract, StorageCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response).map_err(|error| {
        internal_contract_error(operation, format!("序列化存储响应失败：{error}"))
    })?;
    if let Some(artifact) = value.get("artifact") {
        validate_contract_document(artifact).map_err(|error| {
            internal_contract_error(
                operation,
                format!("响应中的 Artifact 违反 v1 契约：{error}"),
            )
        })?;
    }
    validate_storage_command_message(&value).map_err(|error| {
        internal_contract_error(operation, format!("存储响应违反 v1 契约：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        internal_contract_error(operation, format!("存储响应无法转换为生成类型：{error}"))
    })
}

fn storage_error_to_contract(error: StorageServiceError) -> StorageCommandError {
    let mut object = Map::from_iter([
        (
            "apiVersion".to_owned(),
            Value::String(narracut_contracts::NARRACUT_STORAGE_COMMAND_API_VERSION.to_owned()),
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
    if let Some(artifact_id) = error.artifact_id {
        object.insert("artifactId".to_owned(), Value::String(artifact_id));
    }
    contract_error_from_value(Value::Object(object), error.operation)
}

fn invalid_request_error(
    operation: StorageOperation,
    message: impl Into<String>,
) -> StorageCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_STORAGE_COMMAND_API_VERSION,
            "code": StorageErrorCode::InvalidRequest.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn internal_contract_error(
    operation: StorageOperation,
    message: impl Into<String>,
) -> StorageCommandError {
    contract_error_from_value(
        json!({
            "apiVersion": narracut_contracts::NARRACUT_STORAGE_COMMAND_API_VERSION,
            "code": StorageErrorCode::InternalContractError.as_str(),
            "operation": operation.as_str(),
            "message": message.into()
        }),
        operation,
    )
}

fn contract_error_from_value(value: Value, operation: StorageOperation) -> StorageCommandError {
    validate_storage_command_message(&value).unwrap_or_else(|error| {
        panic!(
            "storage error adapter produced invalid schema for {}: {error}; value={value}",
            operation.as_str()
        )
    });
    serde_json::from_value(value).unwrap_or_else(|error| {
        panic!(
            "storage error adapter failed to deserialize generated type for {}: {error}",
            operation.as_str()
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportArtifactFileDto {
    project_path: String,
    expected_project_id: String,
    source_path: String,
    artifact: ArtifactDraft,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactRequestDto {
    project_path: String,
    artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectIdentityDto {
    project_path: String,
    expected_project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRecentProjectsDto {
    limit: u32,
    include_missing: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListIndexedJobsDto {
    owner_project_id: Option<String>,
    statuses: Vec<IndexedJobStatusData>,
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForgetProjectDto {
    owner_project_id: String,
}

#[cfg(test)]
mod tests {
    use super::{
        decode_request, storage_error_to_contract, ArtifactRequestDto, ImportArtifactFileDto,
    };
    use narracut_contracts::{
        validate_storage_command_message, GetArtifactRequest, ImportArtifactFileRequest,
    };
    use narracut_core::{StorageErrorCode, StorageOperation, StorageServiceError};

    #[test]
    fn raw_requests_are_schema_checked_before_dto_conversion() {
        let extra_field = decode_request::<GetArtifactRequest, ArtifactRequestDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "get_artifact",
                "projectPath": "C:/Videos/demo",
                "artifactId": "artifact_demo",
                "unexpected": true
            }),
            StorageOperation::GetArtifact,
        )
        .expect_err("additional fields must fail");
        let value = serde_json::to_value(extra_field).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_storage_command_message(&value).expect("error follows schema");

        let malformed_draft = decode_request::<ImportArtifactFileRequest, ImportArtifactFileDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "import_artifact_file",
                "projectPath": "C:/Videos/demo",
                "expectedProjectId": "project_demo",
                "sourcePath": "C:/Imports/script.md",
                "artifact": {"stageId": "script"}
            }),
            StorageOperation::ImportArtifact,
        )
        .expect_err("incomplete ArtifactDraft must fail DTO conversion");
        let value = serde_json::to_value(malformed_draft).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_storage_command_message(&value).expect("error follows schema");

        let forged_source_hash =
            decode_request::<ImportArtifactFileRequest, ImportArtifactFileDto>(
                serde_json::json!({
                    "apiVersion": "1.0.0",
                    "command": "import_artifact_file",
                    "projectPath": "C:/Videos/demo",
                    "expectedProjectId": "project_demo",
                    "sourcePath": "C:/Imports/source.png",
                    "artifact": {
                        "stageId": "research",
                        "runId": "run_research_001",
                        "kind": "source_image",
                        "mediaType": "image/png",
                        "evidenceRole": "factual_evidence",
                        "source": {
                            "origin": "imported",
                            "sourceUri": "https://example.com/source.png",
                            "author": "Example Author",
                            "license": "CC-BY-4.0",
                            "attributionText": "Example Author / CC-BY-4.0",
                            "sourceContentHash": "sha256:forged",
                            "authorizationRecordIds": ["authorization_001"]
                        },
                        "provenance": []
                    }
                }),
                StorageOperation::ImportArtifact,
            )
            .expect_err("sourceContentHash must be computed by the store");
        let value = serde_json::to_value(forged_source_hash).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_storage_command_message(&value).expect("error follows schema");
    }

    #[test]
    fn core_errors_remain_structured_storage_errors() {
        let contract = storage_error_to_contract(
            StorageServiceError::new(
                StorageErrorCode::ContentCorrupt,
                StorageOperation::VerifyArtifact,
                "hash mismatch",
            )
            .for_artifact("artifact_demo"),
        );
        let value = serde_json::to_value(contract).expect("serialize error");
        assert_eq!(value["code"], "content_corrupt");
        assert_eq!(value["operation"], "verify_artifact");
        assert_eq!(value["artifactId"], "artifact_demo");
        validate_storage_command_message(&value).expect("error follows schema");
    }
}
