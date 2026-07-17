#![forbid(unsafe_code)]

//! NarraCut 统一 AI Provider 边界。
//!
//! 远程 API、本地 CLI 与本地模型都必须实现此处的有界接口；调用方不能传入
//! 任意 shell 或自由形态的 Provider 参数。

mod credential;
mod error;
mod openai;
mod service;
mod types;

pub use credential::{
    CredentialStore, InMemoryCredentialStore, SecretString, SystemCredentialStore,
};
pub use error::{ProviderError, ProviderErrorCode, ProviderOperation};
pub use openai::{HttpResponseData, OpenAiProvider, ProviderHttpTransport, ReqwestTransport};
pub use service::{AiProvider, ProviderService};
pub use types::{
    ProviderCapabilityData, ProviderCatalogData, ProviderCredentialMutationData,
    ProviderCredentialStatusData, ProviderExecutionData, ProviderInputArtifactData,
    ProviderModelCapabilityData, ProviderUsageData, ScriptGenerationConfigData, ScriptSegmentData,
    StructuredProviderRequestData, StructuredProviderResultData, StructuredScriptOutputData,
};

pub const PROVIDER_API_VERSION: &str = "1.0.0";
