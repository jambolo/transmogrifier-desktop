//! Exports and imports projects as zips. A site export holds only the generated app, ready to host anywhere. A
//! backup adds `manifest.json`, `project.json`, `transcript.jsonl`, and `conversation.json`, with the site
//! under `site/`.
//!
//! Imported archives are untrusted: every site file goes through [`Workspace`](super::Workspace), so its path
//! and size limits apply, and a failed import leaves no project behind. The conversation history is copied
//! unchanged.

use std::io::{Cursor, Read, Seek, Write};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use super::store::{Conversation, NoticeKind, Project, ProjectStore, StoreError, TranscriptEntry, parse_transcript};
use crate::llm::ProviderId;

pub const MANIFEST: &str = "manifest.json";
const FORMAT: &str = "transmogrifier-project";
const VERSION: u32 = 1;
const PROJECT: &str = "project.json";
const TRANSCRIPT: &str = "transcript.jsonl";
const CONVERSATION: &str = "conversation.json";
const SITE: &str = "site/";
const INDEX: &str = "index.html";
const MAX_ENTRIES: usize = 1_000;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// An archive that can't be exported or imported; the message says why.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ArchiveError(pub String);

impl ArchiveError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<StoreError> for ArchiveError {
    fn from(error: StoreError) -> Self {
        Self(error.to_string())
    }
}

impl From<std::io::Error> for ArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<zip::result::ZipError> for ArchiveError {
    fn from(error: zip::result::ZipError) -> Self {
        Self(error.to_string())
    }
}

/// Default provider and model for an archive that holds only a site.
#[derive(Debug, Clone)]
pub struct ImportDefaults {
    pub provider: ProviderId,
    pub model: String,
}

/// A zip read into memory and checked, ready to [`import`]. `conversation` is `None` for a site-only archive or
/// a backup made before the project had a conversation.
#[derive(Debug)]
pub struct ArchiveContents {
    pub name: String,
    project: Option<Project>,
    transcript: Vec<TranscriptEntry>,
    conversation: Option<Conversation>,
    site: Vec<(String, String)>,
}

impl ArchiveContents {
    pub fn has_conversation(&self) -> bool {
        self.conversation.is_some()
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
}

/// Writes the generated app alone, at the zip's root.
pub fn export_site(store: &ProjectStore, id: &str, output: impl Write + Seek) -> Result<(), ArchiveError> {
    let mut zip = ZipWriter::new(output);
    for (path, content) in site_files(store, id)? {
        put(&mut zip, &path, &content)?;
    }
    zip.finish()?;
    Ok(())
}

/// Writes the whole project: metadata, transcript, conversation, and the site under `site/`.
pub fn export_backup(store: &ProjectStore, id: &str, output: impl Write + Seek) -> Result<(), ArchiveError> {
    let project = store.project(id).ok_or_else(|| StoreError::UnknownProject(id.to_string()))?;
    let transcript = store.transcript(id);
    let conversation = store.conversation(id)?;
    let files = site_files(store, id)?;
    let json = |error: serde_json::Error| ArchiveError::new(error.to_string());

    let mut zip = ZipWriter::new(output);
    let manifest = Manifest {
        format: FORMAT.to_string(),
        version: VERSION,
    };
    put(&mut zip, MANIFEST, &serde_json::to_string(&manifest).map_err(json)?)?;
    put(&mut zip, PROJECT, &serde_json::to_string(&project).map_err(json)?)?;
    let mut lines = String::new();
    for entry in &transcript {
        lines.push_str(&serde_json::to_string(entry).map_err(json)?);
        lines.push('\n');
    }
    put(&mut zip, TRANSCRIPT, &lines)?;
    if let Some(conversation) = &conversation {
        put(&mut zip, CONVERSATION, &serde_json::to_string(conversation).map_err(json)?)?;
    }
    for (path, content) in files {
        put(&mut zip, &format!("{SITE}{path}"), &content)?;
    }
    zip.finish()?;
    Ok(())
}

/// Reads and checks a backup or a site-only zip. A site-only zip is named `fallback_name`; if all its files are
/// in one folder, that folder becomes the site.
pub fn read(bytes: Vec<u8>, fallback_name: &str) -> Result<ArchiveContents, ArchiveError> {
    let entries = unzip(bytes)?;
    if entries.iter().any(|(name, _)| name == MANIFEST) {
        backup(entries)
    } else {
        site(entries, fallback_name)
    }
}

/// Creates a new project from `contents`. Without `with_conversation`, the model starts a new conversation that
/// sees only the current files, and the transcript says so. On failure no project is left behind.
pub fn import(
    store: &ProjectStore,
    contents: &ArchiveContents,
    defaults: &ImportDefaults,
    with_conversation: bool,
) -> Result<Project, ArchiveError> {
    let source = contents.project.as_ref();
    let project = store.create(
        &contents.name,
        source.map_or(defaults.provider, |p| p.provider),
        source.map_or(defaults.model.as_str(), |p| p.model.as_str()),
    )?;
    let fill = || -> Result<(), ArchiveError> {
        let workspace = store.workspace(&project.id)?;
        for (path, content) in &contents.site {
            workspace.write(path, content).map_err(ArchiveError)?;
        }
        for entry in &contents.transcript {
            store.append_transcript(&project.id, entry)?;
        }
        match &contents.conversation {
            Some(conversation) if with_conversation => store.save_conversation(&project.id, conversation)?,
            _ if !contents.transcript.is_empty() => {
                store.append_transcript(&project.id, &TranscriptEntry::notice(NoticeKind::ImportedWithoutConversation))?
            }
            _ => {}
        }
        Ok(())
    };
    if let Err(error) = fill() {
        let _ = store.delete(&project.id);
        return Err(error);
    }
    Ok(store.project(&project.id).unwrap_or(project))
}

fn site_files(store: &ProjectStore, id: &str) -> Result<Vec<(String, String)>, ArchiveError> {
    let workspace = store.workspace(id)?;
    let files = workspace.list_files();
    Ok(files
        .into_iter()
        .filter_map(|entry| Some((entry.path.clone(), workspace.read(&entry.path).ok()?)))
        .collect())
}

fn backup(entries: Vec<(String, Vec<u8>)>) -> Result<ArchiveContents, ArchiveError> {
    let find = |name: &str| entries.iter().find(|(entry, _)| entry == name).map(|(_, bytes)| text(bytes));
    let manifest: Manifest = decode("manifest", &find(MANIFEST).unwrap_or_default())?;
    if manifest.format != FORMAT {
        return Err(ArchiveError::new("This isn't a Transmogrifier backup"));
    }
    if manifest.version > VERSION {
        return Err(ArchiveError::new("This backup is from a newer version of Transmogrifier"));
    }
    let project_text = find(PROJECT).ok_or_else(|| ArchiveError::new("The backup has no project"))?;
    let project: Project = decode("project", &project_text)?;
    // Lines from a newer version that this one doesn't know are dropped, as the store does.
    let transcript = parse_transcript(&find(TRANSCRIPT).unwrap_or_default());
    let conversation: Option<Conversation> = find(CONVERSATION).map(|text| decode("conversation", &text)).transpose()?;
    if conversation.as_ref().is_some_and(|c| c.history.provider != project.provider) {
        return Err(ArchiveError::new("The backup's conversation doesn't match its AI service"));
    }
    let site = entries
        .iter()
        .filter_map(|(name, bytes)| Some((name.strip_prefix(SITE)?.to_string(), text(bytes))))
        .collect();
    Ok(ArchiveContents {
        name: project.name.clone(),
        project: Some(project),
        transcript,
        conversation,
        site,
    })
}

fn site(entries: Vec<(String, Vec<u8>)>, name: &str) -> Result<ArchiveContents, ArchiveError> {
    let has_index = |entries: &[(String, String)]| entries.iter().any(|(path, _)| path == INDEX);
    let mut files: Vec<(String, String)> = entries.iter().map(|(path, bytes)| (path.clone(), text(bytes))).collect();
    let folder = |path: &str| path.split_once('/').map_or(String::new(), |(first, _)| first.to_string());
    let first = folder(&files[0].0);
    let single_folder = !first.is_empty() && files.iter().all(|(path, _)| folder(path) == first);
    if !has_index(&files) && single_folder {
        let prefix = format!("{first}/");
        for (path, _) in &mut files {
            *path = path.strip_prefix(&prefix).unwrap_or(path).to_string();
        }
    }
    if !has_index(&files) {
        return Err(ArchiveError::new(format!("The zip has no {INDEX}")));
    }
    Ok(ArchiveContents {
        name: name.to_string(),
        project: None,
        transcript: Vec::new(),
        conversation: None,
        site: files,
    })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, ArchiveError> {
    serde_json::from_str(text).map_err(|_| ArchiveError::new(format!("The backup's {what} is damaged or from a newer version")))
}

/// Reads every file, skipping folders and junk, within [`MAX_ENTRIES`] and [`MAX_TOTAL_BYTES`].
fn unzip(bytes: Vec<u8>) -> Result<Vec<(String, Vec<u8>)>, ArchiveError> {
    let unreadable = |_| ArchiveError::new("This isn't a readable zip file");
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(unreadable)?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(unreadable)?;
        let name = file.name().map_err(unreadable)?.to_string();
        if file.is_dir() || is_junk(&name) {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            return Err(ArchiveError::new("The zip has too many files"));
        }
        if entries.iter().any(|(existing, _)| *existing == name) {
            return Err(ArchiveError::new(format!("The zip repeats {name}")));
        }
        // The declared size is untrusted, so the limit is applied to the bytes actually read.
        let mut content = Vec::new();
        file.take(MAX_TOTAL_BYTES - total + 1)
            .read_to_end(&mut content)
            .map_err(|e| unreadable(e.into()))?;
        total += content.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(ArchiveError::new("The zip is too large"));
        }
        entries.push((name, content));
    }
    if entries.is_empty() {
        return Err(ArchiveError::new("The zip is empty or isn't a zip file"));
    }
    Ok(entries)
}

/// Files that operating systems add to zips, which no site needs. `.` and `..` aren't junk: the workspace
/// rejects them along with the import.
fn is_junk(name: &str) -> bool {
    name.split('/')
        .any(|part| part == "__MACOSX" || part == "Thumbs.db" || (part.starts_with('.') && part != "." && part != ".."))
}

fn put<W: Write + Seek>(zip: &mut ZipWriter<W>, path: &str, content: &str) -> Result<(), ArchiveError> {
    zip.start_file(path, SimpleFileOptions::default())?;
    zip.write_all(content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn store() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::new(dir.path());
        (dir, store)
    }

    fn defaults() -> ImportDefaults {
        ImportDefaults {
            provider: ProviderId::Openai,
            model: "gpt-6.1-sol".into(),
        }
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, content) in files {
            put(&mut zip, path, content).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn names(bytes: Vec<u8>) -> Vec<String> {
        unzip(bytes).unwrap().into_iter().map(|(name, _)| name).collect()
    }

    fn sample(store: &ProjectStore) -> Project {
        let project = store.create("Tips", ProviderId::Anthropic, "claude-opus-5-5").unwrap();
        let workspace = store.workspace(&project.id).unwrap();
        workspace.write("index.html", "<h1>Tips</h1>").unwrap();
        workspace.write("js/app.js", "let a;").unwrap();
        store
            .append_transcript(
                &project.id,
                &TranscriptEntry::Prompt {
                    text: "A tip app".into(),
                    fix_errors: false,
                },
            )
            .unwrap();
        let mut conversation = Conversation::new(ProviderId::Anthropic);
        conversation.history.items.push(json!({"role": "user", "content": []}));
        store.save_conversation(&project.id, &conversation).unwrap();
        project
    }

    #[test]
    fn site_export_holds_only_the_site_at_the_root() {
        let (_dir, store) = store();
        let project = sample(&store);
        let mut output = Cursor::new(Vec::new());
        export_site(&store, &project.id, &mut output).unwrap();
        assert_eq!(names(output.into_inner()), ["index.html", "js/app.js"]);
    }

    #[test]
    fn backup_round_trips_with_and_without_the_conversation() {
        let (_dir, store) = store();
        let project = sample(&store);
        let mut output = Cursor::new(Vec::new());
        export_backup(&store, &project.id, &mut output).unwrap();
        let bytes = output.into_inner();
        assert_eq!(
            names(bytes.clone()),
            [
                "manifest.json",
                "project.json",
                "transcript.jsonl",
                "conversation.json",
                "site/index.html",
                "site/js/app.js"
            ]
        );

        let contents = read(bytes, "ignored").unwrap();
        assert!(contents.has_conversation());
        let with = import(&store, &contents, &defaults(), true).unwrap();
        assert_ne!(with.id, project.id);
        assert_eq!(
            (with.name.as_str(), with.provider, with.model.as_str()),
            ("Tips", ProviderId::Anthropic, "claude-opus-5-5")
        );
        assert_eq!(store.workspace(&with.id).unwrap().read("js/app.js").unwrap(), "let a;");
        assert_eq!(
            store.conversation(&with.id).unwrap(),
            store.conversation(&project.id).unwrap()
        );
        assert_eq!(store.transcript(&with.id), store.transcript(&project.id));

        let without = import(&store, &contents, &defaults(), false).unwrap();
        assert_eq!(store.conversation(&without.id).unwrap(), None);
        assert_eq!(
            store.transcript(&without.id).last(),
            Some(&TranscriptEntry::notice(NoticeKind::ImportedWithoutConversation))
        );
    }

    #[test]
    fn site_only_zip_uses_the_file_name_and_defaults() {
        let (_dir, store) = store();
        let contents = read(zip_of(&[("index.html", "x"), ("css/a.css", "y")]), "My site").unwrap();
        assert!(!contents.has_conversation());
        let project = import(&store, &contents, &defaults(), true).unwrap();
        assert_eq!((project.name.as_str(), project.provider), ("My site", ProviderId::Openai));
        assert!(store.transcript(&project.id).is_empty());
    }

    #[test]
    fn a_single_folder_becomes_the_site_and_junk_is_skipped() {
        let bytes = zip_of(&[
            ("app/index.html", "x"),
            ("app/js/a.js", "y"),
            ("__MACOSX/app/._index.html", "junk"),
            ("app/.DS_Store", "junk"),
        ]);
        let contents = read(bytes, "App").unwrap();
        let paths: Vec<&str> = contents.site.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["index.html", "js/a.js"]);
    }

    #[test]
    fn rejects_bad_archives_with_readable_reasons() {
        let message = |bytes: Vec<u8>| read(bytes, "x").unwrap_err().0;
        assert_eq!(message(b"not a zip".to_vec()), "This isn't a readable zip file");
        assert_eq!(message(zip_of(&[(".hidden", "x")])), "The zip is empty or isn't a zip file");
        assert_eq!(message(zip_of(&[("a.html", "x")])), "The zip has no index.html");
        assert_eq!(
            message(zip_of(&[("manifest.json", r#"{"format":"other","version":1}"#)])),
            "This isn't a Transmogrifier backup"
        );
        assert_eq!(
            message(zip_of(&[(
                "manifest.json",
                r#"{"format":"transmogrifier-project","version":2}"#
            )])),
            "This backup is from a newer version of Transmogrifier"
        );
        let manifest = ("manifest.json", r#"{"format":"transmogrifier-project","version":1}"#);
        assert_eq!(message(zip_of(&[manifest])), "The backup has no project");
        assert_eq!(
            message(zip_of(&[manifest, ("project.json", "{}")])),
            "The backup's project is damaged or from a newer version"
        );
        let project = r#"{"id":"1","name":"A","provider":"OPENAI","model":"m","createdAtMillis":1,"updatedAtMillis":1}"#;
        assert_eq!(
            message(zip_of(&[
                manifest,
                ("project.json", project),
                ("conversation.json", r#"{"history":{"provider":"ANTHROPIC"}}"#)
            ])),
            "The backup's conversation doesn't match its AI service"
        );
    }

    #[test]
    fn a_failed_import_leaves_no_project_behind() {
        let (_dir, store) = store();
        let contents = read(zip_of(&[("index.html", "x"), ("app.exe", "y")]), "Bad").unwrap();
        let error = import(&store, &contents, &defaults(), false).unwrap_err();
        assert!(error.0.starts_with("Extension not allowed"), "{error}");
        assert!(store.projects().is_empty());
    }
}
