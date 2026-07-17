use std::{collections::BTreeMap, fmt, sync::Mutex};

use crate::{ProviderError, ProviderErrorCode, ProviderOperation};

const KEYRING_SERVICE: &str = "app.narracut.ai-provider";

#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

pub trait CredentialStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<SecretString>, ProviderError>;
    fn set(&self, provider_id: &str, secret: SecretString) -> Result<(), ProviderError>;
    fn delete(&self, provider_id: &str) -> Result<(), ProviderError>;
}

#[derive(Debug, Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    fn entry(
        provider_id: &str,
        operation: ProviderOperation,
    ) -> Result<keyring::Entry, ProviderError> {
        keyring::Entry::new(KEYRING_SERVICE, provider_id).map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::Internal,
                operation,
                format!("无法连接系统凭据存储：{error}"),
                false,
            )
            .for_provider(provider_id)
        })
    }

    fn map_error(
        provider_id: &str,
        operation: ProviderOperation,
        action: &str,
        error: keyring::Error,
    ) -> ProviderError {
        ProviderError::new(
            ProviderErrorCode::Internal,
            operation,
            format!("{action}系统凭据失败：{error}"),
            false,
        )
        .for_provider(provider_id)
    }
}

impl CredentialStore for SystemCredentialStore {
    fn get(&self, provider_id: &str) -> Result<Option<SecretString>, ProviderError> {
        let operation = ProviderOperation::GetProviderCredentialStatus;
        let entry = Self::entry(provider_id, operation)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(SecretString::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Self::map_error(provider_id, operation, "读取", error)),
        }
    }

    fn set(&self, provider_id: &str, secret: SecretString) -> Result<(), ProviderError> {
        let operation = ProviderOperation::SetProviderCredential;
        let entry = Self::entry(provider_id, operation)?;
        entry
            .set_password(secret.expose())
            .map_err(|error| Self::map_error(provider_id, operation, "写入", error))
    }

    fn delete(&self, provider_id: &str) -> Result<(), ProviderError> {
        let operation = ProviderOperation::DeleteProviderCredential;
        let entry = Self::entry(provider_id, operation)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Self::map_error(provider_id, operation, "删除", error)),
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryCredentialStore {
    values: Mutex<BTreeMap<String, SecretString>>,
}

impl CredentialStore for InMemoryCredentialStore {
    fn get(&self, provider_id: &str) -> Result<Option<SecretString>, ProviderError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| poisoned_store_error(ProviderOperation::GetProviderCredentialStatus))?
            .get(provider_id)
            .cloned())
    }

    fn set(&self, provider_id: &str, secret: SecretString) -> Result<(), ProviderError> {
        self.values
            .lock()
            .map_err(|_| poisoned_store_error(ProviderOperation::SetProviderCredential))?
            .insert(provider_id.to_owned(), secret);
        Ok(())
    }

    fn delete(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.values
            .lock()
            .map_err(|_| poisoned_store_error(ProviderOperation::DeleteProviderCredential))?
            .remove(provider_id);
        Ok(())
    }
}

fn poisoned_store_error(operation: ProviderOperation) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Internal,
        operation,
        "凭据存储锁已损坏。",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::{CredentialStore, InMemoryCredentialStore, SecretString};

    #[test]
    fn in_memory_store_never_requires_real_system_credentials() {
        let store = InMemoryCredentialStore::default();
        assert_eq!(store.get("openai").expect("read succeeds"), None);

        store
            .set("openai", SecretString::new("sk-test-secret-not-real"))
            .expect("set succeeds");
        assert_eq!(
            store
                .get("openai")
                .expect("read succeeds")
                .expect("credential exists")
                .expose(),
            "sk-test-secret-not-real"
        );

        store.delete("openai").expect("delete succeeds");
        assert_eq!(store.get("openai").expect("read succeeds"), None);
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let secret = SecretString::new("must-never-appear");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("must-never-appear"));
        assert!(rendered.contains("REDACTED"));
    }
}
