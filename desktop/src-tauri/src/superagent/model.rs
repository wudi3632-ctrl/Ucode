use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorRef {
    pub source: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    pub source: String,
    pub id: String,
}

impl ThreadRef {
    pub fn key(&self) -> String {
        format!("{}:{}", self.source, self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Addressed {
    Mention,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub id: String,
    pub client_request_id: String,
    pub source: String,
    pub threads: Vec<ThreadRef>,
    pub actor: ActorRef,
    pub human: bool,
    pub addressed: Option<Addressed>,
    pub at: String,
    pub text: String,
    #[serde(default)]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerDisposition {
    pub evaluation_id: Option<String>,
    pub decision_id: Option<String>,
    #[serde(default)]
    pub fanout: BTreeSet<String>,
    #[serde(default)]
    pub read_by: BTreeSet<String>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Explicit,
    Inferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum MandateBasis {
    DesktopDirect,
    Followup { ticket_id: String },
    Migration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Read,
    Search,
    Test,
    ProposeWrite,
    Terminal,
    Git,
    Worktree,
    Skill,
    Subagent,
    Network,
}

/// Immutable authority attached to one Ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mandate {
    pub(crate) strength: Strength,
    pub(crate) policy_version: String,
    pub(crate) basis: MandateBasis,
    pub(crate) actor: ActorRef,
    pub(crate) project_id: String,
    pub(crate) project_fingerprint: String,
    pub(crate) capabilities: BTreeSet<Capability>,
    pub(crate) trigger_id: String,
    pub(crate) thread_key: String,
}

impl Mandate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strength: Strength,
        policy_version: impl Into<String>,
        basis: MandateBasis,
        actor: ActorRef,
        project_id: impl Into<String>,
        project_fingerprint: impl Into<String>,
        capabilities: impl IntoIterator<Item = Capability>,
        trigger_id: impl Into<String>,
        thread_key: impl Into<String>,
    ) -> Self {
        Self {
            strength,
            policy_version: policy_version.into(),
            basis,
            actor,
            project_id: project_id.into(),
            project_fingerprint: project_fingerprint.into(),
            capabilities: capabilities.into_iter().collect(),
            trigger_id: trigger_id.into(),
            thread_key: thread_key.into(),
        }
    }

    pub fn strength(&self) -> Strength {
        self.strength
    }
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }
    pub fn basis(&self) -> &MandateBasis {
        &self.basis
    }
    pub fn actor(&self) -> &ActorRef {
        &self.actor
    }
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    pub fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }
    pub fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }
    pub fn trigger_id(&self) -> &str {
        &self.trigger_id
    }
    pub fn thread_key(&self) -> &str {
        &self.thread_key
    }
    pub fn permits(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    pub id: String,
    pub thread_key: String,
    pub trigger_ids: Vec<String>,
    pub strength: Strength,
    pub policy_version: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub key: String,
    pub reference: ThreadRef,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub trigger_ids: Vec<String>,
    #[serde(default)]
    pub subscriptions: BTreeSet<String>,
    pub evaluating: Option<Evaluation>,
    #[serde(default)]
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Chat,
    ProjectRead,
    Accept,
    Followup,
    Clarify,
    Stop,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub evaluation_id: String,
    pub thread_key: String,
    pub trigger_ids: Vec<String>,
    pub policy_version: String,
    pub at: String,
    pub kind: DecisionKind,
    pub reason_code: String,
    pub ticket_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Pending,
    Running,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    Progress,
    Waiting,
    Ready,
    Blocked,
    Failed,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentReport {
    pub state: ReportState,
    pub summary: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub id: String,
    pub state: AssignmentState,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub ended_reason: Option<String>,
    pub report: Option<AssignmentReport>,
    pub delivered_at: Option<String>,
    pub idle_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketInputKind {
    Trigger,
    Text,
    Note,
    Recovery,
    PolicyChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketInput {
    pub id: String,
    pub kind: TicketInputKind,
    pub trigger_id: Option<String>,
    pub text: Option<String>,
    #[serde(default)]
    pub human: bool,
    pub at: String,
    pub delivered_to: Option<String>,
    pub delivered_at: Option<String>,
    pub read_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketOutcome {
    Completed,
    Dropped,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketEndBy {
    Executor,
    Human,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketEnd {
    pub at: String,
    pub by: TicketEndBy,
    pub outcome: TicketOutcome,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub id: String,
    pub question: String,
    pub trigger_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: String,
    pub thread_key: String,
    pub title: String,
    pub decision_id: String,
    pub mandate: Mandate,
    pub ended: Option<TicketEnd>,
    pub attention: Option<Attention>,
    #[serde(default)]
    pub assignments: Vec<Assignment>,
    #[serde(default)]
    pub inputs: Vec<TicketInput>,
    #[serde(default)]
    pub undelivered: BTreeSet<String>,
    pub created_at: String,
    pub updated_at: String,
    pub origin_trigger_id: String,
}

impl Ticket {
    pub fn is_open(&self) -> bool {
        self.ended.is_none()
    }
    pub fn latest_assignment(&self) -> Option<&Assignment> {
        self.assignments.last()
    }
    pub fn latest_assignment_mut(&mut self) -> Option<&mut Assignment> {
        self.assignments.last_mut()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    Proposed,
    Approved,
    Rejected,
    Running,
    Applied,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: String,
    pub ticket_id: String,
    pub key: String,
    pub kind: String,
    pub payload_digest: String,
    pub state: EffectState,
    pub attempts: u32,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub ticket_id: Option<String>,
    pub trigger_id: String,
    pub kind: String,
    pub content_digest: String,
    pub created_at: String,
}
