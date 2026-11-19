//! The generation loop: streams a turn, runs the tool calls against the project's workspace, sends all results
//! back in one message, and repeats until the model ends its turn, a limit is hit, or the run is cancelled.
//!
//! The conversation is saved before every turn with the input about to be sent as `pending`, so a stopped or
//! failed generation can resume, and the history only grows by complete turns.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::prompt;
use super::tools::{ToolExecutor, path_from_partial_input};
use crate::llm::{
    CompletedTurn, LlmError, LlmErrorKind, LlmProvider, ProviderId, StopReason, ToolCall, ToolResult, TurnEvent, TurnInput,
    TurnRequest,
};
use crate::project::{Conversation, NoticeKind, Project, ProjectStore, StoreError, TranscriptEntry};
use crate::settings::Settings;

/// The form factor this app announces to the model; see [`with_form_factor`].
const FORM_FACTOR: &str = "desktop";

/// Live progress of one generation. Durable results (replies, tool calls, usage, notices) are appended to the
/// project transcript by the engine; these events only drive the in-progress view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GenerationEvent {
    TextDelta {
        text: String,
    },
    ToolRunning {
        name: String,
        path: Option<String>,
    },
    /// The streamed text and running tool so far are now in the transcript.
    TurnPersisted,
    /// A turn failed with a retryable error; it restarts after `delay_seconds` and its partial output is
    /// discarded.
    #[serde(rename_all = "camelCase")]
    Retrying {
        attempt: u32,
        delay_seconds: u64,
        reason: String,
    },
}

pub type EventSink<'a> = &'a (dyn Fn(GenerationEvent) + Send + Sync);

pub struct AgentEngine {
    store: Arc<ProjectStore>,
    providers: HashMap<ProviderId, Arc<dyn LlmProvider>>,
    retry_delays: Vec<Duration>,
}

/// Why the loop ended early.
enum Stop {
    Cancelled,
    Llm(LlmError),
    Store(StoreError),
}

impl From<StoreError> for Stop {
    fn from(error: StoreError) -> Self {
        Stop::Store(error)
    }
}

/// State the cancellation handler needs after the loop is interrupted.
struct Run {
    conversation: Conversation,
    /// Tool calls being executed, and the results finished so far.
    running_calls: Vec<ToolCall>,
    finished_results: Vec<ToolResult>,
}

impl AgentEngine {
    /// Retries rate limits, server errors, and network failures after 2, 5, and 15 seconds.
    pub fn new(store: Arc<ProjectStore>, providers: HashMap<ProviderId, Arc<dyn LlmProvider>>) -> Self {
        Self::with_retry_delays(store, providers, [2, 5, 15].map(Duration::from_secs).to_vec())
    }

    pub fn with_retry_delays(
        store: Arc<ProjectStore>,
        providers: HashMap<ProviderId, Arc<dyn LlmProvider>>,
        retry_delays: Vec<Duration>,
    ) -> Self {
        Self {
            store,
            providers,
            retry_delays,
        }
    }

    /// Runs one prompt to completion, or resumes pending input from a stopped or failed generation when
    /// `prompt` is `None`. Cancelling `cancel` stops generation and records a `STOPPED` notice. Provider
    /// failures are recorded in the transcript; only storage failures are returned.
    pub async fn generate(
        &self,
        project_id: &str,
        prompt: Option<&str>,
        fix_errors: bool,
        settings: &Settings,
        cancel: &CancellationToken,
        on_event: EventSink<'_>,
    ) -> Result<(), StoreError> {
        let store = &self.store;
        let Some(project) = store.project(project_id) else {
            return Ok(());
        };
        let prompt_entry = prompt.map(|text| TranscriptEntry::Prompt {
            text: text.to_string(),
            fix_errors,
        });
        let Some(provider) = self.providers.get(&project.provider) else {
            // A project from a provider this version doesn't offer.
            if let Some(entry) = &prompt_entry {
                store.append_transcript(project_id, entry)?;
            }
            return store.append_transcript(project_id, &TranscriptEntry::notice(NoticeKind::ProviderUnavailable));
        };
        // A conversation from another provider can't be continued in this provider's format.
        let conversation = store
            .conversation(project_id)?
            .filter(|conversation| conversation.history.provider == project.provider)
            .unwrap_or_else(|| Conversation::new(project.provider));

        if let Some(entry) = &prompt_entry {
            store.append_transcript(project_id, entry)?;
        }
        let text = prompt.map(|text| {
            if fix_errors {
                fix_errors_prompt(text)
            } else {
                text.to_string()
            }
        });
        let input = match (conversation.pending.clone(), text) {
            (None, None) => return Ok(()),
            (None, Some(text)) => TurnInput::text(text),
            (Some(pending), None) => pending,
            // A new prompt replaces unsent text but must still carry unsent tool results.
            (Some(pending), Some(text)) => TurnInput {
                tool_results: pending.tool_results,
                text: Some(text),
                resume: false,
            },
        };

        let mut run = Run {
            conversation,
            running_calls: Vec::new(),
            finished_results: Vec::new(),
        };
        match self
            .run(&project, provider.as_ref(), settings, input, &mut run, cancel, on_event)
            .await
        {
            Ok(()) => Ok(()),
            Err(Stop::Store(error)) => Err(error),
            Err(Stop::Cancelled) => {
                if !run.running_calls.is_empty() {
                    let results = run
                        .running_calls
                        .iter()
                        .map(|call| {
                            let done = run.finished_results.iter().find(|result| result.call_id == call.call_id);
                            done.cloned().unwrap_or_else(|| ToolResult {
                                call_id: call.call_id.clone(),
                                content: "Stopped by the user before this ran.".to_string(),
                                is_error: true,
                            })
                        })
                        .collect();
                    run.conversation.pending = Some(TurnInput::results(results));
                    store.save_conversation(project_id, &run.conversation)?;
                }
                store.append_transcript(project_id, &TranscriptEntry::notice(NoticeKind::Stopped))
            }
            Err(Stop::Llm(error)) => {
                let kind = match error.kind {
                    LlmErrorKind::MissingKey | LlmErrorKind::InvalidKey | LlmErrorKind::PermissionDenied => NoticeKind::AuthFailed,
                    _ => NoticeKind::Failed,
                };
                store.append_transcript(
                    project_id,
                    &TranscriptEntry::Notice {
                        kind,
                        detail: Some(error.to_string()),
                    },
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        project: &Project,
        provider: &dyn LlmProvider,
        settings: &Settings,
        mut input: TurnInput,
        run: &mut Run,
        cancel: &CancellationToken,
        on_event: EventSink<'_>,
    ) -> Result<(), Stop> {
        let store = &self.store;
        let id = project.id.as_str();
        let instructions = settings.app_instructions.trim();
        let network = settings.allow_app_network;
        let executor = ToolExecutor::new(store.workspace(id)?);
        let notice = |kind| store.append_transcript(id, &TranscriptEntry::notice(kind));

        for _ in 0..settings.max_tool_iterations {
            run.conversation.pending = Some(input.clone());
            store.save_conversation(id, &run.conversation)?;
            if cancel.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            // Changed instructions ride on the next user text so the system prompt, and with it the cached
            // prefix and Claude's thinking blocks, never changes. The form factor, the network setting, and
            // the app's name (which also covers renames) are announced the same way; the system prompt
            // describes the defaults (a phone, no network).
            let has_text = input.text.is_some();
            let mut sent = input.clone();
            if let Some(text) = &input.text {
                let previous = &run.conversation;
                let mut announced = text.clone();
                if previous.form_factor.as_deref() != Some(FORM_FACTOR) {
                    announced = with_form_factor(&announced);
                }
                if instructions != previous.instructions {
                    announced = with_instructions(instructions, &announced);
                }
                if network != previous.network {
                    announced = with_network(network, &announced);
                }
                if project.name != previous.app_name {
                    announced = with_app_name(&project.name, &announced);
                }
                sent.text = Some(announced);
            }
            let request = TurnRequest {
                model: project.model.clone(),
                effort: settings.effort,
                max_output_tokens: settings.max_output_tokens,
                system_prompt: prompt::SYSTEM.to_string(),
                tools: prompt::tools(),
                history: run.conversation.history.clone(),
                input: sent,
            };
            let (text, turn) = self.run_turn_with_retries(provider, &request, cancel, on_event).await?;
            // A turn refused before any output leaves the history, and so what was delivered, unchanged.
            let delivered = has_text && turn.history.items.len() > run.conversation.history.items.len();
            run.conversation.history = turn.history;
            run.conversation.pending = None;
            if delivered {
                run.conversation.instructions = instructions.to_string();
                run.conversation.network = network;
                run.conversation.app_name = project.name.clone();
                run.conversation.form_factor = Some(FORM_FACTOR.to_string());
            }
            store.save_conversation(id, &run.conversation)?;
            if !text.trim().is_empty() {
                store.append_transcript(
                    id,
                    &TranscriptEntry::Reply {
                        text: text.trim().to_string(),
                    },
                )?;
            }
            store.append_transcript(
                id,
                &TranscriptEntry::Usage {
                    input_tokens: turn.usage.input_tokens,
                    output_tokens: turn.usage.output_tokens,
                    cache_read_tokens: turn.usage.cache_read_input_tokens,
                },
            )?;
            on_event(GenerationEvent::TurnPersisted);

            let stop = turn.stop_reason;
            if stop == StopReason::Paused && turn.tool_calls.is_empty() {
                input = TurnInput::resume();
                continue;
            }
            if turn.tool_calls.is_empty() {
                match stop {
                    StopReason::MaxTokens => notice(NoticeKind::Truncated)?,
                    StopReason::Refusal => notice(NoticeKind::Refused)?,
                    _ => {}
                }
                return Ok(());
            }
            // Never run tools from a truncated or refused turn: their input may be cut off.
            let results = if matches!(stop, StopReason::MaxTokens | StopReason::Refusal) {
                turn.tool_calls
                    .iter()
                    .map(|call| ToolResult {
                        call_id: call.call_id.clone(),
                        content: not_run_reason(stop).to_string(),
                        is_error: true,
                    })
                    .collect()
            } else {
                run.running_calls = turn.tool_calls;
                run.finished_results.clear();
                for call in &run.running_calls {
                    if cancel.is_cancelled() {
                        return Err(Stop::Cancelled);
                    }
                    let path = path_from_partial_input(&call.input_json);
                    on_event(GenerationEvent::ToolRunning {
                        name: call.name.clone(),
                        path: path.clone(),
                    });
                    let result = executor.execute(call);
                    let error = result.is_error.then(|| result.content.clone());
                    run.finished_results.push(result);
                    store.append_transcript(
                        id,
                        &TranscriptEntry::Tool {
                            name: call.name.clone(),
                            path,
                            error,
                        },
                    )?;
                    on_event(GenerationEvent::TurnPersisted);
                }
                run.running_calls.clear();
                std::mem::take(&mut run.finished_results)
            };
            input = TurnInput::results(results);
            if stop == StopReason::Refusal {
                run.conversation.pending = Some(input);
                store.save_conversation(id, &run.conversation)?;
                notice(NoticeKind::Refused)?;
                return Ok(());
            }
        }
        run.conversation.pending = Some(input);
        store.save_conversation(id, &run.conversation)?;
        notice(NoticeKind::IterationLimit)?;
        Ok(())
    }

    /// Streams one turn, retrying retryable failures with backoff. Returns the turn's text with the turn.
    async fn run_turn_with_retries(
        &self,
        provider: &dyn LlmProvider,
        request: &TurnRequest,
        cancel: &CancellationToken,
        on_event: EventSink<'_>,
    ) -> Result<(String, CompletedTurn), Stop> {
        let mut attempt = 0;
        loop {
            let mut text = String::new();
            let result = {
                let mut tool_names: HashMap<String, String> = HashMap::new();
                let mut partial_inputs: HashMap<String, String> = HashMap::new();
                let mut handle = |event: TurnEvent| match event {
                    TurnEvent::TextDelta(delta) => {
                        text.push_str(&delta);
                        on_event(GenerationEvent::TextDelta { text: delta });
                    }
                    TurnEvent::ToolCallStarted { call_id, name } => {
                        tool_names.insert(call_id, name.clone());
                        on_event(GenerationEvent::ToolRunning { name, path: None });
                    }
                    TurnEvent::ToolInputDelta { call_id, partial_json } => {
                        // Show the path as soon as it has streamed, ahead of a long file body.
                        let name = tool_names.get(&call_id).cloned();
                        let partial = partial_inputs.entry(call_id).or_default();
                        let had_path = path_from_partial_input(partial).is_some();
                        partial.push_str(&partial_json);
                        if let (false, Some(path), Some(name)) = (had_path, path_from_partial_input(partial), name) {
                            on_event(GenerationEvent::ToolRunning { name, path: Some(path) });
                        }
                    }
                };
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(Stop::Cancelled),
                    result = provider.run_turn(request, &mut handle) => result,
                }
            };
            match result {
                Ok(turn) => return Ok((text, turn)),
                Err(error) if error.retryable() && attempt < self.retry_delays.len() => {
                    let delay = self.retry_delays[attempt];
                    attempt += 1;
                    on_event(GenerationEvent::Retrying {
                        attempt: attempt as u32,
                        delay_seconds: delay.as_secs(),
                        reason: error.to_string(),
                    });
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(Stop::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                Err(error) => return Err(Stop::Llm(error)),
            }
        }
    }
}

fn not_run_reason(stop: StopReason) -> &'static str {
    match stop {
        StopReason::MaxTokens => "Not run: your output hit the token limit, so this input may be cut off. Write smaller pieces.",
        _ => "Not run: the turn was declined.",
    }
}

fn fix_errors_prompt(log: &str) -> String {
    format!("Running the app produced these errors in the browser console. Fix their causes.\n\n{log}")
}

fn with_app_name(name: &str, text: &str) -> String {
    format!(
        "The user named this app \"{name}\", replacing any earlier name. Use exactly this name wherever the app \
         shows its name, such as the page title and main heading, and don't make up another.\n\n{text}"
    )
}

fn with_network(allowed: bool, text: &str) -> String {
    if allowed {
        format!(
            "The user turned on network access for their apps, overriding the system prompt: the app may now \
             fetch data over https (fetch, XMLHttpRequest, WebSocket) and show images and media from any https \
             URL. Remote scripts, stylesheets, fonts, plain http, and form posts are still blocked. The remote \
             server must allow cross-origin requests (CORS).\n\n{text}"
        )
    } else {
        format!(
            "The user turned network access for their apps off again. The system prompt applies: no network \
             requests.\n\n{text}"
        )
    }
}

fn with_instructions(instructions: &str, text: &str) -> String {
    if instructions.is_empty() {
        format!("The user removed their instructions for all of their apps. Earlier ones no longer apply.\n\n{text}")
    } else {
        format!(
            "The user's instructions for all of their apps, replacing any earlier ones:\n<instructions>\n\
             {instructions}\n</instructions>\n\n{text}"
        )
    }
}

/// The system prompt, shared with every implementation, describes a phone. This app's generated apps open in a
/// desktop browser window, so the difference is announced once per conversation, like the other settings.
fn with_form_factor(text: &str) -> String {
    format!(
        "This app now opens in a desktop browser window, overriding the system prompt's phone guidance: design \
         for a resizable window on a laptop or desktop screen, with mouse and keyboard as the main input. Use \
         the available width with a responsive layout that still works in a narrow window or on a phone, make \
         controls keyboard accessible, and treat hover as an enhancement only. The rest of the system prompt \
         still applies.\n\n{text}"
    )
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
