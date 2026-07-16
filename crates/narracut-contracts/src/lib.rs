#![forbid(unsafe_code)]

//! 由 NarraCut v1 JSON Schema 生成的 Rust 契约类型。
//!
//! Schema 是唯一权威来源；不要在本 crate 中手工复制 TypeScript 类型。

pub const NARRACUT_CONTRACT_VERSION: &str = "1.0.0";

typify::import_types!(schema = "../../packages/contracts/schema/narracut-contracts-v1.schema.json");

#[cfg(test)]
mod tests {
    use super::{NarraCutContractDocument, NARRACUT_CONTRACT_VERSION};
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

            serde_json::from_value::<NarraCutContractDocument>(document)
                .expect("fixture must deserialize through generated Rust contracts");
        }
    }
}
