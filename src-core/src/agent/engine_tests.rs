//! The generation loop against a scripted provider and a real store in a temporary folder.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::llm::{ModelInfo, Usage};

enum Step {
    /// Streams `text`, then finishes with `calls` and `stop`.
    Reply(&'static str, Vec<ToolCall>, StopReason),
    /// Finishes without appending anything, as a refusal before any output does.
    Empty(StopReason),
    Fail(LlmError),
    /// Never answers, like a turn the user stops.
    Hang,
}

#[derive(Default)]
struct ScriptedProvider {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<TurnRequest>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(Vec::new())
    }

    async fn run_turn(
        &self,
        request: &TurnRequest,
        on_event: &mut (dyn FnMut(TurnEvent) + Send),
    ) -> Result<CompletedTurn, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        let step = self.steps.lock().unwrap().pop_front().expect("the script ran out of steps");
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: 3,
            cache_creation_input_tokens: 0,
        };
        match step {
            Step::Reply(text, calls, stop) => {
                if !text.is_empty() {
                    on_event(TurnEvent::TextDelta(text.to_string()));
                }
                for call in &calls {
                    on_event(TurnEvent::ToolCallStarted {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                    });
                    on_event(TurnEvent::ToolInputDelta {
                        call_id: call.call_id.clone(),
                        partial_json: call.input_json.clone(),
                    });
                }
                let mut history = request.history.clone();
                if !request.input.resume {
                    history.items.push(json!({"user": request.input}));
                }
                history.items.push(json!({"assistant": text}));
                Ok(CompletedTurn {
                    tool_calls: calls,
                    stop_reason: stop,
                    usage,
                    history,
                })
            }
            Step::Empty(stop) => Ok(CompletedTurn {
                tool_calls: Vec::new(),
                stop_reason: stop,
                usage,
                history: request.history.clone(),
            }),
            Step::Fail(error) => Err(error),
            Step::Hang => std::future::pending().await,
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<ProjectStore>,
    provider: Arc<ScriptedProvider>,
    engine: AgentEngine,
    project: Project,
    settings: Settings,
    events: Mutex<Vec<GenerationEvent>>,
}

impl Fixture {
    fn new(steps: Vec<Step>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ProjectStore::new(dir.path()));
        let provider = Arc::new(ScriptedProvider {
            steps: Mutex::new(steps.into()),
            ..Default::default()
        });
        let providers: HashMap<ProviderId, Arc<dyn LlmProvider>> =
            HashMap::from([(ProviderId::Anthropic, provider.clone() as Arc<dyn LlmProvider>)]);
        let engine = AgentEngine::with_retry_delays(store.clone(), providers, vec![Duration::ZERO; 2]);
        let project = store.create("Tips", ProviderId::Anthropic, "claude-opus-5-5").unwrap();
        Self {
            _dir: dir,
            store,
            provider,
            engine,
            project,
            settings: Settings::default(),
            events: Mutex::default(),
        }
    }

    async fn generate(&self, prompt: Option<&str>) {
        self.generate_with(prompt, false, &CancellationToken::new()).await;
    }

    async fn generate_with(&self, prompt: Option<&str>, fix_errors: bool, cancel: &CancellationToken) {
        let sink = |event| self.events.lock().unwrap().push(event);
        self.engine
            .generate(&self.project.id, prompt, fix_errors, &self.settings, cancel, &sink)
            .await
            .unwrap();
    }

    fn transcript(&self) -> Vec<TranscriptEntry> {
        self.store.transcript(&self.project.id)
    }

    fn conversation(&self) -> Conversation {
        self.store.conversation(&self.project.id).unwrap().unwrap()
    }

    fn sent(&self) -> Vec<TurnInput> {
        self.provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.input.clone())
            .collect()
    }

    fn last_notice(&self) -> Option<(NoticeKind, Option<String>)> {
        match self.transcript().last() {
            Some(TranscriptEntry::Notice { kind, detail }) => Some((*kind, detail.clone())),
            _ => None,
        }
    }
}

fn call(id: &str, name: &str, input: &str) -> ToolCall {
    ToolCall {
        call_id: id.into(),
        name: name.into(),
        input_json: input.into(),
    }
}

fn write_index() -> ToolCall {
    call("c1", "write_file", r#"{"path":"index.html","content":"<h1>Tips</h1>"}"#)
}

fn usage() -> TranscriptEntry {
    TranscriptEntry::Usage {
        input_tokens: 10,
        output_tokens: 5,
        cache_read_tokens: 3,
    }
}

fn prompt_entry(text: &str) -> TranscriptEntry {
    TranscriptEntry::Prompt {
        text: text.into(),
        fix_errors: false,
    }
}

fn reply(text: &str) -> TranscriptEntry {
    TranscriptEntry::Reply { text: text.into() }
}

fn error(kind: LlmErrorKind, message: &str) -> LlmError {
    LlmError::new(kind, message)
}

#[tokio::test]
async fn runs_tools_and_sends_their_results_back() {
    let f = Fixture::new(vec![
        Step::Reply(
            "I'll create the page. ",
            vec![write_index(), call("c2", "read_file", r#"{"path":"missing.js"}"#)],
            StopReason::ToolUse,
        ),
        Step::Reply("Done.", vec![], StopReason::EndTurn),
    ]);
    f.generate(Some("A tip calculator")).await;

    assert_eq!(
        f.transcript(),
        [
            prompt_entry("A tip calculator"),
            reply("I'll create the page."),
            usage(),
            TranscriptEntry::Tool {
                name: "write_file".into(),
                path: Some("index.html".into()),
                error: None
            },
            TranscriptEntry::Tool {
                name: "read_file".into(),
                path: Some("missing.js".into()),
                error: Some("File not found: missing.js".into())
            },
            reply("Done."),
            usage(),
        ]
    );
    assert_eq!(
        f.store.workspace(&f.project.id).unwrap().read("index.html").unwrap(),
        "<h1>Tips</h1>"
    );
    let sent = f.sent();
    assert_eq!(
        sent[1],
        TurnInput::results(vec![
            ToolResult {
                call_id: "c1".into(),
                content: "Wrote index.html".into(),
                is_error: false
            },
            ToolResult {
                call_id: "c2".into(),
                content: "File not found: missing.js".into(),
                is_error: true
            },
        ])
    );
    let conversation = f.conversation();
    assert_eq!(conversation.pending, None);
    assert_eq!(conversation.history.items.len(), 4);

    let events = f.events.lock().unwrap();
    assert_eq!(
        events[0],
        GenerationEvent::TextDelta {
            text: "I'll create the page. ".into()
        }
    );
    assert_eq!(
        events[1],
        GenerationEvent::ToolRunning {
            name: "write_file".into(),
            path: None
        }
    );
    // The path shows as soon as it has streamed.
    assert_eq!(
        events[2],
        GenerationEvent::ToolRunning {
            name: "write_file".into(),
            path: Some("index.html".into())
        }
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == GenerationEvent::TurnPersisted)
            .count(),
        4
    );

    let request = &f.provider.requests.lock().unwrap()[0];
    assert_eq!(request.system_prompt, prompt::SYSTEM);
    assert_eq!(request.tools, prompt::tools());
    assert_eq!(
        (request.model.as_str(), request.max_output_tokens),
        ("claude-opus-5-5", 64_000)
    );
}

#[tokio::test]
async fn settings_ride_on_the_user_text_once_and_again_when_they_change() {
    let mut f = Fixture::new(vec![
        Step::Reply("One.", vec![], StopReason::EndTurn),
        Step::Reply("Two.", vec![], StopReason::EndTurn),
        Step::Reply("Three.", vec![], StopReason::EndTurn),
    ]);
    f.generate(Some("First")).await;
    let first = f.sent()[0].text.clone().unwrap();
    // The app's name is outermost, then the form factor, then the request itself.
    assert!(
        first.starts_with("The user named this app \"Tips\", replacing any earlier name."),
        "{first}"
    );
    assert!(first.contains("\n\nThis app now opens in a desktop browser window, overriding the system prompt's phone guidance:"));
    assert!(first.ends_with("\n\nFirst"));
    assert!(!first.contains("network") && !first.contains("<instructions>"));
    let delivered = f.conversation();
    assert_eq!(
        (delivered.app_name.as_str(), delivered.form_factor.as_deref()),
        ("Tips", Some("desktop"))
    );

    f.generate(Some("Second")).await;
    assert_eq!(f.sent()[1].text.as_deref(), Some("Second"));

    f.settings.app_instructions = "  Use large text  ".into();
    f.settings.allow_app_network = true;
    f.store.rename(&f.project.id, "Tip jar").unwrap();
    f.generate(Some("Third")).await;
    let third = f.sent()[2].text.clone().unwrap();
    assert!(third.starts_with("The user named this app \"Tip jar\""), "{third}");
    assert!(third.contains("\n\nThe user turned on network access for their apps, overriding the system prompt:"));
    assert!(third.contains(
        "\n\nThe user's instructions for all of their apps, replacing any earlier ones:\n<instructions>\nUse large text\n</instructions>\n\nThird"
    ));
    assert!(!third.contains("desktop browser window"));
    let delivered = f.conversation();
    assert_eq!(
        (
            delivered.instructions.as_str(),
            delivered.network,
            delivered.app_name.as_str()
        ),
        ("Use large text", true, "Tip jar")
    );
}

#[tokio::test]
async fn fix_requests_wrap_the_error_log() {
    let f = Fixture::new(vec![Step::Reply("Fixed.", vec![], StopReason::EndTurn)]);
    f.generate_with(Some("ReferenceError: total is not defined"), true, &CancellationToken::new())
        .await;
    assert_eq!(
        f.transcript()[0],
        TranscriptEntry::Prompt {
            text: "ReferenceError: total is not defined".into(),
            fix_errors: true
        }
    );
    assert!(f.sent()[0].text.as_deref().unwrap().ends_with(
        "\n\nRunning the app produced these errors in the browser console. Fix their causes.\n\nReferenceError: total is not defined"
    ));
}

#[tokio::test]
async fn a_failure_keeps_the_input_pending_so_continue_resends_it() {
    let f = Fixture::new(vec![
        Step::Fail(error(LlmErrorKind::Rejected, "400: bad request")),
        Step::Reply("Done.", vec![], StopReason::EndTurn),
    ]);
    f.generate(Some("A timer")).await;
    assert_eq!(f.last_notice(), Some((NoticeKind::Failed, Some("400: bad request".into()))));
    assert_eq!(f.conversation().pending, Some(TurnInput::text("A timer")));
    assert!(f.conversation().history.items.is_empty());

    f.generate(None).await;
    assert_eq!(f.sent()[1].text, f.sent()[0].text);
    assert_eq!(
        f.transcript()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::Prompt { .. }))
            .count(),
        1
    );
    assert_eq!(f.conversation().pending, None);
}

#[tokio::test]
async fn key_problems_are_reported_as_such() {
    for kind in [
        LlmErrorKind::MissingKey,
        LlmErrorKind::InvalidKey,
        LlmErrorKind::PermissionDenied,
    ] {
        let f = Fixture::new(vec![Step::Fail(error(kind, "no"))]);
        f.generate(Some("A timer")).await;
        assert_eq!(f.last_notice(), Some((NoticeKind::AuthFailed, Some("no".into()))));
    }
}

#[tokio::test]
async fn retryable_failures_are_retried_then_reported() {
    let overloaded = || Step::Fail(error(LlmErrorKind::Server { overloaded: true }, "Overloaded"));
    let f = Fixture::new(vec![overloaded(), Step::Reply("Done.", vec![], StopReason::EndTurn)]);
    f.generate(Some("A timer")).await;
    assert_eq!(f.last_notice(), None);
    assert!(f.events.lock().unwrap().contains(&GenerationEvent::Retrying {
        attempt: 1,
        delay_seconds: 0,
        reason: "Overloaded".into()
    }));

    let f = Fixture::new(vec![overloaded(), overloaded(), overloaded()]);
    f.generate(Some("A timer")).await;
    assert_eq!(f.last_notice(), Some((NoticeKind::Failed, Some("Overloaded".into()))));
    assert_eq!(f.sent().len(), 3);
}

#[tokio::test]
async fn stopping_a_turn_records_it_and_a_new_prompt_replaces_the_unsent_text() {
    let f = Fixture::new(vec![Step::Hang, Step::Reply("Done.", vec![], StopReason::EndTurn)]);
    let cancel = CancellationToken::new();
    let stopper = async {
        tokio::task::yield_now().await;
        cancel.cancel();
    };
    tokio::join!(f.generate_with(Some("A timer"), false, &cancel), stopper);
    assert_eq!(f.last_notice(), Some((NoticeKind::Stopped, None)));
    assert_eq!(f.conversation().pending, Some(TurnInput::text("A timer")));

    f.generate(Some("A stopwatch instead")).await;
    assert!(f.sent()[1].text.as_deref().unwrap().ends_with("\n\nA stopwatch instead"));
    assert!(!f.sent()[1].text.as_deref().unwrap().contains("A timer"));
}

#[tokio::test]
async fn tools_from_a_truncated_turn_are_never_run() {
    let f = Fixture::new(vec![
        Step::Reply("Writing", vec![write_index()], StopReason::MaxTokens),
        Step::Reply("", vec![], StopReason::MaxTokens),
    ]);
    f.generate(Some("A tip calculator")).await;
    assert_eq!(
        f.sent()[1],
        TurnInput::results(vec![ToolResult {
            call_id: "c1".into(),
            content: "Not run: your output hit the token limit, so this input may be cut off. Write smaller pieces.".into(),
            is_error: true
        }])
    );
    assert!(f.store.workspace(&f.project.id).unwrap().list_files().is_empty());
    assert!(
        !f.transcript()
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Tool { .. }))
    );
    assert_eq!(f.last_notice(), Some((NoticeKind::Truncated, None)));
}

#[tokio::test]
async fn a_refused_turn_with_tool_calls_keeps_their_results_pending() {
    let f = Fixture::new(vec![Step::Reply("", vec![write_index()], StopReason::Refusal)]);
    f.generate(Some("Something")).await;
    assert_eq!(f.last_notice(), Some((NoticeKind::Refused, None)));
    let pending = f.conversation().pending.unwrap();
    assert_eq!(pending.tool_results[0].content, "Not run: the turn was declined.");
}

#[tokio::test]
async fn a_refusal_before_any_output_delivers_nothing() {
    let f = Fixture::new(vec![Step::Empty(StopReason::Refusal)]);
    f.generate(Some("Something")).await;
    assert_eq!(
        f.transcript(),
        [
            prompt_entry("Something"),
            usage(),
            TranscriptEntry::notice(NoticeKind::Refused)
        ]
    );
    let conversation = f.conversation();
    assert!(conversation.history.items.is_empty());
    assert_eq!((conversation.app_name.as_str(), conversation.form_factor), ("", None));
}

#[tokio::test]
async fn the_step_limit_pauses_with_the_results_pending() {
    let mut f = Fixture::new(vec![
        Step::Reply("", vec![write_index()], StopReason::ToolUse),
        Step::Reply("Done.", vec![], StopReason::EndTurn),
    ]);
    f.settings.max_tool_iterations = 1;
    f.generate(Some("A tip calculator")).await;
    assert_eq!(f.last_notice(), Some((NoticeKind::IterationLimit, None)));
    assert_eq!(f.conversation().pending.unwrap().tool_results[0].content, "Wrote index.html");

    f.generate(None).await;
    assert_eq!(f.sent()[1].tool_results[0].content, "Wrote index.html");
    assert_eq!(f.transcript().last(), Some(&usage()));
}

#[tokio::test]
async fn a_paused_turn_resumes_without_new_input() {
    let f = Fixture::new(vec![
        Step::Reply("Searching", vec![], StopReason::Paused),
        Step::Reply("Found it.", vec![], StopReason::EndTurn),
    ]);
    f.generate(Some("What's new?")).await;
    assert_eq!(f.sent()[1], TurnInput::resume());
    assert_eq!(f.conversation().history.items.len(), 3);
}

#[tokio::test]
async fn a_conversation_from_another_provider_starts_over() {
    let f = Fixture::new(vec![Step::Reply("Done.", vec![], StopReason::EndTurn)]);
    let mut other = Conversation::new(ProviderId::Openai);
    other.history.items.push(json!({"type": "message"}));
    f.store.save_conversation(&f.project.id, &other).unwrap();
    f.generate(Some("A timer")).await;
    assert!(f.provider.requests.lock().unwrap()[0].history.items.is_empty());
    assert_eq!(f.conversation().history.provider, ProviderId::Anthropic);
}

#[tokio::test]
async fn a_project_without_its_provider_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ProjectStore::new(dir.path()));
    let engine = AgentEngine::new(store.clone(), HashMap::new());
    let project = store.create("Tips", ProviderId::Openai, "gpt-6.1-sol").unwrap();
    let cancel = CancellationToken::new();
    engine
        .generate(&project.id, Some("Hi"), false, &Settings::default(), &cancel, &|_| {})
        .await
        .unwrap();
    assert_eq!(
        store.transcript(&project.id),
        [prompt_entry("Hi"), TranscriptEntry::notice(NoticeKind::ProviderUnavailable)]
    );
    // Neither a missing project nor Continue with nothing pending does anything.
    engine
        .generate("missing", Some("Hi"), false, &Settings::default(), &cancel, &|_| {})
        .await
        .unwrap();
}
