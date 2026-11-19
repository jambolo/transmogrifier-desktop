//! The system prompt and tools. Both must stay byte-identical between requests and between every
//! implementation of Transmogrifier: they form the cached prompt prefix, and Claude's thinking blocks are only
//! valid while the prefix is unchanged. Anything that varies rides on the user text instead (see `engine`).

use serde_json::{Map, Value, json};

use crate::llm::ToolDefinition;

#[rustfmt::skip]
pub const SYSTEM: &str = "\
You build small browser apps that run inside an Android app's built-in browser. The user describes an app or a\n\
change in plain language; you implement it by editing the app's files with the tools provided.\n\
\n\
The app is a static site:\n\
- The entry point is index.html. Use relative paths only. There is no build step and no package manager.\n\
- Plain HTML, CSS, and JavaScript; ES modules are fine.\n\
- The app is self-contained: no remote scripts, stylesheets, fonts, or network requests. They are blocked.\n\
- Allowed file types: .html, .css, .js, .mjs, .json, .svg, .txt, .md. Keep files small and focused.\n\
- Persist user data in localStorage.\n\
\n\
Design for a phone held in portrait: touch-friendly controls at least 44 px tall, readable text, no hover-only\n\
interactions, and layouts that fit the screen width. Respect prefers-color-scheme. Don't use alert(), confirm(),\n\
or prompt() as part of the app's flow; build the UI in the page instead.\n\
\n\
Work in small steps and keep index.html runnable after every change. Read a file before editing it unless you\n\
wrote it in this conversation. Prefer edit_file for small changes and write_file for new files or rewrites.\n\
\n\
Before your first tool call, say in one sentence what you're about to do. When you finish, summarize what\n\
changed in two or three short sentences for someone who can't see the code.";

pub fn tools() -> Vec<ToolDefinition> {
    const PATH: (&str, &str) = ("path", "File path relative to the app root, for example css/style.css.");
    vec![
        tool("list_files", "List every file in the app with its size in bytes.", &[]),
        tool("read_file", "Read a file's full text.", &[PATH]),
        tool(
            "write_file",
            "Create a file or replace its entire contents.",
            &[PATH, ("content", "The complete new file contents.")],
        ),
        tool(
            "edit_file",
            "Replace exactly one occurrence of old_text with new_text in a file. Fails if old_text is missing or \
             appears more than once; include enough surrounding text to make it unique.",
            &[
                PATH,
                ("old_text", "Exact text to replace, including whitespace."),
                ("new_text", "Replacement text."),
            ],
        ),
        tool("delete_file", "Delete a file.", &[PATH]),
    ]
}

/// A tool whose parameters are all required strings, in the order given.
fn tool(name: &'static str, description: &'static str, parameters: &[(&str, &str)]) -> ToolDefinition {
    let mut properties = Map::new();
    for (parameter, description) in parameters {
        properties.insert(parameter.to_string(), json!({"type": "string", "description": description}));
    }
    let required: Vec<&str> = parameters.iter().map(|(parameter, _)| *parameter).collect();
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert("required".into(), json!(required));
    schema.insert("additionalProperties".into(), json!(false));
    ToolDefinition {
        name,
        description,
        input_schema: Value::Object(schema),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    // The shared spec, pinned. A failure here means the prompt prefix no longer matches the other
    // implementations: change them together, never one alone.
    #[test]
    fn system_prompt_is_byte_identical_to_the_shared_spec() {
        assert_eq!(SYSTEM.len(), 1337);
        assert_eq!(fnv1a(SYSTEM.as_bytes()), 0x7545_1636_4650_feab);
        assert!(SYSTEM.starts_with("You build small browser apps"));
        assert!(SYSTEM.ends_with("for someone who can't see the code."));
    }

    #[test]
    fn tool_definitions_are_byte_identical_to_the_shared_spec() {
        let rendered: Vec<String> = tools()
            .iter()
            .map(|tool| format!("{}|{}|{}", tool.name, tool.description, tool.input_schema))
            .collect();
        let path = r#""path":{"type":"string","description":"File path relative to the app root, for example css/style.css."}"#;
        assert_eq!(
            rendered,
            [
                r#"list_files|List every file in the app with its size in bytes.|{"type":"object","properties":{},"required":[],"additionalProperties":false}"#.to_string(),
                format!(r#"read_file|Read a file's full text.|{{"type":"object","properties":{{{path}}},"required":["path"],"additionalProperties":false}}"#),
                format!(r#"write_file|Create a file or replace its entire contents.|{{"type":"object","properties":{{{path},"content":{{"type":"string","description":"The complete new file contents."}}}},"required":["path","content"],"additionalProperties":false}}"#),
                format!(r#"edit_file|Replace exactly one occurrence of old_text with new_text in a file. Fails if old_text is missing or appears more than once; include enough surrounding text to make it unique.|{{"type":"object","properties":{{{path},"old_text":{{"type":"string","description":"Exact text to replace, including whitespace."}},"new_text":{{"type":"string","description":"Replacement text."}}}},"required":["path","old_text","new_text"],"additionalProperties":false}}"#),
                format!(r#"delete_file|Delete a file.|{{"type":"object","properties":{{{path}}},"required":["path"],"additionalProperties":false}}"#),
            ]
        );
    }
}
