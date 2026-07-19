#![forbid(unsafe_code)]

//! 由 NarraCut v1 JSON Schema 生成的 Rust 契约类型。
//!
//! Schema 是唯一权威来源；不要在本 crate 中手工复制 TypeScript 类型。

use std::{error::Error, fmt, sync::OnceLock};

use serde_json::Value;

pub const NARRACUT_CONTRACT_VERSION: &str = "1.0.0";
pub const NARRACUT_PROJECT_COMMAND_API_VERSION: &str = "1.0.0";
pub const NARRACUT_STORAGE_COMMAND_API_VERSION: &str = "1.0.0";
pub const NARRACUT_WORKFLOW_COMMAND_API_VERSION: &str = "1.0.0";
pub const NARRACUT_JOB_COMMAND_API_VERSION: &str = "1.0.0";
pub const NARRACUT_MEDIA_SCHEMA_VERSION: &str = "1.2.0";
pub const NARRACUT_MEDIA_COMMAND_API_VERSION: &str = "1.1.0";
pub const NARRACUT_PROVIDER_API_VERSION: &str = "1.0.0";
pub const NARRACUT_RENDERER_API_VERSION: &str = "1.0.0";
pub const NARRACUT_EXPORT_API_VERSION: &str = "1.0.0";
pub const NARRACUT_EXPORT_MANIFEST_VERSION: &str = "1.0.0";

typify::import_types!(schema = "../../packages/contracts/schema/narracut-contracts-v1.schema.json");
mod project_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-project-commands-v1.schema.json"
    );
}
pub use project_command_types::*;
mod storage_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-storage-commands-v1.schema.json"
    );
}
pub use storage_command_types::*;
pub mod workflow_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-workflow-commands-v1.schema.json"
    );
}
pub use workflow_command_types::{
    GetWorkflowRequest, InitializeWorkflowRequest, ListStageHistoryRequest,
    NarraCutWorkflowCommandMessage, PrepareStageRunRequest, PreviewRegenerationRequest,
    RecordStageRunRequest, RegenerationImpactResult, ReviewStageRunRequest,
    StageConfigUpdateResult, StageHistoryResult, StageReviewResult, StageRunCommitResult,
    StageRunPreparationResult, UpdateStageConfigRequest, WorkflowCommandError, WorkflowSnapshot,
};
pub mod job_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-job-commands-v1.schema.json"
    );
}
pub use job_command_types::{
    CancelJobRequest, EnqueueStageJobRequest, GetJobRequest, JobCommandError, JobEventsResult,
    JobListResult, JobRecoveryResult, JobSnapshot, ListJobEventsRequest, ListJobsRequest,
    NarraCutJobCommandMessage, RecoverJobsRequest, RetryStageJobRequest,
};
pub mod media_types {
    typify::import_types!(schema = "../../packages/contracts/schema/narracut-media-v1.schema.json");
}
pub use media_types::NarraCutMediaDocument;
pub mod media_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-media-commands-v1.schema.json"
    );
}
pub use media_command_types::NarraCutMediaCommandMessage;
pub mod provider_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-provider-v1.schema.json"
    );
}
pub use provider_types::{
    DeleteProviderCredentialRequest, GetProviderCatalogRequest, GetProviderCredentialStatusRequest,
    NarraCutProviderMessage, ProviderCapability, ProviderCatalogResult, ProviderCommandError,
    ProviderCredentialMutationResult, ProviderCredentialStatus, ProviderEvent,
    ProviderInputArtifact, ProviderModelCapability, ProviderUsage, ScriptGenerationConfig,
    ScriptSegment, ScriptStageEnqueueRequest, ScriptStageEnqueueResult,
    SetProviderCredentialRequest, StructuredProviderRequest, StructuredProviderResult,
    StructuredScriptOutput,
};
pub mod renderer_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-renderer-v1.schema.json"
    );
}
pub use renderer_types::{
    ArtifactManifestEntry as RenderArtifactManifestEntry, CreateSceneSnapshotRequest,
    Diagnostic as RendererDiagnostic, EnqueueSceneRenderRequest, EnqueueTimelineRenderRequest,
    GetRenderResultRequest, NarraCutRendererMessage, ProbeRendererRequest,
    RenderConfig as RendererConfig, RenderEvent, RenderJobAcceptedResult, RenderResult,
    RendererCapabilitiesResult, RendererCommandError, RendererIdentity, RendererLimits,
    RendererOperation, SceneSnapshot, SceneSnapshotResult,
    TimelineInputReference as RendererTimelineInputReference,
};
pub mod export_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-export-v1.schema.json"
    );
}
pub use export_types::{
    AdoptedArtifact as ExportAdoptedArtifact, EnqueueExportRequest, ExportCommandError,
    ExportJobAcceptedResult, ExportManifest, ExportQaResult, ExportResult,
    ExportVerificationResult, GetExportResultRequest, LicenseRecord as ExportLicenseRecord,
    ManifestFile as ExportManifestFile, MediaInfo as ExportMediaInfo, NarraCutExportMessage,
    ProvenanceReference as ExportProvenanceReference, QaCheck as ExportQaCheck,
    QaDiagnostic as ExportQaDiagnostic, QaSummary as ExportQaSummary,
    RenderInput as ExportRenderInputReference, RendererIdentity as ExportRendererIdentity,
    RunExportQaRequest, VerifyExportRequest,
};

static CONTRACT_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static PROJECT_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static STORAGE_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static WORKFLOW_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static JOB_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static MEDIA_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static MEDIA_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static PROVIDER_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static RENDERER_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static EXPORT_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();

/// JSON 文档违反 NarraCut 权威 Schema 时返回的全部诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractValidationError {
    pub errors: Vec<String>,
}

impl fmt::Display for ContractValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "contract validation failed: {}",
            self.errors.join("; ")
        )
    }
}

impl Error for ContractValidationError {}

/// Schema 校验或类型反序列化失败。
#[derive(Debug)]
pub enum ContractParseError {
    Validation(ContractValidationError),
    Deserialize(serde_json::Error),
}

impl fmt::Display for ContractParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::Deserialize(error) => {
                write!(formatter, "contract deserialization failed: {error}")
            }
        }
    }
}

impl Error for ContractParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Deserialize(error) => Some(error),
        }
    }
}

/// 使用 Draft 2020-12 权威 Schema 校验一个持久化契约文档。
pub fn validate_contract_document(document: &Value) -> Result<(), ContractValidationError> {
    let errors = contract_validator()
        .iter_errors(document)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 Typify 生成的 Rust 类型。
pub fn parse_contract_document(
    document: Value,
) -> Result<NarraCutContractDocument, ContractParseError> {
    validate_contract_document(&document).map_err(ContractParseError::Validation)?;
    serde_json::from_value(document).map_err(ContractParseError::Deserialize)
}

/// 使用 project-command v1 Schema 校验一条 Tauri command 请求、响应或错误消息。
pub fn validate_project_command_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = project_command_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 project-command v1 判别联合。
pub fn parse_project_command_message(
    message: Value,
) -> Result<NarraCutProjectCommandMessage, ContractParseError> {
    validate_project_command_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 storage-command v1 Schema 校验 Artifact Store、SQLite 索引或缓存命令消息。
pub fn validate_storage_command_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = storage_command_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 storage-command v1 判别联合。
pub fn parse_storage_command_message(
    message: Value,
) -> Result<NarraCutStorageCommandMessage, ContractParseError> {
    validate_storage_command_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 workflow-command v1 Schema 校验阶段图、运行、审核与 stale 传播消息。
pub fn validate_workflow_command_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = workflow_command_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 workflow-command v1 判别联合。
pub fn parse_workflow_command_message(
    message: Value,
) -> Result<NarraCutWorkflowCommandMessage, ContractParseError> {
    validate_workflow_command_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 job-command v1 Schema 校验持久化任务队列的有界桌面命令消息。
pub fn validate_job_command_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = job_command_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 job-command v1 判别联合。
pub fn parse_job_command_message(
    message: Value,
) -> Result<NarraCutJobCommandMessage, ContractParseError> {
    validate_job_command_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 media v1 Schema 校验审核后的媒体、场景计划或最小时间轴文档。
pub fn validate_media_document(document: &Value) -> Result<(), ContractValidationError> {
    let mut errors = media_validator()
        .iter_errors(document)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if document.get("schemaVersion").and_then(Value::as_str) == Some("1.0.0") {
        match document.get("documentType").and_then(Value::as_str) {
            Some("captions_media")
                if document
                    .get("cues")
                    .and_then(Value::as_array)
                    .is_some_and(|cues| cues.iter().any(|cue| cue.get("provenance").is_some())) =>
            {
                errors.push("media 1.0.0 CaptionCue 不能声明 1.1.0 provenance 字段".to_owned());
            }
            Some("scene_plan") => {
                if document.get("cueTraceability").is_some() {
                    errors.push(
                        "media 1.0.0 ScenePlanDocument 不能声明 1.1.0 cueTraceability 字段"
                            .to_owned(),
                    );
                }
                if document
                    .get("scenes")
                    .and_then(Value::as_array)
                    .is_some_and(|scenes| {
                        scenes.iter().any(|scene| scene.get("provenance").is_some())
                    })
                {
                    errors.push(
                        "media 1.0.0 ScenePlanScene 不能声明 1.1.0 provenance 字段".to_owned(),
                    );
                }
            }
            _ => {}
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 media v1 判别联合。
pub fn parse_media_document(document: Value) -> Result<NarraCutMediaDocument, ContractParseError> {
    validate_media_document(&document).map_err(ContractParseError::Validation)?;
    serde_json::from_value(document).map_err(ContractParseError::Deserialize)
}

/// 使用 media-command v1 Schema 校验高层媒体请求、结果或结构化错误。
pub fn validate_media_command_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = media_command_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 media-command v1 判别联合。
pub fn parse_media_command_message(
    message: Value,
) -> Result<NarraCutMediaCommandMessage, ContractParseError> {
    validate_media_command_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 provider v1 Schema 校验能力、凭据、脚本任务和结构化执行消息。
pub fn validate_provider_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = provider_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 provider v1 判别联合。
pub fn parse_provider_message(
    message: Value,
) -> Result<NarraCutProviderMessage, ContractParseError> {
    validate_provider_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 renderer v1 Schema 校验受限渲染请求、事件与结果。
pub fn validate_renderer_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = renderer_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

/// 先执行完整 Schema 校验，再反序列化为 renderer v1 判别联合。
pub fn parse_renderer_message(
    message: Value,
) -> Result<NarraCutRendererMessage, ContractParseError> {
    validate_renderer_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

/// 使用 export v1 Schema 校验 QA、导出和可迁移 Manifest 消息。
pub fn validate_export_message(message: &Value) -> Result<(), ContractValidationError> {
    let errors = export_validator()
        .iter_errors(message)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ContractValidationError { errors })
    }
}

pub fn parse_export_message(message: Value) -> Result<NarraCutExportMessage, ContractParseError> {
    validate_export_message(&message).map_err(ContractParseError::Validation)?;
    serde_json::from_value(message).map_err(ContractParseError::Deserialize)
}

fn contract_validator() -> &'static jsonschema::Validator {
    CONTRACT_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-contracts-v1.schema.json"
        ))
        .expect("checked-in contract schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in contract schema must compile as JSON Schema 2020-12")
    })
}

fn project_command_validator() -> &'static jsonschema::Validator {
    PROJECT_COMMAND_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-project-commands-v1.schema.json"
        ))
        .expect("checked-in project command schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in project command schema must compile as JSON Schema 2020-12")
    })
}

fn storage_command_validator() -> &'static jsonschema::Validator {
    STORAGE_COMMAND_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-storage-commands-v1.schema.json"
        ))
        .expect("checked-in storage command schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in storage command schema must compile as JSON Schema 2020-12")
    })
}

fn workflow_command_validator() -> &'static jsonschema::Validator {
    WORKFLOW_COMMAND_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-workflow-commands-v1.schema.json"
        ))
        .expect("checked-in workflow command schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in workflow command schema must compile as JSON Schema 2020-12")
    })
}

fn job_command_validator() -> &'static jsonschema::Validator {
    JOB_COMMAND_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-job-commands-v1.schema.json"
        ))
        .expect("checked-in job command schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in job command schema must compile as JSON Schema 2020-12")
    })
}

fn media_validator() -> &'static jsonschema::Validator {
    MEDIA_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-media-v1.schema.json"
        ))
        .expect("checked-in media schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in media schema must compile as JSON Schema 2020-12")
    })
}

fn media_command_validator() -> &'static jsonschema::Validator {
    MEDIA_COMMAND_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-media-commands-v1.schema.json"
        ))
        .expect("checked-in media command schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in media command schema must compile as JSON Schema 2020-12")
    })
}

fn provider_validator() -> &'static jsonschema::Validator {
    PROVIDER_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-provider-v1.schema.json"
        ))
        .expect("checked-in provider schema must be valid JSON");

        jsonschema::validator_for(&schema)
            .expect("checked-in provider schema must compile as JSON Schema 2020-12")
    })
}

fn renderer_validator() -> &'static jsonschema::Validator {
    RENDERER_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-renderer-v1.schema.json"
        ))
        .expect("checked-in renderer schema must be valid JSON");
        jsonschema::validator_for(&schema)
            .expect("checked-in renderer schema must compile as JSON Schema 2020-12")
    })
}

fn export_validator() -> &'static jsonschema::Validator {
    EXPORT_VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../../packages/contracts/schema/narracut-export-v1.schema.json"
        ))
        .expect("checked-in export schema must be valid JSON");
        jsonschema::validator_for(&schema)
            .expect("checked-in export schema must compile as JSON Schema 2020-12")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_contract_document, parse_export_message, parse_job_command_message,
        parse_media_command_message, parse_media_document, parse_project_command_message,
        parse_provider_message, parse_renderer_message, parse_storage_command_message,
        parse_workflow_command_message, validate_contract_document, validate_export_message,
        validate_job_command_message, validate_media_command_message, validate_media_document,
        validate_project_command_message, validate_provider_message, validate_renderer_message,
        validate_storage_command_message, validate_workflow_command_message,
        NARRACUT_CONTRACT_VERSION, NARRACUT_EXPORT_API_VERSION, NARRACUT_JOB_COMMAND_API_VERSION,
        NARRACUT_MEDIA_COMMAND_API_VERSION, NARRACUT_MEDIA_SCHEMA_VERSION,
        NARRACUT_PROJECT_COMMAND_API_VERSION, NARRACUT_PROVIDER_API_VERSION,
        NARRACUT_RENDERER_API_VERSION, NARRACUT_STORAGE_COMMAND_API_VERSION,
        NARRACUT_WORKFLOW_COMMAND_API_VERSION,
    };
    use serde::Deserialize;
    use serde_json::Value;

    #[test]
    fn all_valid_fixtures_deserialize_into_generated_types() {
        let documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-documents.json"
        ))
        .expect("valid fixture file must be JSON");

        assert_eq!(documents.len(), 13);

        for document in documents {
            assert_eq!(
                document.get("schemaVersion").and_then(Value::as_str),
                Some(NARRACUT_CONTRACT_VERSION)
            );

            parse_contract_document(document)
                .expect("fixture must validate and deserialize through generated Rust contracts");
        }
    }

    #[test]
    fn all_invalid_fixtures_are_rejected_before_deserialization() {
        let valid_documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-documents.json"
        ))
        .expect("valid fixture file must be JSON");
        let invalid_cases: Vec<InvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-documents.json"
        ))
        .expect("invalid fixture file must be JSON");

        for test_case in invalid_cases {
            let mut document = valid_documents
                .iter()
                .find(|document| {
                    document.get("documentType").and_then(Value::as_str)
                        == Some(test_case.source_document_type.as_str())
                })
                .unwrap_or_else(|| panic!("missing source fixture for {}", test_case.name))
                .clone();

            for patch in test_case.patches() {
                apply_patch(&mut document, patch);
            }

            assert!(
                validate_contract_document(&document).is_err(),
                "invalid fixture was accepted by Rust validator: {}",
                test_case.name
            );
            assert!(
                parse_contract_document(document).is_err(),
                "invalid fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_project_command_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-project-command-messages.json"
        ))
        .expect("valid project command fixture file must be JSON");

        assert_eq!(messages.len(), 14);

        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_PROJECT_COMMAND_API_VERSION)
            );

            parse_project_command_message(message).expect(
                "fixture must validate and deserialize through generated Rust command contracts",
            );
        }
    }

    #[test]
    fn all_invalid_project_command_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-project-command-messages.json"
        ))
        .expect("valid project command fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-project-command-messages.json"
        ))
        .expect("invalid project command fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 11);

        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing source command fixture for {}", test_case.name))
                .clone();

            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }

            assert!(
                validate_project_command_message(&message).is_err(),
                "invalid project command fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_project_command_message(message).is_err(),
                "invalid project command fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_storage_command_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-storage-command-messages.json"
        ))
        .expect("valid storage command fixture file must be JSON");

        assert_eq!(messages.len(), 16);

        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_STORAGE_COMMAND_API_VERSION)
            );

            parse_storage_command_message(message).expect(
                "fixture must validate and deserialize through generated Rust storage contracts",
            );
        }
    }

    #[test]
    fn all_invalid_storage_command_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-storage-command-messages.json"
        ))
        .expect("valid storage command fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-storage-command-messages.json"
        ))
        .expect("invalid storage command fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 15);

        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing storage command fixture for {}", test_case.name))
                .clone();

            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }

            assert!(
                validate_storage_command_message(&message).is_err(),
                "invalid storage command fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_storage_command_message(message).is_err(),
                "invalid storage command fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_workflow_command_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-workflow-command-messages.json"
        ))
        .expect("valid workflow command fixture file must be JSON");

        assert_eq!(messages.len(), 16);

        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_WORKFLOW_COMMAND_API_VERSION)
            );

            parse_workflow_command_message(message).expect(
                "fixture must validate and deserialize through generated Rust workflow contracts",
            );
        }
    }

    #[test]
    fn all_invalid_workflow_command_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-workflow-command-messages.json"
        ))
        .expect("valid workflow command fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-workflow-command-messages.json"
        ))
        .expect("invalid workflow command fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 18);

        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| {
                    panic!("missing workflow command fixture for {}", test_case.name)
                })
                .clone();

            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }

            assert!(
                validate_workflow_command_message(&message).is_err(),
                "invalid workflow command fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_workflow_command_message(message).is_err(),
                "invalid workflow command fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_job_command_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-job-command-messages.json"
        ))
        .expect("valid job command fixture file must be JSON");

        assert_eq!(messages.len(), 12);

        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_JOB_COMMAND_API_VERSION)
            );
            parse_job_command_message(message).expect(
                "fixture must validate and deserialize through generated Rust job contracts",
            );
        }
    }

    #[test]
    fn all_invalid_job_command_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-job-command-messages.json"
        ))
        .expect("valid job command fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-job-command-messages.json"
        ))
        .expect("invalid job command fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 15);

        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing job command fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }
            assert!(
                validate_job_command_message(&message).is_err(),
                "invalid job command fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_job_command_message(message).is_err(),
                "invalid job command fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_media_documents_deserialize_into_generated_types() {
        let documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-media-documents.json"
        ))
        .expect("valid media fixture file must be JSON");

        assert_eq!(documents.len(), 5);
        for document in documents {
            assert!(
                matches!(
                    document.get("schemaVersion").and_then(Value::as_str),
                    Some("1.1.0") | Some(NARRACUT_MEDIA_SCHEMA_VERSION)
                ),
                "valid fixtures include current 1.2 and frozen legacy 1.1 documents"
            );
            parse_media_document(document)
                .expect("media fixture must validate and deserialize through generated Rust types");
        }
    }

    #[test]
    fn media_1_0_documents_remain_readable_but_cannot_claim_1_1_traceability_fields() {
        let documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-media-documents.json"
        ))
        .expect("valid media fixture file must be JSON");

        for document_type in ["captions_media", "scene_plan"] {
            let mut legacy = documents
                .iter()
                .find(|document| {
                    document.get("documentType").and_then(Value::as_str) == Some(document_type)
                })
                .expect("versioned media fixture")
                .clone();
            legacy["schemaVersion"] = Value::String("1.0.0".to_owned());
            match document_type {
                "captions_media" => {
                    for cue in legacy["cues"].as_array_mut().expect("caption cue fixtures") {
                        cue.as_object_mut()
                            .expect("caption cue object")
                            .remove("provenance");
                    }
                }
                "scene_plan" => {
                    legacy
                        .as_object_mut()
                        .expect("scene plan fixture")
                        .remove("cueTraceability");
                    for scene in legacy["scenes"].as_array_mut().expect("scene fixtures") {
                        scene
                            .as_object_mut()
                            .expect("scene object")
                            .remove("provenance");
                    }
                }
                _ => unreachable!(),
            }
            validate_media_document(&legacy).expect("frozen media 1.0 shape remains readable");
            parse_media_document(legacy).expect("media 1.0 shape has a generated Rust type");

            let mut mislabeled = documents
                .iter()
                .find(|document| {
                    document.get("documentType").and_then(Value::as_str) == Some(document_type)
                })
                .expect("versioned media fixture")
                .clone();
            mislabeled["schemaVersion"] = Value::String("1.0.0".to_owned());
            assert!(
                validate_media_document(&mislabeled).is_err(),
                "media 1.0 cannot claim 1.1 fields for {document_type}"
            );
        }
    }

    #[test]
    fn all_invalid_media_documents_are_rejected() {
        let valid_documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-media-documents.json"
        ))
        .expect("valid media fixture file must be JSON");
        let invalid_cases: Vec<InvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-media-documents.json"
        ))
        .expect("invalid media fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 14);
        for test_case in invalid_cases {
            let mut document = valid_documents
                .iter()
                .find(|document| {
                    document.get("documentType").and_then(Value::as_str)
                        == Some(test_case.source_document_type.as_str())
                })
                .unwrap_or_else(|| panic!("missing media source fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut document, patch);
            }
            assert!(
                validate_media_document(&document).is_err(),
                "invalid media fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_media_document(document).is_err(),
                "invalid media fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_media_command_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-media-command-messages.json"
        ))
        .expect("valid media command fixture file must be JSON");

        assert_eq!(messages.len(), 14);
        for message in messages {
            assert!(
                matches!(
                    message.get("apiVersion").and_then(Value::as_str),
                    Some("1.0.0") | Some(NARRACUT_MEDIA_COMMAND_API_VERSION)
                ),
                "valid fixtures include current 1.1 and frozen legacy 1.0 messages"
            );
            parse_media_command_message(message).expect(
                "media command fixture must validate and deserialize through generated Rust types",
            );
        }
    }

    #[test]
    fn all_invalid_media_command_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-media-command-messages.json"
        ))
        .expect("valid media command fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-media-command-messages.json"
        ))
        .expect("invalid media command fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 17);
        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing media command fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }
            assert!(
                validate_media_command_message(&message).is_err(),
                "invalid media command fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_media_command_message(message).is_err(),
                "invalid media command fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_provider_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("valid provider fixture file must be JSON");

        assert_eq!(messages.len(), 24);

        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_PROVIDER_API_VERSION)
            );
            parse_provider_message(message)
                .expect("fixture must validate and deserialize through generated provider types");
        }
    }

    #[test]
    fn all_invalid_provider_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-provider-messages.json"
        ))
        .expect("valid provider fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-provider-messages.json"
        ))
        .expect("invalid provider fixture file must be JSON");

        assert_eq!(invalid_cases.len(), 24);

        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing provider fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }
            assert!(
                validate_provider_message(&message).is_err(),
                "invalid provider fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_provider_message(message).is_err(),
                "invalid provider fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_renderer_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-renderer-messages.json"
        ))
        .expect("valid renderer fixture file must be JSON");
        assert_eq!(messages.len(), 11);
        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_RENDERER_API_VERSION)
            );
            parse_renderer_message(message)
                .expect("renderer fixture must validate and deserialize through generated types");
        }
    }

    #[test]
    fn all_invalid_renderer_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-renderer-messages.json"
        ))
        .expect("valid renderer fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-renderer-messages.json"
        ))
        .expect("invalid renderer fixture file must be JSON");
        assert_eq!(invalid_cases.len(), 10);
        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing renderer fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }
            assert!(
                validate_renderer_message(&message).is_err(),
                "invalid renderer fixture was accepted: {}",
                test_case.name
            );
            assert!(
                parse_renderer_message(message).is_err(),
                "invalid renderer fixture reached generated Rust type: {}",
                test_case.name
            );
        }
    }

    #[test]
    fn all_valid_export_messages_deserialize_into_generated_types() {
        let messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-export-messages.json"
        ))
        .expect("valid export fixture file must be JSON");
        assert_eq!(messages.len(), 5);
        for message in messages {
            assert_eq!(
                message.get("apiVersion").and_then(Value::as_str),
                Some(NARRACUT_EXPORT_API_VERSION)
            );
            parse_export_message(message)
                .expect("export fixture must validate and deserialize through generated types");
        }
    }

    #[test]
    fn all_invalid_export_messages_are_rejected() {
        let valid_messages: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-export-messages.json"
        ))
        .expect("valid export fixture file must be JSON");
        let invalid_cases: Vec<IndexedInvalidFixture> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/invalid-export-messages.json"
        ))
        .expect("invalid export fixture file must be JSON");
        assert_eq!(invalid_cases.len(), 8);
        for test_case in invalid_cases {
            let mut message = valid_messages
                .get(test_case.source_index)
                .unwrap_or_else(|| panic!("missing export source fixture for {}", test_case.name))
                .clone();
            for patch in test_case.patches() {
                apply_patch(&mut message, patch);
            }
            assert!(validate_export_message(&message).is_err());
            assert!(parse_export_message(message).is_err());
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct InvalidFixture {
        name: String,
        source_document_type: String,
        patch: Option<FixturePatch>,
        #[serde(default)]
        patches: Vec<FixturePatch>,
    }

    impl InvalidFixture {
        fn patches(&self) -> Vec<&FixturePatch> {
            self.patch.iter().chain(self.patches.iter()).collect()
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct IndexedInvalidFixture {
        name: String,
        source_index: usize,
        patch: Option<FixturePatch>,
        #[serde(default)]
        patches: Vec<FixturePatch>,
    }

    impl IndexedInvalidFixture {
        fn patches(&self) -> Vec<&FixturePatch> {
            self.patch.iter().chain(self.patches.iter()).collect()
        }
    }

    #[derive(Debug, Deserialize)]
    struct FixturePatch {
        op: PatchOperation,
        path: Vec<PathSegment>,
        value: Option<Value>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum PatchOperation {
        Remove,
        Replace,
    }

    #[derive(Debug, Deserialize)]
    #[serde(untagged)]
    enum PathSegment {
        Key(String),
        Index(usize),
    }

    fn apply_patch(document: &mut Value, patch: &FixturePatch) {
        let (last, parents) = patch.path.split_last().expect("patch path cannot be empty");
        let mut parent = document;

        for segment in parents {
            parent = match segment {
                PathSegment::Key(key) => parent
                    .get_mut(key)
                    .unwrap_or_else(|| panic!("missing fixture key {key}")),
                PathSegment::Index(index) => parent
                    .get_mut(*index)
                    .unwrap_or_else(|| panic!("missing fixture index {index}")),
            };
        }

        match (&patch.op, last, parent) {
            (PatchOperation::Remove, PathSegment::Key(key), Value::Object(object)) => {
                object
                    .remove(key)
                    .unwrap_or_else(|| panic!("missing fixture key {key}"));
            }
            (PatchOperation::Remove, PathSegment::Index(index), Value::Array(array)) => {
                array.remove(*index);
            }
            (PatchOperation::Replace, PathSegment::Key(key), Value::Object(object)) => {
                object.insert(
                    key.clone(),
                    patch.value.clone().expect("replace patch must have value"),
                );
            }
            (PatchOperation::Replace, PathSegment::Index(index), Value::Array(array)) => {
                array[*index] = patch.value.clone().expect("replace patch must have value");
            }
            _ => panic!("fixture patch path does not match document"),
        }
    }
}
