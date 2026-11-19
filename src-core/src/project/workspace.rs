//! Sandboxed text-file access to one generated site.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLimits {
    pub max_path_length: usize,
    pub max_depth: usize,
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    /// Sorted, lower case.
    pub allowed_extensions: Vec<&'static str>,
}

impl Default for WorkspaceLimits {
    fn default() -> Self {
        Self {
            max_path_length: 200,
            max_depth: 6,
            max_files: 100,
            max_file_bytes: 512 * 1024,
            max_total_bytes: 4 * 1024 * 1024,
            allowed_extensions: vec!["css", "html", "js", "json", "md", "mjs", "svg", "txt"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub size_bytes: u64,
}

/// Rejections are values with a message, so they can go back to the model as error tool results.
pub type WorkspaceResult<T> = Result<T, String>;

/// Sandboxed text-file access to one generated site. Paths are relative and `/`-separated.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    limits: WorkspaceLimits,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>, limits: WorkspaceLimits) -> Self {
        Self {
            root: root.into(),
            limits,
        }
    }

    pub fn list_files(&self) -> Vec<FileEntry> {
        let mut entries = Vec::new();
        if let Ok(root) = self.root.canonicalize() {
            walk(&root, &root, &mut entries);
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn read(&self, path: &str) -> WorkspaceResult<String> {
        let file = self.resolve(path)?;
        if !file.is_file() {
            return Err(format!("File not found: {path}"));
        }
        let bytes = fs::read(&file).map_err(|e| format!("Could not read {path}: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn write(&self, path: &str, content: &str) -> WorkspaceResult<()> {
        let file = self.resolve(path)?;
        let size = content.len() as u64;
        let existing = self.list_files();
        let old_size = if file.is_file() {
            fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let total: u64 = existing.iter().map(|e| e.size_bytes).sum();
        if file.is_dir() {
            Err(format!("Path is a directory: {path}"))
        } else if size > self.limits.max_file_bytes {
            Err(format!("File exceeds {} bytes: {path}", self.limits.max_file_bytes))
        } else if !file.is_file() && existing.len() >= self.limits.max_files {
            Err(format!("Too many files (limit {})", self.limits.max_files))
        } else if total - old_size + size > self.limits.max_total_bytes {
            Err(format!("Site exceeds {} bytes in total", self.limits.max_total_bytes))
        } else {
            let write = || {
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&file, content)
            };
            write().map_err(|e| format!("Could not write {path}: {e}"))
        }
    }

    /// Replaces exactly one occurrence of `old_text`.
    pub fn edit(&self, path: &str, old_text: &str, new_text: &str) -> WorkspaceResult<()> {
        if old_text.is_empty() {
            return Err("old_text must not be empty".to_string());
        }
        let content = self.read(path)?;
        let Some(first) = content.find(old_text) else {
            return Err(format!("old_text not found in {path}"));
        };
        // Overlapping occurrences count, so the search resumes one character after the first match.
        let next = first + content[first..].chars().next().map_or(1, char::len_utf8);
        if content[next..].contains(old_text) {
            return Err(format!("old_text matches more than once in {path}"));
        }
        let mut edited = String::with_capacity(content.len() + new_text.len());
        edited.push_str(&content[..first]);
        edited.push_str(new_text);
        edited.push_str(&content[first + old_text.len()..]);
        self.write(path, &edited)
    }

    pub fn delete(&self, path: &str) -> WorkspaceResult<()> {
        let file = self.resolve(path)?;
        if !file.is_file() {
            return Err(format!("File not found: {path}"));
        }
        fs::remove_file(&file).map_err(|e| format!("Could not delete {path}: {e}"))?;
        let root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        let mut dir = file.parent();
        while let Some(current) = dir.filter(|d| *d != root && d.starts_with(&root)) {
            // Fails, ending the walk, once a folder still has something in it.
            if fs::remove_dir(current).is_err() {
                break;
            }
            dir = current.parent();
        }
        Ok(())
    }

    /// Checks `path` against the rules and returns the file it names inside the site directory.
    pub(crate) fn resolve(&self, path: &str) -> WorkspaceResult<PathBuf> {
        let limits = &self.limits;
        let segments: Vec<&str> = path.split('/').collect();
        let name = segments.last().copied().unwrap_or_default();
        let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_lowercase()).unwrap_or_default();
        let reason = if path.is_empty() {
            Some("Path is empty".to_string())
        } else if path.encode_utf16().count() > limits.max_path_length {
            Some(format!("Path longer than {} characters", limits.max_path_length))
        } else if path.starts_with('/') {
            Some("Absolute paths are not allowed".to_string())
        } else if path.chars().any(|c| c == '\\' || c == ':' || c.is_control()) {
            Some("Path contains a disallowed character".to_string())
        } else if segments.iter().any(|s| s.is_empty() || s.starts_with('.')) {
            Some("Path segments must not be empty or start with '.'".to_string())
        } else if segments.len() > limits.max_depth {
            Some(format!("Path deeper than {} levels", limits.max_depth))
        } else if !limits.allowed_extensions.contains(&extension.as_str()) {
            Some(format!(
                "Extension not allowed; use one of [{}]",
                limits.allowed_extensions.join(", ")
            ))
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(format!("{reason}: {path}"));
        }
        let escapes = || format!("Path escapes the site directory: {path}");
        fs::create_dir_all(&self.root).map_err(|_| escapes())?;
        let root = self.root.canonicalize().map_err(|_| escapes())?;
        // Defense in depth against symlinks inside the site directory: resolve the part that exists.
        let mut file = root.clone();
        let mut exists = true;
        for segment in segments {
            file.push(segment);
            if exists {
                match file.canonicalize() {
                    Ok(resolved) => file = resolved,
                    Err(_) => exists = false,
                }
            }
        }
        if file.starts_with(&root) { Ok(file) } else { Err(escapes()) }
    }
}

fn walk(root: &Path, dir: &Path, entries: &mut Vec<FileEntry>) {
    let Ok(children) = fs::read_dir(dir) else { return };
    for child in children.flatten() {
        let path = child.path();
        let Ok(metadata) = fs::metadata(&path) else { continue };
        if metadata.is_dir() {
            walk(root, &path, entries);
        } else if metadata.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            let parts: Vec<String> = relative.iter().map(|part| part.to_string_lossy().into_owned()).collect();
            entries.push(FileEntry {
                path: parts.join("/"),
                size_bytes: metadata.len(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(limits: WorkspaceLimits) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path().join("site"), limits);
        (dir, workspace)
    }

    fn default_workspace() -> (tempfile::TempDir, Workspace) {
        workspace(WorkspaceLimits::default())
    }

    #[test]
    fn writes_reads_and_lists_files() {
        let (_dir, ws) = default_workspace();
        ws.write("index.html", "<h1>Hi</h1>").unwrap();
        ws.write("js/app.js", "let a = 1;").unwrap();
        assert_eq!(ws.read("index.html").unwrap(), "<h1>Hi</h1>");
        assert_eq!(
            ws.list_files(),
            vec![
                FileEntry {
                    path: "index.html".into(),
                    size_bytes: 11
                },
                FileEntry {
                    path: "js/app.js".into(),
                    size_bytes: 10
                },
            ]
        );
    }

    #[test]
    fn rejects_bad_paths_with_the_shared_messages() {
        let (_dir, ws) = default_workspace();
        let long = format!("{}.html", "a".repeat(200));
        let cases = [
            ("", "Path is empty: "),
            (long.as_str(), "Path longer than 200 characters"),
            ("/etc/passwd.txt", "Absolute paths are not allowed: /etc/passwd.txt"),
            ("a\\b.html", "Path contains a disallowed character: a\\b.html"),
            ("c:a.html", "Path contains a disallowed character: c:a.html"),
            ("../a.html", "Path segments must not be empty or start with '.': ../a.html"),
            ("a//b.html", "Path segments must not be empty or start with '.': a//b.html"),
            (
                ".hidden.html",
                "Path segments must not be empty or start with '.': .hidden.html",
            ),
            ("a/b/c/d/e/f/g.html", "Path deeper than 6 levels: a/b/c/d/e/f/g.html"),
            (
                "app.exe",
                "Extension not allowed; use one of [css, html, js, json, md, mjs, svg, txt]: app.exe",
            ),
            (
                "noextension",
                "Extension not allowed; use one of [css, html, js, json, md, mjs, svg, txt]: noextension",
            ),
        ];
        for (path, message) in cases {
            let error = ws.write(path, "x").unwrap_err();
            assert!(error.starts_with(message), "{path:?} gave {error:?}");
        }
        assert!(ws.list_files().is_empty());
    }

    #[test]
    fn accepts_upper_case_extensions_and_the_deepest_path() {
        let (_dir, ws) = default_workspace();
        ws.write("README.MD", "x").unwrap();
        ws.write("a/b/c/d/e/f.html", "x").unwrap();
    }

    #[test]
    fn enforces_size_and_count_limits() {
        let limits = WorkspaceLimits {
            max_files: 2,
            max_file_bytes: 10,
            max_total_bytes: 15,
            ..Default::default()
        };
        let (_dir, ws) = workspace(limits);
        assert_eq!(ws.write("a.txt", "12345678901").unwrap_err(), "File exceeds 10 bytes: a.txt");
        ws.write("a.txt", "1234567890").unwrap();
        assert_eq!(ws.write("b.txt", "123456").unwrap_err(), "Site exceeds 15 bytes in total");
        ws.write("b.txt", "12345").unwrap();
        assert_eq!(ws.write("c.txt", "").unwrap_err(), "Too many files (limit 2)");
        // Replacing a file counts its old size as freed.
        ws.write("a.txt", "123456789").unwrap();
    }

    #[test]
    fn rejects_writing_over_a_directory() {
        let (_dir, ws) = default_workspace();
        ws.write("docs.md/a.txt", "x").unwrap();
        assert_eq!(ws.write("docs.md", "x").unwrap_err(), "Path is a directory: docs.md");
    }

    #[test]
    fn edit_replaces_exactly_one_occurrence() {
        let (_dir, ws) = default_workspace();
        ws.write("a.txt", "one two two").unwrap();
        ws.edit("a.txt", "one", "1").unwrap();
        assert_eq!(ws.read("a.txt").unwrap(), "1 two two");
        assert_eq!(
            ws.edit("a.txt", "two", "2").unwrap_err(),
            "old_text matches more than once in a.txt"
        );
        assert_eq!(ws.edit("a.txt", "three", "3").unwrap_err(), "old_text not found in a.txt");
        assert_eq!(ws.edit("a.txt", "", "3").unwrap_err(), "old_text must not be empty");
        assert_eq!(ws.edit("b.txt", "x", "y").unwrap_err(), "File not found: b.txt");
    }

    #[test]
    fn edit_counts_overlapping_matches() {
        let (_dir, ws) = default_workspace();
        ws.write("a.txt", "aaa").unwrap();
        assert_eq!(
            ws.edit("a.txt", "aa", "b").unwrap_err(),
            "old_text matches more than once in a.txt"
        );
    }

    #[test]
    fn delete_removes_empty_parent_folders() {
        let (dir, ws) = default_workspace();
        ws.write("a/b/c.txt", "x").unwrap();
        ws.write("a/d.txt", "x").unwrap();
        ws.delete("a/b/c.txt").unwrap();
        assert!(!dir.path().join("site/a/b").exists());
        assert!(dir.path().join("site/a").exists());
        ws.delete("a/d.txt").unwrap();
        assert!(!dir.path().join("site/a").exists());
        assert!(dir.path().join("site").exists());
        assert_eq!(ws.delete("a/d.txt").unwrap_err(), "File not found: a/d.txt");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_leave_the_site() {
        let (dir, ws) = default_workspace();
        ws.write("index.html", "x").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("site/link")).unwrap();
        assert_eq!(
            ws.write("link/a.txt", "x").unwrap_err(),
            "Path escapes the site directory: link/a.txt"
        );
        assert!(!outside.join("a.txt").exists());
    }
}
