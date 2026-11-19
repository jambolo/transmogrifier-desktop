//! Maps recorded-format SSE streams without any network. The fixtures are written by hand to the documented
//! wire format, so they can miss what the live API really sends.

use serde_json::{Value, json};

use super::*;
use crate::agent::prompt;

fn sse(fixture: &str) -> Vec<Value> {
    fixture
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

fn request(provider: ProviderId, history: Vec<Value>) -> TurnRequest {
    TurnRequest {
        model: provider.default_model().to_string(),
        effort: Effort::High,
        max_output_tokens: 64_000,
        system_prompt: prompt::SYSTEM.to_string(),
        tools: prompt::tools(),
        history: ProviderHistory {
            provider,
            items: history,
        },
        input: TurnInput::text("A greeting page"),
    }
}

fn expected_call(call_id: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.into(),
        name: "write_file".into(),
        input_json: r#"{"path":"index.html","content":"<h1>Hi</h1>"}"#.into(),
    }
}

fn types(content: &Value) -> Vec<&str> {
    content
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["type"].as_str().unwrap())
        .collect()
}

#[test]
fn anthropic_request_matches_the_wire_format() {
    let request = request(ProviderId::Anthropic, vec![]);
    let body = anthropic::body(&request);
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), prompt::tools().len() + 2);
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(tools[0]["eager_input_streaming"], true);
    assert_eq!(tools[0]["input_schema"], prompt::tools()[0].input_schema);
    assert_eq!(
        tools[tools.len() - 2],
        json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5})
    );
    assert_eq!(
        tools[tools.len() - 1],
        json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 5})
    );
    assert_eq!(body["system"], json!([{"type": "text", "text": prompt::SYSTEM}]));
    assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(
        body["thinking"],
        json!({"type": "adaptive", "display": "updates", "block_binding": {"prefix_mismatch_behavior": "error"}})
    );
    assert_eq!(body["stream"], true);
    assert_eq!(
        anthropic::betas(&request.model),
        [
            "web-fetch-2025-09-10",
            "server-side-fallback-2026-07-01",
            "thinking-display-updates-2026-08-18",
            "thinking-binding-controls-2026-08-01"
        ]
    );
}

#[test]
fn anthropic_sends_each_beta_only_to_models_that_accept_it() {
    let mut request = request(ProviderId::Anthropic, vec![]);
    request.model = "claude-haiku-5-5".into();
    let body = anthropic::body(&request);
    assert_eq!(body.get("fallbacks"), None);
    assert_eq!(
        body["thinking"],
        json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "error"}})
    );
    assert_eq!(
        anthropic::betas("claude-haiku-5-5"),
        ["web-fetch-2025-09-10", "thinking-binding-controls-2026-08-01"]
    );
    assert_eq!(anthropic::betas("some-other-model"), ["web-fetch-2025-09-10"]);
}

#[test]
fn anthropic_pause_turn_resumes_without_a_user_message() {
    assert_eq!(anthropic::stop_reason(Some("pause_turn")), StopReason::Paused);
    assert_eq!(anthropic::stop_reason(Some("stop_sequence")), StopReason::EndTurn);
    assert_eq!(anthropic::stop_reason(None), StopReason::Other);
    assert!(anthropic::user_messages(&TurnInput::resume()).is_empty());
}

#[test]
fn anthropic_tool_turn() {
    let request = request(ProviderId::Anthropic, vec![]);
    let mut mapper = anthropic::TurnMapper::new(&request);
    let mut events = Vec::new();
    for event in sse(include_str!("../../tests/fixtures/anthropic-tool-turn.sse")) {
        events.extend(mapper.on_event(&event).unwrap());
    }
    let turn = mapper.finish().unwrap();
    assert_eq!(
        events,
        [
            TurnEvent::TextDelta("Planning the layout.".into()),
            TurnEvent::TextDelta("\n\nI'll create the page.".into()),
            TurnEvent::ToolCallStarted {
                call_id: "toolu_01".into(),
                name: "write_file".into()
            },
            TurnEvent::ToolInputDelta {
                call_id: "toolu_01".into(),
                partial_json: r#"{"path": "index.html", "#.into()
            },
            TurnEvent::ToolInputDelta {
                call_id: "toolu_01".into(),
                partial_json: r#""content": "<h1>Hi</h1>"}"#.into()
            },
        ]
    );
    assert_eq!(turn.tool_calls, [expected_call("toolu_01")]);
    assert_eq!(turn.stop_reason, StopReason::ToolUse);
    assert_eq!(
        turn.usage,
        Usage {
            input_tokens: 1200,
            output_tokens: 120,
            cache_read_input_tokens: 1000,
            cache_creation_input_tokens: 0
        }
    );

    assert_eq!(turn.history.items.len(), 2);
    assert_eq!(turn.history.items[0], anthropic::user_message(&request.input));
    let assistant = &turn.history.items[1];
    assert_eq!(assistant["role"], "assistant");
    // Thinking blocks must round-trip unchanged, signature included.
    assert_eq!(
        assistant["content"][0],
        json!({"type": "thinking", "thinking": "Planning the layout.", "signature": "sig-abc"})
    );
    assert_eq!(types(&assistant["content"]), ["thinking", "text", "tool_use"]);

    // The stored history must be sent as the next request's messages unchanged.
    let mut next = request.clone();
    next.history = turn.history.clone();
    next.input = TurnInput::text("More");
    let mut expected = turn.history.items.clone();
    expected.push(anthropic::user_message(&next.input));
    assert_eq!(anthropic::body(&next)["messages"], Value::Array(expected));
}

#[test]
fn anthropic_tool_results_come_before_text() {
    let input = TurnInput {
        tool_results: vec![ToolResult {
            call_id: "toolu_01".into(),
            content: "No such file".into(),
            is_error: true,
        }],
        text: Some("Also add a footer".into()),
        resume: false,
    };
    assert_eq!(
        anthropic::user_message(&input).to_string(),
        concat!(
            r#"{"role":"user","content":["#,
            r#"{"type":"tool_result","tool_use_id":"toolu_01","content":"No such file","is_error":true},"#,
            r#"{"type":"text","text":"Also add a footer"}]}"#
        )
    );
}

#[test]
fn anthropic_drops_declined_blocks_before_a_fallback() {
    let content = vec![
        json!({"type": "thinking", "thinking": "", "signature": "s1"}),
        json!({"type": "text", "text": "Partial"}),
        json!({"type": "tool_use", "id": "toolu_x", "name": "read_file", "input": {"path": "a.js"}}),
        json!({"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-5"}}),
        json!({"type": "text", "text": "Final"}),
    ];
    let assistant = anthropic::assistant_message(content);
    let texts: Vec<&str> = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Partial", "Final"]);
    assert!(anthropic::tool_calls(&assistant).is_empty());
}

#[test]
fn anthropic_stream_keeps_web_search_results() {
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_04", "content": [], "usage": {"input_tokens": 10, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"query\": \"fetch cors\"}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [{"type": "web_search_result", "url": "https://example.com", "title": "T", "encrypted_content": "enc", "page_age": null}]}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Because of CORS."}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 5}}),
        json!({"type": "message_stop"}),
    ];
    let request = request(ProviderId::Anthropic, vec![]);
    let mut mapper = anthropic::TurnMapper::new(&request);
    let mut turn_events = Vec::new();
    for event in &events {
        turn_events.extend(mapper.on_event(event).unwrap());
    }
    // A server tool's input is not a client tool call.
    assert_eq!(turn_events, [TurnEvent::TextDelta("Because of CORS.".into())]);
    let turn = mapper.finish().unwrap();
    let content = &turn.history.items[1]["content"];
    assert_eq!(types(content), ["server_tool_use", "web_search_tool_result", "text"]);
    assert_eq!(content[0]["input"], json!({"query": "fetch cors"}));
    assert!(turn.tool_calls.is_empty());
    assert_eq!(turn.stop_reason, StopReason::EndTurn);
}

#[test]
fn anthropic_replay_drops_unresolved_server_tool_calls() {
    let assistant = json!({"role": "assistant", "content": [
        {"type": "tool_use", "id": "toolu_1", "name": "list_files", "input": {}},
        {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}},
        {"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_search", "input": {}},
        {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_2", "content": []}
    ]});
    let replayed = anthropic::without_unresolved_server_tools(&assistant);
    assert_eq!(
        types(&replayed["content"]),
        ["tool_use", "server_tool_use", "web_search_tool_result"]
    );
    assert_eq!(replayed["content"][1]["id"], "srvtoolu_2");
    let user = json!({"role": "user", "content": "plain"});
    assert_eq!(anthropic::without_unresolved_server_tools(&user), user);
}

#[test]
fn anthropic_refusal_before_any_output_appends_nothing() {
    let request = request(ProviderId::Anthropic, vec![json!({"role": "user", "content": []})]);
    let mut mapper = anthropic::TurnMapper::new(&request);
    mapper
        .on_event(&json!({"type": "message_start", "message": {"usage": {"input_tokens": 3}}}))
        .unwrap();
    mapper
        .on_event(&json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}, "usage": {}}))
        .unwrap();
    let turn = mapper.finish().unwrap();
    assert_eq!(turn.stop_reason, StopReason::Refusal);
    assert_eq!(turn.history, request.history);
}

#[test]
fn anthropic_stream_errors_are_classified() {
    let request = request(ProviderId::Anthropic, vec![]);
    let mut mapper = anthropic::TurnMapper::new(&request);
    let error = mapper
        .on_event(&json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}))
        .unwrap_err();
    assert_eq!(error.kind, LlmErrorKind::Server { overloaded: true });
    assert_eq!(error.to_string(), "Overloaded");
    assert!(error.retryable());
    // A stream that ends before it starts is a server failure, and retried.
    assert!(anthropic::TurnMapper::new(&request).finish().unwrap_err().retryable());
}

#[test]
fn openai_request_matches_the_wire_format() {
    let body = openai::body(&request(ProviderId::Openai, vec![]));
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), prompt::tools().len() + 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(tools[1]["parameters"], prompt::tools()[1].input_schema);
    assert_eq!(tools[tools.len() - 1], json!({"type": "web_search"}));
    assert_eq!(body["instructions"], prompt::SYSTEM);
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert_eq!(body["parallel_tool_calls"], true);
    assert_eq!(body["prompt_cache_options"], json!({"mode": "implicit"}));
    assert!(openai::input_items(&TurnInput::resume()).is_empty());

    let mut older = request(ProviderId::Openai, vec![]);
    older.model = "gpt-5.2-codex".into();
    assert_eq!(openai::body(&older).get("prompt_cache_options"), None);
}

#[test]
fn openai_tool_turn() {
    let earlier = vec![json!({"type": "message", "role": "user", "content": "Hi"})];
    let request = request(ProviderId::Openai, earlier.clone());
    let mut mapper = openai::TurnMapper::new(&request);
    let mut events = Vec::new();
    for event in sse(include_str!("../../tests/fixtures/openai-tool-turn.sse")) {
        events.extend(mapper.on_event(&event).unwrap());
    }
    let turn = mapper.finish().unwrap();
    assert_eq!(
        events,
        [
            TurnEvent::TextDelta("I'll create the page.".into()),
            TurnEvent::ToolCallStarted {
                call_id: "call_1".into(),
                name: "write_file".into()
            },
            TurnEvent::ToolInputDelta {
                call_id: "call_1".into(),
                partial_json: r#"{"path":"index.html","#.into()
            },
            TurnEvent::ToolInputDelta {
                call_id: "call_1".into(),
                partial_json: r#""content":"<h1>Hi</h1>"}"#.into()
            },
        ]
    );
    assert_eq!(turn.tool_calls, [expected_call("call_1")]);
    assert_eq!(turn.stop_reason, StopReason::ToolUse);
    assert_eq!(
        turn.usage,
        Usage {
            input_tokens: 200,
            output_tokens: 120,
            cache_read_input_tokens: 1000,
            cache_creation_input_tokens: 0
        }
    );

    // History: earlier items, this turn's input, then every output item verbatim.
    let items = &turn.history.items;
    assert_eq!(items[0], earlier[0]);
    assert_eq!(items[1], openai::input_items(&request.input)[0]);
    assert_eq!(
        types(&Value::Array(items[2..].to_vec())),
        ["reasoning", "message", "function_call"]
    );
    assert_eq!(items[2]["encrypted_content"], "enc-xyz");
    assert_eq!(items[3]["phase"], "commentary");

    // The stored history must be sent as the next request's input unchanged.
    let mut next = request.clone();
    next.history = turn.history.clone();
    next.input = TurnInput::text("More");
    let mut expected = turn.history.items.clone();
    expected.extend(openai::input_items(&next.input));
    assert_eq!(openai::body(&next)["input"], Value::Array(expected));
}

#[test]
fn openai_replay_drops_null_fields() {
    // The shape gpt-6.1-sol returned; OpenAI rejected the null status with a 400 on replay.
    let reasoning =
        json!({"id": "rs_1", "summary": [], "type": "reasoning", "content": null, "encrypted_content": "enc", "status": null});
    let body = openai::body(&request(ProviderId::Openai, vec![reasoning]));
    assert_eq!(
        body["input"][0],
        json!({"id": "rs_1", "summary": [], "type": "reasoning", "encrypted_content": "enc"})
    );
}

#[test]
fn openai_tool_errors_are_marked_in_the_output() {
    let input = TurnInput::results(vec![ToolResult {
        call_id: "call_1".into(),
        content: "No such file".into(),
        is_error: true,
    }]);
    assert_eq!(
        openai::input_items(&input),
        [json!({"type": "function_call_output", "call_id": "call_1", "output": "Error: No such file"})]
    );
}

#[test]
fn openai_stop_reasons() {
    let stop = |response: Value| openai::stop_reason(&response);
    assert_eq!(stop(json!({"status": "completed", "output": []})), StopReason::EndTurn);
    assert_eq!(
        stop(
            json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [{"type": "function_call"}]})
        ),
        StopReason::MaxTokens
    );
    assert_eq!(
        stop(json!({"incomplete_details": {"reason": "content_filter"}})),
        StopReason::Refusal
    );
    assert_eq!(
        stop(json!({"status": "completed", "output": [{"type": "message", "content": [{"type": "refusal", "refusal": "No"}]}]})),
        StopReason::Refusal
    );
    assert_eq!(stop(json!({"status": "in_progress", "output": []})), StopReason::Other);
}

#[test]
fn openai_stream_failures_are_classified() {
    let request = request(ProviderId::Openai, vec![]);
    let mut mapper = openai::TurnMapper::new(&request);
    let error = mapper
        .on_event(&json!({"type": "error", "code": "insufficient_quota", "message": "Out of credit"}))
        .unwrap_err();
    assert_eq!(error.kind, LlmErrorKind::Rejected);
    assert_eq!(error.to_string(), "Out of credit");

    let mut mapper = openai::TurnMapper::new(&request);
    mapper
        .on_event(&json!({"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "Oops"}}}))
        .unwrap();
    let error = mapper.finish().unwrap_err();
    assert_eq!(error.kind, LlmErrorKind::Server { overloaded: false });
    assert!(openai::TurnMapper::new(&request).finish().unwrap_err().retryable());
}

#[test]
fn openai_lists_only_current_coding_models() {
    let page = json!({"data": [
        {"id": "gpt-6.1-sol"},
        {"id": "gpt-6-astra", "shutdown_date": "2027-01-01"},
        {"id": "gpt-5.6-terra", "shutdown_date": "2026-01-01"},
        {"id": "gpt-5.2-codex"},
        {"id": "gpt-6.1-sol-preview"},
        {"id": "text-embedding-4"}
    ]});
    let ids: Vec<String> = openai::coding_models(&page, "2026-11-18")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert_eq!(ids, ["gpt-6.1-sol", "gpt-6-astra", "gpt-5.2-codex"]);
}

#[test]
fn iso_dates_from_day_counts() {
    assert_eq!(openai::iso_date(0), "1970-01-01");
    assert_eq!(openai::iso_date(19_782), "2024-02-29");
    assert_eq!(openai::iso_date(20_775), "2026-11-18");
}

#[test]
fn errors_show_the_service_detail_when_there_is_one() {
    let mut error = LlmError::new(LlmErrorKind::RateLimited, "429: {...}");
    assert_eq!(error.to_string(), "429: {...}");
    error.detail = Some("Slow down".into());
    assert_eq!(error.to_string(), "Slow down");
    assert!(error.retryable());
    assert!(!LlmError::missing_key().retryable());
    assert_eq!(LlmError::missing_key().to_string(), "No API key saved");
}
