use super::{AssignmentState, ReportState, Ticket, TicketOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketStage {
    WorkingQueued,
    WorkingRunning,
    Idle,
    PersonAnswer,
    PersonReview,
    PersonBlocked,
    PersonFailed,
    Ended(TicketOutcome),
}

pub fn ticket_stage(ticket: &Ticket) -> TicketStage {
    if let Some(ended) = &ticket.ended {
        return TicketStage::Ended(ended.outcome);
    }
    if ticket.attention.is_some() {
        return TicketStage::PersonAnswer;
    }
    let Some(assignment) = ticket.latest_assignment() else {
        return TicketStage::WorkingQueued;
    };
    let report = assignment.report.as_ref().map(|report| report.state);
    if report == Some(ReportState::Ready) {
        return TicketStage::PersonReview;
    }
    if report == Some(ReportState::Blocked) && !has_human_input_after_report(ticket, assignment) {
        return TicketStage::PersonBlocked;
    }
    if report == Some(ReportState::Failed) && !has_human_input_after_report(ticket, assignment) {
        return TicketStage::PersonFailed;
    }
    if assignment.state == AssignmentState::Pending {
        return TicketStage::WorkingQueued;
    }
    if report == Some(ReportState::Waiting) || assignment.state == AssignmentState::Ended {
        return TicketStage::Idle;
    }
    TicketStage::WorkingRunning
}

fn has_human_input_after_report(ticket: &Ticket, assignment: &super::Assignment) -> bool {
    let Some(report) = &assignment.report else {
        return false;
    };
    ticket
        .inputs
        .iter()
        .any(|input| input.human && input.at > report.at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superagent::{
        ActorRef, Assignment, AssignmentReport, Capability, Mandate, MandateBasis, Strength,
        TicketInput, TicketInputKind,
    };
    use std::collections::BTreeSet;

    fn ticket(report: Option<ReportState>, state: AssignmentState) -> Ticket {
        Ticket {
            id: "ticket".into(),
            thread_key: "desktop:conversation".into(),
            title: "test".into(),
            decision_id: "decision".into(),
            mandate: Mandate::new(
                Strength::Explicit,
                "v1",
                MandateBasis::DesktopDirect,
                ActorRef {
                    source: "desktop".into(),
                    id: "person".into(),
                },
                "project",
                "fingerprint",
                [Capability::Chat],
                "trigger",
                "desktop:conversation",
            ),
            ended: None,
            attention: None,
            assignments: vec![Assignment {
                id: "assignment".into(),
                state,
                created_at: "2026-01-01T00:00:00Z".into(),
                started_at: None,
                ended_at: None,
                ended_reason: None,
                report: report.map(|state| AssignmentReport {
                    state,
                    summary: String::new(),
                    at: "2026-01-01T00:00:01Z".into(),
                }),
                delivered_at: None,
                idle_at: None,
            }],
            inputs: Vec::new(),
            undelivered: BTreeSet::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            origin_trigger_id: "trigger".into(),
        }
    }

    #[test]
    fn report_states_follow_the_upstream_stage_table() {
        assert_eq!(
            ticket_stage(&ticket(Some(ReportState::Ready), AssignmentState::Running)),
            TicketStage::PersonReview
        );
        assert_eq!(
            ticket_stage(&ticket(
                Some(ReportState::Blocked),
                AssignmentState::Running
            )),
            TicketStage::PersonBlocked
        );
        assert_eq!(
            ticket_stage(&ticket(Some(ReportState::Failed), AssignmentState::Running)),
            TicketStage::PersonFailed
        );
        assert_eq!(
            ticket_stage(&ticket(
                Some(ReportState::Waiting),
                AssignmentState::Running
            )),
            TicketStage::Idle
        );
        assert_eq!(
            ticket_stage(&ticket(
                Some(ReportState::Progress),
                AssignmentState::Running
            )),
            TicketStage::WorkingRunning
        );
    }

    #[test]
    fn human_input_after_blocked_report_returns_the_ticket_to_working() {
        let mut value = ticket(Some(ReportState::Blocked), AssignmentState::Running);
        value.inputs.push(TicketInput {
            id: "input".into(),
            kind: TicketInputKind::Trigger,
            trigger_id: Some("followup".into()),
            text: None,
            human: true,
            at: "2026-01-01T00:00:02Z".into(),
            delivered_to: None,
            delivered_at: None,
            read_at: None,
        });
        assert_eq!(ticket_stage(&value), TicketStage::WorkingRunning);
    }
}
