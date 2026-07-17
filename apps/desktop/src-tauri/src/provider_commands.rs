#![allow(clippy::result_large_err)]

use narracut_contracts::{
    validate_provider_message, DeleteProviderCredentialRequest, GetProviderCatalogRequest,
    GetProviderCredentialStatusRequest, ProviderCatalogResult, ProviderCommandError,
    ProviderCredentialMutationResult, ProviderCredentialStatus, ScriptStageEnqueueRequest,
    ScriptStageEnqueueResult, SetProviderCredentialRequest,
};
use narracut_provider::{
    ProviderError, ProviderErrorCode, ProviderOperation, SecretString, PROVIDER_API_VERSION,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::State;

use crate::provider_runtime::{ProviderRuntime, ScriptEnqueueOptions};

#[tauri::command]
pub async fn get_provider_catalog(
    state: State<'_, ProviderRuntime>,
    request: Value,
) -> Result<ProviderCatalogResult, ProviderCommandError> {
    decode_request::<GetProviderCatalogRequest, GetCatalogDto>(
        request,
        ProviderOperation::GetProviderCatalog,
    )?;
    encode_response(
        state.provider().catalog().map_err(error_to_contract)?,
        ProviderOperation::GetProviderCatalog,
    )
}

#[tauri::command]
pub async fn get_provider_credential_status(
    state: State<'_, ProviderRuntime>,
    request: Value,
) -> Result<ProviderCredentialStatus, ProviderCommandError> {
    let request: ProviderIdDto = decode_request::<GetProviderCredentialStatusRequest, _>(
        request,
        ProviderOperation::GetProviderCredentialStatus,
    )?;
    encode_response(
        state
            .provider()
            .credential_status(&request.provider_id)
            .map_err(error_to_contract)?,
        ProviderOperation::GetProviderCredentialStatus,
    )
}

#[tauri::command]
pub async fn set_provider_credential(
    state: State<'_, ProviderRuntime>,
    request: Value,
) -> Result<ProviderCredentialMutationResult, ProviderCommandError> {
    let request: SetCredentialDto = decode_sensitive_request::<SetProviderCredentialRequest, _>(
        request,
        ProviderOperation::SetProviderCredential,
    )?;
    encode_response(
        state
            .provider()
            .set_credential(&request.provider_id, SecretString::new(request.secret))
            .map_err(error_to_contract)?,
        ProviderOperation::SetProviderCredential,
    )
}

#[tauri::command]
pub async fn delete_provider_credential(
    state: State<'_, ProviderRuntime>,
    request: Value,
) -> Result<ProviderCredentialMutationResult, ProviderCommandError> {
    let request: ProviderIdDto = decode_request::<DeleteProviderCredentialRequest, _>(
        request,
        ProviderOperation::DeleteProviderCredential,
    )?;
    encode_response(
        state
            .provider()
            .delete_credential(&request.provider_id)
            .map_err(error_to_contract)?,
        ProviderOperation::DeleteProviderCredential,
    )
}

#[tauri::command]
pub async fn enqueue_script_stage(
    state: State<'_, ProviderRuntime>,
    request: Value,
) -> Result<ScriptStageEnqueueResult, ProviderCommandError> {
    let request: ScriptEnqueueDto = decode_request::<ScriptStageEnqueueRequest, _>(
        request,
        ProviderOperation::EnqueueScriptStage,
    )?;
    let runtime = state.inner().clone();
    let outcome = tauri::async_runtime::spawn_blocking({
        let runtime = runtime.clone();
        let request = request.clone();
        move || {
            runtime.enqueue_script_stage(ScriptEnqueueOptions {
                project_path: request.project_path,
                expected_project_id: request.expected_project_id,
                provider_id: request.provider_id,
                model: request.model,
                run_id: request.run_id,
                idempotency_key: request.idempotency_key,
                language: request.language,
                max_output_tokens: request.max_output_tokens,
            })
        }
    })
    .await
    .map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::Internal,
            ProviderOperation::EnqueueScriptStage,
            format!("脚本入队后台操作异常终止：{error}"),
            false,
        ))
    })?
    .map_err(error_to_contract)?;
    if !outcome.status.is_terminal() {
        runtime.schedule(
            request.project_path,
            outcome.owner_project_id.clone(),
            outcome.job_id.clone(),
        );
    }
    encode_response(
        json!({
            "apiVersion": PROVIDER_API_VERSION,
            "messageType": "script_stage_enqueue_result",
            "ownerProjectId": outcome.owner_project_id,
            "providerRequestId": outcome.provider_request_id,
            "jobId": outcome.job_id,
            "runId": outcome.run_id,
            "status": outcome.status.as_str(),
        }),
        ProviderOperation::EnqueueScriptStage,
    )
}

fn decode_request<TContract, TDto>(
    request: Value,
    operation: ProviderOperation,
) -> Result<TDto, ProviderCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    decode_request_inner::<TContract, TDto>(request, operation, false)
}

fn decode_sensitive_request<TContract, TDto>(
    request: Value,
    operation: ProviderOperation,
) -> Result<TDto, ProviderCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    decode_request_inner::<TContract, TDto>(request, operation, true)
}

fn decode_request_inner<TContract, TDto>(
    request: Value,
    operation: ProviderOperation,
    redact_diagnostics: bool,
) -> Result<TDto, ProviderCommandError>
where
    TContract: DeserializeOwned + Serialize,
    TDto: DeserializeOwned,
{
    validate_provider_message(&request).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::InvalidRequest,
            operation,
            if redact_diagnostics {
                "敏感凭据请求未通过 provider v1 校验。".to_owned()
            } else {
                format!("请求未通过 provider v1：{error}")
            },
            false,
        ))
    })?;
    let generated: TContract = serde_json::from_value(request).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::InvalidRequest,
            operation,
            if redact_diagnostics {
                "敏感凭据请求无法解析为生成契约。".to_owned()
            } else {
                format!("请求无法解析为生成契约：{error}")
            },
            false,
        ))
    })?;
    let value = serde_json::to_value(generated).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("生成请求类型无法重新序列化：{error}"),
            false,
        ))
    })?;
    serde_json::from_value(value).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::InvalidRequest,
            operation,
            if redact_diagnostics {
                "敏感凭据请求无法转换为 Provider 输入。".to_owned()
            } else {
                format!("请求字段无法转换为 Provider 输入：{error}")
            },
            false,
        ))
    })
}

fn encode_response<TInternal, TContract>(
    response: TInternal,
    operation: ProviderOperation,
) -> Result<TContract, ProviderCommandError>
where
    TInternal: Serialize,
    TContract: DeserializeOwned,
{
    let value = serde_json::to_value(response).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("Provider 响应无法序列化：{error}"),
            false,
        ))
    })?;
    validate_provider_message(&value).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("Provider 响应违反 v1 契约：{error}"),
            false,
        ))
    })?;
    serde_json::from_value(value).map_err(|error| {
        error_to_contract(ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("Provider 响应无法转换为生成类型：{error}"),
            false,
        ))
    })
}

fn error_to_contract(error: ProviderError) -> ProviderCommandError {
    let operation = error.operation;
    let mut value = Map::from_iter([
        (
            "apiVersion".to_owned(),
            Value::String(PROVIDER_API_VERSION.to_owned()),
        ),
        (
            "messageType".to_owned(),
            Value::String("provider_command_error".to_owned()),
        ),
        (
            "operation".to_owned(),
            Value::String(operation.as_str().to_owned()),
        ),
        (
            "code".to_owned(),
            Value::String(error.code.as_str().to_owned()),
        ),
        ("message".to_owned(), Value::String(error.message)),
        ("retryable".to_owned(), Value::Bool(error.retryable)),
    ]);
    if let Some(provider_id) = error.provider_id {
        value.insert("providerId".to_owned(), Value::String(provider_id));
    }
    let value = Value::Object(value);
    validate_provider_message(&value).unwrap_or_else(|schema_error| {
        panic!(
            "provider error adapter produced invalid schema for {}: {schema_error}; value={value}",
            operation.as_str()
        )
    });
    serde_json::from_value(value).unwrap_or_else(|deserialize_error| {
        panic!(
            "provider error adapter failed for {}: {deserialize_error}",
            operation.as_str()
        )
    })
}

#[derive(Debug, Deserialize)]
struct GetCatalogDto {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderIdDto {
    provider_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetCredentialDto {
    provider_id: String,
    secret: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptEnqueueDto {
    project_path: String,
    expected_project_id: String,
    provider_id: String,
    model: String,
    run_id: String,
    idempotency_key: String,
    language: String,
    max_output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::{
        decode_request, decode_sensitive_request, error_to_contract, GetCatalogDto,
        SetCredentialDto,
    };
    use narracut_contracts::{
        validate_provider_message, GetProviderCatalogRequest, SetProviderCredentialRequest,
        NARRACUT_PROVIDER_API_VERSION,
    };
    use narracut_provider::{ProviderError, ProviderErrorCode, ProviderOperation};

    #[test]
    fn raw_provider_requests_are_schema_checked() {
        let error = decode_request::<GetProviderCatalogRequest, GetCatalogDto>(
            serde_json::json!({
                "apiVersion": "1.0.0",
                "messageType": "get_provider_catalog_request",
                "endpoint": "https://attacker.invalid"
            }),
            ProviderOperation::GetProviderCatalog,
        )
        .expect_err("arbitrary endpoint must be rejected");
        let value = serde_json::to_value(error).expect("serialize error");
        assert_eq!(value["code"], "invalid_request");
        validate_provider_message(&value).expect("error follows provider schema");
    }

    #[test]
    fn provider_errors_never_need_a_secret_field() {
        let error = error_to_contract(
            ProviderError::new(
                ProviderErrorCode::CredentialMissing,
                ProviderOperation::ExecuteProviderRequest,
                "未配置凭据",
                false,
            )
            .for_provider("openai_api"),
        );
        let value = serde_json::to_value(error).expect("serialize error");
        assert!(value.get("secret").is_none());
        validate_provider_message(&value).expect("error follows provider schema");
    }

    #[test]
    fn invalid_sensitive_requests_never_echo_the_secret_in_schema_diagnostics() {
        let secret = "must-never-appear";
        let error = decode_sensitive_request::<SetProviderCredentialRequest, SetCredentialDto>(
            serde_json::json!({
                "apiVersion": NARRACUT_PROVIDER_API_VERSION,
                "messageType": "set_provider_credential_request",
                "providerId": "openai_api",
                "secret": secret,
            }),
            ProviderOperation::SetProviderCredential,
        )
        .expect_err("short secret must fail schema validation");
        let value = serde_json::to_value(error).expect("serialize error");
        assert!(!value.to_string().contains(secret));
        validate_provider_message(&value).expect("error follows provider schema");
    }
}
