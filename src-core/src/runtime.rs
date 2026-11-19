//! Running a generated app: serving its files with a content security policy, and collecting its console.
//!
//! The shell serves each project's `site/` on its own origin, so every app has its own storage. Requests
//! outside that origin are blocked by the policy unless network access is allowed, and then limited to https
//! data, images, and media (scripts, styles, and fonts stay local).

use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_CONSOLE_MESSAGES: usize = 500;
const MAX_REPORT_CHARS: usize = 4_000;
const MAX_LOGGED_ERRORS: usize = 20;
const MAX_LOG_CHARS: usize = 16_000;

/// Where the page's reporter script sends console lines and errors. No site file can have this path: the
/// workspace rejects segments that start with a dot.
pub const REPORT_PATH: &str = "/.transmogrifier/report";

/// Installed at document start. Reports console output, uncaught errors with their stacks, and requests the
/// content security policy blocked.
pub const REPORTER_SCRIPT: &str = r#"(() => {
  const nativeFetch = window.fetch.bind(window);
  const origin = location.protocol + '//' + location.host + '/';
  const strip = (value) => String(value || '').split(origin).join('');
  const clip = (value) => strip(value).slice(0, 1500);
  let budget = 500;
  const send = (report) => {
    if (budget-- <= 0) return;
    try {
      const data = encodeURIComponent(JSON.stringify(report));
      nativeFetch('/.transmogrifier/report?d=' + data, { cache: 'no-store' }).catch(() => {});
    } catch (_) {}
  };
  const describe = (value) => {
    if (value instanceof Error) return value.stack || value.name + ': ' + value.message;
    if (typeof value === 'object' && value !== null) {
      try { return JSON.stringify(value); } catch (_) { return String(value); }
    }
    return String(value);
  };
  const levels = { log: 'LOG', info: 'LOG', debug: 'LOG', warn: 'WARNING', error: 'ERROR' };
  for (const [method, level] of Object.entries(levels)) {
    const native = console[method].bind(console);
    console[method] = (...args) => {
      native(...args);
      send({ level, message: clip(args.map(describe).join(' ')) });
    };
  }
  window.addEventListener('error', (event) => {
    if (!(event instanceof ErrorEvent)) return;
    send({
      level: 'ERROR',
      message: clip(event.message),
      source: strip(event.filename),
      line: Number(event.lineno) || 0,
      stack: clip(event.error && event.error.stack)
    });
  });
  // A blocked fetch only rejects with "Failed to fetch", which looks like being offline.
  document.addEventListener('securitypolicyviolation', (event) => {
    if (event.effectiveDirective !== 'connect-src') return;
    send({ message: String(event.blockedURI), network: true });
  });
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    const text = reason instanceof Error ? reason.name + ': ' + reason.message : String(reason);
    send({ level: 'ERROR', message: clip('Uncaught (in promise) ' + text), stack: clip(reason && reason.stack) });
  });
})();"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Level {
    #[default]
    Log,
    Warning,
    Error,
}

/// One line of the generated app's console. `source` is relative to the site root when it is a site file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleMessage {
    pub level: Level,
    pub message: String,
    pub source: String,
    pub line: u32,
    pub stack: Option<String>,
}

impl ConsoleMessage {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            message: message.into(),
            source: String::new(),
            line: 0,
            stack: None,
        }
    }
}

/// What the reporter script sends.
#[derive(Deserialize)]
#[serde(default)]
struct Report {
    level: Level,
    message: String,
    source: String,
    line: u32,
    stack: String,
    /// A request the content security policy blocked; `message` is its URL.
    network: bool,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            level: Level::Error,
            message: String::new(),
            source: String::new(),
            line: 0,
            stack: String::new(),
            network: false,
        }
    }
}

/// The console of one running app.
#[derive(Debug, Default)]
pub struct Console {
    messages: Vec<ConsoleMessage>,
    /// The page tried a request that is blocked only because network access for apps is off.
    network_blocked: bool,
}

impl Console {
    pub fn messages(&self) -> &[ConsoleMessage] {
        &self.messages
    }

    pub fn network_blocked(&self) -> bool {
        self.network_blocked
    }

    pub fn error_count(&self) -> usize {
        self.messages.iter().filter(|message| message.level == Level::Error).count()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn log(&mut self, message: ConsoleMessage) {
        self.messages.push(message);
        if self.messages.len() > MAX_CONSOLE_MESSAGES {
            let excess = self.messages.len() - MAX_CONSOLE_MESSAGES;
            self.messages.drain(..excess);
        }
    }

    /// Records a report from the page's reporter script; unreadable reports are ignored.
    pub fn report(&mut self, json: &str, allow_network: bool) {
        let Ok(report) = serde_json::from_str::<Report>(json) else {
            return;
        };
        let clip = |text: &str| -> String { text.chars().take(MAX_REPORT_CHARS).collect() };
        if report.network {
            // With network access on, the policy still blocks plain http and the like; that's not the setting.
            if !allow_network {
                self.network_blocked = true;
                self.log(ConsoleMessage::error(format!(
                    "Request to {} failed: network access for apps is turned off in Settings.",
                    clip(&report.message)
                )));
            }
            return;
        }
        self.log(ConsoleMessage {
            level: report.level,
            message: clip(&report.message),
            source: report.source,
            line: report.line,
            stack: Some(clip(&report.stack)).filter(|stack| !stack.trim().is_empty()),
        });
    }

    /// The most recent errors in a form suitable for the model, or `None` when there are none.
    pub fn error_log(&self) -> Option<String> {
        let errors: Vec<&ConsoleMessage> = self.messages.iter().filter(|m| m.level == Level::Error).collect();
        if errors.is_empty() {
            return None;
        }
        let recent = &errors[errors.len().saturating_sub(MAX_LOGGED_ERRORS)..];
        let lines: Vec<String> = recent
            .iter()
            .map(|error| {
                let location = if error.source.is_empty() {
                    String::new()
                } else {
                    format!(" ({}:{})", error.source, error.line)
                };
                let stack = error.stack.as_deref().map(|stack| {
                    let indented: Vec<String> = stack.lines().map(|line| format!("    {line}")).collect();
                    format!("\n{}", indented.join("\n"))
                });
                format!("{}{location}{}", error.message, stack.unwrap_or_default())
            })
            .collect();
        let log = lines.join("\n");
        let skip = log.chars().count().saturating_sub(MAX_LOG_CHARS);
        Some(log.chars().skip(skip).collect())
    }
}

/// The headers every response to the page carries. Offline by default, even for a page that tries fetch,
/// WebSocket, or a form post to another host. With network access, the page may fetch data, open secure
/// WebSockets, and show images and media from any https site. Scripts, styles, and fonts stay local, so no
/// third-party code runs with the page's storage.
pub fn headers(allow_network: bool) -> Vec<(&'static str, String)> {
    let network = if allow_network {
        "connect-src 'self' https: wss:; img-src 'self' data: blob: https:; media-src 'self' data: blob: https:; "
    } else {
        ""
    };
    vec![
        (
            "Content-Security-Policy",
            format!(
                "default-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; {network}form-action 'self'; \
                 base-uri 'self'"
            ),
        ),
        ("Cache-Control", "no-store".to_string()),
        ("Permissions-Policy", "camera=(), geolocation=(), microphone=()".to_string()),
    ]
}

fn mime_type(extension: &str) -> &'static str {
    match extension {
        "html" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SiteFile {
    /// A type the module loader accepts.
    pub mime_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The site file a URL path names, or `None` if there is no such file inside `site_dir`. A folder serves its
/// `index.html`.
pub fn site_file(site_dir: &Path, url_path: &str) -> Option<SiteFile> {
    let decoded = percent_decode(url_path);
    let relative = decoded.trim_start_matches('/');
    if relative.contains(['\\', ':', '\0']) {
        return None;
    }
    let name = if relative.is_empty() || relative.ends_with('/') {
        format!("{relative}index.html")
    } else {
        relative.to_string()
    };
    let root = site_dir.canonicalize().ok()?;
    let file = root.join(name).canonicalize().ok()?;
    if !file.starts_with(&root) || !file.is_file() {
        return None;
    }
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    Some(SiteFile {
        mime_type: mime_type(&extension),
        bytes: std::fs::read(&file).ok()?,
    })
}

/// Decodes `%XX` escapes; malformed escapes are kept as they are.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes.get(index + 1..index + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[index], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("site/js")).unwrap();
        fs::write(dir.path().join("site/index.html"), "<h1>Hi</h1>").unwrap();
        fs::write(dir.path().join("site/js/app.mjs"), "export {}").unwrap();
        fs::write(dir.path().join("site/js/index.html"), "inner").unwrap();
        fs::write(dir.path().join("site/my file.txt"), "spaced").unwrap();
        fs::write(dir.path().join("secret.txt"), "secret").unwrap();
        dir
    }

    #[test]
    fn serves_site_files_with_module_friendly_types() {
        let dir = site();
        let root = dir.path().join("site");
        assert_eq!(
            site_file(&root, "/"),
            Some(SiteFile {
                mime_type: "text/html",
                bytes: b"<h1>Hi</h1>".to_vec()
            })
        );
        assert_eq!(site_file(&root, "/js/app.mjs").unwrap().mime_type, "text/javascript");
        assert_eq!(site_file(&root, "/js/").unwrap().bytes, b"inner");
        assert_eq!(site_file(&root, "/my%20file.txt").unwrap().bytes, b"spaced");
    }

    #[test]
    fn serves_nothing_outside_the_site() {
        let dir = site();
        let root = dir.path().join("site");
        for path in [
            "/missing.html",
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/js",
            "/c:/windows/win.ini",
            "/..\\secret.txt",
        ] {
            assert_eq!(site_file(&root, path), None, "{path}");
        }
        assert_eq!(site_file(&dir.path().join("nowhere"), "/"), None);
    }

    #[test]
    fn policy_blocks_the_network_unless_allowed() {
        let offline = &headers(false)[0].1;
        assert_eq!(
            offline,
            "default-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; form-action 'self'; base-uri 'self'"
        );
        let online = &headers(true)[0].1;
        assert!(online.contains("connect-src 'self' https: wss:; img-src 'self' data: blob: https:;"));
        assert!(!online.contains("script-src"));
        assert_eq!(headers(true)[1], ("Cache-Control", "no-store".to_string()));
    }

    #[test]
    fn collects_reports_and_formats_the_error_log() {
        let mut console = Console::default();
        console.report(r#"{"level":"LOG","message":"hello"}"#, false);
        console.report(r#"{"level":"WARNING","message":"careful"}"#, false);
        assert_eq!(console.error_log(), None);
        console.report(
            r#"{"level":"ERROR","message":"Uncaught ReferenceError: total is not defined","source":"js/app.js","line":9,"stack":"ReferenceError: total is not defined\n    at add (js/app.js:9:3)"}"#,
            false,
        );
        console.report(r#"{"message":"Uncaught (in promise) boom","stack":"  "}"#, false);
        console.report("not json", false);
        assert_eq!(console.messages().len(), 4);
        assert_eq!(console.error_count(), 2);
        assert_eq!(
            console.error_log().unwrap(),
            "Uncaught ReferenceError: total is not defined (js/app.js:9)\n    ReferenceError: total is not defined\n        at add (js/app.js:9:3)\nUncaught (in promise) boom"
        );
        console.clear();
        assert!(console.messages().is_empty());
    }

    #[test]
    fn a_blocked_request_is_explained_only_when_the_setting_is_the_cause() {
        let mut console = Console::default();
        console.report(r#"{"message":"https://api.example.com/x","network":true}"#, true);
        assert!(!console.network_blocked());
        assert!(console.messages().is_empty());
        console.report(r#"{"message":"https://api.example.com/x","network":true}"#, false);
        assert!(console.network_blocked());
        assert_eq!(
            console.messages()[0].message,
            "Request to https://api.example.com/x failed: network access for apps is turned off in Settings."
        );
    }

    #[test]
    fn keeps_only_the_most_recent_messages() {
        let mut console = Console::default();
        for index in 0..600 {
            console.log(ConsoleMessage::error(format!("e{index}")));
        }
        assert_eq!(console.messages().len(), 500);
        assert_eq!(console.messages()[0].message, "e100");
        assert_eq!(console.error_log().unwrap().lines().count(), 20);
    }
}
