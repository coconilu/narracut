use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use narracut_contracts::validate_provider_message;

use crate::{
    CredentialStore, ProviderCancellation, ProviderCapabilityData, ProviderCatalogData,
    ProviderCredentialMutationData, ProviderCredentialStatusData, ProviderError, ProviderErrorCode,
    ProviderExecutionData, ProviderExecutionIdentityData, ProviderExecutorBindingData,
    ProviderOperation, SecretString, StructuredProviderRequestData, PROVIDER_API_VERSION,
};

#[async_trait]
pub trait AiProvider: Send + Sync {
    fn capability(&self) -> ProviderCapabilityData;

    fn local_status(&self) -> Result<Option<ProviderCredentialStatusData>, ProviderError> {
        Ok(None)
    }

    fn execution_identity(&self) -> Result<Option<ProviderExecutionIdentityData>, ProviderError> {
        Ok(None)
    }

    fn adapter_version(&self) -> Option<&'static str> {
        None
    }

    async fn execute(
        &self,
        request: &StructuredProviderRequestData,
        credential: Option<&SecretString>,
        cancellation: ProviderCancellation,
    ) -> Result<ProviderExecutionData, ProviderError>;
}

#[derive(Clone)]
pub struct ProviderService {
    credentials: Arc<dyn CredentialStore>,
    providers: Arc<BTreeMap<String, Arc<dyn AiProvider>>>,
}

impl ProviderService {
    pub fn new(
        credentials: Arc<dyn CredentialStore>,
        providers: impl IntoIterator<Item = Arc<dyn AiProvider>>,
    ) -> Result<Self, ProviderError> {
        let mut catalog = BTreeMap::new();
        for provider in providers {
            let provider_id = provider.capability().provider_id;
            if catalog.insert(provider_id.clone(), provider).is_some() {
                return Err(ProviderError::new(
                    ProviderErrorCode::InvalidRequest,
                    ProviderOperation::GetProviderCatalog,
                    format!("Provider 标识重复：{provider_id}"),
                    false,
                ));
            }
        }
        if catalog.is_empty() {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::GetProviderCatalog,
                "至少需要注册一个 AI Provider。",
                false,
            ));
        }
        Ok(Self {
            credentials,
            providers: Arc::new(catalog),
        })
    }

    pub fn catalog(&self) -> Result<ProviderCatalogData, ProviderError> {
        let result = ProviderCatalogData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_catalog_result".to_owned(),
            providers: self
                .providers
                .values()
                .map(|provider| provider.capability())
                .collect(),
        };
        validate_response(&result, ProviderOperation::GetProviderCatalog)?;
        Ok(result)
    }

    pub fn executor_binding(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<ProviderExecutorBindingData, ProviderError> {
        let provider = self.require_provider(provider_id, ProviderOperation::EnqueueScriptStage)?;
        let capability = provider.capability();
        if !model_supports_script(&capability, model, None) {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::EnqueueScriptStage,
                "所选 Provider/模型不支持结构化脚本任务。",
                false,
            )
            .for_provider(provider_id));
        }
        let execution_identity = provider.execution_identity()?;
        let execution_mode = execution_mode(&capability)?;
        let provider_version = match &execution_identity {
            Some(identity) => encode_execution_identity(identity)?,
            None => PROVIDER_API_VERSION.to_owned(),
        };
        Ok(ProviderExecutorBindingData {
            provider_id: provider_id.to_owned(),
            provider_version,
            execution_mode: execution_mode.to_owned(),
            model: model.to_owned(),
            execution_identity,
        })
    }

    pub fn supports_executor(
        &self,
        provider_id: &str,
        provider_version: &str,
        execution_mode_value: &str,
        model: &str,
    ) -> bool {
        let Ok(provider) =
            self.require_provider(provider_id, ProviderOperation::ExecuteProviderRequest)
        else {
            return false;
        };
        let capability = provider.capability();
        if execution_mode(&capability).ok() != Some(execution_mode_value)
            || !model_supports_script(&capability, model, None)
        {
            return false;
        }
        match provider.adapter_version() {
            Some(adapter_version) => decode_execution_identity(provider_version)
                .is_ok_and(|identity| identity.adapter_version == adapter_version),
            None => provider_version == PROVIDER_API_VERSION,
        }
    }

    pub fn frozen_execution_identity(
        &self,
        provider_id: &str,
        provider_version: &str,
    ) -> Result<Option<ProviderExecutionIdentityData>, ProviderError> {
        let provider =
            self.require_provider(provider_id, ProviderOperation::ExecuteProviderRequest)?;
        match provider.adapter_version() {
            Some(adapter_version) => {
                let identity = decode_execution_identity(provider_version)?;
                if identity.adapter_version != adapter_version {
                    return Err(ProviderError::new(
                        ProviderErrorCode::ProviderUnavailable,
                        ProviderOperation::ExecuteProviderRequest,
                        "本地 Provider 的冻结适配器版本不受当前运行时支持。",
                        false,
                    )
                    .for_provider(provider_id));
                }
                Ok(Some(identity))
            }
            None if provider_version == PROVIDER_API_VERSION => Ok(None),
            None => Err(ProviderError::new(
                ProviderErrorCode::ProviderUnavailable,
                ProviderOperation::ExecuteProviderRequest,
                "远程 Provider 的冻结版本与当前运行时不一致。",
                false,
            )
            .for_provider(provider_id)),
        }
    }

    pub fn credential_status(
        &self,
        provider_id: &str,
    ) -> Result<ProviderCredentialStatusData, ProviderError> {
        let provider =
            self.require_provider(provider_id, ProviderOperation::GetProviderCredentialStatus)?;
        let capability = provider.capability();
        let result = match capability.credential_storage.as_str() {
            "system_keyring" => ProviderCredentialStatusData {
                api_version: PROVIDER_API_VERSION.to_owned(),
                message_type: "provider_credential_status".to_owned(),
                provider_id: provider_id.to_owned(),
                configured: self.credentials.get(provider_id)?.is_some(),
                storage: "system_keyring".to_owned(),
                installed: None,
                logged_in: None,
                version_supported: None,
                cli_version: None,
                diagnostic_code: None,
                diagnostic: None,
            },
            "none" => provider.local_status()?.ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCode::Internal,
                    ProviderOperation::GetProviderCredentialStatus,
                    "本地 Provider 未提供可用性诊断。",
                    false,
                )
                .for_provider(provider_id)
            })?,
            _ => {
                return Err(ProviderError::new(
                    ProviderErrorCode::Internal,
                    ProviderOperation::GetProviderCredentialStatus,
                    "Provider 声明了未知的凭据策略。",
                    false,
                )
                .for_provider(provider_id));
            }
        };
        if result.provider_id != provider_id || result.storage != capability.credential_storage {
            return Err(ProviderError::new(
                ProviderErrorCode::Internal,
                ProviderOperation::GetProviderCredentialStatus,
                "Provider 可用性诊断身份与能力声明不一致。",
                false,
            )
            .for_provider(provider_id));
        }
        validate_response(&result, ProviderOperation::GetProviderCredentialStatus)?;
        Ok(result)
    }

    pub fn set_credential(
        &self,
        provider_id: &str,
        secret: SecretString,
    ) -> Result<ProviderCredentialMutationData, ProviderError> {
        let provider =
            self.require_provider(provider_id, ProviderOperation::SetProviderCredential)?;
        require_keyring_policy(
            &provider.capability(),
            ProviderOperation::SetProviderCredential,
        )?;
        if !(20..=4096).contains(&secret.expose().len()) {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::SetProviderCredential,
                "API Key 长度必须位于 20..=4096。",
                false,
            )
            .for_provider(provider_id));
        }
        self.credentials.set(provider_id, secret)?;
        let result = ProviderCredentialMutationData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_credential_mutation_result".to_owned(),
            provider_id: provider_id.to_owned(),
            action: "stored".to_owned(),
            configured: true,
            storage: "system_keyring".to_owned(),
        };
        validate_response(&result, ProviderOperation::SetProviderCredential)?;
        Ok(result)
    }

    pub fn delete_credential(
        &self,
        provider_id: &str,
    ) -> Result<ProviderCredentialMutationData, ProviderError> {
        let provider =
            self.require_provider(provider_id, ProviderOperation::DeleteProviderCredential)?;
        require_keyring_policy(
            &provider.capability(),
            ProviderOperation::DeleteProviderCredential,
        )?;
        self.credentials.delete(provider_id)?;
        let result = ProviderCredentialMutationData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_credential_mutation_result".to_owned(),
            provider_id: provider_id.to_owned(),
            action: "deleted".to_owned(),
            configured: false,
            storage: "system_keyring".to_owned(),
        };
        validate_response(&result, ProviderOperation::DeleteProviderCredential)?;
        Ok(result)
    }

    pub async fn execute(
        &self,
        request: &StructuredProviderRequestData,
        cancellation: ProviderCancellation,
    ) -> Result<ProviderExecutionData, ProviderError> {
        let value = serde_json::to_value(request).map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                format!("结构化 Provider 请求无法序列化：{error}"),
                false,
            )
        })?;
        validate_provider_message(&value).map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                format!("结构化 Provider 请求违反 v1 契约：{error}"),
                false,
            )
            .for_provider(&request.provider_id)
        })?;
        let provider = self.require_provider(
            &request.provider_id,
            ProviderOperation::ExecuteProviderRequest,
        )?;
        let capability = provider.capability();
        let model_supported = model_supports_script(
            &capability,
            &request.model,
            Some(request.config.max_output_tokens),
        );
        if !model_supported {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                "所选模型不支持当前结构化任务或输出上限。",
                false,
            )
            .for_provider(&request.provider_id));
        }
        let credential = match capability.credential_storage.as_str() {
            "system_keyring" => {
                Some(self.credentials.get(&request.provider_id)?.ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorCode::CredentialMissing,
                        ProviderOperation::ExecuteProviderRequest,
                        "尚未在系统凭据库中配置该 Provider 的 API Key。",
                        false,
                    )
                    .for_provider(&request.provider_id)
                })?)
            }
            "none" => None,
            _ => {
                return Err(ProviderError::new(
                    ProviderErrorCode::Internal,
                    ProviderOperation::ExecuteProviderRequest,
                    "Provider 声明了未知的凭据策略。",
                    false,
                )
                .for_provider(&request.provider_id));
            }
        };
        if capability.transport == "local_cli" && request.execution_identity.is_none() {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                "本地 CLI 请求缺少冻结执行身份。",
                false,
            )
            .for_provider(&request.provider_id));
        }
        if capability.transport != "local_cli" && request.execution_identity.is_some() {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                "远程 API 请求不得携带本地 CLI 执行身份。",
                false,
            )
            .for_provider(&request.provider_id));
        }
        let execution = provider
            .execute(request, credential.as_ref(), cancellation)
            .await?;
        let result = &execution.result;
        if result.provider_request_id != request.provider_request_id
            || result.provider_id != request.provider_id
            || result.model != request.model
            || result.status != "completed"
        {
            return Err(ProviderError::new(
                ProviderErrorCode::ProviderResponseInvalid,
                ProviderOperation::ExecuteProviderRequest,
                "Provider 结果身份与结构化请求不一致。",
                false,
            )
            .for_provider(&request.provider_id));
        }
        validate_response(result, ProviderOperation::ExecuteProviderRequest)?;
        Ok(execution)
    }

    fn require_provider(
        &self,
        provider_id: &str,
        operation: ProviderOperation,
    ) -> Result<Arc<dyn AiProvider>, ProviderError> {
        self.providers.get(provider_id).cloned().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                operation,
                format!("不支持的 Provider：{provider_id}"),
                false,
            )
            .for_provider(provider_id)
        })
    }
}

fn require_keyring_policy(
    capability: &ProviderCapabilityData,
    operation: ProviderOperation,
) -> Result<(), ProviderError> {
    if capability.credential_storage == "system_keyring" {
        return Ok(());
    }
    Err(ProviderError::new(
        ProviderErrorCode::CredentialUnsupported,
        operation,
        "该 Provider 使用本机 Codex 登录态，不支持设置或删除 API Key。",
        false,
    )
    .for_provider(&capability.provider_id))
}

fn execution_mode(capability: &ProviderCapabilityData) -> Result<&'static str, ProviderError> {
    match capability.transport.as_str() {
        "remote_api" => Ok("remote_api"),
        "local_cli" => Ok("codex_cli"),
        "local_model" => Ok("local"),
        _ => Err(ProviderError::new(
            ProviderErrorCode::Internal,
            ProviderOperation::ExecuteProviderRequest,
            "Provider 声明了未知 transport。",
            false,
        )
        .for_provider(&capability.provider_id)),
    }
}

fn model_supports_script(
    capability: &ProviderCapabilityData,
    model_id: &str,
    max_output_tokens: Option<u32>,
) -> bool {
    capability.models.iter().any(|model| {
        model.model_id == model_id
            && model.structured_outputs
            && model
                .supported_tasks
                .iter()
                .any(|task| task == "script_generation")
            && max_output_tokens.is_none_or(|limit| limit <= model.max_output_tokens)
    })
}

fn encode_execution_identity(
    identity: &ProviderExecutionIdentityData,
) -> Result<String, ProviderError> {
    validate_identity(identity)?;
    let encoded = format!(
        "{}|cli={}|exe={}",
        identity.adapter_version, identity.cli_version, identity.executable_hash
    );
    if encoded.len() > 255 {
        return Err(identity_error("冻结 CLI 执行身份超过 255 字节上限。"));
    }
    Ok(encoded)
}

fn decode_execution_identity(
    encoded: &str,
) -> Result<ProviderExecutionIdentityData, ProviderError> {
    if encoded.len() > 255 {
        return Err(identity_error("冻结 CLI 执行身份超过 255 字节上限。"));
    }
    let mut parts = encoded.split('|');
    let adapter_version = parts.next().unwrap_or_default();
    let cli_version = parts
        .next()
        .and_then(|part| part.strip_prefix("cli="))
        .unwrap_or_default();
    let executable_hash = parts
        .next()
        .and_then(|part| part.strip_prefix("exe="))
        .unwrap_or_default();
    if parts.next().is_some() {
        return Err(identity_error("冻结 CLI 执行身份包含额外字段。"));
    }
    let identity = ProviderExecutionIdentityData {
        adapter_version: adapter_version.to_owned(),
        cli_version: cli_version.to_owned(),
        executable_hash: executable_hash.to_owned(),
    };
    validate_identity(&identity)?;
    Ok(identity)
}

fn validate_identity(identity: &ProviderExecutionIdentityData) -> Result<(), ProviderError> {
    let adapter_ok = !identity.adapter_version.is_empty()
        && identity.adapter_version.len() <= 80
        && identity
            .adapter_version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'));
    let version_ok = semver::Version::parse(&identity.cli_version).is_ok();
    let hash_ok = identity
        .executable_hash
        .strip_prefix("sha256:")
        .is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
    if adapter_ok && version_ok && hash_ok {
        Ok(())
    } else {
        Err(identity_error("冻结 CLI 执行身份格式无效。"))
    }
}

fn identity_error(message: impl Into<String>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::ProviderUnavailable,
        ProviderOperation::ExecuteProviderRequest,
        message,
        false,
    )
    .for_provider("local_codex")
}

fn validate_response<T: serde::Serialize>(
    response: &T,
    operation: ProviderOperation,
) -> Result<(), ProviderError> {
    let value = serde_json::to_value(response).map_err(|error| {
        ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("Provider 响应无法序列化：{error}"),
            false,
        )
    })?;
    validate_provider_message(&value).map_err(|error| {
        ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("Provider 响应违反 v1 契约：{error}"),
            false,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::{AiProvider, ProviderService};
    use crate::{
        InMemoryCredentialStore, ProviderCapabilityData, ProviderError, ProviderExecutionData,
        ProviderModelCapabilityData, SecretString, StructuredProviderRequestData,
    };

    struct NeverCalledProvider;

    #[async_trait]
    impl AiProvider for NeverCalledProvider {
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
            _request: &StructuredProviderRequestData,
            _credential: Option<&SecretString>,
            _cancellation: crate::ProviderCancellation,
        ) -> Result<ProviderExecutionData, ProviderError> {
            panic!("credential gate should stop execution")
        }
    }

    #[tokio::test]
    async fn execution_requires_a_system_credential_without_exposing_it() {
        let service = ProviderService::new(
            Arc::new(InMemoryCredentialStore::default()),
            [Arc::new(NeverCalledProvider) as Arc<dyn AiProvider>],
        )
        .expect("service");
        let fixture = serde_json::from_str::<Vec<serde_json::Value>>(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("fixture");
        let request = fixture
            .into_iter()
            .find(|value| value["messageType"] == "provider_request")
            .expect("request fixture");
        let request = serde_json::from_value(request).expect("internal request DTO");
        let error = service
            .execute(&request, crate::ProviderCancellation::default())
            .await
            .expect_err("credential missing");
        assert_eq!(error.code.as_str(), "credential_missing");
        assert!(!error.to_string().contains("secret"));
    }
}
