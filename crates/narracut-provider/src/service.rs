use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use narracut_contracts::validate_provider_message;

use crate::{
    CredentialStore, ProviderCapabilityData, ProviderCatalogData, ProviderCredentialMutationData,
    ProviderCredentialStatusData, ProviderError, ProviderErrorCode, ProviderExecutionData,
    ProviderOperation, SecretString, StructuredProviderRequestData, PROVIDER_API_VERSION,
};

#[async_trait]
pub trait AiProvider: Send + Sync {
    fn capability(&self) -> ProviderCapabilityData;

    async fn execute(
        &self,
        request: &StructuredProviderRequestData,
        credential: &SecretString,
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

    pub fn credential_status(
        &self,
        provider_id: &str,
    ) -> Result<ProviderCredentialStatusData, ProviderError> {
        self.require_provider(provider_id, ProviderOperation::GetProviderCredentialStatus)?;
        let result = ProviderCredentialStatusData {
            api_version: PROVIDER_API_VERSION.to_owned(),
            message_type: "provider_credential_status".to_owned(),
            provider_id: provider_id.to_owned(),
            configured: self.credentials.get(provider_id)?.is_some(),
            storage: "system_keyring".to_owned(),
        };
        validate_response(&result, ProviderOperation::GetProviderCredentialStatus)?;
        Ok(result)
    }

    pub fn set_credential(
        &self,
        provider_id: &str,
        secret: SecretString,
    ) -> Result<ProviderCredentialMutationData, ProviderError> {
        self.require_provider(provider_id, ProviderOperation::SetProviderCredential)?;
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
        self.require_provider(provider_id, ProviderOperation::DeleteProviderCredential)?;
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
        let model_supported = capability.models.iter().any(|model| {
            model.model_id == request.model
                && model.structured_outputs
                && model
                    .supported_tasks
                    .iter()
                    .any(|task| task == &request.task)
                && request.config.max_output_tokens <= model.max_output_tokens
        });
        if !model_supported {
            return Err(ProviderError::new(
                ProviderErrorCode::InvalidRequest,
                ProviderOperation::ExecuteProviderRequest,
                "所选模型不支持当前结构化任务或输出上限。",
                false,
            )
            .for_provider(&request.provider_id));
        }
        let credential = self.credentials.get(&request.provider_id)?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::CredentialMissing,
                ProviderOperation::ExecuteProviderRequest,
                "尚未在系统凭据库中配置该 Provider 的 API Key。",
                false,
            )
            .for_provider(&request.provider_id)
        })?;
        let execution = provider.execute(request, &credential).await?;
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
            _credential: &SecretString,
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
            .execute(&request)
            .await
            .expect_err("credential missing");
        assert_eq!(error.code.as_str(), "credential_missing");
        assert!(!error.to_string().contains("secret"));
    }
}
