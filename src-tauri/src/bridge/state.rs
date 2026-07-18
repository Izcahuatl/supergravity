use crate::core::approvals::ApprovalBroker;
#[cfg(test)]
use crate::core::config::MemKeyStore;
use crate::core::config::{AppConfig, KeyStore, OsKeyStore};
use crate::core::store::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// A currently-running agent in one conversation.
pub struct RunningAgent {
    pub cancel: CancellationToken,
    pub broker: Arc<ApprovalBroker>,
}

/// Shared app state (managed by Tauri; cloned Arcs into agent tasks).
pub struct AppState {
    pub store: Arc<Store>,
    pub keys: Arc<dyn KeyStore>,
    pub agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    pub config_path: PathBuf,
    pub ui_config: Arc<Mutex<AppConfig>>,
}

impl AppState {
    /// Production state: OS keychain + real DB file.
    pub fn production(store: Store, config_path: PathBuf, ui_config: AppConfig) -> Self {
        Self::with_keys(store, Arc::new(OsKeyStore), config_path, ui_config)
    }

    pub fn with_keys(
        store: Store,
        keys: Arc<dyn KeyStore>,
        config_path: PathBuf,
        ui_config: AppConfig,
    ) -> Self {
        AppState {
            store: Arc::new(store),
            keys,
            agents: Arc::new(Mutex::new(HashMap::new())),
            config_path,
            ui_config: Arc::new(Mutex::new(ui_config)),
        }
    }

    #[cfg(test)]
    pub fn test() -> Self {
        Self::with_keys(
            Store::open_in_memory().unwrap(),
            Arc::new(MemKeyStore::new()),
            PathBuf::from("test-config.toml"),
            AppConfig::default(),
        )
    }
}
