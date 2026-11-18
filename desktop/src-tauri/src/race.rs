use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use thiserror::Error;

pub const RACE_SCHEMA_VERSION: u8 = 1;
pub const TASK_CAPSULE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaceState {
    Prepared,
    Generating,
    Validating,
    Decided,
    AwaitingReview,
    NoWinner,
    Applied,
    Rejected,
    Stale,
    Cancelled,
}

impl RaceState {
    pub fn can_transition_to(self, next: Self) -> bool {
        use RaceState::*;
        matches!(
            (self, next),
            (Prepared, Generating)
                | (Prepared, Cancelled)
                | (Generating, Validating)
                | (Generating, Cancelled)
                | (Validating, Decided)
                | (Validating, NoWinner)
                | (Validating, Cancelled)
                | (Decided, AwaitingReview)
                | (Decided, NoWinner)
                | (AwaitingReview, Applied)
                | (AwaitingReview, Rejected)
                | (AwaitingReview, Stale)
        )
    }

    pub fn is_recoverable(self) -> bool {
        matches!(
            self,
            Self::Prepared
                | Self::Generating
                | Self::Validating
                | Self::Decided
                | Self::AwaitingReview
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Queued,
    Generating,
    Proposed,
    Validating,
    Passed,
    Failed,
    Rejected,
    Cancelled,
}

impl CandidateState {
    pub fn can_transition_to(self, next: Self) -> bool {
        use CandidateState::*;
        matches!(
            (self, next),
            (Queued, Generating)
                | (Queued, Cancelled)
                | (Generating, Proposed)
                | (Generating, Failed)
                | (Generating, Cancelled)
                | (Proposed, Validating)
                | (Proposed, Rejected)
                | (Proposed, Cancelled)
                | (Validating, Passed)
                | (Validating, Failed)
                | (Validating, Rejected)
                | (Validating, Cancelled)
        )
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("illegal race transition: {from:?} -> {to:?}")]
    Race { from: RaceState, to: RaceState },
    #[error("illegal candidate transition: {from:?} -> {to:?}")]
    Candidate {
        from: CandidateState,
        to: CandidateState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStrategy {
    MinimalChange,
    CompatibilityFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardFailure {
    WorkspaceEscape,
    SyntaxError,
    InterfaceLost,
    RequiredTestFailed,
    TestDeleted,
    NonReplayable,
    EmptyChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationFact {
    pub check_id: String,
    pub outcome: ValidationOutcome,
    pub summary: String,
    #[serde(default)]
    pub required: bool,
    pub hard_failure: Option<HardFailure>,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub evidence_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMetrics {
    pub modified_files: u32,
    pub unrelated_files: u32,
    pub deleted_files: u32,
    pub changed_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Race {
    pub schema_version: u8,
    pub id: String,
    pub ticket_id: String,
    pub project_fingerprint: String,
    pub objective: String,
    pub state: RaceState,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    pub verdict: Option<Verdict>,
    pub created_at: String,
    pub updated_at: String,
}

impl Race {
    pub fn transition(
        &mut self,
        next: RaceState,
        at: impl Into<String>,
    ) -> Result<(), TransitionError> {
        if !self.state.can_transition_to(next) {
            return Err(TransitionError::Race {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        self.updated_at = at.into();
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub schema_version: u8,
    pub id: String,
    pub race_id: String,
    pub strategy: CandidateStrategy,
    pub state: CandidateState,
    pub provider: String,
    pub model: String,
    pub workspace_path: Option<String>,
    pub diff: Option<String>,
    pub base_digest: String,
    pub result_digest: Option<String>,
    #[serde(default)]
    pub facts: Vec<ValidationFact>,
    #[serde(default)]
    pub metrics: ChangeMetrics,
    pub generation_attempts: u8,
    pub created_at: String,
    pub updated_at: String,
}

impl Candidate {
    pub fn transition(
        &mut self,
        next: CandidateState,
        at: impl Into<String>,
    ) -> Result<(), TransitionError> {
        if !self.state.can_transition_to(next) {
            return Err(TransitionError::Candidate {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        self.updated_at = at.into();
        Ok(())
    }

    fn disqualifying_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.state != CandidateState::Passed {
            reasons.push(format!("candidate state is {:?}, not passed", self.state));
        }
        if self
            .diff
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
            || self.metrics.modified_files == 0
            || self.metrics.changed_lines == 0
        {
            reasons.push("candidate contains no replayable change".into());
        }
        for fact in &self.facts {
            if let Some(failure) = fact.hard_failure {
                reasons.push(format!("hard failure {failure:?}: {}", fact.summary));
            } else if fact.required && fact.outcome != ValidationOutcome::Passed {
                reasons.push(format!("required check {} did not pass", fact.check_id));
            }
        }
        reasons
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Winner,
    NoWinner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateDecision {
    pub candidate_id: String,
    pub eligible: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub schema_version: u8,
    pub kind: VerdictKind,
    pub winner_id: Option<String>,
    #[serde(default)]
    pub ranked_candidate_ids: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<CandidateDecision>,
    pub summary: String,
    pub decided_at: String,
}

/// Selects only mechanically passing candidates. This function is deliberately
/// model-free and deterministic: metrics rank candidates, then ID breaks ties.
pub fn judge(candidates: &[Candidate], decided_at: impl Into<String>) -> Verdict {
    let mut decisions = Vec::with_capacity(candidates.len());
    let mut eligible = Vec::new();
    for candidate in candidates {
        let reasons = candidate.disqualifying_reasons();
        if reasons.is_empty() {
            eligible.push(candidate);
        }
        decisions.push(CandidateDecision {
            candidate_id: candidate.id.clone(),
            eligible: reasons.is_empty(),
            reasons,
        });
    }
    eligible.sort_by(|left, right| compare_candidates(left, right));
    let ranked_candidate_ids = eligible
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect();
    let winner_id = eligible.first().map(|candidate| candidate.id.clone());
    let kind = if winner_id.is_some() {
        VerdictKind::Winner
    } else {
        VerdictKind::NoWinner
    };
    let summary = winner_id
        .as_ref()
        .map(|id| format!("{id} passed all required validation and ranked first"))
        .unwrap_or_else(|| "no candidate passed all required validation".into());
    Verdict {
        schema_version: RACE_SCHEMA_VERSION,
        kind,
        winner_id,
        ranked_candidate_ids,
        decisions,
        summary,
        decided_at: decided_at.into(),
    }
}

fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    left.metrics
        .modified_files
        .cmp(&right.metrics.modified_files)
        .then_with(|| {
            left.metrics
                .unrelated_files
                .cmp(&right.metrics.unrelated_files)
        })
        .then_with(|| left.metrics.deleted_files.cmp(&right.metrics.deleted_files))
        .then_with(|| left.metrics.changed_lines.cmp(&right.metrics.changed_lines))
        .then_with(|| left.id.cmp(&right.id))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapsuleCandidate {
    pub id: String,
    pub strategy: CandidateStrategy,
    pub state: CandidateState,
    pub provider: String,
    pub model: String,
    pub diff: Option<String>,
    pub result_digest: Option<String>,
    pub facts: Vec<ValidationFact>,
    pub metrics: ChangeMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCapsule {
    pub schema_version: u8,
    pub race_id: String,
    pub ticket_id: String,
    pub project_fingerprint: String,
    pub objective: String,
    pub allowed_paths: Vec<String>,
    pub acceptance: Vec<String>,
    pub candidates: Vec<CapsuleCandidate>,
    pub verdict: Option<Verdict>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn project_capsule(race: &Race, candidates: &[Candidate]) -> TaskCapsule {
    TaskCapsule {
        schema_version: TASK_CAPSULE_SCHEMA_VERSION,
        race_id: race.id.clone(),
        ticket_id: race.ticket_id.clone(),
        project_fingerprint: race.project_fingerprint.clone(),
        objective: race.objective.clone(),
        allowed_paths: race.allowed_paths.clone(),
        acceptance: race.acceptance.clone(),
        candidates: candidates
            .iter()
            .map(|candidate| CapsuleCandidate {
                id: candidate.id.clone(),
                strategy: candidate.strategy,
                state: candidate.state,
                provider: candidate.provider.clone(),
                model: candidate.model.clone(),
                diff: candidate.diff.clone(),
                result_digest: candidate.result_digest.clone(),
                facts: candidate.facts.clone(),
                metrics: candidate.metrics.clone(),
            })
            .collect(),
        verdict: race.verdict.clone(),
        created_at: race.created_at.clone(),
        updated_at: race.updated_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, state: CandidateState, metrics: ChangeMetrics) -> Candidate {
        Candidate {
            schema_version: RACE_SCHEMA_VERSION,
            id: id.into(),
            race_id: "race-1".into(),
            strategy: CandidateStrategy::MinimalChange,
            state,
            provider: "bundled".into(),
            model: "qwen".into(),
            workspace_path: Some(format!("private/{id}")),
            diff: Some("diff --git a/a b/a\n+fixed".into()),
            base_digest: "base".into(),
            result_digest: Some("result".into()),
            facts: vec![ValidationFact {
                check_id: "required-tests".into(),
                outcome: ValidationOutcome::Passed,
                summary: "2 passed".into(),
                required: true,
                hard_failure: None,
                command: Some("cargo test".into()),
                exit_code: Some(0),
                evidence_digests: vec!["digest".into()],
            }],
            metrics,
            generation_attempts: 1,
            created_at: "created".into(),
            updated_at: "updated".into(),
        }
    }

    #[test]
    fn state_machines_accept_only_declared_transitions() {
        let mut race = Race {
            schema_version: 1,
            id: "race-1".into(),
            ticket_id: "ticket-1".into(),
            project_fingerprint: "project".into(),
            objective: "fix".into(),
            state: RaceState::Prepared,
            allowed_paths: vec![],
            acceptance: vec![],
            verdict: None,
            created_at: "created".into(),
            updated_at: "created".into(),
        };
        race.transition(RaceState::Generating, "now").unwrap();
        assert_eq!(race.state, RaceState::Generating);
        assert!(matches!(
            race.transition(RaceState::Applied, "later"),
            Err(TransitionError::Race { .. })
        ));

        let mut candidate = candidate(
            "candidate-a",
            CandidateState::Queued,
            ChangeMetrics::default(),
        );
        candidate
            .transition(CandidateState::Generating, "now")
            .unwrap();
        assert!(matches!(
            candidate.transition(CandidateState::Passed, "later"),
            Err(TransitionError::Candidate { .. })
        ));
    }

    #[test]
    fn hard_failure_can_never_win() {
        let mut unsafe_candidate = candidate(
            "candidate-a",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 1,
                changed_lines: 1,
                ..Default::default()
            },
        );
        unsafe_candidate.facts.push(ValidationFact {
            check_id: "path-boundary".into(),
            outcome: ValidationOutcome::Failed,
            summary: "escaped project root".into(),
            required: true,
            hard_failure: Some(HardFailure::WorkspaceEscape),
            command: None,
            exit_code: None,
            evidence_digests: vec![],
        });
        let safe = candidate(
            "candidate-b",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 2,
                changed_lines: 20,
                ..Default::default()
            },
        );
        let verdict = judge(&[unsafe_candidate, safe], "now");
        assert_eq!(verdict.winner_id.as_deref(), Some("candidate-b"));
        assert!(!verdict.decisions[0].eligible);
    }

    #[test]
    fn all_failures_produce_no_winner() {
        let failed = candidate(
            "candidate-a",
            CandidateState::Failed,
            ChangeMetrics {
                modified_files: 1,
                changed_lines: 2,
                ..Default::default()
            },
        );
        let no_op = candidate(
            "candidate-b",
            CandidateState::Passed,
            ChangeMetrics::default(),
        );
        let verdict = judge(&[failed, no_op], "now");
        assert_eq!(verdict.kind, VerdictKind::NoWinner);
        assert!(verdict.winner_id.is_none());
        assert!(verdict.ranked_candidate_ids.is_empty());
    }

    #[test]
    fn passing_candidates_use_metrics_then_stable_id() {
        let larger = candidate(
            "candidate-a",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 2,
                changed_lines: 10,
                ..Default::default()
            },
        );
        let smaller_z = candidate(
            "candidate-z",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 1,
                changed_lines: 4,
                ..Default::default()
            },
        );
        let smaller_b = candidate(
            "candidate-b",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 1,
                changed_lines: 4,
                ..Default::default()
            },
        );
        let verdict = judge(&[larger, smaller_z, smaller_b], "now");
        assert_eq!(verdict.winner_id.as_deref(), Some("candidate-b"));
        assert_eq!(
            verdict.ranked_candidate_ids,
            vec!["candidate-b", "candidate-z", "candidate-a"]
        );
    }

    #[test]
    fn capsule_projection_is_serializable_and_excludes_workspace_path() {
        let mut race = Race {
            schema_version: 1,
            id: "race-1".into(),
            ticket_id: "ticket-1".into(),
            project_fingerprint: "project".into(),
            objective: "fix".into(),
            state: RaceState::AwaitingReview,
            allowed_paths: vec!["src/**".into()],
            acceptance: vec!["cargo test".into()],
            verdict: None,
            created_at: "created".into(),
            updated_at: "updated".into(),
        };
        let candidate = candidate(
            "candidate-a",
            CandidateState::Passed,
            ChangeMetrics {
                modified_files: 1,
                changed_lines: 2,
                ..Default::default()
            },
        );
        race.verdict = Some(judge(std::slice::from_ref(&candidate), "now"));
        let json = serde_json::to_string(&project_capsule(&race, &[candidate])).unwrap();
        assert!(!json.contains("workspacePath"));
        let restored: TaskCapsule = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.schema_version, TASK_CAPSULE_SCHEMA_VERSION);
        assert_eq!(
            restored.verdict.unwrap().winner_id.as_deref(),
            Some("candidate-a")
        );
    }
}
