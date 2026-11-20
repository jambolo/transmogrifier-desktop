//! Projects on disk: `<root>/<id>/` holds `project.json`, `transcript.jsonl` (one entry per line, appended),
//! `conversation.json`, and the generated app in `site/`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::workspace::{Workspace, WorkspaceLimits};
use crate::llm::{ProviderHistory, ProviderId, TurnInput};

const PROJECT: &str = "project.json";
const TRANSCRIPT: &str = "transcript.jsonl";
const CONVERSATION: &str = "conversation.json";
const SITE: &str = "site";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Unknown project {0}")]
    UnknownProject(String),
    #[error("Conversation history is append-only")]
    NotAppendOnly,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub provider: ProviderId,
    pub model: String,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// Fields this version doesn't know, kept so another version's data survives a rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Provider-neutral record of a conversation for display. The provider's own wire-format history is opaque, so
/// the workspace renders this instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TranscriptEntry {
    #[serde(rename_all = "camelCase")]
    Prompt {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fix_errors: bool,
    },
    Reply {
        text: String,
    },
    /// One executed tool call; `error` is set when the tool failed.
    Tool {
        name: String,
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
    },
    Notice {
        kind: NoticeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl TranscriptEntry {
    pub fn notice(kind: NoticeKind) -> Self {
        TranscriptEntry::Notice { kind, detail: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NoticeKind {
    /// The user pressed Stop; the conversation can continue.
    Stopped,
    IterationLimit,
    /// Output hit the token limit before the model finished.
    Truncated,
    Refused,
    /// The API key is missing or was rejected.
    AuthFailed,
    Failed,
    /// The project's provider isn't available in this version of the app.
    ProviderUnavailable,
    /// The project was imported without its conversation; the model starts again from the current files.
    ImportedWithoutConversation,
    /// The user reset the conversation; the model starts again from the current files.
    ConversationReset,
}

/// The provider conversation for one project: append-only `history` plus `pending` input that has not been sent
/// successfully yet (a turn that failed or was stopped resumes from it). `instructions` are the user's standing
/// instructions, `network` the network setting, and `app_name` the name the user gave the app, as last
/// delivered in `history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub history: ProviderHistory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<TurnInput>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_name: String,
    /// The form factor last delivered in `history`; absent means the system prompt's default, a phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_factor: Option<String>,
    /// Fields this version doesn't know, kept so another version's data survives a rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Conversation {
    pub fn new(provider: ProviderId) -> Self {
        Self {
            history: ProviderHistory::new(provider),
            pending: None,
            instructions: String::new(),
            network: false,
            app_name: String::new(),
            form_factor: None,
            extra: Map::new(),
        }
    }
}

pub struct ProjectStore {
    root: PathBuf,
    limits: WorkspaceLimits,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    lock: Mutex<()>,
}

impl ProjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_clock(root, || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        })
    }

    pub fn with_clock(root: impl Into<PathBuf>, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            root: root.into(),
            limits: WorkspaceLimits::default(),
            clock: Box::new(clock),
            lock: Mutex::new(()),
        }
    }

    /// Every readable project, most recently updated first.
    pub fn projects(&self) -> Vec<Project> {
        let _guard = self.guard();
        let mut projects: Vec<Project> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| read_project(&entry.path()))
            .collect();
        projects.sort_by_key(|project| std::cmp::Reverse(project.updated_at_millis));
        projects
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        let _guard = self.guard();
        read_project(&self.dir(id)?)
    }

    pub fn create(&self, name: &str, provider: ProviderId, model: &str) -> Result<Project, StoreError> {
        let _guard = self.guard();
        let now = (self.clock)();
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            provider,
            model: model.to_string(),
            created_at_millis: now,
            updated_at_millis: now,
            extra: Map::new(),
        };
        let dir = self.root.join(&project.id);
        fs::create_dir_all(&dir)?;
        write_atomically(&dir.join(PROJECT), &serde_json::to_string(&project)?)?;
        Ok(project)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), StoreError> {
        let _guard = self.guard();
        self.touch(id, |project| project.name = name.to_string())
    }

    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        let _guard = self.guard();
        match self.dir(id) {
            Some(dir) if dir.is_dir() => Ok(fs::remove_dir_all(dir)?),
            _ => Ok(()),
        }
    }

    /// Forgets the conversation so the model starts again from the site's files; the site stays and the
    /// transcript gains a [`NoticeKind::ConversationReset`] notice.
    pub fn reset_conversation(&self, id: &str) -> Result<(), StoreError> {
        {
            let _guard = self.guard();
            let dir = self.existing_dir(id)?;
            match fs::remove_file(dir.join(CONVERSATION)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        self.append_transcript(id, &TranscriptEntry::notice(NoticeKind::ConversationReset))
    }

    /// The transcript, without lines this version can't read.
    pub fn transcript(&self, id: &str) -> Vec<TranscriptEntry> {
        let _guard = self.guard();
        let text = self
            .dir(id)
            .and_then(|dir| fs::read_to_string(dir.join(TRANSCRIPT)).ok())
            .unwrap_or_default();
        parse_transcript(&text)
    }

    pub fn append_transcript(&self, id: &str, entry: &TranscriptEntry) -> Result<(), StoreError> {
        let _guard = self.guard();
        let Some(dir) = self.dir(id).filter(|dir| dir.is_dir()) else {
            return Ok(());
        };
        let mut file = fs::OpenOptions::new().create(true).append(true).open(dir.join(TRANSCRIPT))?;
        file.write_all(format!("{}\n", serde_json::to_string(entry)?).as_bytes())?;
        self.touch(id, |_| {})
    }

    pub fn conversation(&self, id: &str) -> Result<Option<Conversation>, StoreError> {
        let _guard = self.guard();
        self.read_conversation(id)
    }

    /// Fails with [`StoreError::NotAppendOnly`] if `conversation` does not extend the stored history; a
    /// conversation with a different provider replaces it.
    pub fn save_conversation(&self, id: &str, conversation: &Conversation) -> Result<(), StoreError> {
        let _guard = self.guard();
        let dir = self.existing_dir(id)?;
        if let Some(old) = self.read_conversation(id)? {
            let extends = conversation.history.items.starts_with(&old.history.items);
            let same_provider = old.history.provider == conversation.history.provider;
            if same_provider && !extends {
                return Err(StoreError::NotAppendOnly);
            }
        }
        write_atomically(&dir.join(CONVERSATION), &serde_json::to_string(conversation)?)
    }

    /// Sandboxed file access to one project's `site/` directory.
    pub fn workspace(&self, id: &str) -> Result<Workspace, StoreError> {
        Ok(Workspace::new(self.site_dir(id)?, self.limits.clone()))
    }

    /// The generated app of project `id`, which the runner serves.
    pub fn site_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        Ok(self.existing_dir(id)?.join(SITE))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The project's folder, or `None` for an ID that isn't a plain folder name.
    fn dir(&self, id: &str) -> Option<PathBuf> {
        let plain = !id.is_empty() && !id.starts_with('.') && !id.contains(['/', '\\', ':']);
        plain.then(|| self.root.join(id))
    }

    fn existing_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        self.dir(id)
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| StoreError::UnknownProject(id.to_string()))
    }

    fn read_conversation(&self, id: &str) -> Result<Option<Conversation>, StoreError> {
        let Some(file) = self.dir(id).map(|dir| dir.join(CONVERSATION)).filter(|file| file.is_file()) else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&fs::read_to_string(file)?)?))
    }

    /// Applies `change` and stamps the project as updated now.
    fn touch(&self, id: &str, change: impl FnOnce(&mut Project)) -> Result<(), StoreError> {
        let Some(dir) = self.dir(id) else { return Ok(()) };
        let Some(mut project) = read_project(&dir) else {
            return Ok(());
        };
        change(&mut project);
        project.updated_at_millis = (self.clock)();
        write_atomically(&dir.join(PROJECT), &serde_json::to_string(&project)?)
    }
}

pub(crate) fn parse_transcript(text: &str) -> Vec<TranscriptEntry> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn read_project(dir: &Path) -> Option<Project> {
    let project: Project = serde_json::from_str(&fs::read_to_string(dir.join(PROJECT)).ok()?).ok()?;
    // The folder is the project's identity; a mismatch means the folder was copied or renamed by hand.
    (dir.file_name()?.to_str()? == project.id).then_some(project)
}

fn write_atomically(file: &Path, text: &str) -> Result<(), StoreError> {
    let mut temp = file.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    fs::write(&temp, text)?;
    Ok(fs::rename(&temp, file)?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use serde_json::json;

    use super::*;
    use crate::llm::ToolResult;

    fn store() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let time = Arc::new(AtomicI64::new(1_000));
        let store = ProjectStore::with_clock(dir.path(), move || time.fetch_add(1, Ordering::SeqCst));
        (dir, store)
    }

    #[test]
    fn project_json_matches_the_shared_layout() {
        let (dir, store) = store();
        let project = store
            .create("Tip calculator", ProviderId::Anthropic, "claude-opus-5-5")
            .unwrap();
        let text = fs::read_to_string(dir.path().join(&project.id).join("project.json")).unwrap();
        assert_eq!(
            text,
            format!(
                r#"{{"id":"{}","name":"Tip calculator","provider":"ANTHROPIC","model":"claude-opus-5-5","createdAtMillis":1000,"updatedAtMillis":1000}}"#,
                project.id
            )
        );
    }

    #[test]
    fn transcript_lines_match_the_shared_layout() {
        let entries = [
            (
                TranscriptEntry::Prompt {
                    text: "Hi".into(),
                    fix_errors: false,
                },
                r#"{"type":"prompt","text":"Hi"}"#,
            ),
            (
                TranscriptEntry::Prompt {
                    text: "Err".into(),
                    fix_errors: true,
                },
                r#"{"type":"prompt","text":"Err","fixErrors":true}"#,
            ),
            (
                TranscriptEntry::Reply { text: "Done".into() },
                r#"{"type":"reply","text":"Done"}"#,
            ),
            (
                TranscriptEntry::Tool {
                    name: "list_files".into(),
                    path: None,
                    error: None,
                },
                r#"{"type":"tool","name":"list_files","path":null}"#,
            ),
            (
                TranscriptEntry::Tool {
                    name: "edit_file".into(),
                    path: Some("a.js".into()),
                    error: Some("no".into()),
                },
                r#"{"type":"tool","name":"edit_file","path":"a.js","error":"no"}"#,
            ),
            (
                TranscriptEntry::Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 3,
                },
                r#"{"type":"usage","inputTokens":1,"outputTokens":2,"cacheReadTokens":3}"#,
            ),
            (
                TranscriptEntry::notice(NoticeKind::IterationLimit),
                r#"{"type":"notice","kind":"ITERATION_LIMIT"}"#,
            ),
            (
                TranscriptEntry::Notice {
                    kind: NoticeKind::Failed,
                    detail: Some("why".into()),
                },
                r#"{"type":"notice","kind":"FAILED","detail":"why"}"#,
            ),
        ];
        for (entry, line) in entries {
            assert_eq!(serde_json::to_string(&entry).unwrap(), line);
            assert_eq!(serde_json::from_str::<TranscriptEntry>(line).unwrap(), entry);
        }
    }

    #[test]
    fn conversation_json_matches_the_shared_layout() {
        let mut conversation = Conversation::new(ProviderId::Openai);
        assert_eq!(
            serde_json::to_string(&conversation).unwrap(),
            r#"{"history":{"provider":"OPENAI"}}"#
        );
        conversation.history.items.push(json!({"role": "user"}));
        conversation.pending = Some(TurnInput {
            tool_results: vec![ToolResult {
                call_id: "c1".into(),
                content: "ok".into(),
                is_error: false,
            }],
            text: Some("More".into()),
            resume: false,
        });
        conversation.instructions = "Use large text".into();
        conversation.network = true;
        conversation.app_name = "Tips".into();
        let text = serde_json::to_string(&conversation).unwrap();
        assert_eq!(
            text,
            concat!(
                r#"{"history":{"provider":"OPENAI","items":[{"role":"user"}]},"#,
                r#""pending":{"toolResults":[{"callId":"c1","content":"ok","isError":false}],"text":"More"},"#,
                r#""instructions":"Use large text","network":true,"appName":"Tips"}"#
            )
        );
        assert_eq!(serde_json::from_str::<Conversation>(&text).unwrap(), conversation);
    }

    #[test]
    fn unknown_fields_survive_a_rewrite() {
        let (dir, store) = store();
        let project = store.create("A", ProviderId::Anthropic, "m").unwrap();
        let file = dir.path().join(&project.id).join("conversation.json");
        fs::write(&file, r#"{"history":{"provider":"ANTHROPIC"},"future":{"x":1}}"#).unwrap();
        let conversation = store.conversation(&project.id).unwrap().unwrap();
        store.save_conversation(&project.id, &conversation).unwrap();
        assert!(fs::read_to_string(&file).unwrap().contains(r#""future":{"x":1}"#));
    }

    #[test]
    fn lists_projects_most_recently_updated_first() {
        let (_dir, store) = store();
        let first = store.create("First", ProviderId::Anthropic, "m").unwrap();
        let second = store.create("Second", ProviderId::Openai, "m").unwrap();
        assert_eq!(
            store.projects().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Second", "First"]
        );
        store.rename(&first.id, "Renamed").unwrap();
        assert_eq!(
            store.projects().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Renamed", "Second"]
        );
        store.delete(&second.id).unwrap();
        assert_eq!(store.projects().len(), 1);
        assert_eq!(store.project(&second.id), None);
    }

    #[test]
    fn appending_to_the_transcript_touches_the_project_and_skips_unreadable_lines() {
        let (dir, store) = store();
        let project = store.create("A", ProviderId::Anthropic, "m").unwrap();
        store
            .append_transcript(&project.id, &TranscriptEntry::Reply { text: "one".into() })
            .unwrap();
        let file = dir.path().join(&project.id).join("transcript.jsonl");
        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str("{\"type\":\"from-the-future\"}\n\nnot json\n");
        fs::write(&file, text).unwrap();
        store
            .append_transcript(&project.id, &TranscriptEntry::notice(NoticeKind::Stopped))
            .unwrap();
        assert_eq!(
            store.transcript(&project.id),
            vec![
                TranscriptEntry::Reply { text: "one".into() },
                TranscriptEntry::notice(NoticeKind::Stopped)
            ]
        );
        assert!(store.project(&project.id).unwrap().updated_at_millis > project.updated_at_millis);
        // A deleted project's transcript is silently gone.
        store.delete(&project.id).unwrap();
        store
            .append_transcript(&project.id, &TranscriptEntry::notice(NoticeKind::Stopped))
            .unwrap();
        assert!(store.transcript(&project.id).is_empty());
    }

    #[test]
    fn conversation_history_is_append_only_per_provider() {
        let (_dir, store) = store();
        let project = store.create("A", ProviderId::Anthropic, "m").unwrap();
        assert_eq!(store.conversation(&project.id).unwrap(), None);
        let mut conversation = Conversation::new(ProviderId::Anthropic);
        conversation.history.items = vec![json!(1), json!(2)];
        store.save_conversation(&project.id, &conversation).unwrap();
        conversation.history.items.push(json!(3));
        store.save_conversation(&project.id, &conversation).unwrap();
        conversation.history.items = vec![json!(1), json!(9)];
        assert!(matches!(
            store.save_conversation(&project.id, &conversation),
            Err(StoreError::NotAppendOnly)
        ));
        // Another provider's conversation replaces it.
        store
            .save_conversation(&project.id, &Conversation::new(ProviderId::Openai))
            .unwrap();
        assert!(matches!(
            store.save_conversation("missing", &conversation),
            Err(StoreError::UnknownProject(_))
        ));
    }

    #[test]
    fn resetting_the_conversation_keeps_the_transcript_and_site() {
        let (_dir, store) = store();
        let project = store.create("A", ProviderId::Anthropic, "m").unwrap();
        store
            .append_transcript(&project.id, &TranscriptEntry::Reply { text: "one".into() })
            .unwrap();
        store
            .save_conversation(&project.id, &Conversation::new(ProviderId::Anthropic))
            .unwrap();
        store.reset_conversation(&project.id).unwrap();
        assert_eq!(store.conversation(&project.id).unwrap(), None);
        assert_eq!(
            store.transcript(&project.id),
            vec![
                TranscriptEntry::Reply { text: "one".into() },
                TranscriptEntry::notice(NoticeKind::ConversationReset)
            ]
        );
        store.reset_conversation(&project.id).unwrap();
        assert!(matches!(
            store.reset_conversation("missing"),
            Err(StoreError::UnknownProject(_))
        ));
    }

    #[test]
    fn rejects_ids_that_are_not_plain_folder_names() {
        let (_dir, store) = store();
        for id in ["", "..", "a/b", "a\\b", "c:x"] {
            assert_eq!(store.project(id), None);
            assert!(store.workspace(id).is_err());
        }
    }
}
