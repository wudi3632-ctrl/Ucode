use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    Open,
    Queued,
    Running,
    Waiting,
    Stopping,
    Stopped,
    Completed,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Queued,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Queued,
    Thinking,
    ToolRequested,
    WaitingApproval,
    Executing,
    Completed,
    Failed,
    Cancelled,
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
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub id: String,
    pub ticket_id: String,
    pub state: AssignmentState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("invalid ticket transition from {from:?} to {to:?}")]
    InvalidTicket { from: TicketState, to: TicketState },
}

impl TicketState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Stopped | Self::Completed | Self::Failed | Self::RolledBack
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use TicketState::*;
        matches!(
            (self, next),
            (Open, Queued | Running | Failed)
                | (Queued, Running | Stopping | Failed)
                | (Running, Waiting | Stopping | Completed | Failed)
                | (Waiting, Running | Stopping | Failed)
                | (Stopping, Stopped | Failed)
                | (Completed, RolledBack)
        )
    }

    pub fn transition(
        &mut self,
        next: Self,
        assignments: &mut [Assignment],
    ) -> Result<(), TransitionError> {
        if !self.can_transition_to(next) {
            return Err(TransitionError::InvalidTicket {
                from: *self,
                to: next,
            });
        }
        *self = next;
        if next.is_terminal() {
            for assignment in assignments {
                assignment.state = match next {
                    Self::Completed => AssignmentState::Completed,
                    Self::Stopped | Self::RolledBack => AssignmentState::Cancelled,
                    Self::Failed => AssignmentState::Failed,
                    _ => unreachable!("terminal state was checked"),
                };
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_ticket_closes_assignments() {
        let mut state = TicketState::Running;
        let mut assignments = vec![Assignment {
            id: "assignment-1".into(),
            ticket_id: "ticket-1".into(),
            state: AssignmentState::Running,
        }];

        state
            .transition(TicketState::Completed, &mut assignments)
            .unwrap();

        assert_eq!(state, TicketState::Completed);
        assert_eq!(assignments[0].state, AssignmentState::Completed);
    }

    #[test]
    fn terminal_ticket_cannot_be_restarted() {
        let mut state = TicketState::Completed;
        let error = state.transition(TicketState::Running, &mut []).unwrap_err();
        assert_eq!(
            error,
            TransitionError::InvalidTicket {
                from: TicketState::Completed,
                to: TicketState::Running
            }
        );
    }
}
