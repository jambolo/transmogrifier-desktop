//! Any server that speaks the streaming Chat Completions format: hosted OpenAI-compatible services and local
//! model servers. The address comes from the connection the user saved and the API key is optional.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::http::{self, EventStream};
use super::{
    CompletedTurn, KeySource, LlmError, LlmErrorKind, LlmProvider, ModelInfo, ProviderId, StopReason, ToolCall, ToolDefinition,
    TurnEvent, TurnInput, TurnRequest, Usage,
};

pub struct ChatCompletionsProvider {
    endpoint: Option<String>,
    api_key: KeySource,
    client: reqwest::Client,
}

impl ChatCompletionsProvider {
    /// `endpoint` is the base address, which `/models` and `/chat/completions` are appended to.
    pub fn new(endpoint: Option<String>, api_key: KeySource) -> Self {
        Self {
            endpoint,
            api_key,
            client: http::client(),
        }
    }

    fn url(&self, path: &str) -> Result<String, LlmError> {
        let base = self
            .endpoint
            .as_deref()
            .ok_or_else(|| LlmError::new(LlmErrorKind::Rejected, "No server address saved"))?;
        Ok(format!("{}/{path}", base.trim_end_matches('/')))
    }

    /// Adds the key when one is saved; servers that need none ignore its absence.
    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (self.api_key)() {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }
}

#[async_trait]
impl LlmProvider for ChatCompletionsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenaiCompatible
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let request = self.authorized(self.client.get(self.url("models")?));
        let response = request.send().await.map_err(http::network_error)?;
        let status = response.status().as_u16();
        let body = http::body_text(response).await?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &body));
        }
        Ok(models(&serde_json::from_str(&body).unwrap_or(Value::Null)))
    }

    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError> {
        if request.model.trim().is_empty() {
            return Err(LlmError::new(LlmErrorKind::Rejected, "No model chosen"));
        }
        let response = self
            .authorized(self.client.post(self.url("chat/completions")?))
            .json(&body(request))
            .send()
            .await
            .map_err(http::network_error)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(http_error(status, &http::body_text(response).await?));
        }
        let mut mapper = TurnMapper::new(request);
        let mut stream = EventStream::new(response);
        while let Some(event) = stream.next().await? {
            for turn_event in mapper.on_event(&event)? {
                on_event(turn_event);
            }
        }
        mapper.finish()
    }
}

/// Every model the server lists, by ID.
pub(crate) fn models(page: &Value) -> Vec<ModelInfo> {
    let mut models: Vec<ModelInfo> = page["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str())
        .map(|id| ModelInfo {
            id: id.to_string(),
            display_name: id.to_string(),
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

/// The server's explanation: `error.message`, or a bare string under `error` or `detail`.
fn detail(json: &Value) -> Option<String> {
    http::error_field(json, "message")
        .or_else(|| json["error"].as_str().map(str::to_string))
        .or_else(|| json["detail"].as_str().map(str::to_string))
}

fn http_error(status: u16, body: &str) -> LlmError {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = match status {
        401 => LlmErrorKind::InvalidKey,
        403 => LlmErrorKind::PermissionDenied,
        429 => LlmErrorKind::RateLimited,
        s if s >= 500 => LlmErrorKind::Server { overloaded: s == 503 },
        _ => LlmErrorKind::Rejected,
    };
    http::status_error(kind, status, body, detail(&json))
}

/// An error sent as a chunk after a 200. `code` is an HTTP status on the services that send one.
fn stream_error(chunk: &Value) -> LlmError {
    let code = &chunk["error"]["code"];
    let status = code.as_u64().or_else(|| code.as_str()?.parse().ok());
    let kind = match status {
        Some(429) => LlmErrorKind::RateLimited,
        Some(s) if s >= 500 => LlmErrorKind::Server { overloaded: s == 503 },
        _ => LlmErrorKind::Rejected,
    };
    let detail = detail(chunk);
    LlmError::new(kind, detail.as_deref().unwrap_or("The response failed")).with_detail(detail)
}

/// The request body. Only fields every server accepts are sent: no effort, output limit, or usage options,
/// which some servers reject. The system prompt leads the messages and isn't stored in the history.
pub(crate) fn body(request: &TurnRequest) -> Value {
    let messages: Vec<Value> = std::iter::once(json!({"role": "system", "content": request.system_prompt}))
        .chain(request.history.items.iter().cloned())
        .chain(input_messages(&request.input))
        .collect();
    json!({
        "model": request.model,
        "messages": messages,
        "tools": request.tools.iter().map(tool).collect::<Vec<Value>>(),
        "stream": true,
    })
}

fn tool(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": definition.name,
            "description": definition.description,
            "parameters": definition.input_schema,
        },
    })
}

/// Input messages in wire format: tool results first, then the user's text.
pub(crate) fn input_messages(input: &TurnInput) -> Vec<Value> {
    let mut messages: Vec<Value> = input
        .tool_results
        .iter()
        .map(|result| {
            // Tool messages have no error flag.
            let content = if result.is_error {
                format!("Error: {}", result.content)
            } else {
                result.content.clone()
            };
            json!({"role": "tool", "tool_call_id": result.call_id, "content": content})
        })
        .collect();
    if let Some(text) = &input.text {
        messages.push(json!({"role": "user", "content": text}));
    }
    messages
}

pub(crate) fn usage(usage: &Value) -> Usage {
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
    Usage {
        // Cached tokens are counted inside prompt_tokens; Usage excludes them, as Claude does.
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
    }
}

/// One tool call, assembled from its streamed fragments.
struct Call {
    index: u64,
    /// The ID the server sent, if any.
    wire_id: Option<String>,
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
}

/// Maps one streamed completion to [`TurnEvent`]s and assembles the assistant message.
pub(crate) struct TurnMapper<'a> {
    request: &'a TurnRequest,
    text: String,
    calls: Vec<Call>,
    finish_reason: Option<String>,
    usage: Usage,
}

impl<'a> TurnMapper<'a> {
    pub fn new(request: &'a TurnRequest) -> Self {
        Self {
            request,
            text: String::new(),
            calls: Vec::new(),
            finish_reason: None,
            usage: Usage::default(),
        }
    }

    pub fn on_event(&mut self, chunk: &Value) -> Result<Vec<TurnEvent>, LlmError> {
        if chunk.get("error").is_some_and(|error| !error.is_null()) {
            return Err(stream_error(chunk));
        }
        if chunk["usage"].is_object() {
            self.usage = usage(&chunk["usage"]);
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        let mut events = Vec::new();
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.text.push_str(text);
            events.push(TurnEvent::TextDelta(text.to_string()));
        }
        for (position, fragment) in delta["tool_calls"].as_array().into_iter().flatten().enumerate() {
            self.on_tool_call(fragment, position as u64, &mut events);
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        Ok(events)
    }

    fn on_tool_call(&mut self, fragment: &Value, position: u64, events: &mut Vec<TurnEvent>) {
        let index = fragment["index"].as_u64().unwrap_or(position);
        let id = fragment["id"].as_str().filter(|id| !id.is_empty());
        let function = &fragment["function"];
        // Some servers give every call index 0, so a different ID also starts a new call.
        let current = self
            .calls
            .iter()
            .rposition(|call| call.index == index)
            .filter(|&at| id.is_none() || self.calls[at].wire_id.is_none() || self.calls[at].wire_id.as_deref() == id);
        let at = current.unwrap_or_else(|| {
            // Some servers send no IDs; one is made up so the result can name its call.
            let made_up = format!("call_{}_{}", self.request.history.items.len(), self.calls.len());
            self.calls.push(Call {
                index,
                wire_id: None,
                call_id: id.map_or(made_up, str::to_string),
                name: String::new(),
                arguments: String::new(),
                started: false,
            });
            self.calls.len() - 1
        });
        let call = &mut self.calls[at];
        if call.wire_id.is_none() {
            call.wire_id = id.map(str::to_string);
        }
        if call.name.is_empty() {
            call.name = function["name"].as_str().unwrap_or_default().to_string();
        }
        // Arguments are a JSON string, sent in pieces; some servers send the whole object instead.
        let arguments = match &function["arguments"] {
            Value::String(piece) => piece.clone(),
            Value::Object(_) => function["arguments"].to_string(),
            _ => String::new(),
        };
        call.arguments.push_str(&arguments);
        if call.name.is_empty() {
            return;
        }
        let partial_json = if call.started {
            arguments
        } else {
            call.started = true;
            events.push(TurnEvent::ToolCallStarted {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
            });
            call.arguments.clone()
        };
        if !partial_json.is_empty() {
            events.push(TurnEvent::ToolInputDelta {
                call_id: call.call_id.clone(),
                partial_json,
            });
        }
    }

    pub fn finish(self) -> Result<CompletedTurn, LlmError> {
        let Some(reason) = self.finish_reason.as_deref() else {
            return Err(LlmError::new(
                LlmErrorKind::Server { overloaded: false },
                "The response ended early",
            ));
        };
        let tool_calls: Vec<ToolCall> = self
            .calls
            .iter()
            .filter(|call| !call.name.is_empty())
            .map(|call| ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                input_json: if call.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    call.arguments.clone()
                },
            })
            .collect();
        let stop_reason = match reason {
            "length" => StopReason::MaxTokens,
            "content_filter" => StopReason::Refusal,
            // Some servers report "stop" for a turn that calls tools.
            _ if !tool_calls.is_empty() => StopReason::ToolUse,
            "stop" => StopReason::EndTurn,
            _ => StopReason::Other,
        };
        let mut message = json!({"role": "assistant", "content": self.text});
        if !tool_calls.is_empty() {
            if self.text.is_empty() {
                message["content"] = Value::Null;
            }
            message["tool_calls"] = tool_calls.iter().map(stored_call).collect();
        }
        let mut history = self.request.history.clone();
        history.items.extend(input_messages(&self.request.input));
        history.items.push(message);
        Ok(CompletedTurn {
            tool_calls,
            stop_reason,
            usage: self.usage,
            history,
        })
    }
}

/// A tool call as kept in the history. Servers parse the arguments of every replayed call, so arguments that
/// aren't a JSON object (cut off, or malformed) are stored as `{}`; the model still gets the tool's error.
fn stored_call(call: &ToolCall) -> Value {
    let valid = serde_json::from_str::<Value>(&call.input_json).is_ok_and(|input| input.is_object());
    let arguments = if valid { call.input_json.as_str() } else { "{}" };
    json!({"id": call.call_id, "type": "function", "function": {"name": call.name, "arguments": arguments}})
}
