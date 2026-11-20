//! Provider-neutral LLM types. Everything outside `llm` depends only on these; each provider's wire format is
//! confined to its own submodule.

pub mod anthropic;
pub mod chat_completions;
pub mod gemini;
mod http;
pub mod openai;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The supported wire formats. A project records the one its conversation is stored in; which service speaks
/// it, and where, is a connection in the user's settings. Adding one takes a variant here, an [`LlmProvider`]
/// adapter in its own `llm` submodule, its arm in [`adapter`], and a label in the front end's `strings.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Gemini,
    /// Any server that speaks the Chat Completions format, hosted or local, at an address the user saves.
    OpenaiCompatible,
}

impl ProviderId {
    pub const ALL: [ProviderId; 4] = [
        ProviderId::Anthropic,
        ProviderId::Openai,
        ProviderId::Gemini,
        ProviderId::OpenaiCompatible,
    ];

    /// The model for new apps when the user hasn't chosen one; empty where only the user can know it.
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderId::Anthropic => "claude-opus-5-5",
            ProviderId::Openai => "gpt-6.1-sol",
            ProviderId::Gemini => "gemini-4-carbon",
            ProviderId::OpenaiCompatible => "",
        }
    }

    /// Whether the provider is reached at an address the user saves. Such a provider needs no API key, and
    /// the user can save any number of connections to it.
    pub fn uses_endpoint(self) -> bool {
        matches!(self, ProviderId::OpenaiCompatible)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Effort {
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// The value Claude and OpenAI use on the wire.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// Provider-neutral tool definition; each provider maps it to its own wire format.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Conversation history in the provider's own wire format. Opaque outside the provider and only ever appended
/// to: Claude requires thinking blocks to round-trip unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderHistory {
    pub provider: ProviderId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Value>,
}

impl ProviderHistory {
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            items: Vec::new(),
        }
    }
}

/// The next user turn: results for every tool call of the previous assistant turn (which must come first), then
/// optional text. `resume` adds nothing: it continues a turn the provider paused ([`StopReason::Paused`]).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume: bool,
}

impl TurnInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    pub fn results(tool_results: Vec<ToolResult>) -> Self {
        Self {
            tool_results,
            ..Self::default()
        }
    }

    pub fn resume() -> Self {
        Self {
            resume: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub model: String,
    pub effort: Effort,
    pub max_output_tokens: u32,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub history: ProviderHistory,
    pub input: TurnInput,
}

/// `Paused`: a long server-side tool loop (web search) stopped early; resend the history to continue it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    Paused,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// A complete tool call, reported after the turn's output has finished streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub input_json: String,
}

/// Live progress of one streamed turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    TextDelta(String),
    /// A tool call began streaming; its input follows as `ToolInputDelta`s.
    ToolCallStarted {
        call_id: String,
        name: String,
    },
    ToolInputDelta {
        call_id: String,
        partial_json: String,
    },
}

/// A finished turn. `history` is the request history with this turn's input and the model's output appended.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedTurn {
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub history: ProviderHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetworkFailure {
    HostNotFound,
    TimedOut,
    ConnectionRefused,
    Insecure,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    MissingKey,
    /// The service didn't recognize the key (401).
    InvalidKey,
    /// The key is valid but isn't allowed to make this request (403).
    PermissionDenied,
    RateLimited,
    /// `overloaded` when the service is too busy to answer rather than failing.
    Server {
        overloaded: bool,
    },
    Network(NetworkFailure),
    /// The request itself was rejected (400, 404, ...); retrying unchanged will not help.
    Rejected,
}

/// Provider failures in a provider-neutral form, so the engine can decide whether to retry and the UI can
/// explain them. `status` is the HTTP status when the service answered; `detail` is the service's own
/// explanation, which becomes the message when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    pub status: Option<u16>,
    pub detail: Option<String>,
}

impl LlmError {
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            detail: None,
        }
    }

    pub fn missing_key() -> Self {
        Self::new(LlmErrorKind::MissingKey, "No API key saved")
    }

    pub(crate) fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            LlmErrorKind::RateLimited | LlmErrorKind::Server { .. } | LlmErrorKind::Network(_)
        )
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.detail.as_deref().unwrap_or(&self.message))
    }
}

impl std::error::Error for LlmError {}

/// Reads a connection's saved API key, or `None` when there isn't one.
pub type KeySource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// The adapter for one connection. `endpoint` is the connection's server address, for a provider reached at one.
pub fn adapter(provider: ProviderId, endpoint: Option<String>, api_key: KeySource) -> Arc<dyn LlmProvider> {
    match provider {
        ProviderId::Anthropic => Arc::new(anthropic::AnthropicProvider::new(api_key)),
        ProviderId::Openai => Arc::new(openai::OpenAiProvider::new(api_key)),
        ProviderId::Gemini => Arc::new(gemini::GeminiProvider::new(api_key)),
        ProviderId::OpenaiCompatible => Arc::new(chat_completions::ChatCompletionsProvider::new(endpoint, api_key)),
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError>;

    /// Streams one turn, reporting progress to `on_event`. No history is produced for a failed turn. Dropping
    /// the returned future closes the stream, which is how Stop interrupts a turn in progress.
    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError>;
}

#[cfg(test)]
mod tests;
