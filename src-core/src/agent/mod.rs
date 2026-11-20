//! Generation: the loop, the system prompt, and the tools the model edits a site with.

mod engine;
pub mod prompt;
mod tools;

pub use engine::{AgentEngine, Connector, EventSink, GenerationEvent};
pub use tools::{ToolExecutor, path_from_partial_input};
