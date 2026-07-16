use narracut_contracts::{
    validate_project_command_message, CopyProjectRequest, CreateProjectRequest,
    InspectProjectRequest, MigrateProjectRequest, MoveProjectToTrashRequest, OpenProjectRequest,
    ProjectCommandError, ProjectCopyResult, ProjectDescriptor, ProjectInspection,
    ProjectMigrationResult, ProjectTrashResult, RenameProjectRequest, SetProjectArchivedRequest,
};
use narracut_core::{
    CopyProjectOptions, CreateProjectOptions, ProjectErrorCode, ProjectOperation, ProjectService,
    ProjectServiceError, StorageService,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

#[tauri::command]
pub async fn inspect_project(
    state: State<'_, ProjectService>,
    request: Value,
) -> Result<ProjectInspection, ProjectCommandError> {
    let service = state.inner().clone();
    run_blocking(ProjectOperation::Inspect, move || {
        let request: ProjectPathDto =
            decode_request::<InspectProjectRequest, _>(request, ProjectOperation::Inspect)?;
        let result = service
            .inspect_project(request.project_path)
            .map_err(project_error_to_contract)?;
        encode_response(result, ProjectOperation::Inspect)
    })
    .await
}

#[tauri::command]
pub async fn open_project(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectDescriptor, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::Open, move || {
        let request: ProjectPathDto =
            decode_request::<OpenProjectRequest, _>(request, ProjectOperation::Open)?;
        let result = service
            .open_project(request.project_path)
            .map_err(project_error_to_contract)?;
        let _index_result = index.record_recent_project(&result);
        encode_response(result, ProjectOperation::Open)
    })
    .await
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectDescriptor, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::Create, move || {
        let request: CreateProjectDto =
            decode_request::<CreateProjectRequest, _>(request, ProjectOperation::Create)?;
        let result = service
            .create_project(CreateProjectOptions {
                parent_path: request.parent_path,
                directory_name: request.directory_name,
                name: request.name,
                workflow_definition_id: request.workflow_definition_id,
                default_locale: request.default_locale,
            })
            .map_err(project_error_to_contract)?;
        let _index_result = index.record_recent_project(&result);
        encode_response(result, ProjectOperation::Create)
    })
    .await
}

#[tauri::command]
pub async fn migrate_project(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectMigrationResult, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::Migrate, move || {
        let request: MigrateProjectDto =
            decode_request::<MigrateProjectRequest, _>(request, ProjectOperation::Migrate)?;
        let result = service
            .migrate_project(request.project_path, request.expected_source_format_version)
            .map_err(project_error_to_contract)?;
        let _index_result = index.record_recent_project(&result.project);
        encode_response(result, ProjectOperation::Migrate)
    })
    .await
}

#[tauri::command]
pub async fn rename_project(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectDescriptor, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::Rename, move || {
        let request: RenameProjectDto =
            decode_request::<RenameProjectRequest, _>(request, ProjectOperation::Rename)?;
        let result = service
            .rename_project(request.project_path, &request.new_name)
            .map_err(project_error_to_contract)?;
        let _index_result = index.record_recent_project(&result);
        encode_response(result, ProjectOperation::Rename)
    })
    .await
}

#[tauri::command]
pub async fn copy_project(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectCopyResult, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::Copy, move || {
        let request: CopyProjectDto =
            decode_request::<CopyProjectRequest, _>(request, ProjectOperation::Copy)?;
        let result = service
            .copy_project(CopyProjectOptions {
                source_project_path: request.source_project_path,
                destination_parent_path: request.destination_parent_path,
                directory_name: request.directory_name,
                name: request.name,
            })
            .map_err(project_error_to_contract)?;
        let _index_result =
            index.rebuild_project_index(&result.project.project_path, &result.project.project_id);
        encode_response(result, ProjectOperation::Copy)
    })
    .await
}

#[tauri::command]
pub async fn set_project_archived(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectDescriptor, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::SetArchived, move || {
        let request: SetProjectArchivedDto =
            decode_request::<SetProjectArchivedRequest, _>(request, ProjectOperation::SetArchived)?;
        let result = service
            .set_project_archived(request.project_path, request.archived)
            .map_err(project_error_to_contract)?;
        let _index_result = index.record_recent_project(&result);
        encode_response(result, ProjectOperation::SetArchived)
    })
    .await
}

#[tauri::command]
pub async fn move_project_to_trash(
    state: State<'_, ProjectService>,
    index: State<'_, StorageService>,
    request: Value,
) -> Result<ProjectTrashResult, ProjectCommandError> {
    let service = state.inner().clone();
    let index = index.inner().clone();
    run_blocking(ProjectOperation::MoveToTrash, move || {
        let request: MoveProjectToTrashDto =
            decode_request::<MoveProjectToTrashRequest, _>(request, ProjectOperation::MoveToTrash)?;
        let result = service
            .move_project_to_trash(request.project_path, &request.expected_project_id)
            .map_err(project_error_to_contract)?;
        let _index_result = index.forget_project(&result.project_id);
        encode_response(result, ProjectOperation::MoveToTrash)
    })
    .await
}

async fn run_blocking<T, F>(operation: ProjectOperation, task: F) -> Result<T, ProjectCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ProjectCommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            internal_contract_error(operation, format!("后台文件操作异常终止：{error}"))
        })?
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: ProjectOperation,
) -> Result<TDto, ProjectCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_project_command_message(&request).map_err(|error| {
        project_error_to_contract(ProjectServiceError::new(
            ProjectErrorCode::InvalidRequest,
            operation,
            format!("command 请求不满足 project-command v1：{error}"),
        ))
    })?;
    let contract = serde_json::from_value::<TContract>(request).map_err(|error| {
        project_error_to_contract(ProjectServiceError::new(
            ProjectErrorCode::InvalidRequest,
            operation,
            format!("command 请求与当前操作不匹配：{error}"),
        ))
    })?;
    let value = serde_json::to_value(contract).map_err(|error| {
        internal_contract_error(operation, format!("序列化已校验 command 请求失败：{error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        internal_contract_error(operation, format!("读取已校验 command 请求失败：{error}"))
    })
}

fn encode_response<TData, TContract>(
    response: TData,
    operation: ProjectOperation,
) -> Result<TContract, ProjectCommandError>
where
    TData: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response).map_err(|error| {
        internal_contract_error(operation, format!("序列化 command 响应失败：{error}"))
    })?;
    validate_project_command_message(&value).map_err(|error| {
        internal_contract_error(
            operation,
            format!("核心响应不满足 project-command v1：{error}"),
        )
    })?;
    serde_json::from_value(value).map_err(|error| {
        internal_contract_error(operation, format!("构造 command 响应失败：{error}"))
    })
}

fn project_error_to_contract(error: ProjectServiceError) -> ProjectCommandError {
    let mut value = Map::from_iter([
        (
            "apiVersion".to_owned(),
            Value::String(narracut_core::PROJECT_COMMAND_API_VERSION.to_owned()),
        ),
        (
            "code".to_owned(),
            Value::String(error.code.as_str().to_owned()),
        ),
        ("message".to_owned(), Value::String(error.message)),
        (
            "operation".to_owned(),
            Value::String(error.operation.as_str().to_owned()),
        ),
    ]);
    if let Some(path) = error.path {
        value.insert("path".to_owned(), Value::String(path));
    }
    if let Some(version) = error.expected_version {
        value.insert("expectedVersion".to_owned(), json!(version));
    }
    if let Some(version) = error.detected_version {
        value.insert("detectedVersion".to_owned(), json!(version));
    }
    let value = Value::Object(value);
    debug_assert!(validate_project_command_message(&value).is_ok());
    serde_json::from_value(value).expect("ProjectServiceError mapping follows command schema")
}

fn internal_contract_error(
    operation: ProjectOperation,
    message: impl Into<String>,
) -> ProjectCommandError {
    project_error_to_contract(ProjectServiceError::new(
        ProjectErrorCode::InternalContractError,
        operation,
        message,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectPathDto {
    project_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectDto {
    parent_path: String,
    directory_name: String,
    name: String,
    workflow_definition_id: String,
    default_locale: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateProjectDto {
    project_path: String,
    expected_source_format_version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameProjectDto {
    project_path: String,
    new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyProjectDto {
    source_project_path: String,
    destination_parent_path: String,
    directory_name: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProjectArchivedDto {
    project_path: String,
    archived: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveProjectToTrashDto {
    project_path: String,
    expected_project_id: String,
}

#[cfg(test)]
mod tests {
    use super::{decode_request, project_error_to_contract, CreateProjectDto, MigrateProjectDto};
    use narracut_contracts::{
        validate_project_command_message, CreateProjectRequest, MigrateProjectRequest,
        ProjectCommandError,
    };
    use narracut_core::{ProjectErrorCode, ProjectOperation, ProjectServiceError};

    #[test]
    fn generated_request_decodes_only_after_schema_validation() {
        let request = serde_json::json!({
            "apiVersion": "1.0.0",
            "command": "create_project",
            "parentPath": "C:/Videos",
            "directoryName": "demo",
            "name": "演示项目",
            "workflowDefinitionId": "workflow_standard_v1"
        });
        let decoded: CreateProjectDto =
            decode_request::<CreateProjectRequest, _>(request, ProjectOperation::Create)
                .expect("decode request");
        assert_eq!(decoded.directory_name, "demo");
    }

    #[test]
    fn malformed_raw_requests_return_structured_invalid_request_errors() {
        for request in [
            serde_json::json!({
                "apiVersion": "2.0.0",
                "command": "create_project",
                "parentPath": "C:/Videos",
                "directoryName": "demo",
                "name": "演示项目",
                "workflowDefinitionId": "workflow_standard_v1"
            }),
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "open_project",
                "projectPath": "C:/Videos/demo"
            }),
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "create_project",
                "parentPath": "C:/Videos",
                "directoryName": 42,
                "name": "演示项目",
                "workflowDefinitionId": "workflow_standard_v1"
            }),
        ] {
            let error = decode_request::<CreateProjectRequest, CreateProjectDto>(
                request,
                ProjectOperation::Create,
            )
            .expect_err("malformed raw request must fail inside the command boundary");
            let value = serde_json::to_value(error).expect("serialize command error");
            assert_eq!(value["code"], "invalid_request");
            assert_eq!(value["operation"], "create");
            validate_project_command_message(&value).expect("structured error follows schema");
        }

        let oversized_version = decode_request::<MigrateProjectRequest, MigrateProjectDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "command": "migrate_project",
                "projectPath": "C:/Videos/legacy",
                "expectedSourceFormatVersion": 4294967296_u64
            }),
            ProjectOperation::Migrate,
        )
        .expect_err("version wider than the core u32 boundary must fail schema validation");
        let value = serde_json::to_value(oversized_version).expect("serialize command error");
        assert_eq!(value["code"], "invalid_request");
        assert_eq!(value["operation"], "migrate");
        validate_project_command_message(&value).expect("structured error follows schema");
    }

    #[test]
    fn service_errors_are_project_command_errors() {
        let error: ProjectCommandError = project_error_to_contract(
            ProjectServiceError::new(
                ProjectErrorCode::MigrationRequired,
                ProjectOperation::Open,
                "需要迁移",
            )
            .with_versions(1, 0),
        );
        let value = serde_json::to_value(error).expect("serialize error");
        validate_project_command_message(&value).expect("error contract");
        assert_eq!(value["code"], "migration_required");
    }
}
