//! Tauri shell — the thin adapter layer.
//!
//! This binary owns the windows and exposes the core to the web UI as a small set of commands. It contains no
//! application logic — that lives in the core crate. Keep this layer thin: translate between core types and the
//! JSON protocol the front end renders (`src/api.ts` mirrors it), and keep blocking work off the UI thread.
//!
//! Two kinds of window exist. The `main` window is the app's own UI and the only one allowed to invoke
//! commands. Every other window runs a generated app (see `runner`) and is refused.

// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod runner;
mod secrets;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use transmogrifier_desktop_core::agent::{AgentEngine, GenerationEvent};
use transmogrifier_desktop_core::llm::anthropic::AnthropicProvider;
use transmogrifier_desktop_core::llm::openai::OpenAiProvider;
use transmogrifier_desktop_core::llm::{KeySource, LlmError, LlmErrorKind, LlmProvider, ModelInfo, NetworkFailure, ProviderId};
use transmogrifier_desktop_core::project::archive::{self, ImportDefaults};
use transmogrifier_desktop_core::project::{FileEntry, NoticeKind, Project, ProjectStore, TranscriptEntry};
use transmogrifier_desktop_core::settings::{Settings, SettingsStore};

use runner::{ConsoleView, RunnerStatus, Runners};

/// The app's own window; the only one that may invoke commands.
pub const MAIN_WINDOW: &str = "main";

const EXTERNAL_SCHEMES: [&str; 4] = ["http:", "https:", "mailto:", "tel:"];

pub struct AppState {
    store: Arc<ProjectStore>,
    settings: Arc<SettingsStore>,
    providers: HashMap<ProviderId, Arc<dyn LlmProvider>>,
    engine: Arc<AgentEngine>,
    /// Generations in progress, by project ID.
    generations: Mutex<HashMap<String, CancellationToken>>,
    pub runners: Runners,
}

impl AppState {
    fn new(data_dir: &Path) -> Self {
        let store = Arc::new(ProjectStore::new(data_dir.join("projects")));
        let settings = Arc::new(SettingsStore::new(
            data_dir.join("settings.json"),
            Box::new(secrets::KeyringSecrets),
        ));
        let key = |provider: ProviderId| -> KeySource {
            let settings = settings.clone();
            Arc::new(move || settings.api_key(provider))
        };
        let providers: HashMap<ProviderId, Arc<dyn LlmProvider>> = HashMap::from([
            (
                ProviderId::Anthropic,
                Arc::new(AnthropicProvider::new(key(ProviderId::Anthropic))) as Arc<dyn LlmProvider>,
            ),
            (
                ProviderId::Openai,
                Arc::new(OpenAiProvider::new(key(ProviderId::Openai))) as Arc<dyn LlmProvider>,
            ),
        ]);
        let engine = Arc::new(AgentEngine::new(store.clone(), providers.clone()));
        Self {
            store,
            settings,
            providers,
            engine,
            generations: Mutex::default(),
            runners: Runners::default(),
        }
    }

    fn generations(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.generations.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ---------------------------------------------------------------------------
// Protocol — the JSON shapes the front end consumes (mirrored in src/api.ts).
// Core types that cross the boundary (`Project`, `TranscriptEntry`, `Settings`,
// `FileEntry`, `ModelInfo`, `GenerationEvent`, `ConsoleMessage`) serialize in
// their stored form.
// ---------------------------------------------------------------------------

/// Everything the workspace view shows about one project.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectView {
    project: Project,
    transcript: Vec<TranscriptEntry>,
    files: Vec<FileEntry>,
    generating: bool,
    runner: RunnerStatus,
}

/// Why a connection check failed, in a form the front end can explain.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct KeyTestFailure {
    /// `MISSING_KEY`, `INVALID_KEY`, `PERMISSION_DENIED`, `RATE_LIMITED`, `OVERLOADED`, `SERVER`, `REJECTED`,
    /// or `NETWORK`.
    kind: &'static str,
    network: Option<NetworkFailure>,
    status: Option<u16>,
    detail: Option<String>,
}

impl From<LlmError> for KeyTestFailure {
    fn from(error: LlmError) -> Self {
        let (kind, network) = match error.kind {
            LlmErrorKind::MissingKey => ("MISSING_KEY", None),
            LlmErrorKind::InvalidKey => ("INVALID_KEY", None),
            LlmErrorKind::PermissionDenied => ("PERMISSION_DENIED", None),
            LlmErrorKind::RateLimited => ("RATE_LIMITED", None),
            LlmErrorKind::Server { overloaded: true } => ("OVERLOADED", None),
            LlmErrorKind::Server { overloaded: false } => ("SERVER", None),
            LlmErrorKind::Rejected => ("REJECTED", None),
            LlmErrorKind::Network(failure) => ("NETWORK", Some(failure)),
        };
        Self {
            kind,
            network,
            status: error.status,
            detail: error.detail,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
enum KeyTest {
    Passed { models: Vec<ModelInfo> },
    Failed { error: KeyTestFailure },
}

/// A zip the user picked, read and checked but not yet imported.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveInfo {
    path: String,
    name: String,
    has_conversation: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GenerationProgress {
    project_id: String,
    event: GenerationEvent,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GenerationFinished {
    project_id: String,
}

type CommandResult<T> = Result<T, String>;

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Runs blocking work (file dialogs, disk, the credential store) off the async runtime's workers.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> CommandResult<T> + Send + 'static) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(text)?
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.load()
}

/// Saves everything except API keys and their hints.
#[tauri::command]
fn update_settings(state: State<'_, AppState>, settings: Settings) -> CommandResult<Settings> {
    state.settings.update(|current| *current = settings)
}

/// Saves the key in the credential store, or removes it when `key` is `None`.
#[tauri::command]
async fn set_api_key(app: AppHandle, provider: ProviderId, key: Option<String>) -> CommandResult<Settings> {
    blocking(move || app.state::<AppState>().settings.set_api_key(provider, key.as_deref())).await
}

/// Checks the saved key by listing the models it can use.
#[tauri::command]
async fn test_api_key(state: State<'_, AppState>, provider: ProviderId) -> CommandResult<KeyTest> {
    let provider = state
        .providers
        .get(&provider)
        .ok_or("This AI service isn't available")?
        .clone();
    Ok(match provider.list_models().await {
        Ok(models) => KeyTest::Passed { models },
        Err(error) => KeyTest::Failed { error: error.into() },
    })
}

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Vec<Project> {
    state.store.projects()
}

/// Creates an app with the service and model chosen for new apps.
#[tauri::command]
fn create_project(state: State<'_, AppState>, name: String) -> CommandResult<Project> {
    let settings = state.settings.load();
    let provider = settings.default_provider;
    state
        .store
        .create(name.trim(), provider, &settings.default_model(provider))
        .map_err(text)
}

#[tauri::command]
fn rename_project(state: State<'_, AppState>, id: String, name: String) -> CommandResult<()> {
    state.store.rename(&id, name.trim()).map_err(text)
}

#[tauri::command]
fn delete_project(app: AppHandle, state: State<'_, AppState>, id: String) -> CommandResult<()> {
    if let Some(cancel) = state.generations().get(&id) {
        cancel.cancel();
    }
    runner::forget(&app, &id);
    state.store.delete(&id).map_err(text)
}

#[tauri::command]
fn get_project(app: AppHandle, state: State<'_, AppState>, id: String) -> CommandResult<ProjectView> {
    let project = state.store.project(&id).ok_or("This app no longer exists")?;
    Ok(ProjectView {
        project,
        transcript: state.store.transcript(&id),
        files: state.store.workspace(&id).map_err(text)?.list_files(),
        generating: state.generations().contains_key(&id),
        runner: runner::status_of(&app, &id),
    })
}

/// A site file's text, or `None` if it can't be read.
#[tauri::command]
fn read_project_file(state: State<'_, AppState>, id: String, path: String) -> CommandResult<Option<String>> {
    Ok(state.store.workspace(&id).map_err(text)?.read(&path).ok())
}

/// Starts a generation in the background: a new request, or a continuation of the pending one when `prompt`
/// is `None`. Progress arrives as `generation` events, then one `generation-finished`.
#[tauri::command]
fn generate(app: AppHandle, state: State<'_, AppState>, id: String, prompt: Option<String>, fix_errors: bool) -> CommandResult<()> {
    let cancel = CancellationToken::new();
    {
        let mut generations = state.generations();
        if generations.contains_key(&id) {
            return Ok(());
        }
        generations.insert(id.clone(), cancel.clone());
    }
    if fix_errors {
        runner::clear_errors(&app, &id);
    }
    let settings = state.settings.load();
    let engine = state.engine.clone();
    let store = state.store.clone();
    tauri::async_runtime::spawn(async move {
        let progress = {
            let (app, id) = (app.clone(), id.clone());
            move |event| {
                let _ = app.emit_to(
                    MAIN_WINDOW,
                    "generation",
                    GenerationProgress {
                        project_id: id.clone(),
                        event,
                    },
                );
            }
        };
        let result = engine
            .generate(&id, prompt.as_deref(), fix_errors, &settings, &cancel, &progress)
            .await;
        if let Err(error) = result {
            let notice = TranscriptEntry::Notice {
                kind: NoticeKind::Failed,
                detail: Some(error.to_string()),
            };
            let _ = store.append_transcript(&id, &notice);
        }
        app.state::<AppState>().generations().remove(&id);
        let _ = app.emit_to(MAIN_WINDOW, "generation-finished", GenerationFinished { project_id: id });
    });
    Ok(())
}

#[tauri::command]
fn stop_generation(state: State<'_, AppState>, id: String) {
    if let Some(cancel) = state.generations().get(&id) {
        cancel.cancel();
    }
}

/// Opens the project's app in its own window.
#[tauri::command]
async fn run_app(app: AppHandle, id: String) -> CommandResult<()> {
    let state = app.state::<AppState>();
    let project = state.store.project(&id).ok_or("This app no longer exists")?;
    let site_dir = state.store.site_dir(&id).map_err(text)?;
    runner::open(&app, &id, &project.name, site_dir, state.settings.load().allow_app_network)
}

#[tauri::command]
fn reload_app(app: AppHandle, id: String) -> CommandResult<()> {
    runner::reload(&app, &id)
}

#[tauri::command]
fn close_app(app: AppHandle, id: String) {
    runner::close(&app, &id);
}

#[tauri::command]
fn get_console(app: AppHandle, id: String) -> ConsoleView {
    runner::console(&app, &id)
}

/// A file name for an export: the app's name without characters file systems reject.
fn export_file_name(name: &str, backup: bool) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let safe: String = safe.trim().chars().take(60).collect();
    let safe = if safe.is_empty() { "app" } else { safe.as_str() };
    if backup {
        format!("{safe} backup.zip")
    } else {
        format!("{safe}.zip")
    }
}

/// Asks where to save, then writes the app's files alone or a full backup. Returns the path, or `None` if the
/// user cancelled.
#[tauri::command]
async fn export_project(app: AppHandle, id: String, backup: bool) -> CommandResult<Option<String>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let project = state.store.project(&id).ok_or("This app no longer exists")?;
        let dialog = app.dialog().file().add_filter("Zip", &["zip"]);
        let Some(target) = dialog
            .set_file_name(export_file_name(&project.name, backup))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = target.into_path().map_err(text)?;
        let file = fs::File::create(&path).map_err(text)?;
        let result = if backup {
            archive::export_backup(&state.store, &id, file)
        } else {
            archive::export_site(&state.store, &id, file)
        };
        result.map_err(text)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
}

fn read_archive(path: &Path) -> CommandResult<archive::ArchiveContents> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = fs::read(path).map_err(|_| "The file couldn't be opened".to_string())?;
    archive::read(bytes, name.trim()).map_err(text)
}

/// Asks for a zip and checks it. Returns `None` if the user cancelled.
#[tauri::command]
async fn pick_archive(app: AppHandle) -> CommandResult<Option<ArchiveInfo>> {
    blocking(move || {
        let Some(picked) = app.dialog().file().add_filter("Zip", &["zip"]).blocking_pick_file() else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(text)?;
        let contents = read_archive(&path)?;
        Ok(Some(ArchiveInfo {
            path: path.to_string_lossy().into_owned(),
            has_conversation: contents.has_conversation(),
            name: contents.name,
        }))
    })
    .await
}

/// Imports a zip as a new app. An archive without a project uses the service and model for new apps.
#[tauri::command]
async fn import_archive(app: AppHandle, path: String, with_conversation: bool) -> CommandResult<Project> {
    blocking(move || {
        let state = app.state::<AppState>();
        let contents = read_archive(Path::new(&path))?;
        let settings = state.settings.load();
        let provider = settings.default_provider;
        let defaults = ImportDefaults {
            provider,
            model: settings.default_model(provider),
        };
        archive::import(&state.store, &contents, &defaults, with_conversation).map_err(text)
    })
    .await
}

/// Opens a link from a generated app in the system browser, after the user agreed.
#[tauri::command]
fn open_external(app: AppHandle, url: String) -> CommandResult<()> {
    if !EXTERNAL_SCHEMES.iter().any(|scheme| url.starts_with(scheme)) {
        return Err("This kind of link can't be opened".to_string());
    }
    app.opener().open_url(url, None::<&str>).map_err(text)
}

fn main() {
    // Work around WebKitGTK's DMA-BUF renderer leaving stale paint on some GPU stacks (WSLg, NVIDIA).
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    let commands: Box<dyn Fn(tauri::ipc::Invoke) -> bool + Send + Sync> = Box::new(tauri::generate_handler![
        get_settings,
        update_settings,
        set_api_key,
        test_api_key,
        list_projects,
        create_project,
        rename_project,
        delete_project,
        get_project,
        read_project_file,
        generate,
        stop_generation,
        run_app,
        reload_app,
        close_app,
        get_console,
        export_project,
        pick_archive,
        import_archive,
        open_external
    ]);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol(runner::SCHEME, |context, request| {
            runner::serve(context.app_handle(), context.webview_label(), &request)
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            app.manage(AppState::new(&data_dir));
            Ok(())
        })
        // Generated apps run in their own windows and must never reach the app's commands.
        .invoke_handler(move |invoke| {
            if invoke.message.webview().label() != MAIN_WINDOW {
                invoke.resolver.reject("Not allowed from this window");
                return true;
            }
            commands(invoke)
        })
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}

// The command handlers are thin wrappers over the core crate (where the logic
// and its tests live); what is worth pinning down here is the protocol — the
// exact JSON shapes the front end deserializes (`src/api.ts` mirrors them).
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_test_serializes_to_the_wire_shape_the_frontend_expects() {
        let passed = KeyTest::Passed {
            models: vec![ModelInfo {
                id: "m1".into(),
                display_name: "Model 1".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&passed).unwrap(),
            json!({"result": "passed", "models": [{"id": "m1", "displayName": "Model 1"}]})
        );
        let mut error = LlmError::new(LlmErrorKind::Network(NetworkFailure::HostNotFound), "dns");
        error.status = None;
        let failed = KeyTest::Failed { error: error.into() };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            json!({"result": "failed", "error": {"kind": "NETWORK", "network": "HOST_NOT_FOUND", "status": null, "detail": null}})
        );
    }

    #[test]
    fn key_test_failures_keep_the_service_explanation() {
        let mut error = LlmError::new(LlmErrorKind::Server { overloaded: true }, "529");
        error.status = Some(529);
        error.detail = Some("Overloaded".into());
        assert_eq!(
            KeyTestFailure::from(error),
            KeyTestFailure {
                kind: "OVERLOADED",
                network: None,
                status: Some(529),
                detail: Some("Overloaded".into())
            }
        );
    }

    #[test]
    fn generation_progress_serializes_to_the_wire_shape_the_frontend_expects() {
        let progress = GenerationProgress {
            project_id: "p1".into(),
            event: GenerationEvent::ToolRunning {
                name: "write_file".into(),
                path: Some("index.html".into()),
            },
        };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({"projectId": "p1", "event": {"type": "toolRunning", "name": "write_file", "path": "index.html"}})
        );
        let retry = GenerationEvent::Retrying {
            attempt: 1,
            delay_seconds: 2,
            reason: "busy".into(),
        };
        assert_eq!(
            serde_json::to_value(&retry).unwrap(),
            json!({"type": "retrying", "attempt": 1, "delaySeconds": 2, "reason": "busy"})
        );
        assert_eq!(
            serde_json::to_value(GenerationEvent::TurnPersisted).unwrap(),
            json!({"type": "turnPersisted"})
        );
    }

    #[test]
    fn settings_serialize_to_the_wire_shape_the_frontend_expects() {
        assert_eq!(
            serde_json::to_value(Settings::default()).unwrap(),
            json!({
                "defaultProvider": "ANTHROPIC",
                "defaultModels": {},
                "effort": "HIGH",
                "maxToolIterations": 30,
                "maxOutputTokens": 64000,
                "appInstructions": "",
                "allowAppNetwork": false,
                "keyHints": {}
            })
        );
    }

    #[test]
    fn export_names_are_safe_file_names() {
        assert_eq!(export_file_name("Tip jar", false), "Tip jar.zip");
        assert_eq!(export_file_name("a/b:c?", true), "a_b_c_ backup.zip");
        assert_eq!(export_file_name("  ", false), "app.zip");
        assert_eq!(export_file_name(&"x".repeat(100), false).len(), 64);
    }
}
