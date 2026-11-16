use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SqliteRepositoryError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ticket not found: {0}")]
    TicketNotFound(String),
    #[error("idempotency key {key} already belongs to {existing}")]
    IdempotencyConflict { key: String, existing: String },
    #[error("the stored mandate for ticket {0} cannot be changed")]
    MandateConflict(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistTriggerResult {
    Stored,
    Duplicate { trigger_id: String },
}

/// Transactional SQLite repository for the authoritative Superagent records.
/// The desktop storage migration owns schema creation; this adapter only accepts
/// a database that has already reached schema 6.
pub struct SqliteRepositories {
    connection: Connection,
}

impl SqliteRepositories {
    pub fn open(path: &Path) -> Result<Self, SqliteRepositoryError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { connection })
    }

    pub fn persist_trigger(
        &mut self,
        thread: &Thread,
        trigger: &Trigger,
    ) -> Result<PersistTriggerResult, SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        if let Some(trigger_id) =
            idempotent_target(&transaction, "trigger", &trigger.client_request_id)?
        {
            transaction.commit()?;
            return Ok(PersistTriggerResult::Duplicate { trigger_id });
        }

        upsert_thread(&transaction, thread)?;
        transaction.execute(
            "INSERT INTO triggers(id,client_request_id,source,actor_json,human,addressed,at,text,meta_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                trigger.id,
                trigger.client_request_id,
                trigger.source,
                serde_json::to_string(&trigger.actor)?,
                trigger.human,
                trigger.addressed.as_ref().map(enum_text).transpose()?,
                trigger.at,
                trigger.text,
                serde_json::to_string(&trigger.meta)?,
            ],
        )?;
        for (ordinal, reference) in trigger.threads.iter().enumerate() {
            let key = reference.key();
            if key != thread.key {
                transaction.execute(
                    "INSERT OR IGNORE INTO threads(key,conversation_id,project_id,source,source_id,title,created_at,updated_at) VALUES(?1,NULL,?2,?3,?4,?5,?6,?6)",
                    params![key, thread.project_id, reference.source, reference.id, thread.title, trigger.at],
                )?;
            }
            transaction.execute(
                "INSERT INTO trigger_threads(trigger_id,thread_key,ordinal) VALUES(?1,?2,?3)",
                params![trigger.id, key, ordinal as i64],
            )?;
        }
        transaction.execute(
            "INSERT INTO trigger_dispositions(trigger_id,fanout_json,read_by_json,created_at) VALUES(?1,'[]','[]',?2)",
            params![trigger.id, trigger.at],
        )?;
        insert_idempotency(
            &transaction,
            "trigger",
            &trigger.client_request_id,
            "trigger",
            &trigger.id,
            &digest(&serde_json::to_vec(trigger)?),
            &trigger.at,
        )?;
        transaction.commit()?;
        Ok(PersistTriggerResult::Stored)
    }

    pub fn persist_evaluation(
        &mut self,
        evaluation: &Evaluation,
    ) -> Result<(), SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO evaluations(id,thread_key,trigger_ids_json,strength,policy_version,state,started_at) VALUES(?1,?2,?3,?4,?5,'active',?6)",
            params![
                evaluation.id,
                evaluation.thread_key,
                serde_json::to_string(&evaluation.trigger_ids)?,
                enum_text(&evaluation.strength)?,
                evaluation.policy_version,
                evaluation.started_at,
            ],
        )?;
        for trigger_id in &evaluation.trigger_ids {
            transaction.execute(
                "UPDATE trigger_dispositions SET evaluation_id=?1 WHERE trigger_id=?2",
                params![evaluation.id, trigger_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn persist_decision(&mut self, decision: &Decision) -> Result<(), SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO decisions(id,evaluation_id,thread_key,trigger_ids_json,policy_version,kind,reason_code,ticket_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                decision.id,
                decision.evaluation_id,
                decision.thread_key,
                serde_json::to_string(&decision.trigger_ids)?,
                decision.policy_version,
                enum_text(&decision.kind)?,
                decision.reason_code,
                decision.ticket_id,
                decision.at,
            ],
        )?;
        transaction.execute(
            "UPDATE evaluations SET state='finished',finished_at=?1 WHERE id=?2",
            params![decision.at, decision.evaluation_id],
        )?;
        for trigger_id in &decision.trigger_ids {
            transaction.execute(
                "UPDATE trigger_dispositions SET decision_id=?1 WHERE trigger_id=?2",
                params![decision.id, trigger_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn persist_ticket(&mut self, ticket: &Ticket) -> Result<(), SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        let stored_mandate = load_mandate(&transaction, &ticket.id)?;
        if let Some(stored) = stored_mandate {
            if stored != ticket.mandate {
                return Err(SqliteRepositoryError::MandateConflict(ticket.id.clone()));
            }
        } else {
            let request: String = transaction.query_row(
                "SELECT text FROM triggers WHERE id=?1",
                [&ticket.origin_trigger_id],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO tickets(id,conversation_id,project_id,request,title,state,policy_version,created_at,updated_at,thread_key,superagent_decision_id,ended_json,attention_json,origin_trigger_id,search_text) VALUES(?1,NULL,?2,?3,?4,'open',?5,?6,?7,?8,?9,?10,?11,?12,lower(?4 || ' ' || ?3))",
                params![
                    ticket.id,
                    ticket.mandate.project_id(),
                    request,
                    ticket.title,
                    ticket.mandate.policy_version().parse::<i64>().unwrap_or(6),
                    ticket.created_at,
                    ticket.updated_at,
                    ticket.thread_key,
                    ticket.decision_id,
                    option_json(&ticket.ended)?,
                    option_json(&ticket.attention)?,
                    ticket.origin_trigger_id,
                ],
            )?;
            insert_mandate(&transaction, ticket)?;
        }
        persist_ticket_state(&transaction, ticket)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn load_ticket(&self, ticket_id: &str) -> Result<Ticket, SqliteRepositoryError> {
        let mandate = load_mandate(&self.connection, ticket_id)?
            .ok_or_else(|| SqliteRepositoryError::TicketNotFound(ticket_id.to_string()))?;
        let mut ticket = self.connection.query_row(
            "SELECT thread_key,title,superagent_decision_id,ended_json,attention_json,created_at,updated_at,origin_trigger_id FROM tickets WHERE id=?1",
            [ticket_id],
            |row| {
                Ok(Ticket {
                    id: ticket_id.to_string(),
                    thread_key: row.get(0)?,
                    title: row.get(1)?,
                    decision_id: row.get(2)?,
                    mandate: mandate.clone(),
                    ended: json_option(row.get::<_, Option<String>>(3)?)?,
                    attention: json_option(row.get::<_, Option<String>>(4)?)?,
                    assignments: Vec::new(),
                    inputs: Vec::new(),
                    undelivered: BTreeSet::new(),
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    origin_trigger_id: row.get(7)?,
                })
            },
        )?;

        let mut assignments = self.connection.prepare(
            "SELECT id,superagent_state,created_at,started_at,ended_at,ended_reason,delivered_at,idle_at FROM assignments WHERE ticket_id=?1 ORDER BY created_at,id",
        )?;
        ticket.assignments = assignments
            .query_map([ticket_id], |row| {
                let id: String = row.get(0)?;
                let report = self.connection.query_row(
                    "SELECT state,summary,created_at FROM reports WHERE assignment_id=?1 ORDER BY created_at DESC LIMIT 1",
                    [&id],
                    |report| {
                        Ok(AssignmentReport {
                            state: parse_enum(report.get::<_, String>(0)?)?,
                            summary: report.get(1)?,
                            at: report.get(2)?,
                        })
                    },
                ).optional()?;
                Ok(Assignment {
                    id,
                    state: parse_enum(row.get::<_, String>(1)?)?,
                    created_at: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                    ended_reason: row.get(5)?,
                    report,
                    delivered_at: row.get(6)?,
                    idle_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(assignments);

        let mut inputs = self.connection.prepare(
            "SELECT id,kind,trigger_id,text,human,created_at,delivered_to,delivered_at,read_at,undelivered FROM inputs WHERE ticket_id=?1 ORDER BY created_at,id",
        )?;
        let rows = inputs.query_map([ticket_id], |row| {
            let undelivered: bool = row.get(9)?;
            let input = TicketInput {
                id: row.get(0)?,
                kind: parse_enum(row.get::<_, String>(1)?)?,
                trigger_id: row.get(2)?,
                text: row.get(3)?,
                human: row.get(4)?,
                at: row.get(5)?,
                delivered_to: row.get(6)?,
                delivered_at: row.get(7)?,
                read_at: row.get(8)?,
            };
            Ok((input, undelivered))
        })?;
        for row in rows {
            let (input, undelivered) = row?;
            if undelivered {
                ticket.undelivered.insert(input.id.clone());
            }
            ticket.inputs.push(input);
        }
        Ok(ticket)
    }

    pub fn persist_effect(&mut self, effect: &Effect) -> Result<Effect, SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        if let Some(existing) = load_effect_by_key(&transaction, &effect.key)? {
            if existing.payload_digest != effect.payload_digest {
                return Err(SqliteRepositoryError::IdempotencyConflict {
                    key: effect.key.clone(),
                    existing: existing.id,
                });
            }
            transaction.commit()?;
            return Ok(existing);
        }
        transaction.execute(
            "INSERT INTO effects(id,ticket_id,risk,state,created_at,updated_at,effect_key,kind,payload_digest,attempts,resolved_at,idempotency_key) VALUES(?1,?2,'mandated',?3,?4,?4,?5,?6,?7,?8,?9,?5)",
            params![effect.id,effect.ticket_id,enum_text(&effect.state)?,effect.created_at,effect.key,effect.kind,effect.payload_digest,effect.attempts,effect.resolved_at],
        )?;
        transaction.commit()?;
        Ok(effect.clone())
    }

    pub fn persist_delivery(
        &mut self,
        thread_key: &str,
        idempotency_key: &str,
        delivery: &Delivery,
    ) -> Result<Delivery, SqliteRepositoryError> {
        let transaction = self.connection.transaction()?;
        if let Some(existing) = load_delivery_by_key(&transaction, idempotency_key)? {
            if existing.content_digest != delivery.content_digest {
                return Err(SqliteRepositoryError::IdempotencyConflict {
                    key: idempotency_key.to_string(),
                    existing: existing.id,
                });
            }
            transaction.commit()?;
            return Ok(existing);
        }
        transaction.execute(
            "INSERT INTO deliveries(id,thread_key,ticket_id,trigger_id,kind,summary,evidence_json,content_digest,idempotency_key,created_at) VALUES(?1,?2,?3,?4,?5,'','[]',?6,?7,?8)",
            params![delivery.id,thread_key,delivery.ticket_id,delivery.trigger_id,delivery.kind,delivery.content_digest,idempotency_key,delivery.created_at],
        )?;
        transaction.commit()?;
        Ok(delivery.clone())
    }
}

fn upsert_thread(
    transaction: &Transaction<'_>,
    thread: &Thread,
) -> Result<(), SqliteRepositoryError> {
    transaction.execute(
        "INSERT INTO threads(key,conversation_id,project_id,source,source_id,title,created_at,updated_at) VALUES(?1,NULL,?2,?3,?4,?5,?6,?7) ON CONFLICT(key) DO UPDATE SET title=excluded.title,updated_at=excluded.updated_at",
        params![thread.key,thread.project_id,thread.reference.source,thread.reference.id,thread.title,thread.created_at,thread.updated_at],
    )?;
    Ok(())
}

fn insert_mandate(
    transaction: &Transaction<'_>,
    ticket: &Ticket,
) -> Result<(), SqliteRepositoryError> {
    let mandate = &ticket.mandate;
    transaction.execute(
        "INSERT INTO mandates(ticket_id,strength,policy_version,basis_json,actor_json,project_id,project_fingerprint,capabilities_json,trigger_id,thread_key,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![ticket.id,enum_text(&mandate.strength())?,mandate.policy_version(),serde_json::to_string(mandate.basis())?,serde_json::to_string(mandate.actor())?,mandate.project_id(),mandate.project_fingerprint(),serde_json::to_string(mandate.capabilities())?,mandate.trigger_id(),mandate.thread_key(),ticket.created_at],
    )?;
    Ok(())
}

fn persist_ticket_state(
    transaction: &Transaction<'_>,
    ticket: &Ticket,
) -> Result<(), SqliteRepositoryError> {
    transaction.execute(
        "UPDATE tickets SET title=?1,state=?2,updated_at=?3,ended_json=?4,attention_json=?5 WHERE id=?6",
        params![ticket.title,if ticket.ended.is_some() { "completed" } else { "open" },ticket.updated_at,option_json(&ticket.ended)?,option_json(&ticket.attention)?,ticket.id],
    )?;
    for assignment in &ticket.assignments {
        transaction.execute(
            "INSERT INTO assignments(id,ticket_id,state,superagent_state,created_at,updated_at,started_at,ended_at,ended_reason,delivered_at,idle_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET state=excluded.state,superagent_state=excluded.superagent_state,updated_at=excluded.updated_at,started_at=excluded.started_at,ended_at=excluded.ended_at,ended_reason=excluded.ended_reason,delivered_at=excluded.delivered_at,idle_at=excluded.idle_at",
            params![assignment.id,ticket.id,legacy_assignment_state(assignment.state),enum_text(&assignment.state)?,assignment.created_at,ticket.updated_at,assignment.started_at,assignment.ended_at,assignment.ended_reason,assignment.delivered_at,assignment.idle_at],
        )?;
        if let Some(report) = &assignment.report {
            transaction.execute(
                "INSERT OR REPLACE INTO reports(id,assignment_id,state,summary,created_at) VALUES(?1,?2,?3,?4,?5)",
                params![format!("report:{}:{}",assignment.id,report.at),assignment.id,enum_text(&report.state)?,report.summary,report.at],
            )?;
        }
    }
    for input in &ticket.inputs {
        transaction.execute(
            "INSERT INTO inputs(id,ticket_id,kind,trigger_id,text,human,created_at,delivered_to,delivered_at,read_at,undelivered) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET text=excluded.text,human=excluded.human,delivered_to=excluded.delivered_to,delivered_at=excluded.delivered_at,read_at=excluded.read_at,undelivered=excluded.undelivered",
            params![input.id,ticket.id,enum_text(&input.kind)?,input.trigger_id,input.text,input.human,input.at,input.delivered_to,input.delivered_at,input.read_at,ticket.undelivered.contains(&input.id)],
        )?;
    }
    Ok(())
}

fn load_mandate(
    connection: &Connection,
    ticket_id: &str,
) -> Result<Option<Mandate>, SqliteRepositoryError> {
    Ok(connection.query_row(
        "SELECT strength,policy_version,basis_json,actor_json,project_id,project_fingerprint,capabilities_json,trigger_id,thread_key FROM mandates WHERE ticket_id=?1",
        [ticket_id],
        |row| {
            Ok(Mandate::new(
                parse_enum(row.get::<_, String>(0)?)?,
                row.get::<_, String>(1)?,
                parse_json(row.get::<_, String>(2)?)?,
                parse_json(row.get::<_, String>(3)?)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                parse_json::<BTreeSet<Capability>>(row.get::<_, String>(6)?)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        },
    ).optional()?)
}

fn load_effect_by_key(
    transaction: &Transaction<'_>,
    key: &str,
) -> Result<Option<Effect>, SqliteRepositoryError> {
    Ok(transaction.query_row(
        "SELECT id,ticket_id,effect_key,kind,payload_digest,state,attempts,created_at,resolved_at FROM effects WHERE effect_key=?1",
        [key],
        |row| Ok(Effect { id: row.get(0)?, ticket_id: row.get(1)?, key: row.get(2)?, kind: row.get(3)?, payload_digest: row.get(4)?, state: parse_enum(row.get::<_,String>(5)?)?, attempts: row.get(6)?, created_at: row.get(7)?, resolved_at: row.get(8)? }),
    ).optional()?)
}

fn load_delivery_by_key(
    transaction: &Transaction<'_>,
    key: &str,
) -> Result<Option<Delivery>, SqliteRepositoryError> {
    Ok(transaction.query_row(
        "SELECT id,ticket_id,trigger_id,kind,content_digest,created_at FROM deliveries WHERE idempotency_key=?1",
        [key],
        |row| Ok(Delivery { id: row.get(0)?, ticket_id: row.get(1)?, trigger_id: row.get(2)?, kind: row.get(3)?, content_digest: row.get(4)?, created_at: row.get(5)? }),
    ).optional()?)
}

fn idempotent_target(
    transaction: &Transaction<'_>,
    namespace: &str,
    key: &str,
) -> Result<Option<String>, rusqlite::Error> {
    transaction
        .query_row(
            "SELECT target_id FROM idempotency_keys WHERE namespace=?1 AND key=?2",
            params![namespace, key],
            |row| row.get(0),
        )
        .optional()
}

#[allow(clippy::too_many_arguments)]
fn insert_idempotency(
    transaction: &Transaction<'_>,
    namespace: &str,
    key: &str,
    target_kind: &str,
    target_id: &str,
    content_digest: &str,
    created_at: &str,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "INSERT INTO idempotency_keys(namespace,key,target_kind,target_id,content_digest,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![namespace,key,target_kind,target_id,content_digest,created_at],
    )?;
    Ok(())
}

fn enum_text<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_value(value).and_then(|value| match value {
        serde_json::Value::String(value) => Ok(value),
        other => serde_json::to_string(&other),
    })
}

fn parse_enum<T: DeserializeOwned>(value: String) -> Result<T, rusqlite::Error> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn parse_json<T: DeserializeOwned>(value: String) -> Result<T, rusqlite::Error> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn json_option<T: DeserializeOwned>(value: Option<String>) -> Result<Option<T>, rusqlite::Error> {
    value.map(parse_json).transpose()
}

fn option_json<T: serde::Serialize>(
    value: &Option<T>,
) -> Result<Option<String>, serde_json::Error> {
    value.as_ref().map(serde_json::to_string).transpose()
}

fn legacy_assignment_state(state: AssignmentState) -> &'static str {
    match state {
        AssignmentState::Pending => "queued",
        AssignmentState::Running => "running",
        AssignmentState::Ended => "completed",
    }
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::ProjectLocator;
    use crate::storage::Storage;
    use std::collections::BTreeSet;
    use tempfile::tempdir;

    fn trigger(project_id: &str) -> (Thread, Trigger) {
        let reference = ThreadRef {
            source: "desktop".into(),
            id: "conversation".into(),
        };
        (
            Thread {
                key: reference.key(),
                reference: reference.clone(),
                project_id: project_id.into(),
                title: "Conversation".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                trigger_ids: vec!["trigger".into()],
                subscriptions: BTreeSet::new(),
                evaluating: None,
                pending: vec!["trigger".into()],
            },
            Trigger {
                id: "trigger".into(),
                client_request_id: "request".into(),
                source: "desktop".into(),
                threads: vec![reference],
                actor: ActorRef {
                    source: "desktop".into(),
                    id: "person".into(),
                },
                human: true,
                addressed: Some(Addressed::Direct),
                at: "2026-01-01T00:00:00Z".into(),
                text: "Fix it".into(),
                meta: serde_json::json!({}),
            },
        )
    }

    fn repository() -> (tempfile::TempDir, SqliteRepositories, String) {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        let mut storage = Storage::open(&database).unwrap();
        let project = storage
            .upsert_project(
                &ProjectLocator {
                    absolute_path: directory.path().display().to_string(),
                    relative_path: None,
                    fingerprint: "sqlite-repository".into(),
                },
                true,
            )
            .unwrap();
        drop(storage);
        (
            directory,
            SqliteRepositories::open(&database).unwrap(),
            project.id,
        )
    }

    #[test]
    fn normalized_ticket_graph_round_trips_and_preserves_mandate() {
        let (_directory, mut repository, project_id) = repository();
        let (thread, trigger) = trigger(&project_id);
        assert_eq!(
            repository.persist_trigger(&thread, &trigger).unwrap(),
            PersistTriggerResult::Stored
        );
        assert_eq!(
            repository.persist_trigger(&thread, &trigger).unwrap(),
            PersistTriggerResult::Duplicate {
                trigger_id: trigger.id.clone()
            }
        );
        let evaluation = Evaluation {
            id: "evaluation".into(),
            thread_key: thread.key.clone(),
            trigger_ids: vec![trigger.id.clone()],
            strength: Strength::Explicit,
            policy_version: "policy-1".into(),
            started_at: "2026-01-01T00:00:01Z".into(),
        };
        repository.persist_evaluation(&evaluation).unwrap();
        let decision = Decision {
            id: "decision".into(),
            evaluation_id: evaluation.id.clone(),
            thread_key: thread.key.clone(),
            trigger_ids: evaluation.trigger_ids.clone(),
            policy_version: evaluation.policy_version.clone(),
            at: "2026-01-01T00:00:02Z".into(),
            kind: DecisionKind::Accept,
            reason_code: "explicit".into(),
            ticket_id: Some("ticket".into()),
        };
        repository.persist_decision(&decision).unwrap();
        let mandate = Mandate::new(
            Strength::Explicit,
            "policy-1",
            MandateBasis::DesktopDirect,
            trigger.actor.clone(),
            project_id,
            "sqlite-repository",
            [Capability::Read, Capability::ProposeWrite],
            &trigger.id,
            &thread.key,
        );
        let input = TicketInput {
            id: "input".into(),
            kind: TicketInputKind::Trigger,
            trigger_id: Some(trigger.id.clone()),
            text: None,
            human: true,
            at: "2026-01-01T00:00:02Z".into(),
            delivered_to: None,
            delivered_at: None,
            read_at: None,
        };
        let ticket = Ticket {
            id: "ticket".into(),
            thread_key: thread.key,
            title: "Fix it".into(),
            decision_id: decision.id,
            mandate,
            ended: None,
            attention: None,
            assignments: vec![Assignment {
                id: "assignment".into(),
                state: AssignmentState::Pending,
                created_at: "2026-01-01T00:00:02Z".into(),
                started_at: None,
                ended_at: None,
                ended_reason: None,
                report: None,
                delivered_at: None,
                idle_at: None,
            }],
            inputs: vec![input.clone()],
            undelivered: BTreeSet::from([input.id]),
            created_at: "2026-01-01T00:00:02Z".into(),
            updated_at: "2026-01-01T00:00:02Z".into(),
            origin_trigger_id: trigger.id,
        };
        repository.persist_ticket(&ticket).unwrap();
        assert_eq!(repository.load_ticket(&ticket.id).unwrap(), ticket);

        let mut changed = ticket.clone();
        changed.mandate = Mandate::new(
            Strength::Explicit,
            "policy-2",
            MandateBasis::Migration,
            changed.mandate.actor().clone(),
            changed.mandate.project_id(),
            changed.mandate.project_fingerprint(),
            [Capability::Terminal],
            changed.mandate.trigger_id(),
            changed.mandate.thread_key(),
        );
        assert!(matches!(
            repository.persist_ticket(&changed),
            Err(SqliteRepositoryError::MandateConflict(_))
        ));
        assert_eq!(repository.load_ticket(&ticket.id).unwrap(), ticket);
    }

    #[test]
    fn effect_and_delivery_keys_are_transactionally_idempotent() {
        let (_directory, mut repository, project_id) = repository();
        let (thread, trigger) = trigger(&project_id);
        repository.persist_trigger(&thread, &trigger).unwrap();

        // Foreign keys require a real ticket for an Effect, while a chat Delivery may be ticketless.
        let evaluation = Evaluation {
            id: "evaluation".into(),
            thread_key: thread.key.clone(),
            trigger_ids: vec![trigger.id.clone()],
            strength: Strength::Explicit,
            policy_version: "1".into(),
            started_at: trigger.at.clone(),
        };
        repository.persist_evaluation(&evaluation).unwrap();
        let decision = Decision {
            id: "decision".into(),
            evaluation_id: evaluation.id.clone(),
            thread_key: thread.key.clone(),
            trigger_ids: evaluation.trigger_ids.clone(),
            policy_version: "1".into(),
            at: trigger.at.clone(),
            kind: DecisionKind::Accept,
            reason_code: "test".into(),
            ticket_id: Some("ticket".into()),
        };
        repository.persist_decision(&decision).unwrap();
        let ticket = Ticket {
            id: "ticket".into(),
            thread_key: thread.key.clone(),
            title: "test".into(),
            decision_id: decision.id,
            mandate: Mandate::new(
                Strength::Explicit,
                "1",
                MandateBasis::DesktopDirect,
                trigger.actor.clone(),
                project_id,
                "sqlite-repository",
                [Capability::ProposeWrite],
                &trigger.id,
                &thread.key,
            ),
            ended: None,
            attention: None,
            assignments: Vec::new(),
            inputs: Vec::new(),
            undelivered: BTreeSet::new(),
            created_at: trigger.at.clone(),
            updated_at: trigger.at.clone(),
            origin_trigger_id: trigger.id.clone(),
        };
        repository.persist_ticket(&ticket).unwrap();
        let effect = Effect {
            id: "effect".into(),
            ticket_id: ticket.id.clone(),
            key: "write:file:digest".into(),
            kind: "write_file".into(),
            payload_digest: "digest".into(),
            state: EffectState::Proposed,
            attempts: 0,
            created_at: trigger.at.clone(),
            resolved_at: None,
        };
        assert_eq!(repository.persist_effect(&effect).unwrap(), effect);
        assert_eq!(repository.persist_effect(&effect).unwrap(), effect);
        let mut conflict = effect.clone();
        conflict.id = "effect-2".into();
        conflict.payload_digest = "different".into();
        assert!(matches!(
            repository.persist_effect(&conflict),
            Err(SqliteRepositoryError::IdempotencyConflict { .. })
        ));

        let delivery = Delivery {
            id: "delivery".into(),
            ticket_id: None,
            trigger_id: trigger.id,
            kind: "answer".into(),
            content_digest: "answer-digest".into(),
            created_at: trigger.at,
        };
        assert_eq!(
            repository
                .persist_delivery(&thread.key, "answer:request", &delivery)
                .unwrap(),
            delivery
        );
        assert_eq!(
            repository
                .persist_delivery(&thread.key, "answer:request", &delivery)
                .unwrap(),
            delivery
        );
        assert_eq!(
            repository
                .connection
                .query_row("SELECT COUNT(*) FROM effects", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            repository
                .connection
                .query_row("SELECT COUNT(*) FROM deliveries", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
