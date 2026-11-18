use super::{Capability, Mandate, ReportState, Strength, TicketInputKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use thiserror::Error;

pub const EXECUTOR_PROTOCOL_V1: &str = "ucode.executor.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorMode {
    Chat,
    ProjectRead,
    Action,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorCandidate {
    pub race_id: String,
    pub candidate_id: String,
    pub strategy: String,
    pub allowed_paths: Vec<String>,
    pub acceptance: Vec<String>,
    pub generation_attempt: u8,
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorAssignment {
    pub ticket_id: String,
    pub assignment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorMandate {
    pub strength: Strength,
    pub policy_version: String,
    pub project_id: String,
    pub project_fingerprint: String,
    pub capabilities: BTreeSet<Capability>,
    pub trigger_id: String,
    pub thread_key: String,
}

impl From<&Mandate> for ExecutorMandate {
    fn from(value: &Mandate) -> Self {
        Self {
            strength: value.strength(),
            policy_version: value.policy_version().to_string(),
            project_id: value.project_id().to_string(),
            project_fingerprint: value.project_fingerprint().to_string(),
            capabilities: value.capabilities().clone(),
            trigger_id: value.trigger_id().to_string(),
            thread_key: value.thread_key().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorInput {
    pub id: String,
    pub kind: TicketInputKind,
    pub trigger_id: Option<String>,
    pub text: String,
    pub human: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorHistoryItem {
    pub role: HistoryRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    pub path: String,
    pub size_bytes: u64,
    pub content_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorWorkspace {
    pub canonical_root: String,
    pub fingerprint: String,
    pub files: Vec<WorkspaceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorAttachment {
    pub id: String,
    pub display_name: String,
    pub source: String,
    pub relative_path: Option<String>,
    pub content_digest: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorRequest {
    pub version: String,
    pub request_id: String,
    pub turn_id: String,
    pub thread_key: String,
    pub mode: ExecutorMode,
    pub assignment: Option<ExecutorAssignment>,
    pub mandate: Option<ExecutorMandate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<ExecutorCandidate>,
    pub inputs: Vec<ExecutorInput>,
    pub history: Vec<ExecutorHistoryItem>,
    pub workspace: ExecutorWorkspace,
    pub attachment_token: Option<String>,
    pub attachments: Vec<ExecutorAttachment>,
    #[serde(default)]
    pub metadata: Value,
}

impl ExecutorRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != EXECUTOR_PROTOCOL_V1 {
            return Err(ProtocolError::UnsupportedVersion(self.version.clone()));
        }
        if self.request_id.trim().is_empty()
            || self.turn_id.trim().is_empty()
            || self.thread_key.trim().is_empty()
        {
            return Err(ProtocolError::MissingIdentity);
        }
        if self.inputs.is_empty() {
            return Err(ProtocolError::MissingInput);
        }
        if self.history.len() > 24 || self.inputs.len() > 32 || self.attachments.len() > 8 {
            return Err(ProtocolError::ContextLimit);
        }
        if matches!(self.mode, ExecutorMode::Action | ExecutorMode::Candidate)
            && (self.assignment.is_none() || self.mandate.is_none())
        {
            return Err(ProtocolError::ActionAuthorityMissing);
        }
        if !matches!(self.mode, ExecutorMode::Action | ExecutorMode::Candidate)
            && (self.assignment.is_some() || self.mandate.is_some())
        {
            return Err(ProtocolError::UnexpectedAuthority);
        }
        if let Some(mandate) = &self.mandate {
            if mandate.thread_key != self.thread_key
                || mandate.project_fingerprint != self.workspace.fingerprint
            {
                return Err(ProtocolError::AuthorityMismatch);
            }
        }
        match (&self.mode, &self.candidate) {
            (ExecutorMode::Candidate, Some(candidate))
                if !candidate.race_id.trim().is_empty()
                    && !candidate.candidate_id.trim().is_empty()
                    && matches!(
                        candidate.strategy.as_str(),
                        "minimal_change" | "compatibility_first"
                    )
                    && !candidate.allowed_paths.is_empty()
                    && candidate.allowed_paths.len() <= 20
                    && !candidate.acceptance.is_empty()
                    && candidate.acceptance.len() <= 16
                    && matches!(candidate.generation_attempt, 1 | 2) => {}
            (ExecutorMode::Candidate, _) => return Err(ProtocolError::CandidateConfiguration),
            (_, Some(_)) => return Err(ProtocolError::UnexpectedCandidate),
            _ => {}
        }
        if self.attachments.is_empty() != self.attachment_token.is_none() {
            return Err(ProtocolError::AttachmentTokenMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ExecutorEventPayload {
    Start,
    TextDelta {
        delta: String,
    },
    Activity {
        activity_id: String,
        kind: String,
        title: String,
        status: String,
        #[serde(default)]
        evidence: Value,
    },
    EffectProposal {
        effect_key: String,
        kind: String,
        capability: Capability,
        payload_digest: String,
        payload: Value,
    },
    Report {
        state: ReportState,
        summary: String,
    },
    Complete {
        content: String,
        content_digest: String,
    },
    Failure {
        code: String,
        message: String,
        retryable: bool,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorEvent {
    pub version: String,
    pub request_id: String,
    pub turn_id: String,
    pub sequence: u64,
    pub timestamp: String,
    #[serde(flatten)]
    pub payload: ExecutorEventPayload,
}

impl ExecutorEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            ExecutorEventPayload::Complete { .. }
                | ExecutorEventPayload::Failure { .. }
                | ExecutorEventPayload::Cancelled
        )
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported Executor protocol version: {0}")]
    UnsupportedVersion(String),
    #[error("requestId, turnId and threadKey are required")]
    MissingIdentity,
    #[error("at least one Input is required")]
    MissingInput,
    #[error("Executor context exceeds its bounded limits")]
    ContextLimit,
    #[error("action mode requires an Assignment and Mandate")]
    ActionAuthorityMissing,
    #[error("chat and project-read modes cannot carry action authority")]
    UnexpectedAuthority,
    #[error("candidate mode requires bounded candidate configuration")]
    CandidateConfiguration,
    #[error("only candidate mode can carry candidate configuration")]
    UnexpectedCandidate,
    #[error("Mandate does not match the Thread or workspace fingerprint")]
    AuthorityMismatch,
    #[error("attachment token and payload must be present together")]
    AttachmentTokenMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn shared_executor_fixture_round_trips() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("protocol/executor-v1.fixture.json");
        let bytes = std::fs::read(fixture).unwrap();
        let request: ExecutorRequest = serde_json::from_slice(&bytes).unwrap();
        request.validate().unwrap();
        let encoded = serde_json::to_value(&request).unwrap();
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(encoded, original);
    }

    #[test]
    fn action_authority_and_attachment_binding_are_enforced() {
        let request = ExecutorRequest {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: "request".into(),
            turn_id: "turn".into(),
            thread_key: "desktop:thread".into(),
            mode: ExecutorMode::Action,
            assignment: None,
            mandate: None,
            candidate: None,
            inputs: vec![ExecutorInput {
                id: "input".into(),
                kind: TicketInputKind::Trigger,
                trigger_id: Some("trigger".into()),
                text: "fix it".into(),
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
        };
        assert_eq!(
            request.validate(),
            Err(ProtocolError::ActionAuthorityMissing)
        );
        let mut chat = request;
        chat.mode = ExecutorMode::Chat;
        chat.attachment_token = Some("attachment-token".into());
        assert_eq!(chat.validate(), Err(ProtocolError::AttachmentTokenMismatch));
    }

    #[test]
    fn event_payload_fields_use_the_node_camel_case_contract() {
        let event = ExecutorEvent {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: "request".into(),
            turn_id: "turn".into(),
            sequence: 2,
            timestamp: "2026-01-01T00:00:00Z".into(),
            payload: ExecutorEventPayload::EffectProposal {
                effect_key: "write:file:digest".into(),
                kind: "write_file".into(),
                capability: Capability::ProposeWrite,
                payload_digest: "digest".into(),
                payload: serde_json::json!({ "path": "file" }),
            },
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "effect_proposal");
        assert_eq!(value["effectKey"], "write:file:digest");
        assert_eq!(value["payloadDigest"], "digest");
        assert!(value.get("effect_key").is_none());
    }

    #[test]
    fn candidate_mode_requires_authority_and_bounded_configuration() {
        let mut capabilities = BTreeSet::new();
        capabilities.insert(Capability::ProposeWrite);
        let mut request = ExecutorRequest {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: "candidate-request".into(),
            turn_id: "candidate-turn".into(),
            thread_key: "desktop:race:race-1".into(),
            mode: ExecutorMode::Candidate,
            assignment: Some(ExecutorAssignment {
                ticket_id: "ticket-1".into(),
                assignment_id: "candidate-a".into(),
            }),
            mandate: Some(ExecutorMandate {
                strength: Strength::Explicit,
                policy_version: "race-v1".into(),
                project_id: "project-1".into(),
                project_fingerprint: "candidate-workspace".into(),
                capabilities,
                trigger_id: "trigger-1".into(),
                thread_key: "desktop:race:race-1".into(),
            }),
            candidate: Some(ExecutorCandidate {
                race_id: "race-1".into(),
                candidate_id: "candidate-a".into(),
                strategy: "minimal_change".into(),
                allowed_paths: vec!["src/app.rs".into()],
                acceptance: vec!["cargo test".into()],
                generation_attempt: 1,
                feedback: None,
            }),
            inputs: vec![ExecutorInput {
                id: "input".into(),
                kind: TicketInputKind::Trigger,
                trigger_id: Some("trigger-1".into()),
                text: "fix it".into(),
                human: true,
            }],
            history: Vec::new(),
            workspace: ExecutorWorkspace {
                canonical_root: "C:/candidate".into(),
                fingerprint: "candidate-workspace".into(),
                files: Vec::new(),
            },
            attachment_token: None,
            attachments: Vec::new(),
            metadata: Value::Null,
        };
        request.validate().unwrap();
        request.candidate.as_mut().unwrap().generation_attempt = 3;
        assert_eq!(
            request.validate(),
            Err(ProtocolError::CandidateConfiguration)
        );
    }
}
