//! User settings and API keys. Settings are a JSON file; keys live only in a [`SecretStore`] (the operating
//! system's credential store in the app), never in the file and never in the front end.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::llm::{Effort, ProviderId};

pub const MAX_APP_INSTRUCTIONS_LENGTH: usize = 4_000;
pub const TOOL_ITERATIONS_RANGE: (u32, u32) = (1, 200);
pub const OUTPUT_TOKENS_RANGE: (u32, u32) = (1_024, 128_000);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub default_provider: ProviderId,
    /// The user's choices; providers without one use [`ProviderId::default_model`].
    pub default_models: BTreeMap<ProviderId, String>,
    pub effort: Effort,
    pub max_tool_iterations: u32,
    pub max_output_tokens: u32,
    /// The user's standing instructions for every app; blank for none.
    pub app_instructions: String,
    /// Whether a running app may make https requests to other sites.
    pub allow_app_network: bool,
    /// Last four characters of each saved key, for display.
    pub key_hints: BTreeMap<ProviderId, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_provider: ProviderId::Anthropic,
            default_models: BTreeMap::new(),
            effort: Effort::High,
            max_tool_iterations: 30,
            max_output_tokens: 64_000,
            app_instructions: String::new(),
            allow_app_network: false,
            key_hints: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn default_model(&self, provider: ProviderId) -> String {
        self.default_models
            .get(&provider)
            .cloned()
            .unwrap_or_else(|| provider.default_model().to_string())
    }

    /// Brings every value inside its allowed range.
    fn normalized(mut self) -> Self {
        let (low, high) = TOOL_ITERATIONS_RANGE;
        self.max_tool_iterations = self.max_tool_iterations.clamp(low, high);
        let (low, high) = OUTPUT_TOKENS_RANGE;
        self.max_output_tokens = self.max_output_tokens.clamp(low, high);
        if self.app_instructions.chars().count() > MAX_APP_INSTRUCTIONS_LENGTH {
            self.app_instructions = self.app_instructions.chars().take(MAX_APP_INSTRUCTIONS_LENGTH).collect();
        }
        self
    }
}

/// Where API keys are kept. Errors are messages for the user.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider: ProviderId) -> Result<Option<String>, String>;
    fn set(&self, provider: ProviderId, key: &str) -> Result<(), String>;
    fn delete(&self, provider: ProviderId) -> Result<(), String>;
}

pub struct SettingsStore {
    file: PathBuf,
    secrets: Box<dyn SecretStore>,
    lock: Mutex<()>,
}

impl SettingsStore {
    pub fn new(file: impl Into<PathBuf>, secrets: Box<dyn SecretStore>) -> Self {
        Self {
            file: file.into(),
            secrets,
            lock: Mutex::new(()),
        }
    }

    /// The saved settings, or the defaults when there are none or the file is unreadable.
    pub fn load(&self) -> Settings {
        let _guard = self.guard();
        self.read()
    }

    /// Changes everything except the key hints, which only [`set_api_key`](Self::set_api_key) touches.
    pub fn update(&self, transform: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let _guard = self.guard();
        let old = self.read();
        let mut new = old.clone();
        transform(&mut new);
        new.key_hints = old.key_hints;
        self.write(new.normalized())
    }

    /// Saves `key`, or removes the saved key when it is `None`.
    pub fn set_api_key(&self, provider: ProviderId, key: Option<&str>) -> Result<Settings, String> {
        let _guard = self.guard();
        let mut settings = self.read();
        match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => {
                self.secrets.set(provider, key)?;
                let skip = key.chars().count().saturating_sub(4);
                settings.key_hints.insert(provider, key.chars().skip(skip).collect());
                // New apps must not default to a service the user can't use.
                if !settings.key_hints.contains_key(&settings.default_provider) {
                    settings.default_provider = provider;
                }
            }
            None => {
                self.secrets.delete(provider)?;
                settings.key_hints.remove(&provider);
            }
        }
        self.write(settings)
    }

    /// The saved key. If the credential store has lost it, the hint is cleared so the user is asked again.
    pub fn api_key(&self, provider: ProviderId) -> Option<String> {
        let _guard = self.guard();
        match self.secrets.get(provider) {
            Ok(Some(key)) => Some(key),
            Ok(None) => {
                let mut settings = self.read();
                if settings.key_hints.remove(&provider).is_some() {
                    let _ = self.write(settings);
                }
                None
            }
            Err(_) => None,
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read(&self) -> Settings {
        let text = fs::read_to_string(&self.file).unwrap_or_default();
        serde_json::from_str::<Settings>(&text).unwrap_or_default().normalized()
    }

    fn write(&self, settings: Settings) -> Result<Settings, String> {
        let save = || -> std::io::Result<()> {
            if let Some(parent) = self.file.parent() {
                fs::create_dir_all(parent)?;
            }
            let temp = self.file.with_extension("json.tmp");
            fs::write(&temp, serde_json::to_string(&settings)?)?;
            fs::rename(&temp, &self.file)
        };
        save().map_err(|e| format!("Couldn't save settings: {e}"))?;
        Ok(settings)
    }
}

/// Keeps keys in memory; for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<ProviderId, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, provider: ProviderId) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(&provider).cloned())
    }

    fn set(&self, provider: ProviderId, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(provider, key.to_string());
        Ok(())
    }

    fn delete(&self, provider: ProviderId) -> Result<(), String> {
        self.0.lock().unwrap().remove(&provider);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("settings.json"), Box::new(MemorySecrets::default()));
        (dir, store)
    }

    #[test]
    fn starts_with_the_defaults() {
        let (_dir, store) = store();
        let settings = store.load();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.default_model(ProviderId::Anthropic), "claude-opus-5-5");
        assert_eq!(settings.default_model(ProviderId::Openai), "gpt-6.1-sol");
    }

    #[test]
    fn updates_persist_and_are_clamped() {
        let (_dir, store) = store();
        store
            .update(|s| {
                s.max_tool_iterations = 0;
                s.max_output_tokens = 1_000_000;
                s.app_instructions = "x".repeat(5_000);
                s.default_models.insert(ProviderId::Openai, "gpt-6-luna".into());
                s.key_hints.insert(ProviderId::Openai, "fake".into());
            })
            .unwrap();
        let settings = store.load();
        assert_eq!((settings.max_tool_iterations, settings.max_output_tokens), (1, 128_000));
        assert_eq!(settings.app_instructions.len(), 4_000);
        assert_eq!(settings.default_model(ProviderId::Openai), "gpt-6-luna");
        assert!(settings.key_hints.is_empty());
    }

    #[test]
    fn keys_stay_out_of_the_settings_file() {
        let (dir, store) = store();
        let settings = store.set_api_key(ProviderId::Openai, Some("  sk-secret-1234  ")).unwrap();
        assert_eq!(settings.key_hints.get(&ProviderId::Openai).map(String::as_str), Some("1234"));
        // The only saved key becomes the service for new apps.
        assert_eq!(settings.default_provider, ProviderId::Openai);
        assert_eq!(store.api_key(ProviderId::Openai).as_deref(), Some("sk-secret-1234"));
        let file = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!file.contains("secret"), "{file}");

        let settings = store.set_api_key(ProviderId::Openai, None).unwrap();
        assert!(settings.key_hints.is_empty());
        assert_eq!(store.api_key(ProviderId::Openai), None);
    }

    #[test]
    fn a_key_lost_from_the_credential_store_clears_its_hint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, r#"{"keyHints":{"ANTHROPIC":"abcd"}}"#).unwrap();
        let store = SettingsStore::new(&file, Box::new(MemorySecrets::default()));
        assert_eq!(store.load().key_hints.len(), 1);
        assert_eq!(store.api_key(ProviderId::Anthropic), None);
        assert!(store.load().key_hints.is_empty());
    }

    #[test]
    fn an_unreadable_file_falls_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "not json").unwrap();
        let store = SettingsStore::new(&file, Box::new(MemorySecrets::default()));
        assert_eq!(store.load(), Settings::default());
    }
}
