//! Transmogrifier's core: everything the app does that isn't a window.
//!
//! This crate has no Tauri dependency. The shell (`src-tauri`) owns the windows and exposes these modules to
//! the web UI as commands.
//!
//! - [`llm`]: the provider-neutral [`llm::LlmProvider`] interface and an adapter for each wire format.
//! - [`agent`]: the generation loop, the system prompt, and the tool definitions and executor.
//! - [`project`]: project storage (files, metadata, conversation history) and zip export/import.
//! - [`settings`]: the connections to AI services, limits, and API-key storage.
//! - [`runtime`]: serving a generated app to its window and collecting its console.

pub mod agent;
pub mod llm;
pub mod project;
pub mod runtime;
pub mod settings;
