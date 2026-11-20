//! Google Gemini through the streaming `generateContent` API.

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::http::{self, EventStream};
use super::{
    CompletedTurn, Effort, KeySource, LlmError, LlmErrorKind, LlmProvider, ModelInfo, ProviderId, StopReason, ToolCall,
    ToolDefinition, TurnEvent, TurnInput, TurnRequest, Usage,
};

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const KEY_HEADER: &str = "x-goog-api-key";

/// The most output any Gemini model accepts a request for.
const MAX_OUTPUT_TOKENS: u32 = 65_536;

// The list also holds image, speech, and embedding models, which can't build an app.
static OTHER_PURPOSE_MODELS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"image|tts|audio|live|embedding|robotics|computer-use").unwrap());

// Thinking levels are only accepted from Gemini 3 on.
static MAJOR_VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^gemini-(\d+)").unwrap());

pub struct GeminiProvider {
    api_key: KeySource,
    client: reqwest::Client,
    base_url: String,
}

impl GeminiProvider {
    pub fn new(api_key: KeySource) -> Self {
        Self {
            api_key,
            client: http::client(),
            base_url: BASE_URL.to_string(),
        }
    }

    fn key(&self) -> Result<String, LlmError> {
        (self.api_key)().ok_or_else(LlmError::missing_key)
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let key = self.key()?;
        let request = self
            .client
            .get(format!("{}/models?pageSize=1000", self.base_url))
            .header(KEY_HEADER, key);
        let response = request.send().await.map_err(http::network_error)?;
        let status = response.status().as_u16();
        let body = http::body_text(response).await?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &body));
        }
        Ok(text_models(&serde_json::from_str(&body).unwrap_or(Value::Null)))
    }

    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError> {
        let key = self.key()?;
        let response = self
            .client
            .post(format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, request.model
            ))
            .header(KEY_HEADER, key)
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

/// The Gemini models that generate text, newest first.
pub(crate) fn text_models(page: &Value) -> Vec<ModelInfo> {
    let mut models: Vec<ModelInfo> = page["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| {
            let methods = model["supportedGenerationMethods"].as_array();
            methods.is_some_and(|methods| methods.iter().any(|method| method == "generateContent"))
        })
        .filter_map(|model| {
            let id = model["name"].as_str()?.strip_prefix("models/")?;
            (id.starts_with("gemini-") && !OTHER_PURPOSE_MODELS.is_match(id)).then(|| ModelInfo {
                id: id.to_string(),
                display_name: model["displayName"].as_str().unwrap_or(id).to_string(),
            })
        })
        .collect();
    models.sort_by(|a, b| b.id.cmp(&a.id));
    models
}

fn error_kind(status: u64, invalid_key: bool) -> LlmErrorKind {
    match status {
        _ if invalid_key => LlmErrorKind::InvalidKey,
        401 => LlmErrorKind::InvalidKey,
        403 => LlmErrorKind::PermissionDenied,
        429 => LlmErrorKind::RateLimited,
        s if s >= 500 => LlmErrorKind::Server { overloaded: s == 503 },
        _ => LlmErrorKind::Rejected,
    }
}

/// Gemini answers a bad key with a 400 whose details name the reason.
fn invalid_key(error: &Value) -> bool {
    let details = error["details"].as_array();
    details.is_some_and(|details| details.iter().any(|detail| detail["reason"] == "API_KEY_INVALID"))
}

fn http_error(status: u16, body: &str) -> LlmError {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = error_kind(u64::from(status), invalid_key(&json["error"]));
    http::status_error(kind, status, body, http::error_field(&json, "message"))
}

/// An error sent as a chunk after a 200.
fn stream_error(error: &Value) -> LlmError {
    let kind = error_kind(error["code"].as_u64().unwrap_or(0), invalid_key(error));
    let message = error["message"].as_str();
    LlmError::new(kind, message.unwrap_or("The response failed")).with_detail(message.map(str::to_string))
}

fn thinking_level(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        // The most every model accepts.
        Effort::High | Effort::Xhigh | Effort::Max => "high",
    }
}

/// The request body. The history is replayed whole and unchanged, so thought signatures round-trip.
pub(crate) fn body(request: &TurnRequest) -> Value {
    let history = &request.history.items;
    let contents: Vec<Value> = history
        .iter()
        .cloned()
        .chain(input_contents(history, &request.input))
        .collect();
    let mut config = Map::new();
    config.insert(
        "maxOutputTokens".into(),
        json!(request.max_output_tokens.min(MAX_OUTPUT_TOKENS)),
    );
    let major = MAJOR_VERSION.captures(&request.model).and_then(|c| c[1].parse::<u32>().ok());
    if major.is_some_and(|major| major >= 3) {
        config.insert(
            "thinkingConfig".into(),
            json!({"thinkingLevel": thinking_level(request.effort)}),
        );
    }
    json!({
        "systemInstruction": {"parts": [{"text": request.system_prompt}]},
        "contents": contents,
        "tools": [{"functionDeclarations": request.tools.iter().map(tool).collect::<Vec<Value>>()}],
        "generationConfig": config,
    })
}

fn tool(definition: &ToolDefinition) -> Value {
    json!({
        "name": definition.name,
        "description": definition.description,
        "parametersJsonSchema": definition.input_schema,
    })
}

/// The ID a function call is known by outside this module: the one Gemini gave it, or one made from its
/// position in the history when it has none.
fn call_id(call: &Value, content_index: usize, part_index: usize) -> String {
    match call["id"].as_str() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => format!("call_{content_index}_{part_index}"),
    }
}

/// Input contents in wire format: function responses first, then the user's text in a content of its own.
/// A response must repeat its call's name and ID, which are looked up in the model's last turn.
pub(crate) fn input_contents(history: &[Value], input: &TurnInput) -> Vec<Value> {
    let last_model_turn = history
        .iter()
        .enumerate()
        .rev()
        .find(|(_, content)| content["role"] == "model");
    let calls: Vec<(String, &Value)> = last_model_turn
        .into_iter()
        .flat_map(|(content_index, content)| {
            let parts = content["parts"].as_array().into_iter().flatten().enumerate();
            parts.filter_map(move |(part_index, part)| {
                let call = part.get("functionCall")?;
                Some((call_id(call, content_index, part_index), call))
            })
        })
        .collect();
    let responses: Vec<Value> = input
        .tool_results
        .iter()
        .filter_map(|result| {
            let (_, call) = calls.iter().find(|(id, _)| *id == result.call_id)?;
            let key = if result.is_error { "error" } else { "output" };
            let mut response = Map::new();
            if let Some(id) = call.get("id").filter(|id| !id.is_null()) {
                response.insert("id".into(), id.clone());
            }
            response.insert("name".into(), call["name"].clone());
            response.insert("response".into(), json!({key: result.content}));
            Some(json!({"functionResponse": response}))
        })
        .collect();
    let mut contents = Vec::new();
    if !responses.is_empty() {
        contents.push(json!({"role": "user", "parts": responses}));
    }
    if let Some(text) = &input.text {
        contents.push(json!({"role": "user", "parts": [{"text": text}]}));
    }
    contents
}

pub(crate) fn usage(metadata: &Value) -> Usage {
    let count = |name: &str| metadata[name].as_u64().unwrap_or(0);
    let cached = count("cachedContentTokenCount");
    Usage {
        // Cached tokens are counted inside promptTokenCount; Usage excludes them, as Claude does.
        input_tokens: count("promptTokenCount").saturating_sub(cached),
        output_tokens: count("candidatesTokenCount") + count("thoughtsTokenCount"),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
    }
}

/// Whether a part is nothing but text, so that the next piece of the same text can be joined to it. A part
/// with a thought signature is never joined to another: the signature belongs to that part as it was sent.
fn plain_text(part: &Value) -> bool {
    part["text"].is_string()
        && part
            .as_object()
            .is_some_and(|part| part.keys().all(|key| key == "text" || key == "thought"))
}

/// Maps one streamed response to [`TurnEvent`]s and assembles the model's content from its parts.
pub(crate) struct TurnMapper<'a> {
    request: &'a TurnRequest,
    input: Vec<Value>,
    parts: Vec<Value>,
    finish_reason: Option<String>,
    blocked: bool,
    usage: Usage,
}

impl<'a> TurnMapper<'a> {
    pub fn new(request: &'a TurnRequest) -> Self {
        Self {
            request,
            input: input_contents(&request.history.items, &request.input),
            parts: Vec::new(),
            finish_reason: None,
            blocked: false,
            usage: Usage::default(),
        }
    }

    /// Where the model's content will sit in the history.
    fn content_index(&self) -> usize {
        self.request.history.items.len() + self.input.len()
    }

    pub fn on_event(&mut self, chunk: &Value) -> Result<Vec<TurnEvent>, LlmError> {
        if chunk["error"].is_object() {
            return Err(stream_error(&chunk["error"]));
        }
        if chunk["usageMetadata"].is_object() {
            self.usage = usage(&chunk["usageMetadata"]);
        }
        // The prompt itself was refused; no candidate follows.
        self.blocked |= chunk["promptFeedback"]["blockReason"].is_string();
        let candidate = &chunk["candidates"][0];
        let mut events = Vec::new();
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            self.on_part(part, &mut events);
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        Ok(events)
    }

    fn on_part(&mut self, part: &Value, events: &mut Vec<TurnEvent>) {
        if let Some(call) = part.get("functionCall") {
            let call_id = call_id(call, self.content_index(), self.parts.len());
            events.push(TurnEvent::ToolCallStarted {
                call_id: call_id.clone(),
                name: call["name"].as_str().unwrap_or_default().to_string(),
            });
            // The arguments arrive whole.
            events.push(TurnEvent::ToolInputDelta {
                call_id,
                partial_json: arguments(call),
            });
        }
        let Some(text) = part["text"].as_str() else {
            self.parts.push(part.clone());
            return;
        };
        let thought = part["thought"] == true;
        if !thought && !text.is_empty() {
            events.push(TurnEvent::TextDelta(text.to_string()));
        }
        if !plain_text(part) {
            self.parts.push(part.clone());
        } else if let Some(last) = self
            .parts
            .last_mut()
            .filter(|last| plain_text(last) && (last["thought"] == true) == thought)
        {
            let joined = format!("{}{text}", last["text"].as_str().unwrap_or_default());
            last["text"] = json!(joined);
        } else if !text.is_empty() {
            self.parts.push(part.clone());
        }
    }

    pub fn finish(self) -> Result<CompletedTurn, LlmError> {
        if self.finish_reason.is_none() && !self.blocked {
            return Err(LlmError::new(
                LlmErrorKind::Server { overloaded: false },
                "The response ended early",
            ));
        }
        let content_index = self.content_index();
        let tool_calls: Vec<ToolCall> = self
            .parts
            .iter()
            .enumerate()
            .filter_map(|(part_index, part)| {
                let call = part.get("functionCall")?;
                Some(ToolCall {
                    call_id: call_id(call, content_index, part_index),
                    name: call["name"].as_str()?.to_string(),
                    input_json: arguments(call),
                })
            })
            .collect();
        let stop_reason = match self.finish_reason.as_deref() {
            _ if self.blocked => StopReason::Refusal,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            Some("SAFETY" | "RECITATION" | "LANGUAGE" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII") => StopReason::Refusal,
            // A turn that calls functions also finishes with STOP.
            _ if !tool_calls.is_empty() => StopReason::ToolUse,
            Some("STOP") => StopReason::EndTurn,
            _ => StopReason::Other,
        };
        let mut history = self.request.history.clone();
        // A prompt refused before any output leaves the history unchanged.
        if !(self.blocked && self.parts.is_empty()) {
            history.items.extend(self.input);
            // A content must have parts, so a turn without output adds none.
            if !self.parts.is_empty() {
                history.items.push(json!({"role": "model", "parts": self.parts}));
            }
        }
        Ok(CompletedTurn {
            tool_calls,
            stop_reason,
            usage: self.usage,
            history,
        })
    }
}

/// A function call's arguments as JSON text.
fn arguments(call: &Value) -> String {
    match &call["args"] {
        Value::Null => "{}".to_string(),
        args => args.to_string(),
    }
}
