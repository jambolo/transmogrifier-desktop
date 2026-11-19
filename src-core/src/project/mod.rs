//! Project storage: files, metadata, conversation history, and zip export/import.

pub mod archive;
mod store;
mod workspace;

pub use store::{Conversation, NoticeKind, Project, ProjectStore, StoreError, TranscriptEntry};
pub use workspace::{FileEntry, Workspace, WorkspaceLimits, WorkspaceResult};
