use crate::core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const KEYRING_SERVICE: &str = "supergravity";

/// API-key storage. Keys are addressed by provider id.
/// Implementations are synchronous (OS keychain IPC) — async callers MUST use
/// `tokio::task::spawn_blocking` or Tauri's sync-command mechanism.
pub trait KeyStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<String>>;
    fn set(&self, provider_id: &str, key: &str) -> Result<()>;
    fn delete(&self, provider_id: &str) -> Result<()>;
}

/// OS keychain-backed store (Windows Credential Manager / macOS Keychain / Secret Service).
pub struct OsKeyStore;

impl KeyStore for OsKeyStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
            .map_err(|e| Error::Config(e.to_string()))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Config(e.to_string())),
        }
    }

    fn set(&self, provider_id: &str, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
            .map_err(|e| Error::Config(e.to_string()))?;
        entry
            .set_password(key)
            .map_err(|e| Error::Config(e.to_string()))
    }

    fn delete(&self, provider_id: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
            .map_err(|e| Error::Config(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Config(e.to_string())),
        }
    }
}

/// In-memory store for tests and headless development.
pub struct MemKeyStore {
    inner: Mutex<HashMap<String, String>>,
}

impl MemKeyStore {
    pub fn new() -> Self {
        MemKeyStore {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for MemKeyStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>> {
        Ok(self.inner.lock().unwrap().get(provider_id).cloned())
    }

    fn set(&self, provider_id: &str, key: &str) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .insert(provider_id.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, provider_id: &str) -> Result<()> {
        self.inner.lock().unwrap().remove(provider_id);
        Ok(())
    }
}

/// Non-secret app config persisted as TOML.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub last_workspace_id: Option<String>,
    pub last_conversation_id: Option<String>,
    /// Default approval mode for NEW conversations ("auto" | "manual"; None = manual).
    pub default_approval_mode: Option<String>,
    /// Toast/flash notifications while unfocused (None = on).
    pub notifications_enabled: Option<bool>,
    /// External (sandbox-crossing) tool policy: "ask" | "allow" | "block" (None = ask).
    pub external_policy: Option<String>,
    /// Run workshop python without prompting (None = on).
    pub workshop_python_no_ask: Option<bool>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<AppConfig> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| Error::Config(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Platform app-data directory for supergravity (e.g. `%APPDATA%\supergravity\data`).
pub fn data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "supergravity")
        .ok_or_else(|| Error::Config("cannot determine app data dir".into()))?;
    Ok(dirs.data_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_keystore_roundtrip() {
        let ks = MemKeyStore::new();
        assert_eq!(ks.get("openai").unwrap(), None);
        ks.set("openai", "sk-test").unwrap();
        assert_eq!(ks.get("openai").unwrap(), Some("sk-test".into()));
        ks.delete("openai").unwrap();
        assert_eq!(ks.get("openai").unwrap(), None);
    }

    #[test]
    fn mem_keystore_delete_missing_is_ok() {
        let ks = MemKeyStore::new();
        assert!(ks.delete("nope").is_ok());
    }

    #[test]
    fn app_config_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = AppConfig {
            last_workspace_id: Some("w1".into()),
            last_conversation_id: Some("c1".into()),
            default_approval_mode: Some("auto".into()),
            notifications_enabled: Some(false),
            external_policy: Some("ask".into()),
            workshop_python_no_ask: Some(true),
        };
        cfg.save(&path).unwrap();
        let back = AppConfig::load(&path).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn app_config_missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn app_config_default_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::default().save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap(), AppConfig::default());
    }
}
