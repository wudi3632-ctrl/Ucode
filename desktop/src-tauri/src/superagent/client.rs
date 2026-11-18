use super::{ExecutorEvent, ExecutorRequest, EXECUTOR_PROTOCOL_V1};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutorClientError {
    #[error("Executor URL must bind to loopback")]
    NonLoopback,
    #[error("Executor request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Executor returned invalid protocol data: {0}")]
    Protocol(String),
}

#[derive(Clone)]
pub struct ExecutorClient {
    base_url: String,
    client: Client,
}

impl ExecutorClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, ExecutorClientError> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if !(base_url.starts_with("http://127.0.0.1:") || base_url.starts_with("http://localhost:"))
        {
            return Err(ExecutorClientError::NonLoopback);
        }
        Ok(Self {
            base_url,
            client: Client::builder()
                .timeout(Duration::from_secs(210))
                .build()?,
        })
    }

    pub fn run<F>(
        &self,
        request: &ExecutorRequest,
        mut on_event: F,
    ) -> Result<ExecutorEvent, ExecutorClientError>
    where
        F: FnMut(&ExecutorEvent),
    {
        request
            .validate()
            .map_err(|error| ExecutorClientError::Protocol(error.to_string()))?;
        let mut cursor = StreamCursor::default();
        let mut reconnects = 0;
        loop {
            let response = self
                .client
                .post(format!(
                    "{}/v3/executor/runs?afterSequence={}",
                    self.base_url, cursor.sequence
                ))
                .json(request)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status);
            let response = match response {
                Ok(response) => response,
                Err(_) if cursor.sequence > 0 && reconnects < 1 => {
                    reconnects += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let mut ended_without_terminal = true;
            for line in BufReader::new(response).lines() {
                let line =
                    line.map_err(|error| ExecutorClientError::Protocol(error.to_string()))?;
                if line.trim().is_empty() {
                    continue;
                }
                let event: ExecutorEvent = serde_json::from_str(&line)
                    .map_err(|error| ExecutorClientError::Protocol(error.to_string()))?;
                let Some(event) = cursor.accept(event, request)? else {
                    continue;
                };
                on_event(&event);
                if event.is_terminal() {
                    return Ok(event);
                }
                ended_without_terminal = true;
            }
            if ended_without_terminal && reconnects < 1 {
                reconnects += 1;
                continue;
            }
            return Err(ExecutorClientError::Protocol(
                "stream ended without a terminal event".into(),
            ));
        }
    }

    pub fn cancel(&self, request_id: &str) -> Result<Value, ExecutorClientError> {
        let response = self
            .client
            .post(format!(
                "{}/v3/executor/runs/{request_id}/cancel",
                self.base_url
            ))
            .json(&serde_json::json!({}))
            .send()?
            .error_for_status()?
            .json()?;
        Ok(response)
    }
}

#[derive(Default)]
struct StreamCursor {
    sequence: u64,
}

impl StreamCursor {
    fn accept(
        &mut self,
        event: ExecutorEvent,
        request: &ExecutorRequest,
    ) -> Result<Option<ExecutorEvent>, ExecutorClientError> {
        if event.version != EXECUTOR_PROTOCOL_V1
            || event.request_id != request.request_id
            || event.turn_id != request.turn_id
        {
            return Err(ExecutorClientError::Protocol(
                "event identity does not match the request".into(),
            ));
        }
        if event.sequence <= self.sequence {
            return Ok(None);
        }
        if event.sequence != self.sequence + 1 {
            return Err(ExecutorClientError::Protocol(format!(
                "event sequence skipped from {} to {}",
                self.sequence, event.sequence
            )));
        }
        self.sequence = event.sequence;
        Ok(Some(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superagent::{
        ExecutorEventPayload, ExecutorInput, ExecutorMode, ExecutorWorkspace, TicketInputKind,
    };
    use serde_json::Value;

    fn request() -> ExecutorRequest {
        ExecutorRequest {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: "request".into(),
            turn_id: "turn".into(),
            thread_key: "desktop:conversation".into(),
            mode: ExecutorMode::Chat,
            assignment: None,
            mandate: None,
            candidate: None,
            inputs: vec![ExecutorInput {
                id: "input".into(),
                kind: TicketInputKind::Trigger,
                trigger_id: Some("trigger".into()),
                text: "hello".into(),
                human: true,
            }],
            history: Vec::new(),
            workspace: ExecutorWorkspace {
                canonical_root: "C:/project".into(),
                fingerprint: "fingerprint".into(),
                files: Vec::new(),
            },
            attachment_token: None,
            attachments: Vec::new(),
            metadata: Value::Null,
        }
    }

    fn event(sequence: u64) -> ExecutorEvent {
        ExecutorEvent {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: "request".into(),
            turn_id: "turn".into(),
            sequence,
            timestamp: "2026-01-01T00:00:00Z".into(),
            payload: ExecutorEventPayload::TextDelta { delta: "x".into() },
        }
    }

    #[test]
    fn client_refuses_non_loopback_executor() {
        assert!(matches!(
            ExecutorClient::new("https://example.com"),
            Err(ExecutorClientError::NonLoopback)
        ));
    }

    #[test]
    fn replayed_events_are_ignored_but_sequence_gaps_fail() {
        let request = request();
        let mut cursor = StreamCursor::default();
        assert!(cursor.accept(event(1), &request).unwrap().is_some());
        assert!(cursor.accept(event(1), &request).unwrap().is_none());
        assert!(matches!(
            cursor.accept(event(3), &request),
            Err(ExecutorClientError::Protocol(_))
        ));
    }
}
