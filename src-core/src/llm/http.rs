//! HTTP plumbing shared by the providers: the client, server-sent events, and error mapping.

use std::error::Error as _;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use super::{LlmError, LlmErrorKind, NetworkFailure};

/// Retries happen in the engine, where they can be shown to the user, so the client makes one attempt.
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(600))
        .build()
        .expect("the HTTP client configuration is valid")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Incremental server-sent-events parser. Bytes are buffered until a full line arrives, so multi-byte characters
/// split across chunks decode correctly.
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(SseEvent {
                        event: std::mem::take(&mut self.event),
                        data: self.data.join("\n"),
                    });
                    self.data.clear();
                }
                self.event.clear();
            } else if let Some(value) = field(&line, "data") {
                self.data.push(value.to_string());
            } else if let Some(value) = field(&line, "event") {
                self.event = value.to_string();
            }
        }
        events
    }
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?.strip_prefix(':')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Reads a streaming response as parsed JSON events.
pub(crate) struct EventStream {
    response: reqwest::Response,
    parser: SseParser,
    queued: std::collections::VecDeque<SseEvent>,
}

impl EventStream {
    pub fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            parser: SseParser::default(),
            queued: Default::default(),
        }
    }

    /// The next event's JSON payload, or `None` at the end of the stream. Non-JSON payloads are skipped.
    pub async fn next(&mut self) -> Result<Option<Value>, LlmError> {
        loop {
            while let Some(event) = self.queued.pop_front() {
                if let Ok(json) = serde_json::from_str::<Value>(&event.data) {
                    return Ok(Some(json));
                }
            }
            match self.response.chunk().await.map_err(network_error)? {
                Some(chunk) => self.queued.extend(self.parser.push(&chunk)),
                None => return Ok(None),
            }
        }
    }
}

/// Collects a non-streaming body, for error responses and model lists.
pub(crate) async fn body_text(response: reqwest::Response) -> Result<String, LlmError> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.map_err(network_error)?);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn network_error(error: reqwest::Error) -> LlmError {
    let mut chain = String::new();
    let mut refused = false;
    let mut source: Option<&dyn std::error::Error> = Some(&error);
    while let Some(e) = source {
        chain.push_str(&e.to_string().to_lowercase());
        chain.push('\n');
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            refused |= matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
            );
        }
        source = e.source();
    }
    let failure = if chain.contains("dns") || chain.contains("lookup") || chain.contains("name or service") {
        NetworkFailure::HostNotFound
    } else if error.is_timeout() {
        NetworkFailure::TimedOut
    } else if refused {
        NetworkFailure::ConnectionRefused
    } else if chain.contains("certificate") || chain.contains("tls") || chain.contains("ssl") {
        NetworkFailure::Insecure
    } else {
        NetworkFailure::Other
    };
    let message = error.source().map(|s| s.to_string()).unwrap_or_else(|| error.to_string());
    LlmError::new(LlmErrorKind::Network(failure), message)
}

/// A string field of the error object in an error body: `error.<name>`, or `<name>` at the top level.
pub(crate) fn error_field(body: &Value, name: &str) -> Option<String> {
    let error = body.get("error").filter(|e| e.is_object()).unwrap_or(body);
    error.get(name)?.as_str().map(str::to_string)
}

/// An error for a non-success HTTP status; `kind` decides what the status means for the provider.
pub(crate) fn status_error(kind: LlmErrorKind, status: u16, body: &str, detail: Option<String>) -> LlmError {
    let snippet: String = body.chars().take(300).collect();
    LlmError {
        kind,
        message: format!("{status}: {snippet}"),
        status: Some(status),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = SseParser::default();
        assert_eq!(parser.push(b"event: a\ndata: {\"x\":"), vec![]);
        let events = parser.push("1}\r\n\r\n: comment\ndata: caf\u{e9}\n\n".as_bytes());
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: "a".into(),
                    data: "{\"x\":1}".into()
                },
                SseEvent {
                    event: String::new(),
                    data: "caf\u{e9}".into()
                },
            ]
        );
    }

    #[test]
    fn joins_multiple_data_lines() {
        let mut parser = SseParser::default();
        let events = parser.push(b"data: one\ndata:two\n\n");
        assert_eq!(events[0].data, "one\ntwo");
    }

    #[test]
    fn reads_error_fields_at_either_level() {
        let nested = serde_json::json!({"error": {"message": "bad", "code": "c"}});
        assert_eq!(error_field(&nested, "message").as_deref(), Some("bad"));
        let flat = serde_json::json!({"message": "flat"});
        assert_eq!(error_field(&flat, "message").as_deref(), Some("flat"));
        assert_eq!(error_field(&flat, "code"), None);
    }
}
