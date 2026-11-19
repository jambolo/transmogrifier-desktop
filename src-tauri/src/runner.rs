//! Running generated apps. Each app opens in its own window, served from its project's `site/` folder over the
//! `site` protocol on a per-project origin, so apps can't read each other's storage.
//!
//! The page gets none of the app's commands (see the guard in `main.rs`) and no file access. Its console and
//! uncaught errors arrive through the reporter script's requests to its own origin.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use transmogrifier_desktop_core::runtime::{self, Console, ConsoleMessage};

use crate::{AppState, MAIN_WINDOW};

pub const SCHEME: &str = "site";
const EXTERNAL_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];

/// One project's app: running in a window, or closed with its last console kept for **Try to fix it**.
struct Runner {
    project_id: String,
    site_dir: PathBuf,
    allow_network: bool,
    console: Console,
    open: bool,
}

/// Runners by host: the lower-case project ID, which is the app's origin.
#[derive(Default)]
pub struct Runners(Mutex<HashMap<String, Runner>>);

impl Runners {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Runner>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What the main window shows about a project's app.
#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunnerStatus {
    pub project_id: String,
    pub running: bool,
    pub error_count: usize,
    pub network_blocked: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleView {
    pub messages: Vec<ConsoleMessage>,
    /// The errors in the form sent to the model, or `None` when there are none.
    pub error_log: Option<String>,
    pub network_blocked: bool,
    pub running: bool,
}

#[derive(Serialize, Clone)]
struct ExternalLink {
    url: String,
}

fn host(project_id: &str) -> String {
    project_id.to_lowercase()
}

fn label(host: &str) -> String {
    format!("runner-{host}")
}

fn status(runner: &Runner) -> RunnerStatus {
    RunnerStatus {
        project_id: runner.project_id.clone(),
        running: runner.open,
        error_count: runner.console.error_count(),
        network_blocked: runner.console.network_blocked(),
    }
}

fn notify(app: &AppHandle, status: RunnerStatus) {
    let _ = app.emit_to(MAIN_WINDOW, "runner", status);
}

/// Whether `url` is inside the app's own origin. Windows serves custom protocols as `http(s)://<scheme>.<host>`.
fn is_site_url(url: &Url, host: &str) -> bool {
    match url.scheme() {
        SCHEME => url.host_str() == Some(host),
        "http" | "https" => url.host_str() == Some(format!("{SCHEME}.{host}").as_str()),
        _ => false,
    }
}

/// Opens the project's app in its own window, or brings the window to the front if it's already open.
pub fn open(app: &AppHandle, project_id: &str, name: &str, site_dir: PathBuf, allow_network: bool) -> Result<(), String> {
    let host = host(project_id);
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("This app's ID can't be used to open it".to_string());
    }
    let label = label(&host);
    if let Some(window) = app.get_webview_window(&label) {
        return window.set_focus().map_err(|e| e.to_string());
    }
    let state = app.state::<AppState>();
    let runner = Runner {
        project_id: project_id.to_string(),
        site_dir,
        allow_network,
        console: Console::default(),
        open: true,
    };
    notify(app, status(&runner));
    state.runners.lock().insert(host.clone(), runner);

    let url = Url::parse(&format!("{SCHEME}://{host}/")).map_err(|e| e.to_string())?;
    let navigation = (app.clone(), host.clone());
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(url))
        .title(name)
        .inner_size(1200.0, 800.0)
        .min_inner_size(320.0, 320.0)
        .initialization_script(runtime::REPORTER_SCRIPT)
        .on_navigation(move |url| {
            let (app, host) = &navigation;
            // Frames built from markup or blobs inherit the page's policy.
            if is_site_url(url, host) || matches!(url.scheme(), "about" | "blob") {
                return true;
            }
            // A navigation away from the app opens in the system browser only if the user agrees.
            if EXTERNAL_SCHEMES.contains(&url.scheme()) {
                let _ = app.emit_to(MAIN_WINDOW, "external-link", ExternalLink { url: url.to_string() });
            }
            false
        })
        .build();
    let window = match built {
        Ok(window) => window,
        Err(error) => {
            closed(app, &host);
            return Err(error.to_string());
        }
    };
    let closing = (app.clone(), host);
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            closed(&closing.0, &closing.1);
        }
    });
    Ok(())
}

fn closed(app: &AppHandle, host: &str) {
    let state = app.state::<AppState>();
    let status = state.runners.lock().get_mut(host).map(|runner| {
        runner.open = false;
        status(runner)
    });
    if let Some(status) = status {
        notify(app, status);
    }
}

/// Loads the app again, including any changes, with an empty console.
pub fn reload(app: &AppHandle, project_id: &str) -> Result<(), String> {
    let host = host(project_id);
    let Some(window) = app.get_webview_window(&label(&host)) else {
        return Ok(());
    };
    let state = app.state::<AppState>();
    let status = state.runners.lock().get_mut(&host).map(|runner| {
        runner.console.clear();
        status(runner)
    });
    if let Some(status) = status {
        notify(app, status);
    }
    window.eval("window.location.reload()").map_err(|e| e.to_string())
}

/// Closes the app's window, if it's open.
pub fn close(app: &AppHandle, project_id: &str) {
    if let Some(window) = app.get_webview_window(&label(&host(project_id))) {
        let _ = window.close();
    }
}

/// Forgets a deleted project's app.
pub fn forget(app: &AppHandle, project_id: &str) {
    close(app, project_id);
    app.state::<AppState>().runners.lock().remove(&host(project_id));
}

/// Drops the last run's errors once they have been sent to the model.
pub fn clear_errors(app: &AppHandle, project_id: &str) {
    let state = app.state::<AppState>();
    let status = state
        .runners
        .lock()
        .get_mut(&host(project_id))
        .filter(|runner| !runner.open)
        .map(|runner| {
            runner.console.clear();
            status(runner)
        });
    if let Some(status) = status {
        notify(app, status);
    }
}

pub fn console(app: &AppHandle, project_id: &str) -> ConsoleView {
    let state = app.state::<AppState>();
    let runners = state.runners.lock();
    match runners.get(&host(project_id)) {
        Some(runner) => ConsoleView {
            messages: runner.console.messages().to_vec(),
            error_log: runner.console.error_log(),
            network_blocked: runner.console.network_blocked(),
            running: runner.open,
        },
        None => ConsoleView::default(),
    }
}

pub fn status_of(app: &AppHandle, project_id: &str) -> RunnerStatus {
    let state = app.state::<AppState>();
    let runners = state.runners.lock();
    runners.get(&host(project_id)).map(status).unwrap_or_else(|| RunnerStatus {
        project_id: project_id.to_string(),
        ..Default::default()
    })
}

fn respond(status: StatusCode, mime_type: &str, allow_network: bool, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", format!("{mime_type}; charset=utf-8"));
    for (name, value) in runtime::headers(allow_network) {
        response = response.header(name, value);
    }
    response.body(Cow::Owned(body)).expect("the response headers are valid")
}

/// Answers a request on the `site` protocol. Only an app's own window is served, and only its own site.
pub fn serve(app: &AppHandle, webview_label: &str, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let uri = request.uri();
    let host = uri.host().unwrap_or_default().to_lowercase();
    let state = app.state::<AppState>();
    let mut runners = state.runners.lock();
    let Some(runner) = runners
        .get_mut(&host)
        .filter(|runner| runner.open && webview_label == label(&host))
    else {
        return respond(StatusCode::FORBIDDEN, "text/plain", false, Vec::new());
    };
    let allow_network = runner.allow_network;
    let (response, changed) = if uri.path() == runtime::REPORT_PATH {
        let report = uri
            .query()
            .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("d=")))
            .map(runtime::percent_decode)
            .unwrap_or_default();
        runner.console.report(&report, allow_network);
        (respond(StatusCode::NO_CONTENT, "text/plain", allow_network, Vec::new()), true)
    } else {
        match runtime::site_file(&runner.site_dir, uri.path()) {
            Some(file) => (respond(StatusCode::OK, file.mime_type, allow_network, file.bytes), false),
            None => {
                let path = runtime::percent_decode(uri.path().trim_start_matches('/'));
                // Browsers ask for a favicon on their own; a site without one isn't broken.
                let expected = path == "favicon.ico";
                if !expected {
                    let path = if path.is_empty() { "index.html" } else { path.as_str() };
                    runner
                        .console
                        .log(ConsoleMessage::error(format!("Could not load {path}: 404 Not Found")));
                }
                (
                    respond(StatusCode::NOT_FOUND, "text/plain", allow_network, Vec::new()),
                    !expected,
                )
            }
        }
    };
    let status = changed.then(|| status(runner));
    drop(runners);
    if let Some(status) = status {
        notify(app, status);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_apps_own_origin_on_every_platform() {
        let url = |text: &str| Url::parse(text).unwrap();
        assert!(is_site_url(&url("site://abc-1/js/app.js"), "abc-1"));
        assert!(is_site_url(&url("http://site.abc-1/"), "abc-1"));
        assert!(is_site_url(&url("https://site.abc-1/index.html"), "abc-1"));
        assert!(!is_site_url(&url("site://other/"), "abc-1"));
        assert!(!is_site_url(&url("https://abc-1/"), "abc-1"));
        assert!(!is_site_url(&url("https://example.com/"), "abc-1"));
        assert!(!is_site_url(&url("data:text/html,hi"), "abc-1"));
    }

    #[test]
    fn hosts_and_labels_are_lower_case() {
        assert_eq!(host("ABC-1"), "abc-1");
        assert_eq!(label("abc-1"), "runner-abc-1");
    }

    #[test]
    fn status_serializes_to_the_wire_shape_the_frontend_expects() {
        let status = RunnerStatus {
            project_id: "p1".into(),
            running: true,
            error_count: 2,
            network_blocked: false,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({"projectId": "p1", "running": true, "errorCount": 2, "networkBlocked": false})
        );
    }
}
