//! Validates tool input against its schema and runs it on the workspace.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::prompt;
use crate::llm::{ToolCall, ToolResult};
use crate::project::Workspace;

static PATH_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""path"\s*:\s*"((?:\\.|[^"\\])*)""#).unwrap());

/// Validates tool input against its schema and runs it on the [`Workspace`]. Every failure, including input
/// that is not valid JSON, becomes an error result for the model rather than a failure of the generation.
pub struct ToolExecutor {
    workspace: Workspace,
    /// Each tool's required string parameters, in schema order.
    schemas: Vec<(&'static str, Vec<String>)>,
}

impl ToolExecutor {
    pub fn new(workspace: Workspace) -> Self {
        let schemas = prompt::tools()
            .into_iter()
            .map(|tool| {
                let required = tool.input_schema["required"].as_array().into_iter().flatten();
                (
                    tool.name,
                    required.filter_map(|name| name.as_str().map(str::to_string)).collect(),
                )
            })
            .collect();
        Self { workspace, schemas }
    }

    pub fn execute(&self, call: &ToolCall) -> ToolResult {
        let outcome = self.parse(call).and_then(|input| self.run(&call.name, &input));
        match outcome {
            Ok(content) => ToolResult {
                call_id: call.call_id.clone(),
                content,
                is_error: false,
            },
            Err(content) => ToolResult {
                call_id: call.call_id.clone(),
                content,
                is_error: true,
            },
        }
    }

    fn run(&self, name: &str, input: &HashMap<String, String>) -> Result<String, String> {
        let arg = |name: &str| input.get(name).map(String::as_str).unwrap_or_default();
        let path = arg("path");
        match name {
            "list_files" => {
                let files = self.workspace.list_files();
                let lines: Vec<String> = files
                    .iter()
                    .map(|file| format!("{} ({} bytes)", file.path, file.size_bytes))
                    .collect();
                Ok(if lines.is_empty() {
                    "No files yet.".to_string()
                } else {
                    lines.join("\n")
                })
            }
            "read_file" => self.workspace.read(path),
            "write_file" => self.workspace.write(path, arg("content")).map(|()| format!("Wrote {path}")),
            "edit_file" => self
                .workspace
                .edit(path, arg("old_text"), arg("new_text"))
                .map(|()| format!("Edited {path}")),
            "delete_file" => self.workspace.delete(path).map(|()| format!("Deleted {path}")),
            _ => Err(format!("Unknown tool {name}")),
        }
    }

    fn parse(&self, call: &ToolCall) -> Result<HashMap<String, String>, String> {
        let required = self
            .schemas
            .iter()
            .find(|(name, _)| *name == call.name)
            .map(|(_, required)| required)
            .ok_or_else(|| format!("Unknown tool {}", call.name))?;
        // Built with the JSON library so quotes in the bad input are escaped.
        let invalid = || json!({"INVALID_JSON": call.input_json}).to_string();
        let parsed: Value = serde_json::from_str(&call.input_json).map_err(|_| invalid())?;
        let object = parsed.as_object().ok_or_else(invalid)?;
        let unexpected: Vec<&str> = object
            .keys()
            .map(String::as_str)
            .filter(|key| !required.iter().any(|r| r == key))
            .collect();
        let missing: Vec<&str> = required
            .iter()
            .map(String::as_str)
            .filter(|name| !object.get(*name).is_some_and(Value::is_string))
            .collect();
        if !unexpected.is_empty() {
            Err(format!("Unexpected parameters: {}", unexpected.join(", ")))
        } else if !missing.is_empty() {
            Err(format!("Missing string parameters: {}", missing.join(", ")))
        } else {
            Ok(object
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect())
        }
    }
}

/// Best-effort path from a tool call's partial input JSON, for the live activity log.
pub fn path_from_partial_input(partial_json: &str) -> Option<String> {
    let escaped = PATH_PATTERN.captures(partial_json)?.get(1)?.as_str();
    serde_json::from_str(&format!("\"{escaped}\"")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::WorkspaceLimits;

    fn executor() -> (tempfile::TempDir, ToolExecutor) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path().join("site"), WorkspaceLimits::default());
        (dir, ToolExecutor::new(workspace))
    }

    fn call(name: &str, input: &str) -> ToolCall {
        ToolCall {
            call_id: "c1".into(),
            name: name.into(),
            input_json: input.into(),
        }
    }

    fn run(executor: &ToolExecutor, name: &str, input: &str) -> (String, bool) {
        let result = executor.execute(&call(name, input));
        assert_eq!(result.call_id, "c1");
        (result.content, result.is_error)
    }

    #[test]
    fn runs_every_tool_with_the_shared_result_text() {
        let (_dir, ex) = executor();
        assert_eq!(run(&ex, "list_files", "{}"), ("No files yet.".into(), false));
        assert_eq!(
            run(&ex, "write_file", r#"{"path":"index.html","content":"<h1>Hi</h1>"}"#),
            ("Wrote index.html".into(), false)
        );
        assert_eq!(
            run(&ex, "read_file", r#"{"path":"index.html"}"#),
            ("<h1>Hi</h1>".into(), false)
        );
        assert_eq!(
            run(&ex, "edit_file", r#"{"path":"index.html","old_text":"Hi","new_text":"Yo"}"#),
            ("Edited index.html".into(), false)
        );
        assert_eq!(run(&ex, "list_files", "{}"), ("index.html (11 bytes)".into(), false));
        assert_eq!(
            run(&ex, "delete_file", r#"{"path":"index.html"}"#),
            ("Deleted index.html".into(), false)
        );
    }

    #[test]
    fn failures_become_error_results() {
        let (_dir, ex) = executor();
        assert_eq!(
            run(&ex, "read_file", r#"{"path":"a.js"}"#),
            ("File not found: a.js".into(), true)
        );
        assert_eq!(run(&ex, "shell", "{}"), ("Unknown tool shell".into(), true));
        assert_eq!(
            run(&ex, "read_file", r#"{"path": "a"#),
            (r#"{"INVALID_JSON":"{\"path\": \"a"}"#.into(), true)
        );
        assert_eq!(run(&ex, "read_file", "[]"), (r#"{"INVALID_JSON":"[]"}"#.into(), true));
        assert_eq!(
            run(&ex, "read_file", r#"{"path":"a.js","mode":"r","x":1}"#),
            ("Unexpected parameters: mode, x".into(), true)
        );
        assert_eq!(
            run(&ex, "edit_file", r#"{"path":"a.js","old_text":1}"#),
            ("Missing string parameters: old_text, new_text".into(), true)
        );
    }

    #[test]
    fn finds_the_path_in_partial_input() {
        assert_eq!(
            path_from_partial_input(r#"{"path": "js/a\"b.js", "content": "<h"#).as_deref(),
            Some("js/a\"b.js")
        );
        assert_eq!(path_from_partial_input(r#"{"path": "js/a"#), None);
        assert_eq!(path_from_partial_input("{}"), None);
    }
}
