use super::*;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveResult {
    Stored,
    Duplicate { trigger_id: String },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("trigger has no thread")]
    TriggerWithoutThread,
    #[error("client request id is empty")]
    EmptyClientRequestId,
    #[error("thread not found: {0}")]
    ThreadNotFound(String),
    #[error("thread already has an active evaluation: {0}")]
    EvaluationActive(String),
    #[error("thread has no pending trigger: {0}")]
    NothingToEvaluate(String),
    #[error("evaluation not found: {0}")]
    EvaluationNotFound(String),
    #[error("mandate does not match the evaluation")]
    MandateMismatch,
    #[error("trigger not found: {0}")]
    TriggerNotFound(String),
    #[error("ticket not found: {0}")]
    TicketNotFound(String),
    #[error("ticket has ended: {0}")]
    TicketEnded(String),
    #[error("ticket was stopped by a person and cannot resume: {0}")]
    MandateWithdrawn(String),
    #[error("ticket mandate no longer passes policy revalidation: {0}")]
    MandateRevoked(String),
    #[error("assignment is not running: {0}")]
    AssignmentNotRunning(String),
    #[error("ticket is not retryable: {0}")]
    TicketNotRetryable(String),
    #[error("input does not belong to ticket: {0}")]
    InputNotFound(String),
    #[error("capability is not granted by the mandate: {0:?}")]
    CapabilityDenied(Capability),
    #[error("effect not found: {0}")]
    EffectNotFound(String),
    #[error("effect cannot transition from {from:?} to {to:?}")]
    InvalidEffectTransition { from: EffectState, to: EffectState },
}

pub struct Engine {
    repositories: MemoryRepositories,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(MemoryRepositories::default())
    }
}

impl Engine {
    pub fn new(repositories: MemoryRepositories) -> Self {
        Self { repositories }
    }
    pub fn repositories(&self) -> &MemoryRepositories {
        &self.repositories
    }
    pub fn repositories_mut(&mut self) -> &mut MemoryRepositories {
        &mut self.repositories
    }

    pub fn receive(
        &mut self,
        trigger: Trigger,
        project_id: &str,
        title: &str,
    ) -> Result<ReceiveResult, EngineError> {
        if trigger.threads.is_empty() {
            return Err(EngineError::TriggerWithoutThread);
        }
        if trigger.client_request_id.trim().is_empty() {
            return Err(EngineError::EmptyClientRequestId);
        }
        if let Some(trigger_id) = self
            .repositories
            .trigger_requests
            .get(&trigger.client_request_id)
        {
            return Ok(ReceiveResult::Duplicate {
                trigger_id: trigger_id.clone(),
            });
        }
        if self.repositories.triggers.contains_key(&trigger.id) {
            return Ok(ReceiveResult::Duplicate {
                trigger_id: trigger.id,
            });
        }

        let now = now();
        for reference in &trigger.threads {
            let key = reference.key();
            let thread = self
                .repositories
                .threads
                .entry(key.clone())
                .or_insert_with(|| Thread {
                    key,
                    reference: reference.clone(),
                    project_id: project_id.to_string(),
                    title: title.to_string(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    trigger_ids: Vec::new(),
                    subscriptions: BTreeSet::new(),
                    evaluating: None,
                    pending: Vec::new(),
                });
            if !thread.trigger_ids.contains(&trigger.id) {
                thread.trigger_ids.push(trigger.id.clone());
                thread.pending.push(trigger.id.clone());
            }
            thread.updated_at = now.clone();
        }
        self.repositories
            .trigger_requests
            .insert(trigger.client_request_id.clone(), trigger.id.clone());
        self.repositories.dispositions.insert(
            trigger.id.clone(),
            TriggerDisposition {
                at: now,
                ..Default::default()
            },
        );
        self.repositories
            .triggers
            .insert(trigger.id.clone(), trigger);
        Ok(ReceiveResult::Stored)
    }

    pub fn begin_evaluation(
        &mut self,
        thread_key: &str,
        strength: Strength,
        policy_version: &str,
    ) -> Result<Evaluation, EngineError> {
        let thread = self
            .repositories
            .threads
            .get_mut(thread_key)
            .ok_or_else(|| EngineError::ThreadNotFound(thread_key.to_string()))?;
        if thread.evaluating.is_some() {
            return Err(EngineError::EvaluationActive(thread_key.to_string()));
        }
        if thread.pending.is_empty() {
            return Err(EngineError::NothingToEvaluate(thread_key.to_string()));
        }
        let evaluation = Evaluation {
            id: id("eval"),
            thread_key: thread_key.to_string(),
            trigger_ids: std::mem::take(&mut thread.pending),
            strength,
            policy_version: policy_version.to_string(),
            started_at: now(),
        };
        for trigger_id in &evaluation.trigger_ids {
            if let Some(disposition) = self.repositories.dispositions.get_mut(trigger_id) {
                disposition.evaluation_id = Some(evaluation.id.clone());
            }
        }
        thread.evaluating = Some(evaluation.clone());
        Ok(evaluation)
    }

    pub fn decide_without_ticket(
        &mut self,
        evaluation_id: &str,
        kind: DecisionKind,
        reason_code: &str,
    ) -> Result<Decision, EngineError> {
        let (thread_key, evaluation) = self.take_evaluation(evaluation_id)?;
        let decision = Decision {
            id: id("decision"),
            evaluation_id: evaluation.id,
            thread_key,
            trigger_ids: evaluation.trigger_ids,
            policy_version: evaluation.policy_version,
            at: now(),
            kind,
            reason_code: reason_code.to_string(),
            ticket_id: None,
        };
        self.finish_decision(&decision);
        Ok(decision)
    }

    pub fn accept(
        &mut self,
        evaluation_id: &str,
        title: &str,
        mandate: Mandate,
    ) -> Result<Ticket, EngineError> {
        let (thread_key, evaluation) = self.take_evaluation(evaluation_id)?;
        let primary = evaluation
            .trigger_ids
            .first()
            .ok_or(EngineError::MandateMismatch)?;
        if mandate.thread_key() != thread_key
            || mandate.trigger_id() != primary
            || mandate.policy_version() != evaluation.policy_version
            || mandate.strength() != evaluation.strength
        {
            self.restore_evaluation(&thread_key, evaluation);
            return Err(EngineError::MandateMismatch);
        }
        let decision_id = id("decision");
        let ticket_id = id("ticket");
        let created_at = now();
        let assignment = pending_assignment();
        let input = TicketInput {
            id: id("input"),
            kind: TicketInputKind::Trigger,
            trigger_id: Some(primary.clone()),
            text: None,
            human: self
                .repositories
                .triggers
                .get(primary)
                .is_some_and(|trigger| trigger.human),
            at: created_at.clone(),
            delivered_to: None,
            delivered_at: None,
            read_at: None,
        };
        let ticket = Ticket {
            id: ticket_id.clone(),
            thread_key: thread_key.clone(),
            title: title.to_string(),
            decision_id: decision_id.clone(),
            mandate,
            ended: None,
            attention: None,
            assignments: vec![assignment],
            inputs: vec![input.clone()],
            undelivered: BTreeSet::from([input.id]),
            created_at: created_at.clone(),
            updated_at: created_at,
            origin_trigger_id: primary.clone(),
        };
        let decision = Decision {
            id: decision_id,
            evaluation_id: evaluation.id,
            thread_key: thread_key.clone(),
            trigger_ids: evaluation.trigger_ids,
            policy_version: evaluation.policy_version,
            at: now(),
            kind: DecisionKind::Accept,
            reason_code: "accepted_explicit".into(),
            ticket_id: Some(ticket_id.clone()),
        };
        self.repositories
            .threads
            .get_mut(&thread_key)
            .expect("thread exists")
            .subscriptions
            .insert(ticket_id.clone());
        self.repositories.tickets.insert(ticket_id, ticket.clone());
        self.finish_decision(&decision);
        Ok(ticket)
    }

    pub fn add_followup(
        &mut self,
        ticket_id: &str,
        trigger_id: &str,
        policy_valid: bool,
    ) -> Result<TicketInput, EngineError> {
        let trigger = self
            .repositories
            .triggers
            .get(trigger_id)
            .ok_or_else(|| EngineError::TriggerNotFound(trigger_id.to_string()))?;
        let ticket = self
            .repositories
            .tickets
            .get_mut(ticket_id)
            .ok_or_else(|| EngineError::TicketNotFound(ticket_id.to_string()))?;
        if let Some(ended) = &ticket.ended {
            if ended.by == TicketEndBy::Human {
                return Err(EngineError::MandateWithdrawn(ticket_id.to_string()));
            }
            if !policy_valid {
                return Err(EngineError::MandateRevoked(ticket_id.to_string()));
            }
            ticket.ended = None;
            ticket.assignments.push(pending_assignment());
        }
        if let Some(existing) = ticket
            .inputs
            .iter()
            .find(|input| input.trigger_id.as_deref() == Some(trigger_id))
        {
            return Ok(existing.clone());
        }
        let input = TicketInput {
            id: id("input"),
            kind: TicketInputKind::Trigger,
            trigger_id: Some(trigger.id.clone()),
            text: None,
            human: trigger.human,
            at: now(),
            delivered_to: None,
            delivered_at: None,
            read_at: None,
        };
        ticket.undelivered.insert(input.id.clone());
        ticket.inputs.push(input.clone());
        ticket.updated_at = now();
        if let Some(disposition) = self.repositories.dispositions.get_mut(trigger_id) {
            disposition.fanout.insert(ticket_id.to_string());
        }
        Ok(input)
    }

    pub fn start_assignment(&mut self, ticket_id: &str) -> Result<String, EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let assignment_id = {
            let assignment = ticket
                .latest_assignment_mut()
                .ok_or_else(|| EngineError::AssignmentNotRunning(ticket_id.to_string()))?;
            if assignment.state != AssignmentState::Pending {
                return Err(EngineError::AssignmentNotRunning(assignment.id.clone()));
            }
            assignment.state = AssignmentState::Running;
            assignment.started_at = Some(now());
            assignment.id.clone()
        };
        ticket.updated_at = now();
        Ok(assignment_id)
    }

    pub fn deliver_inputs(&mut self, ticket_id: &str) -> Result<Vec<String>, EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let assignment_id = ticket
            .latest_assignment()
            .filter(|assignment| assignment.state == AssignmentState::Running)
            .map(|assignment| assignment.id.clone())
            .ok_or_else(|| EngineError::AssignmentNotRunning(ticket_id.to_string()))?;
        let input_ids: Vec<_> = ticket.undelivered.iter().cloned().collect();
        let delivered_at = now();
        for input in ticket
            .inputs
            .iter_mut()
            .filter(|input| input_ids.contains(&input.id))
        {
            input.delivered_to = Some(assignment_id.clone());
            input.delivered_at = Some(delivered_at.clone());
        }
        ticket.undelivered.clear();
        if let Some(assignment) = ticket.latest_assignment_mut() {
            assignment.delivered_at = Some(delivered_at);
        }
        Ok(input_ids)
    }

    pub fn mark_inputs_read(
        &mut self,
        ticket_id: &str,
        input_ids: &[String],
    ) -> Result<(), EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let read_at = now();
        for input_id in input_ids {
            let input = ticket
                .inputs
                .iter_mut()
                .find(|input| input.id == *input_id)
                .ok_or_else(|| EngineError::InputNotFound(input_id.clone()))?;
            if input.delivered_to.is_none() {
                return Err(EngineError::InputNotFound(input_id.clone()));
            }
            if input.read_at.is_none() {
                input.read_at = Some(read_at.clone());
            }
        }
        Ok(())
    }

    pub fn report(
        &mut self,
        ticket_id: &str,
        state: ReportState,
        summary: &str,
    ) -> Result<(), EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let at = now();
        let assignment = ticket
            .latest_assignment_mut()
            .filter(|assignment| assignment.state == AssignmentState::Running)
            .ok_or_else(|| EngineError::AssignmentNotRunning(ticket_id.to_string()))?;
        assignment.report = Some(AssignmentReport {
            state,
            summary: summary.to_string(),
            at: at.clone(),
        });
        match state {
            ReportState::Ready | ReportState::Dropped => {
                assignment.state = AssignmentState::Ended;
                assignment.ended_at = Some(at.clone());
                ticket.ended = Some(TicketEnd {
                    at,
                    by: TicketEndBy::Executor,
                    outcome: if state == ReportState::Ready {
                        TicketOutcome::Completed
                    } else {
                        TicketOutcome::Dropped
                    },
                    reason: None,
                });
            }
            ReportState::Blocked | ReportState::Failed => {
                assignment.state = AssignmentState::Ended;
                assignment.ended_at = Some(at);
            }
            ReportState::Progress | ReportState::Waiting => {}
        }
        ticket.updated_at = now();
        Ok(())
    }

    pub fn retry_ticket(&mut self, ticket_id: &str) -> Result<String, EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let retryable = ticket
            .latest_assignment()
            .and_then(|assignment| assignment.report.as_ref())
            .is_some_and(|report| {
                matches!(report.state, ReportState::Blocked | ReportState::Failed)
            });
        if !retryable {
            return Err(EngineError::TicketNotRetryable(ticket_id.to_string()));
        }
        let assignment = pending_assignment();
        let assignment_id = assignment.id.clone();
        ticket.assignments.push(assignment);
        ticket.updated_at = now();
        Ok(assignment_id)
    }

    pub fn stop_ticket(
        &mut self,
        ticket_id: &str,
        reason: Option<String>,
    ) -> Result<(), EngineError> {
        let ticket = self
            .repositories
            .tickets
            .get_mut(ticket_id)
            .ok_or_else(|| EngineError::TicketNotFound(ticket_id.to_string()))?;
        if ticket
            .ended
            .as_ref()
            .is_some_and(|ended| ended.by == TicketEndBy::Human)
        {
            return Ok(());
        }
        let at = now();
        if let Some(assignment) = ticket
            .latest_assignment_mut()
            .filter(|assignment| assignment.state != AssignmentState::Ended)
        {
            assignment.state = AssignmentState::Ended;
            assignment.ended_at = Some(at.clone());
            assignment.ended_reason = Some("stopped_by_person".into());
        }
        ticket.ended = Some(TicketEnd {
            at,
            by: TicketEndBy::Human,
            outcome: TicketOutcome::Stopped,
            reason,
        });
        ticket.undelivered.clear();
        ticket.updated_at = now();
        if let Some(thread) = self.repositories.threads.get_mut(&ticket.thread_key) {
            thread.subscriptions.remove(ticket_id);
        }
        Ok(())
    }

    pub fn interrupt_assignment(&mut self, ticket_id: &str) -> Result<Option<String>, EngineError> {
        let ticket = self.open_ticket_mut(ticket_id)?;
        let latest = ticket
            .latest_assignment_mut()
            .ok_or_else(|| EngineError::AssignmentNotRunning(ticket_id.to_string()))?;
        if latest.state != AssignmentState::Running {
            return Ok(None);
        }
        if latest.report.as_ref().is_some_and(|report| {
            matches!(
                report.state,
                ReportState::Waiting | ReportState::Blocked | ReportState::Failed
            )
        }) {
            return Ok(None);
        }
        latest.state = AssignmentState::Ended;
        latest.ended_at = Some(now());
        latest.ended_reason = Some("interrupted".into());
        let recovery = TicketInput {
            id: id("input"),
            kind: TicketInputKind::Recovery,
            trigger_id: None,
            text: Some("Previous Executor stopped before the Assignment finished.".into()),
            human: false,
            at: now(),
            delivered_to: None,
            delivered_at: None,
            read_at: None,
        };
        ticket.undelivered.insert(recovery.id.clone());
        ticket.inputs.push(recovery);
        let assignment = pending_assignment();
        let id = assignment.id.clone();
        ticket.assignments.push(assignment);
        ticket.updated_at = now();
        Ok(Some(id))
    }

    pub fn propose_effect(
        &mut self,
        ticket_id: &str,
        capability: Capability,
        key: &str,
        kind: &str,
        payload: &[u8],
    ) -> Result<(Effect, bool), EngineError> {
        let ticket = self
            .repositories
            .tickets
            .get(ticket_id)
            .ok_or_else(|| EngineError::TicketNotFound(ticket_id.to_string()))?;
        if !ticket.is_open() {
            return Err(EngineError::TicketEnded(ticket_id.to_string()));
        }
        if !ticket.mandate.permits(capability) {
            return Err(EngineError::CapabilityDenied(capability));
        }
        let composite = format!("{ticket_id}:{key}");
        if let Some(effect_id) = self.repositories.effect_keys.get(&composite) {
            return Ok((
                self.repositories
                    .effects
                    .get(effect_id)
                    .expect("effect key points to effect")
                    .clone(),
                false,
            ));
        }
        let effect = Effect {
            id: id("effect"),
            ticket_id: ticket_id.to_string(),
            key: key.to_string(),
            kind: kind.to_string(),
            payload_digest: digest(payload),
            state: EffectState::Proposed,
            attempts: 0,
            created_at: now(),
            resolved_at: None,
        };
        self.repositories
            .effect_keys
            .insert(composite, effect.id.clone());
        self.repositories
            .effects
            .insert(effect.id.clone(), effect.clone());
        Ok((effect, true))
    }

    pub fn transition_effect(
        &mut self,
        effect_id: &str,
        next: EffectState,
    ) -> Result<Effect, EngineError> {
        let effect = self
            .repositories
            .effects
            .get_mut(effect_id)
            .ok_or_else(|| EngineError::EffectNotFound(effect_id.to_string()))?;
        let allowed = matches!(
            (effect.state, next),
            (
                EffectState::Proposed,
                EffectState::Approved | EffectState::Rejected
            ) | (EffectState::Approved, EffectState::Running)
                | (
                    EffectState::Running,
                    EffectState::Applied | EffectState::Failed | EffectState::Unknown
                )
                | (EffectState::Failed, EffectState::Running)
        );
        if !allowed {
            if effect.state == next {
                return Ok(effect.clone());
            }
            return Err(EngineError::InvalidEffectTransition {
                from: effect.state,
                to: next,
            });
        }
        effect.state = next;
        if next == EffectState::Running {
            effect.attempts += 1;
        }
        if matches!(next, EffectState::Rejected | EffectState::Applied) {
            effect.resolved_at = Some(now());
        }
        Ok(effect.clone())
    }

    pub fn record_delivery(
        &mut self,
        idempotency_key: &str,
        ticket_id: Option<String>,
        trigger_id: &str,
        kind: &str,
        content: &[u8],
    ) -> (Delivery, bool) {
        if self.repositories.delivery_keys.contains(idempotency_key) {
            let delivery = self
                .repositories
                .deliveries
                .values()
                .find(|delivery| delivery.id == idempotency_key)
                .expect("delivery key points to delivery")
                .clone();
            return (delivery, false);
        }
        let delivery = Delivery {
            id: idempotency_key.to_string(),
            ticket_id,
            trigger_id: trigger_id.to_string(),
            kind: kind.to_string(),
            content_digest: digest(content),
            created_at: now(),
        };
        self.repositories
            .delivery_keys
            .insert(idempotency_key.to_string());
        self.repositories
            .deliveries
            .insert(delivery.id.clone(), delivery.clone());
        (delivery, true)
    }

    fn open_ticket_mut(&mut self, ticket_id: &str) -> Result<&mut Ticket, EngineError> {
        let ticket = self
            .repositories
            .tickets
            .get_mut(ticket_id)
            .ok_or_else(|| EngineError::TicketNotFound(ticket_id.to_string()))?;
        if !ticket.is_open() {
            return Err(EngineError::TicketEnded(ticket_id.to_string()));
        }
        Ok(ticket)
    }

    fn take_evaluation(
        &mut self,
        evaluation_id: &str,
    ) -> Result<(String, Evaluation), EngineError> {
        let thread_key = self
            .repositories
            .threads
            .iter()
            .find_map(|(key, thread)| {
                (thread.evaluating.as_ref().map(|item| item.id.as_str()) == Some(evaluation_id))
                    .then(|| key.clone())
            })
            .ok_or_else(|| EngineError::EvaluationNotFound(evaluation_id.to_string()))?;
        let evaluation = self
            .repositories
            .threads
            .get_mut(&thread_key)
            .expect("thread exists")
            .evaluating
            .take()
            .expect("evaluation exists");
        Ok((thread_key, evaluation))
    }

    fn restore_evaluation(&mut self, thread_key: &str, evaluation: Evaluation) {
        if let Some(thread) = self.repositories.threads.get_mut(thread_key) {
            thread.evaluating = Some(evaluation);
        }
    }

    fn finish_decision(&mut self, decision: &Decision) {
        for trigger_id in &decision.trigger_ids {
            if let Some(disposition) = self.repositories.dispositions.get_mut(trigger_id) {
                disposition.decision_id = Some(decision.id.clone());
            }
        }
        self.repositories
            .decisions
            .insert(decision.id.clone(), decision.clone());
    }
}

fn pending_assignment() -> Assignment {
    Assignment {
        id: id("assignment"),
        state: AssignmentState::Pending,
        created_at: now(),
        started_at: None,
        ended_at: None,
        ended_reason: None,
        report: None,
        delivered_at: None,
        idle_at: None,
    }
}

fn id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
fn now() -> String {
    Utc::now().to_rfc3339()
}
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(id: &str, request_id: &str, conversation: &str, text: &str) -> Trigger {
        Trigger {
            id: id.to_string(),
            client_request_id: request_id.to_string(),
            source: "desktop".into(),
            threads: vec![ThreadRef {
                source: "desktop".into(),
                id: conversation.into(),
            }],
            actor: ActorRef {
                source: "desktop".into(),
                id: "local-user".into(),
            },
            human: true,
            addressed: Some(Addressed::Direct),
            at: now(),
            text: text.into(),
            meta: serde_json::json!({}),
        }
    }

    fn accepted(engine: &mut Engine) -> Ticket {
        let message = trigger("trigger-1", "request-1", "conversation-1", "fix it");
        engine
            .receive(message.clone(), "project-1", "Fix it")
            .unwrap();
        let evaluation = engine
            .begin_evaluation("desktop:conversation-1", Strength::Explicit, "policy-1")
            .unwrap();
        let mandate = Mandate::new(
            Strength::Explicit,
            "policy-1",
            MandateBasis::DesktopDirect,
            message.actor,
            "project-1",
            "fingerprint-1",
            [Capability::Read, Capability::ProposeWrite],
            "trigger-1",
            "desktop:conversation-1",
        );
        engine.accept(&evaluation.id, "Fix it", mandate).unwrap()
    }

    #[test]
    fn duplicate_client_request_creates_one_trigger() {
        let mut engine = Engine::default();
        let first = trigger("trigger-1", "request-1", "conversation-1", "hello");
        let duplicate = trigger("trigger-2", "request-1", "conversation-1", "hello");
        assert_eq!(
            engine.receive(first, "project-1", "Hello").unwrap(),
            ReceiveResult::Stored
        );
        assert_eq!(
            engine.receive(duplicate, "project-1", "Hello").unwrap(),
            ReceiveResult::Duplicate {
                trigger_id: "trigger-1".into()
            }
        );
        assert_eq!(engine.repositories().triggers.len(), 1);
        assert_eq!(
            engine.repositories().threads["desktop:conversation-1"]
                .trigger_ids
                .len(),
            1
        );
    }

    #[test]
    fn a_thread_has_at_most_one_evaluation() {
        let mut engine = Engine::default();
        engine
            .receive(
                trigger("trigger-1", "request-1", "conversation-1", "hello"),
                "project-1",
                "Hello",
            )
            .unwrap();
        engine
            .begin_evaluation("desktop:conversation-1", Strength::Explicit, "policy-1")
            .unwrap();
        assert_eq!(
            engine.begin_evaluation("desktop:conversation-1", Strength::Explicit, "policy-1"),
            Err(EngineError::EvaluationActive(
                "desktop:conversation-1".into()
            ))
        );
    }

    #[test]
    fn mandate_must_match_the_evaluation_and_is_read_only_after_acceptance() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        assert_eq!(ticket.mandate.project_id(), "project-1");
        assert!(ticket.mandate.permits(Capability::Read));
        assert!(!ticket.mandate.permits(Capability::Terminal));
    }

    #[test]
    fn input_delivery_and_reads_are_idempotent() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        engine.start_assignment(&ticket.id).unwrap();
        let delivered = engine.deliver_inputs(&ticket.id).unwrap();
        assert_eq!(delivered.len(), 1);
        assert!(engine.deliver_inputs(&ticket.id).unwrap().is_empty());
        engine.mark_inputs_read(&ticket.id, &delivered).unwrap();
        let first_read = engine.repositories().tickets[&ticket.id].inputs[0]
            .read_at
            .clone();
        engine.mark_inputs_read(&ticket.id, &delivered).unwrap();
        assert_eq!(
            engine.repositories().tickets[&ticket.id].inputs[0].read_at,
            first_read
        );
    }

    #[test]
    fn failed_assignment_retries_without_duplicating_the_trigger() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        engine.start_assignment(&ticket.id).unwrap();
        engine
            .report(&ticket.id, ReportState::Failed, "model failed")
            .unwrap();
        let next = engine.retry_ticket(&ticket.id).unwrap();
        let stored = &engine.repositories().tickets[&ticket.id];
        assert_eq!(stored.assignments.len(), 2);
        assert_eq!(stored.assignments[1].id, next);
        assert_eq!(stored.inputs.len(), 1);
        assert_eq!(engine.repositories().triggers.len(), 1);
    }

    #[test]
    fn human_stop_withdraws_work_and_refuses_followups() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        engine.start_assignment(&ticket.id).unwrap();
        engine.stop_ticket(&ticket.id, Some("stop".into())).unwrap();
        engine
            .receive(
                trigger("trigger-2", "request-2", "conversation-1", "continue"),
                "project-1",
                "Fix it",
            )
            .unwrap();
        assert_eq!(
            engine.add_followup(&ticket.id, "trigger-2", true),
            Err(EngineError::MandateWithdrawn(ticket.id.clone()))
        );
        assert_eq!(
            engine.repositories().tickets[&ticket.id]
                .latest_assignment()
                .unwrap()
                .state,
            AssignmentState::Ended
        );
    }

    #[test]
    fn report_state_derives_ticket_stage() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        assert_eq!(ticket_stage(&ticket), TicketStage::WorkingQueued);
        engine.start_assignment(&ticket.id).unwrap();
        engine
            .report(&ticket.id, ReportState::Waiting, "approve")
            .unwrap();
        assert_eq!(
            ticket_stage(&engine.repositories().tickets[&ticket.id]),
            TicketStage::Idle
        );
    }

    #[test]
    fn ended_tickets_have_no_running_assignment() {
        for state in [ReportState::Ready, ReportState::Dropped] {
            let mut engine = Engine::default();
            let ticket = accepted(&mut engine);
            engine.start_assignment(&ticket.id).unwrap();
            engine.report(&ticket.id, state, "finished").unwrap();
            let stored = &engine.repositories().tickets[&ticket.id];
            assert!(stored.ended.is_some());
            assert!(stored
                .assignments
                .iter()
                .all(|assignment| assignment.state != AssignmentState::Running));
        }
    }

    #[test]
    fn ended_executor_ticket_resumes_only_after_policy_revalidation() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        engine.start_assignment(&ticket.id).unwrap();
        engine
            .report(&ticket.id, ReportState::Ready, "finished")
            .unwrap();
        engine
            .receive(
                trigger("trigger-2", "request-2", "conversation-1", "continue"),
                "project-1",
                "Fix it",
            )
            .unwrap();
        assert_eq!(
            engine.add_followup(&ticket.id, "trigger-2", false),
            Err(EngineError::MandateRevoked(ticket.id.clone()))
        );
        assert!(engine.repositories().tickets[&ticket.id].ended.is_some());

        let input = engine.add_followup(&ticket.id, "trigger-2", true).unwrap();
        let resumed = &engine.repositories().tickets[&ticket.id];
        assert!(resumed.ended.is_none());
        assert!(resumed.undelivered.contains(&input.id));
        assert_eq!(
            resumed
                .latest_assignment()
                .map(|assignment| assignment.state),
            Some(AssignmentState::Pending)
        );
    }

    #[test]
    fn effect_and_delivery_are_idempotent() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        let (first, created) = engine
            .propose_effect(
                &ticket.id,
                Capability::ProposeWrite,
                "write:src/main.rs:abc",
                "write_file",
                b"payload",
            )
            .unwrap();
        let (same, created_again) = engine
            .propose_effect(
                &ticket.id,
                Capability::ProposeWrite,
                "write:src/main.rs:abc",
                "write_file",
                b"payload",
            )
            .unwrap();
        assert!(created);
        assert!(!created_again);
        assert_eq!(first.id, same.id);
        engine
            .transition_effect(&first.id, EffectState::Approved)
            .unwrap();
        engine
            .transition_effect(&first.id, EffectState::Running)
            .unwrap();
        engine
            .transition_effect(&first.id, EffectState::Applied)
            .unwrap();
        assert_eq!(
            engine
                .transition_effect(&first.id, EffectState::Applied)
                .unwrap()
                .attempts,
            1
        );

        let (delivery, fresh) = engine.record_delivery(
            "delivery-1",
            Some(ticket.id),
            "trigger-1",
            "answer",
            b"done",
        );
        let (same_delivery, fresh_again) =
            engine.record_delivery("delivery-1", None, "trigger-1", "answer", b"different");
        assert!(fresh);
        assert!(!fresh_again);
        assert_eq!(delivery, same_delivery);
    }

    #[test]
    fn interrupted_running_assignment_gets_recovery_input_and_new_assignment() {
        let mut engine = Engine::default();
        let ticket = accepted(&mut engine);
        engine.start_assignment(&ticket.id).unwrap();
        let replacement = engine.interrupt_assignment(&ticket.id).unwrap();
        assert!(replacement.is_some());
        let stored = &engine.repositories().tickets[&ticket.id];
        assert_eq!(stored.assignments.len(), 2);
        assert!(stored
            .inputs
            .iter()
            .any(|input| input.kind == TicketInputKind::Recovery));
    }
}
