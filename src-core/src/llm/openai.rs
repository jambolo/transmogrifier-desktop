//! OpenAI coding models through the streaming Responses API.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::http::{self, EventStream};
use super::{
    CompletedTurn, KeySource, LlmError, LlmErrorKind, LlmProvider, ModelInfo, ProviderId, StopReason, ToolCall, ToolDefinition,
    TurnEvent, TurnInput, TurnRequest, Usage,
};

const BASE_URL: &str = "https://api.openai.com";

// `*codex*` model IDs are deprecated, so the list is filtered to the coding families.
static CODING_MODELS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:gpt-6(\.\d+)?-(astra|sol|luna)|gpt-5\.6-(sol|terra|luna)|gpt-[\d.]+-codex)$").unwrap());

// Explicit implicit-mode caching is only accepted from GPT-5.6 on; older models cache automatically.
static CACHE_OPTIONS_MODELS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^gpt-(6(\.\d+)?|5\.6)-.*$").unwrap());

pub struct OpenAiProvider {
    api_key: KeySource,
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiProvider {
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
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Openai
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let key = self.key()?;
        let request = self.client.get(format!("{}/v1/models", self.base_url)).bearer_auth(key);
        let response = request.send().await.map_err(http::network_error)?;
        let status = response.status().as_u16();
        let body = http::body_text(response).await?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &body));
        }
        let page: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Ok(coding_models(&page, &iso_date(seconds as i64 / 86_400)))
    }

    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError> {
        let key = self.key()?;
        let response = self
            .client
            .post(format!("{}/v1/responses", self.base_url))
            .bearer_auth(key)
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

/// The models offered for new apps: coding families that haven't been shut down as of `today` (`YYYY-MM-DD`).
pub(crate) fn coding_models(page: &Value, today: &str) -> Vec<ModelInfo> {
    let mut models: Vec<ModelInfo> = page["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| model["shutdown_date"].as_str().is_none_or(|date| date > today))
        .filter_map(|model| model["id"].as_str())
        .filter(|id| CODING_MODELS.is_match(id))
        .map(|id| ModelInfo {
            id: id.to_string(),
            display_name: id.to_string(),
        })
        .collect();
    models.sort_by(|a, b| b.id.cmp(&a.id));
    models
}

/// The civil date of a day count since the Unix epoch.
pub(crate) fn iso_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn http_error(status: u16, body: &str) -> LlmError {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = http::error_field(&json, "message");
    let kind = match status {
        401 => LlmErrorKind::InvalidKey,
        403 => LlmErrorKind::PermissionDenied,
        // An exhausted quota is also a 429, but waiting won't fix it.
        429 if http::error_field(&json, "code").as_deref() == Some("insufficient_quota") => LlmErrorKind::Rejected,
        429 => LlmErrorKind::RateLimited,
        s if s >= 500 => LlmErrorKind::Server { overloaded: s == 503 },
        _ => LlmErrorKind::Rejected,
    };
    http::status_error(kind, status, body, detail)
}

/// Only errors known to be transient are retried; quota and request errors fail at once.
fn stream_error(code: Option<&str>, message: Option<&str>) -> LlmError {
    let kind = match code {
        Some("rate_limit_exceeded") => LlmErrorKind::RateLimited,
        Some("server_is_overloaded" | "slow_down") => LlmErrorKind::Server { overloaded: true },
        Some("server_error" | "internal_error") => LlmErrorKind::Server { overloaded: false },
        _ => LlmErrorKind::Rejected,
    };
    LlmError::new(kind, message.unwrap_or("The response failed")).with_detail(message.map(str::to_string))
}

/// The request body. History is replayed statelessly (`store: false`): every earlier input and output item is
/// sent again, including encrypted reasoning, so nothing depends on server-side conversation state.
pub(crate) fn body(request: &TurnRequest) -> Value {
    let items: Vec<Value> = request
        .history
        .items
        .iter()
        .cloned()
        .chain(input_items(&request.input))
        .map(without_nulls)
        .collect();
    let mut tools: Vec<Value> = request.tools.iter().map(tool).collect();
    // OpenAI runs web search itself; it stays constant so the cached prefix holds.
    tools.push(json!({"type": "web_search"}));

    let mut body = Map::new();
    body.insert("model".into(), json!(request.model));
    body.insert("instructions".into(), json!(request.system_prompt));
    body.insert("input".into(), Value::Array(items));
    body.insert("tools".into(), Value::Array(tools));
    body.insert("max_output_tokens".into(), json!(request.max_output_tokens));
    body.insert("store".into(), json!(false));
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    body.insert("reasoning".into(), json!({"effort": request.effort.wire()}));
    body.insert("parallel_tool_calls".into(), json!(true));
    if CACHE_OPTIONS_MODELS.is_match(&request.model) {
        body.insert("prompt_cache_options".into(), json!({"mode": "implicit"}));
    }
    body.insert("stream".into(), json!(true));
    Value::Object(body)
}

/// Output items carry null fields that OpenAI rejects as input (`status` on reasoning items). Stored history
/// keeps them; only the copy sent is cleaned, and a null field says nothing, so the replay loses nothing.
fn without_nulls(element: Value) -> Value {
    match element {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_nulls).collect()),
        other => other,
    }
}

fn tool(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": definition.name,
        "description": definition.description,
        "parameters": definition.input_schema,
        "strict": true,
    })
}

/// Input items in wire format: tool outputs first, then the user's text.
pub(crate) fn input_items(input: &TurnInput) -> Vec<Value> {
    let mut items: Vec<Value> = input
        .tool_results
        .iter()
        .map(|result| {
            // Function outputs have no error flag.
            let output = if result.is_error {
                format!("Error: {}", result.content)
            } else {
                result.content.clone()
            };
            json!({"type": "function_call_output", "call_id": result.call_id, "output": output})
        })
        .collect();
    if let Some(text) = &input.text {
        items.push(json!({"type": "message", "role": "user", "content": text}));
    }
    items
}

fn output(response: &Value) -> impl Iterator<Item = &Value> {
    response["output"].as_array().into_iter().flatten()
}

pub(crate) fn stop_reason(response: &Value) -> StopReason {
    let incomplete = response["incomplete_details"]["reason"].as_str();
    let refused = output(response)
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .any(|part| part["type"] == "refusal");
    if incomplete == Some("max_output_tokens") {
        StopReason::MaxTokens
    } else if incomplete == Some("content_filter") || refused {
        StopReason::Refusal
    } else if output(response).any(|item| item["type"] == "function_call") {
        StopReason::ToolUse
    } else if response["status"] == "completed" {
        StopReason::EndTurn
    } else {
        StopReason::Other
    }
}

pub(crate) fn tool_calls(response: &Value) -> Vec<ToolCall> {
    output(response)
        .filter(|item| item["type"] == "function_call")
        .filter_map(|item| {
            Some(ToolCall {
                call_id: item["call_id"].as_str()?.to_string(),
                name: item["name"].as_str()?.to_string(),
                input_json: item["arguments"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

pub(crate) fn usage(response: &Value) -> Usage {
    let usage = &response["usage"];
    let input = usage["input_tokens"].as_u64().unwrap_or(0);
    let cached = usage["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
    Usage {
        // OpenAI counts cached tokens inside input_tokens; Usage excludes them, as Claude does.
        input_tokens: input.saturating_sub(cached),
        output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: usage["input_tokens_details"]["cache_write_tokens"].as_u64().unwrap_or(0),
    }
}

/// Maps one streamed response to [`TurnEvent`]s and keeps the final response.
pub(crate) struct TurnMapper<'a> {
    request: &'a TurnRequest,
    call_ids: HashMap<String, String>,
    response: Option<Value>,
    emitted_text: bool,
    separator_due: bool,
}

impl<'a> TurnMapper<'a> {
    pub fn new(request: &'a TurnRequest) -> Self {
        Self {
            request,
            call_ids: HashMap::new(),
            response: None,
            emitted_text: false,
            separator_due: false,
        }
    }

    pub fn on_event(&mut self, event: &Value) -> Result<Vec<TurnEvent>, LlmError> {
        match event["type"].as_str().unwrap_or_default() {
            // An error sent mid-stream after a 200, such as an exhausted quota.
            "error" => {
                let error = event.get("error").filter(|e| e.is_object()).unwrap_or(event);
                return Err(stream_error(error["code"].as_str(), error["message"].as_str()));
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                self.response = Some(event["response"].clone());
            }
            "response.output_item.added" => {
                let item = &event["item"];
                if item["type"] == "message" {
                    self.separator_due = self.emitted_text;
                }
                if item["type"] == "function_call"
                    && let (Some(call_id), Some(name)) = (item["call_id"].as_str(), item["name"].as_str())
                {
                    if let Some(id) = item["id"].as_str() {
                        self.call_ids.insert(id.to_string(), call_id.to_string());
                    }
                    return Ok(vec![TurnEvent::ToolCallStarted {
                        call_id: call_id.to_string(),
                        name: name.to_string(),
                    }]);
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str().filter(|d| !d.is_empty()) {
                    let prefix = if self.separator_due { "\n\n" } else { "" };
                    self.separator_due = false;
                    self.emitted_text = true;
                    return Ok(vec![TurnEvent::TextDelta(format!("{prefix}{delta}"))]);
                }
            }
            "response.function_call_arguments.delta" => {
                let call_id = event["item_id"].as_str().and_then(|id| self.call_ids.get(id));
                if let (Some(call_id), Some(delta)) = (call_id, event["delta"].as_str()) {
                    return Ok(vec![TurnEvent::ToolInputDelta {
                        call_id: call_id.clone(),
                        partial_json: delta.to_string(),
                    }]);
                }
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    pub fn finish(self) -> Result<CompletedTurn, LlmError> {
        let response = self
            .response
            .ok_or_else(|| LlmError::new(LlmErrorKind::Server { overloaded: false }, "The response ended early"))?;
        if response["status"] == "failed" {
            let error = &response["error"];
            return Err(stream_error(error["code"].as_str(), error["message"].as_str()));
        }
        let mut history = self.request.history.clone();
        history.items.extend(input_items(&self.request.input));
        // Output items are appended exactly as received, so reasoning and message phases round-trip.
        history.items.extend(output(&response).cloned());
        Ok(CompletedTurn {
            tool_calls: tool_calls(&response),
            stop_reason: stop_reason(&response),
            usage: usage(&response),
            history,
        })
    }
}
