//! Claude through the streaming beta Messages API.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use super::http::{self, EventStream};
use super::{
    CompletedTurn, KeySource, LlmError, LlmErrorKind, LlmProvider, ModelInfo, ProviderId, StopReason, ToolCall, ToolDefinition,
    TurnEvent, TurnInput, TurnRequest, Usage,
};

const BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

const WEB_FETCH_BETA: &str = "web-fetch-2025-09-10";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const THINKING_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

// Features that only some models accept; the rest of the request is the same for every model.
const FALLBACK_MODELS: [&str; 4] = ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"];
const PROGRESS_UPDATE_MODELS: [&str; 4] = ["claude-fable-5-1", "claude-fable-5", "claude-opus-5-5", "claude-sonnet-5-5"];
const PRESERVED_THINKING_MODELS: [&str; 4] = ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];

// Capped so a runaway search loop can't run up the bill.
const MAX_WEB_USES: u32 = 5;

pub struct AnthropicProvider {
    api_key: KeySource,
    client: reqwest::Client,
    base_url: String,
}

impl AnthropicProvider {
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

    fn get(&self, path: &str, key: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base_url))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let key = self.key()?;
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        loop {
            // Model IDs are URL-safe, so the cursor needs no escaping.
            let cursor = after.as_ref().map(|id| format!("&after_id={id}")).unwrap_or_default();
            let request = self.get(&format!("/v1/models?limit=1000{cursor}"), &key);
            let response = request.send().await.map_err(http::network_error)?;
            let status = response.status().as_u16();
            let body = http::body_text(response).await?;
            if !(200..300).contains(&status) {
                return Err(http_error(status, &body));
            }
            let page: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            for model in page["data"].as_array().into_iter().flatten() {
                if let Some(id) = model["id"].as_str() {
                    let display_name = model["display_name"].as_str().unwrap_or(id).to_string();
                    models.push(ModelInfo {
                        id: id.to_string(),
                        display_name,
                    });
                }
            }
            after = page["last_id"].as_str().map(str::to_string);
            if page["has_more"].as_bool() != Some(true) || after.is_none() {
                return Ok(models);
            }
        }
    }

    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError> {
        let key = self.key()?;
        let response = self
            .client
            .post(format!("{}/v1/messages?beta=true", self.base_url))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", betas(&request.model).join(","))
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

fn http_error(status: u16, body: &str) -> LlmError {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = json["error"]["message"].as_str().map(str::to_string);
    let kind = match status {
        401 => LlmErrorKind::InvalidKey,
        403 => LlmErrorKind::PermissionDenied,
        429 => LlmErrorKind::RateLimited,
        // 5xx and 529 overloaded.
        s if s >= 500 => LlmErrorKind::Server {
            overloaded: s == 529 || json["error"]["type"] == "overloaded_error",
        },
        _ => LlmErrorKind::Rejected,
    };
    http::status_error(kind, status, body, detail)
}

/// An `error` event sent mid-stream, after a 200.
fn stream_error(event: &Value) -> LlmError {
    let error = &event["error"];
    let kind = match error["type"].as_str().unwrap_or_default() {
        "authentication_error" => LlmErrorKind::InvalidKey,
        "permission_error" => LlmErrorKind::PermissionDenied,
        "rate_limit_error" => LlmErrorKind::RateLimited,
        "overloaded_error" => LlmErrorKind::Server { overloaded: true },
        "api_error" | "timeout_error" => LlmErrorKind::Server { overloaded: false },
        _ => LlmErrorKind::Rejected,
    };
    LlmError::new(kind, error.to_string()).with_detail(error["message"].as_str().map(str::to_string))
}

/// Each beta header is sent only to models that accept it.
pub(crate) fn betas(model: &str) -> Vec<&'static str> {
    let mut betas = vec![WEB_FETCH_BETA];
    if FALLBACK_MODELS.contains(&model) {
        betas.push(FALLBACK_BETA);
    }
    if PROGRESS_UPDATE_MODELS.contains(&model) {
        betas.push(THINKING_UPDATES_BETA);
    }
    if PRESERVED_THINKING_MODELS.contains(&model) {
        betas.push(THINKING_BINDING_BETA);
    }
    betas
}

/// The request body. History is replayed verbatim.
pub(crate) fn body(request: &TurnRequest) -> Value {
    let model = request.model.as_str();
    let mut tools: Vec<Value> = request.tools.iter().map(tool).collect();
    // Anthropic runs the web tools itself; they stay constant so the cached prefix holds.
    tools.push(json!({"type": "web_search_20250305", "name": "web_search", "max_uses": MAX_WEB_USES}));
    tools.push(json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": MAX_WEB_USES}));

    let mut messages: Vec<Value> = request.history.items.iter().map(without_unresolved_server_tools).collect();
    messages.extend(user_messages(&request.input));

    let mut thinking = Map::new();
    thinking.insert("type".into(), json!("adaptive"));
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("max_tokens".into(), json!(request.max_output_tokens));
    body.insert("output_config".into(), json!({"effort": request.effort.wire()}));
    body.insert("system".into(), json!([{"type": "text", "text": request.system_prompt}]));
    body.insert("tools".into(), Value::Array(tools));
    // Automatic caching of the longest stable prefix: tools, system prompt, and history.
    body.insert("cache_control".into(), json!({"type": "ephemeral"}));
    body.insert("messages".into(), Value::Array(messages));
    if FALLBACK_MODELS.contains(&model) {
        body.insert("fallbacks".into(), json!("default"));
    }
    if PROGRESS_UPDATE_MODELS.contains(&model) {
        // Notes between tool calls come back as thinking blocks; "updates" makes them readable.
        thinking.insert("display".into(), json!("updates"));
    }
    if PRESERVED_THINKING_MODELS.contains(&model) {
        // History is append-only, so a prefix mismatch is a bug worth surfacing rather than hiding.
        thinking.insert("block_binding".into(), json!({"prefix_mismatch_behavior": "error"}));
    }
    body.insert("thinking".into(), Value::Object(thinking));
    body.insert("stream".into(), json!(true));
    Value::Object(body)
}

fn tool(definition: &ToolDefinition) -> Value {
    json!({
        "name": definition.name,
        "description": definition.description,
        "input_schema": definition.input_schema,
        "strict": true,
        "eager_input_streaming": true,
    })
}

/// The messages to append to history; none when resuming a paused turn.
pub(crate) fn user_messages(input: &TurnInput) -> Vec<Value> {
    if input.resume { Vec::new() } else { vec![user_message(input)] }
}

/// The user message in wire format: tool results first, as the API requires, then any text.
pub(crate) fn user_message(input: &TurnInput) -> Value {
    let mut content = Vec::new();
    for result in &input.tool_results {
        let mut block = Map::new();
        block.insert("type".into(), json!("tool_result"));
        block.insert("tool_use_id".into(), json!(result.call_id));
        block.insert("content".into(), json!(result.content));
        if result.is_error {
            block.insert("is_error".into(), json!(true));
        }
        content.push(Value::Object(block));
    }
    if let Some(text) = &input.text {
        content.push(json!({"type": "text", "text": text}));
    }
    json!({"role": "user", "content": content})
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type")?.as_str()
}

/// The assistant message to append to history. After a mid-output refusal fallback, blocks before the last
/// `fallback` marker other than text belong to the declined model and must not be echoed back.
pub(crate) fn assistant_message(content: Vec<Value>) -> Value {
    let kept = match content.iter().rposition(|block| block_type(block) == Some("fallback")) {
        None => content,
        Some(boundary) => content
            .into_iter()
            .enumerate()
            .filter(|(index, block)| *index > boundary || (*index < boundary && block_type(block) == Some("text")))
            .map(|(_, block)| block)
            .collect(),
    };
    json!({"role": "assistant", "content": kept})
}

/// A turn that also calls a client tool can end before a server tool's result arrives, and the API rejects
/// replaying that call without its result. Only the request drops it; the stored history is unchanged.
pub(crate) fn without_unresolved_server_tools(message: &Value) -> Value {
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return message.clone();
    };
    let resolved: HashSet<&str> = content
        .iter()
        .filter(|block| block_type(block).is_some_and(|t| t.ends_with("_tool_result")))
        .filter_map(|block| block.get("tool_use_id")?.as_str())
        .collect();
    let kept: Vec<Value> = content
        .iter()
        .filter(|block| {
            block_type(block) != Some("server_tool_use")
                || block
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| resolved.contains(id))
        })
        .cloned()
        .collect();
    if kept.len() == content.len() {
        return message.clone();
    }
    let mut message = message.clone();
    message["content"] = Value::Array(kept);
    message
}

pub(crate) fn stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("end_turn" | "stop_sequence") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Refusal,
        Some("pause_turn") => StopReason::Paused,
        _ => StopReason::Other,
    }
}

/// Tool calls in an appended assistant message, in order.
pub(crate) fn tool_calls(assistant: &Value) -> Vec<ToolCall> {
    let blocks = assistant.get("content").and_then(Value::as_array);
    blocks
        .into_iter()
        .flatten()
        .filter(|block| block_type(block) == Some("tool_use"))
        .filter_map(|block| {
            Some(ToolCall {
                call_id: block.get("id")?.as_str()?.to_string(),
                name: block.get("name")?.as_str()?.to_string(),
                input_json: block.get("input").cloned().unwrap_or_else(|| json!({})).to_string(),
            })
        })
        .collect()
}

/// Maps one streamed turn to [`TurnEvent`]s and accumulates the assistant message.
pub(crate) struct TurnMapper<'a> {
    request: &'a TurnRequest,
    blocks: Vec<Value>,
    partial_inputs: HashMap<usize, String>,
    tool_ids: HashMap<usize, String>,
    stop_reason: Option<String>,
    usage: Usage,
    started: bool,
    emitted_text: bool,
    separator_due: bool,
}

impl<'a> TurnMapper<'a> {
    pub fn new(request: &'a TurnRequest) -> Self {
        Self {
            request,
            blocks: Vec::new(),
            partial_inputs: HashMap::new(),
            tool_ids: HashMap::new(),
            stop_reason: None,
            usage: Usage::default(),
            started: false,
            emitted_text: false,
            separator_due: false,
        }
    }

    pub fn on_event(&mut self, event: &Value) -> Result<Vec<TurnEvent>, LlmError> {
        let index = event["index"].as_u64().unwrap_or(0) as usize;
        match event["type"].as_str().unwrap_or_default() {
            "error" => return Err(stream_error(event)),
            "message_start" => {
                self.started = true;
                self.merge_usage(&event["message"]["usage"]);
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                self.merge_usage(&event["usage"]);
            }
            "content_block_start" => {
                let block = event["content_block"].clone();
                self.separator_due = self.emitted_text;
                let started = (block_type(&block) == Some("tool_use"))
                    .then(|| Some((block["id"].as_str()?.to_string(), block["name"].as_str()?.to_string())))
                    .flatten();
                if self.blocks.len() <= index {
                    self.blocks.resize(index + 1, Value::Null);
                }
                self.blocks[index] = block;
                if let Some((call_id, name)) = started {
                    self.tool_ids.insert(index, call_id.clone());
                    return Ok(vec![TurnEvent::ToolCallStarted { call_id, name }]);
                }
            }
            "content_block_delta" => return Ok(self.on_delta(index, &event["delta"])),
            "content_block_stop" => {
                // Input cut off by the token limit doesn't parse; such calls are never run.
                if let Some(partial) = self.partial_inputs.remove(&index).filter(|p| !p.trim().is_empty())
                    && let Some(block) = self.blocks.get_mut(index).filter(|b| b.is_object())
                {
                    block["input"] = serde_json::from_str(&partial).unwrap_or_else(|_| json!({}));
                }
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    fn on_delta(&mut self, index: usize, delta: &Value) -> Vec<TurnEvent> {
        let Some(block) = self.blocks.get_mut(index).filter(|b| b.is_object()) else {
            return Vec::new();
        };
        let text = match delta["type"].as_str().unwrap_or_default() {
            "text_delta" => append(block, "text", &delta["text"]),
            // Progress notes between tool calls arrive as thinking blocks under display "updates".
            "thinking_delta" => append(block, "thinking", &delta["thinking"]),
            "signature_delta" => {
                block["signature"] = delta["signature"].clone();
                None
            }
            "citations_delta" => {
                match block.get_mut("citations").and_then(Value::as_array_mut) {
                    Some(citations) => citations.push(delta["citation"].clone()),
                    None => block["citations"] = json!([delta["citation"]]),
                }
                None
            }
            "input_json_delta" => {
                let partial = delta["partial_json"].as_str().unwrap_or_default();
                self.partial_inputs.entry(index).or_default().push_str(partial);
                return match self.tool_ids.get(&index) {
                    Some(id) => {
                        vec![TurnEvent::ToolInputDelta {
                            call_id: id.clone(),
                            partial_json: partial.to_string(),
                        }]
                    }
                    None => Vec::new(),
                };
            }
            _ => None,
        };
        match text.filter(|t| !t.is_empty()) {
            Some(text) => {
                let prefix = if self.separator_due { "\n\n" } else { "" };
                self.separator_due = false;
                self.emitted_text = true;
                vec![TurnEvent::TextDelta(format!("{prefix}{text}"))]
            }
            None => Vec::new(),
        }
    }

    fn merge_usage(&mut self, usage: &Value) {
        let merge = |field: &str, target: &mut u64| {
            if let Some(value) = usage[field].as_u64() {
                *target = value;
            }
        };
        merge("input_tokens", &mut self.usage.input_tokens);
        merge("output_tokens", &mut self.usage.output_tokens);
        merge("cache_read_input_tokens", &mut self.usage.cache_read_input_tokens);
        merge("cache_creation_input_tokens", &mut self.usage.cache_creation_input_tokens);
    }

    pub fn finish(self) -> Result<CompletedTurn, LlmError> {
        if !self.started {
            return Err(LlmError::new(
                LlmErrorKind::Server { overloaded: false },
                "The response ended early",
            ));
        }
        let content: Vec<Value> = self.blocks.into_iter().filter(Value::is_object).collect();
        let assistant = assistant_message(content);
        let has_content = assistant["content"].as_array().is_some_and(|c| !c.is_empty());
        let mut history = self.request.history.clone();
        // A refusal before any output leaves nothing to append; an empty assistant turn would be rejected later.
        if has_content {
            history.items.extend(user_messages(&self.request.input));
            history.items.push(assistant.clone());
        }
        Ok(CompletedTurn {
            tool_calls: tool_calls(&assistant),
            stop_reason: stop_reason(self.stop_reason.as_deref()),
            usage: self.usage,
            history,
        })
    }
}

/// Appends a string delta to a block field and returns the delta.
fn append(block: &mut Value, field: &str, delta: &Value) -> Option<String> {
    let delta = delta.as_str()?;
    let current = block.get(field).and_then(Value::as_str).unwrap_or_default();
    block[field] = Value::String(format!("{current}{delta}"));
    Some(delta.to_string())
}
