#![forbid(unsafe_code)]

//! 由 NarraCut v1 JSON Schema 生成的 Rust 契约类型。
//!
//! Schema 是唯一权威来源；不要在本 crate 中手工复制 TypeScript 类型。

use std::{error::Error, fmt, sync::OnceLock};

use serde_json::Value;

pub const NARRACUT_CONTRACT_VERSION: &str = "1.0.0";
pub const NARRACUT_PROJECT_COMMAND_API_VERSION: &str = "1.0.0";

typify::import_types!(schema = "../../packages/contracts/schema/narracut-contracts-v1.schema.json");
mod project_command_types {
    typify::import_types!(
        schema = "../../packages/contracts/schema/narracut-project-commands-v1.schema.json"
    );
}
pub use project_command_types::*;

static CONTRACT_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static PROJECT_COMMAND_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();

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

#[cfg(test)]
mod tests {
    use super::{
        parse_contract_document, parse_project_command_message, validate_contract_document,
        validate_project_command_message, NARRACUT_CONTRACT_VERSION,
        NARRACUT_PROJECT_COMMAND_API_VERSION,
    };
    use serde::Deserialize;
    use serde_json::Value;

    #[test]
    fn all_valid_fixtures_deserialize_into_generated_types() {
        let documents: Vec<Value> = serde_json::from_str(include_str!(
            "../../../packages/contracts/fixtures/valid-documents.json"
        ))
        .expect("valid fixture file must be JSON");

        assert_eq!(documents.len(), 8);

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

        assert_eq!(invalid_cases.len(), 9);

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
