//! User settings and API keys. Settings are a JSON file; keys live only in a [`SecretStore`] (the operating
//! system's credential store in the app), never in the file and never in the front end. Server addresses
//! aren't secrets and are kept in the file.
//!
//! An AI service the user can make apps with is a [`Connection`]. Claude, ChatGPT, and Gemini have one fixed
//! connection each; a provider reached at an address the user saves can have any number.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::llm::{Effort, ProviderId};

pub const MAX_APP_INSTRUCTIONS_LENGTH: usize = 4_000;
pub const MAX_CONNECTION_NAME_LENGTH: usize = 60;
pub const TOOL_ITERATIONS_RANGE: (u32, u32) = (1, 200);
pub const OUTPUT_TOKENS_RANGE: (u32, u32) = (1_024, 128_000);

/// The providers with one fixed connection each, and that connection's ID. The IDs also name the keys in the
/// credential store, so they must never change.
const BUILT_IN: [(ProviderId, &str); 3] = [
    (ProviderId::Anthropic, "anthropic"),
    (ProviderId::Openai, "openai"),
    (ProviderId::Gemini, "gemini"),
];

const UNKNOWN_CONNECTION: &str = "This AI service is no longer set up";

/// One AI service the user can make apps with: a provider, and what it takes to reach it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    /// The user's name for a server; empty for a built-in connection, which is named after its provider.
    #[serde(default)]
    pub name: String,
    pub protocol: ProviderId,
    /// The base address that request paths are appended to, for a provider reached at one
    /// ([`ProviderId::uses_endpoint`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The user's choice of model for new apps; empty to use [`ProviderId::default_model`].
    #[serde(default)]
    pub model: String,
    /// Last four characters of the saved key, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
}

impl Connection {
    fn built_in(protocol: ProviderId, id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: String::new(),
            protocol,
            endpoint: None,
            model: String::new(),
            key_hint: None,
        }
    }

    /// Whether this is the one fixed connection of its provider, which a project needn't name.
    pub fn is_built_in(&self) -> bool {
        BUILT_IN.iter().any(|(_, id)| *id == self.id)
    }

    /// The ID a project records for the connection: none for a built-in one, which the provider identifies.
    pub fn reference(&self) -> Option<&str> {
        (!self.is_built_in()).then_some(&self.id)
    }

    /// The model for new apps: the user's choice, or the provider's default.
    pub fn effective_model(&self) -> &str {
        if self.model.is_empty() {
            self.protocol.default_model()
        } else {
            &self.model
        }
    }

    /// Whether new apps can use the connection: it has a key, or an address and a model.
    pub fn is_ready(&self) -> bool {
        if self.protocol.uses_endpoint() {
            self.endpoint.is_some() && !self.effective_model().is_empty()
        } else {
            self.key_hint.is_some()
        }
    }
}

/// Changes to a saved connection; a `None` field is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionEdit {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// The built-in connections, then the user's servers in the order they were added.
    pub connections: Vec<Connection>,
    /// The connection the last new app used, which the next one starts with.
    pub last_connection: String,
    pub effort: Effort,
    pub max_tool_iterations: u32,
    pub max_output_tokens: u32,
    /// The user's standing instructions for every app; blank for none.
    pub app_instructions: String,
    /// Whether a running app may make https requests to other sites.
    pub allow_app_network: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: BUILT_IN
                .iter()
                .map(|(protocol, id)| Connection::built_in(*protocol, id))
                .collect(),
            last_connection: BUILT_IN[0].1.to_string(),
            effort: Effort::High,
            max_tool_iterations: 30,
            max_output_tokens: 64_000,
            app_instructions: String::new(),
            allow_app_network: false,
        }
    }
}

impl Settings {
    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections.iter().find(|connection| connection.id == id)
    }

    /// The connection a project uses. A project names its connection only when its provider can have several;
    /// one that names none uses the provider's only connection. `None` when the connection was removed, or
    /// the project came from an install with other connections.
    pub fn connection_for(&self, provider: ProviderId, connection: Option<&str>) -> Option<&Connection> {
        let mut candidates = self.connections.iter().filter(|candidate| candidate.protocol == provider);
        match connection {
            Some(id) => candidates.find(|candidate| candidate.id == id),
            None => candidates.next().filter(|_| candidates.next().is_none()),
        }
    }

    /// The connection a new app starts with: the last one used.
    pub fn new_app_connection(&self) -> &Connection {
        self.connection(&self.last_connection).unwrap_or(&self.connections[0])
    }

    fn connection_mut(&mut self, id: &str) -> Result<&mut Connection, String> {
        self.connections
            .iter_mut()
            .find(|connection| connection.id == id)
            .ok_or_else(|| UNKNOWN_CONNECTION.to_string())
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

        // Every built-in connection exists exactly once and comes first; the rest are servers with their own IDs.
        let saved = std::mem::take(&mut self.connections);
        let mut seen = HashSet::new();
        for (protocol, id) in BUILT_IN {
            let found = saved.iter().find(|connection| connection.id == id);
            self.connections.push(Connection {
                model: found.map(|connection| connection.model.clone()).unwrap_or_default(),
                key_hint: found.and_then(|connection| connection.key_hint.clone()),
                ..Connection::built_in(protocol, id)
            });
            seen.insert(id.to_string());
        }
        for connection in saved {
            if connection.protocol.uses_endpoint() && !connection.id.is_empty() && seen.insert(connection.id.clone()) {
                self.connections.push(connection);
            }
        }
        for connection in &mut self.connections {
            connection.name = connection.name.trim().to_string();
            connection.model = connection.model.trim().to_string();
        }

        // New apps must not start with a service the user can't use.
        if !self.connection(&self.last_connection).is_some_and(Connection::is_ready) {
            let ready = self.connections.iter().find(|connection| connection.is_ready());
            self.last_connection = ready.unwrap_or(&self.connections[0]).id.clone();
        }
        self
    }
}

/// Where API keys are kept, by connection ID. Errors are messages for the user.
pub trait SecretStore: Send + Sync {
    fn get(&self, connection: &str) -> Result<Option<String>, String>;
    fn set(&self, connection: &str, key: &str) -> Result<(), String>;
    fn delete(&self, connection: &str) -> Result<(), String>;
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

    /// Changes everything except the connections, which have their own methods.
    pub fn update(&self, transform: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let _guard = self.guard();
        let old = self.read();
        let mut new = old.clone();
        transform(&mut new);
        new.connections = old.connections;
        self.write(new.normalized())
    }

    /// Saves a connection to a server at `endpoint`, the base address that request paths are appended to, such
    /// as `http://localhost:11434/v1`. The new connection is the last in [`Settings::connections`].
    pub fn add_connection(&self, name: &str, endpoint: &str) -> Result<Settings, String> {
        let _guard = self.guard();
        let mut settings = self.read();
        let connection = Connection {
            id: uuid::Uuid::new_v4().to_string(),
            name: checked_name(&settings, None, name)?,
            protocol: ProviderId::OpenaiCompatible,
            endpoint: Some(checked_endpoint(endpoint)?),
            model: String::new(),
            key_hint: None,
        };
        settings.connections.push(connection);
        self.write(settings.normalized())
    }

    /// Changes a connection's model, or a server's name or address.
    pub fn edit_connection(&self, id: &str, edit: ConnectionEdit) -> Result<Settings, String> {
        let _guard = self.guard();
        let mut settings = self.read();
        let name = edit.name.map(|name| checked_name(&settings, Some(id), &name)).transpose()?;
        let endpoint = edit.endpoint.map(|endpoint| checked_endpoint(&endpoint)).transpose()?;
        let connection = settings.connection_mut(id)?;
        if (name.is_some() || endpoint.is_some()) && connection.is_built_in() {
            return Err("This AI service has a fixed name and address".to_string());
        }
        if let Some(name) = name {
            connection.name = name;
        }
        if let Some(endpoint) = endpoint {
            connection.endpoint = Some(endpoint);
        }
        if let Some(model) = edit.model {
            connection.model = model;
        }
        self.write(settings.normalized())
    }

    /// Forgets a server and its key. Apps made with it keep its ID and need another connection.
    pub fn remove_connection(&self, id: &str) -> Result<Settings, String> {
        let _guard = self.guard();
        let mut settings = self.read();
        if settings.connection_mut(id)?.is_built_in() {
            return Err("This AI service can't be removed".to_string());
        }
        self.secrets.delete(id)?;
        settings.connections.retain(|connection| connection.id != id);
        self.write(settings.normalized())
    }

    /// Saves `key` for the connection, or removes the saved key when it is `None`.
    pub fn set_api_key(&self, id: &str, key: Option<&str>) -> Result<Settings, String> {
        let _guard = self.guard();
        let mut settings = self.read();
        let connection = settings.connection_mut(id)?;
        match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => {
                self.secrets.set(id, key)?;
                let skip = key.chars().count().saturating_sub(4);
                connection.key_hint = Some(key.chars().skip(skip).collect());
            }
            None => {
                self.secrets.delete(id)?;
                connection.key_hint = None;
            }
        }
        self.write(settings.normalized())
    }

    /// The connection's saved key. If the credential store has lost it, the hint is cleared so the user is
    /// asked again.
    pub fn api_key(&self, id: &str) -> Option<String> {
        let _guard = self.guard();
        match self.secrets.get(id) {
            Ok(Some(key)) => Some(key),
            Ok(None) => {
                let mut settings = self.read();
                let hint = settings
                    .connection_mut(id)
                    .ok()
                    .and_then(|connection| connection.key_hint.take());
                if hint.is_some() {
                    let _ = self.write(settings.normalized());
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

/// A server's name, trimmed, if it is usable and no other server (than `id`) has it.
fn checked_name(settings: &Settings, id: Option<&str>, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name for the service".to_string());
    }
    if name.chars().count() > MAX_CONNECTION_NAME_LENGTH {
        return Err(format!("Use a name of at most {MAX_CONNECTION_NAME_LENGTH} characters"));
    }
    let taken = |other: &Connection| Some(other.id.as_str()) != id && other.name.to_lowercase() == name.to_lowercase();
    if settings.connections.iter().any(taken) {
        return Err("Another service already has this name".to_string());
    }
    Ok(name.to_string())
}

/// A server address without its trailing slash, if it is a web address.
fn checked_endpoint(endpoint: &str) -> Result<String, String> {
    let endpoint = endpoint.trim();
    let url = reqwest::Url::parse(endpoint).ok();
    if !url.is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host()) {
        return Err("Enter a web address that starts with http:// or https://".to_string());
    }
    Ok(endpoint.trim_end_matches('/').to_string())
}

/// Keeps keys in memory; for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, connection: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(connection).cloned())
    }

    fn set(&self, connection: &str, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(connection.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, connection: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(connection);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLLAMA: &str = "http://localhost:11434/v1";

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("settings.json"), Box::new(MemorySecrets::default()));
        (dir, store)
    }

    fn model(model: &str) -> ConnectionEdit {
        ConnectionEdit {
            model: Some(model.to_string()),
            ..ConnectionEdit::default()
        }
    }

    /// The ID of the server added last.
    fn last_id(settings: &Settings) -> String {
        settings.connections.last().unwrap().id.clone()
    }

    #[test]
    fn starts_with_the_built_in_connections() {
        let (_dir, store) = store();
        let settings = store.load();
        assert_eq!(settings, Settings::default());
        let ids: Vec<&str> = settings.connections.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["anthropic", "openai", "gemini"]);
        assert!(settings.connections.iter().all(|c| c.is_built_in() && !c.is_ready()));
        assert_eq!(settings.connection("anthropic").unwrap().effective_model(), "claude-opus-5-5");
        assert_eq!(settings.connection("openai").unwrap().effective_model(), "gpt-6.1-sol");
        assert_eq!(settings.new_app_connection().id, "anthropic");
    }

    #[test]
    fn updates_persist_and_are_clamped_but_leave_connections_alone() {
        let (_dir, store) = store();
        store
            .update(|s| {
                s.max_tool_iterations = 0;
                s.max_output_tokens = 1_000_000;
                s.app_instructions = "x".repeat(5_000);
                s.connections[1].key_hint = Some("fake".into());
                s.connections.clear();
            })
            .unwrap();
        let settings = store.load();
        assert_eq!((settings.max_tool_iterations, settings.max_output_tokens), (1, 128_000));
        assert_eq!(settings.app_instructions.len(), 4_000);
        assert_eq!(settings.connections, Settings::default().connections);
    }

    #[test]
    fn keys_stay_out_of_the_settings_file() {
        let (dir, store) = store();
        let settings = store.set_api_key("openai", Some("  sk-secret-1234  ")).unwrap();
        assert_eq!(settings.connection("openai").unwrap().key_hint.as_deref(), Some("1234"));
        // The only connection with a key becomes the one new apps start with.
        assert_eq!(settings.last_connection, "openai");
        assert_eq!(store.api_key("openai").as_deref(), Some("sk-secret-1234"));
        let file = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!file.contains("secret"), "{file}");

        let settings = store.set_api_key("openai", None).unwrap();
        assert_eq!(settings.connection("openai").unwrap().key_hint, None);
        assert_eq!(store.api_key("openai"), None);
        assert!(store.set_api_key("missing", Some("key")).is_err());
    }

    #[test]
    fn new_apps_move_to_a_connection_that_is_ready() {
        let (_dir, store) = store();
        store.set_api_key("anthropic", Some("key-1")).unwrap();
        store.set_api_key("gemini", Some("key-2")).unwrap();
        let settings = store.update(|s| s.last_connection = "gemini".into()).unwrap();
        assert_eq!(settings.new_app_connection().id, "gemini");
        let settings = store.set_api_key("gemini", None).unwrap();
        assert_eq!(settings.new_app_connection().id, "anthropic");
        let settings = store.update(|s| s.last_connection = "missing".into()).unwrap();
        assert_eq!(settings.last_connection, "anthropic");
    }

    #[test]
    fn a_server_is_ready_once_it_has_a_model() {
        let (dir, store) = store();
        let settings = store.add_connection("  Ollama ", &format!(" {OLLAMA}/ ")).unwrap();
        let id = last_id(&settings);
        let server = settings.connection(&id).unwrap();
        assert_eq!((server.name.as_str(), server.endpoint.as_deref()), ("Ollama", Some(OLLAMA)));
        assert_eq!(server.protocol, ProviderId::OpenaiCompatible);
        assert!(!server.is_built_in() && !server.is_ready());
        assert_eq!(settings.last_connection, "anthropic");

        // No key is needed, and blank models don't count.
        let settings = store.edit_connection(&id, model("  ")).unwrap();
        assert!(!settings.connection(&id).unwrap().is_ready());
        let settings = store.edit_connection(&id, model(" qwen3-coder ")).unwrap();
        assert_eq!(settings.connection(&id).unwrap().effective_model(), "qwen3-coder");
        assert_eq!(settings.last_connection, id);
        let file = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(
            file.contains(&format!(
                r#"{{"id":"{id}","name":"Ollama","protocol":"OPENAI_COMPATIBLE","endpoint":"{OLLAMA}","model":"qwen3-coder"}}"#
            )),
            "{file}"
        );
    }

    #[test]
    fn several_servers_keep_their_own_addresses_and_keys() {
        let (_dir, store) = store();
        let first = last_id(&store.add_connection("Ollama", OLLAMA).unwrap());
        let second = last_id(&store.add_connection("OpenRouter", "https://openrouter.ai/api/v1").unwrap());
        assert_ne!(first, second);
        store.set_api_key(&second, Some("sk-or-9876")).unwrap();
        assert_eq!(store.api_key(&first), None);
        assert_eq!(store.api_key(&second).as_deref(), Some("sk-or-9876"));

        let edit = ConnectionEdit {
            name: Some("Router".into()),
            endpoint: Some("https://example.com/v1/".into()),
            model: None,
        };
        let settings = store.edit_connection(&second, edit).unwrap();
        let server = settings.connection(&second).unwrap();
        assert_eq!(
            (server.name.as_str(), server.endpoint.as_deref()),
            ("Router", Some("https://example.com/v1"))
        );
        assert_eq!(server.key_hint.as_deref(), Some("9876"));
        assert_eq!(settings.connection(&first).unwrap().endpoint.as_deref(), Some(OLLAMA));

        // Removing a server forgets its key too.
        let settings = store.remove_connection(&second).unwrap();
        assert_eq!(settings.connections.len(), 4);
        assert_eq!(store.api_key(&second), None);
        assert!(store.remove_connection(&second).is_err());
    }

    #[test]
    fn servers_need_a_web_address_and_a_name_of_their_own() {
        let (_dir, store) = store();
        for bad in ["localhost:11434", "ftp://example.com", "http://", "not an address", ""] {
            assert!(store.add_connection("Server", bad).is_err(), "{bad}");
        }
        assert!(store.add_connection("  ", OLLAMA).is_err());
        assert!(store.add_connection(&"x".repeat(61), OLLAMA).is_err());
        assert_eq!(store.load().connections.len(), 3);

        let id = last_id(&store.add_connection("Ollama", OLLAMA).unwrap());
        assert!(store.add_connection("ollama", OLLAMA).is_err());
        // A server may keep its own name.
        let rename = |name: &str| ConnectionEdit {
            name: Some(name.to_string()),
            ..ConnectionEdit::default()
        };
        assert!(store.edit_connection(&id, rename("OLLAMA")).is_ok());
        let bad_address = ConnectionEdit {
            endpoint: Some("nowhere".into()),
            ..ConnectionEdit::default()
        };
        assert!(store.edit_connection(&id, bad_address).is_err());
    }

    #[test]
    fn built_in_connections_are_fixed_apart_from_their_model() {
        let (_dir, store) = store();
        let settings = store.edit_connection("openai", model("gpt-6-luna")).unwrap();
        assert_eq!(settings.connection("openai").unwrap().effective_model(), "gpt-6-luna");
        let rename = ConnectionEdit {
            name: Some("Mine".into()),
            ..ConnectionEdit::default()
        };
        assert!(store.edit_connection("openai", rename).is_err());
        let move_it = ConnectionEdit {
            endpoint: Some("https://example.com/v1".into()),
            ..ConnectionEdit::default()
        };
        assert!(store.edit_connection("anthropic", move_it).is_err());
        assert!(store.remove_connection("gemini").is_err());
        assert_eq!(store.load().connections.len(), 3);
    }

    #[test]
    fn a_project_finds_its_connection() {
        let (_dir, store) = store();
        let settings = store.load();
        // A built-in provider's only connection needs no ID.
        assert_eq!(settings.connection_for(ProviderId::Gemini, None).unwrap().id, "gemini");
        assert_eq!(settings.connection_for(ProviderId::OpenaiCompatible, None), None);
        // An ID only ever selects a connection that speaks the project's format.
        assert_eq!(settings.connection_for(ProviderId::OpenaiCompatible, Some("anthropic")), None);

        let first = last_id(&store.add_connection("Ollama", OLLAMA).unwrap());
        let settings = store.load();
        assert_eq!(settings.connection_for(ProviderId::OpenaiCompatible, None).unwrap().id, first);
        let second = last_id(&store.add_connection("LM Studio", "http://localhost:1234/v1").unwrap());
        let settings = store.load();
        assert_eq!(settings.connection_for(ProviderId::OpenaiCompatible, None), None);
        assert_eq!(
            settings
                .connection_for(ProviderId::OpenaiCompatible, Some(&second))
                .unwrap()
                .name,
            "LM Studio"
        );
        assert_eq!(settings.connection_for(ProviderId::OpenaiCompatible, Some("gone")), None);
    }

    #[test]
    fn damaged_connection_lists_are_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(
            &file,
            concat!(
                r#"{"connections":["#,
                r#"{"id":"s1","name":" A ","protocol":"OPENAI_COMPATIBLE","endpoint":"http://a/v1","model":"m"},"#,
                r#"{"id":"s1","name":"Twin","protocol":"OPENAI_COMPATIBLE","endpoint":"http://b/v1"},"#,
                r#"{"id":"s2","name":"Not a server","protocol":"ANTHROPIC"},"#,
                r#"{"id":"gemini","name":"Renamed","protocol":"OPENAI","endpoint":"http://c/v1","keyHint":"abcd"}"#,
                r#"],"lastConnection":"s2"}"#
            ),
        )
        .unwrap();
        let settings = SettingsStore::new(&file, Box::new(MemorySecrets::default())).load();
        let ids: Vec<&str> = settings.connections.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["anthropic", "openai", "gemini", "s1"]);
        let gemini = settings.connection("gemini").unwrap();
        assert_eq!(
            (gemini.protocol, gemini.name.as_str(), &gemini.endpoint),
            (ProviderId::Gemini, "", &None)
        );
        assert_eq!(gemini.key_hint.as_deref(), Some("abcd"));
        assert_eq!(settings.connection("s1").unwrap().name, "A");
        assert_eq!(settings.last_connection, "gemini");
    }

    #[test]
    fn a_key_lost_from_the_credential_store_clears_its_hint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(
            &file,
            r#"{"connections":[{"id":"anthropic","protocol":"ANTHROPIC","keyHint":"abcd"}]}"#,
        )
        .unwrap();
        let store = SettingsStore::new(&file, Box::new(MemorySecrets::default()));
        assert_eq!(
            store.load().connection("anthropic").unwrap().key_hint.as_deref(),
            Some("abcd")
        );
        assert_eq!(store.api_key("anthropic"), None);
        assert_eq!(store.load().connection("anthropic").unwrap().key_hint, None);
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
