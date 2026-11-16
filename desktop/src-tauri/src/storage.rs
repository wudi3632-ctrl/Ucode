use crate::domain::{AssignmentState, TicketState};
use crate::portable::ProjectLocator;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

const CURRENT_SCHEMA_VERSION: i64 = 8;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("ticket not found: {0}")]
    TicketNotFound(String),
    #[error("database schema {found} is newer than this UCode build supports ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub request: String,
    pub state: TicketState,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
    pub archived_at: Option<String>,
    pub parent_ticket_id: Option<String>,
    pub branch_event_sequence: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AppSettings {
    pub stream_enabled: bool,
    pub terminal_shell: String,
    pub git_user_name: String,
    pub git_user_email: String,
    pub theme: String,
    pub reduce_motion: bool,
    pub sound_enabled: bool,
    pub sound_volume: u8,
    pub completion_notifications_enabled: bool,
    pub allow_network_providers: bool,
    pub model_provider: String,
    pub bundled_model_profile: String,
    pub provider_base_url: String,
    pub provider_model: String,
    pub ui_font_size: u8,
    pub code_font_size: u8,
    pub sidebar_width: u16,
    pub inspector_width: u16,
}

pub const ONBOARDING_STEPS: [&str; 7] = [
    "project",
    "conversation",
    "review_change",
    "terminal",
    "git",
    "agent",
    "evidence",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct OnboardingState {
    pub schema_version: u8,
    pub welcome_state: String,
    pub completed_steps: Vec<String>,
    pub skipped_steps: Vec<String>,
    pub dismissed_tips: Vec<String>,
    pub updated_at: String,
    pub demo_project_id: Option<String>,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            welcome_state: "new".into(),
            completed_steps: Vec::new(),
            skipped_steps: Vec::new(),
            dismissed_tips: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
            demo_project_id: None,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            stream_enabled: true,
            terminal_shell: "powershell.exe".into(),
            git_user_name: String::new(),
            git_user_email: String::new(),
            theme: "dark".into(),
            reduce_motion: false,
            sound_enabled: true,
            sound_volume: 55,
            completion_notifications_enabled: true,
            allow_network_providers: false,
            model_provider: "bundled".into(),
            bundled_model_profile: crate::model_packs::DEFAULT_MODEL_PROFILE.into(),
            provider_base_url: "http://127.0.0.1:1234/v1".into(),
            provider_model: String::new(),
            ui_font_size: 18,
            code_font_size: 14,
            sidebar_width: 276,
            inspector_width: 404,
        }
    }
}

impl AppSettings {
    fn normalized(mut self) -> Self {
        let defaults = Self::default();
        if !matches!(self.theme.as_str(), "dark" | "light" | "system") {
            self.theme = defaults.theme;
        }
        self.sound_volume = self.sound_volume.min(100);
        self.ui_font_size = self.ui_font_size.clamp(14, 32);
        self.code_font_size = self.code_font_size.clamp(12, 28);
        self.sidebar_width = self.sidebar_width.clamp(210, 420);
        self.inspector_width = self.inspector_width.clamp(320, 760);
        if !matches!(self.model_provider.as_str(), "bundled" | "openai-compatible") {
            self.model_provider = defaults.model_provider;
        }
        let shell_name = std::path::Path::new(self.terminal_shell.trim())
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(shell_name.as_str(), "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" | "cmd" | "cmd.exe") {
            self.terminal_shell = defaults.terminal_shell;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub sequence: i64,
    pub ticket_id: Option<String>,
    pub event_type: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReceipt {
    pub ticket_id: String,
    pub request: String,
    pub route: Option<String>,
    pub outcome: String,
    pub sources: Vec<Value>,
    pub commands: Vec<Value>,
    pub tests: Vec<Value>,
    pub effects: Vec<Value>,
    pub decisions: Vec<Value>,
    pub final_state: Value,
    pub content_digest: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskContext {
    pub objective: String,
    pub target_files: Vec<String>,
    pub constraints: Vec<String>,
    pub verification: Vec<String>,
    pub confirmed_facts: Vec<String>,
    pub unresolved_questions: Vec<String>,
    pub revision: u32,
    pub updated_at: String,
}

impl TaskContext {
    fn from_request(request: &str) -> Self {
        let mut context = Self {
            objective: request.trim().to_string(),
            target_files: extract_context_paths(request),
            constraints: extract_context_clauses(request, ContextClause::Constraint),
            verification: extract_context_clauses(request, ContextClause::Verification),
            confirmed_facts: Vec::new(),
            unresolved_questions: Vec::new(),
            revision: 1,
            updated_at: Utc::now().to_rfc3339(),
        };
        context.refresh_questions(request);
        context
    }

    fn merge_message(&mut self, message: &str) {
        let value = message.trim();
        if value.is_empty() {
            return;
        }
        let paths = extract_context_paths(value);
        let correction = contains_context_marker(
            value,
            &["改为", "改成", "不是", "而是", "只修改", "仅修改", "目标是", "instead", "only edit"],
        );
        if correction {
            self.objective = value.to_string();
            if !paths.is_empty() {
                self.target_files = paths;
            }
        } else {
            extend_unique(&mut self.target_files, paths);
        }
        extend_unique(&mut self.constraints, extract_context_clauses(value, ContextClause::Constraint));
        extend_unique(&mut self.verification, extract_context_clauses(value, ContextClause::Verification));
        self.revision = self.revision.saturating_add(1);
        self.updated_at = Utc::now().to_rfc3339();
        self.refresh_questions(value);
    }

    fn refresh_questions(&mut self, latest: &str) {
        self.unresolved_questions.clear();
        if looks_like_mutation(&self.objective) && self.target_files.is_empty() {
            self.unresolved_questions.push("需要明确目标文件或允许的项目范围".into());
        }
        if looks_like_mutation(&self.objective)
            && self.verification.is_empty()
            && !contains_context_marker(latest, &["不用测试", "无需测试", "不运行测试", "without tests"])
        {
            self.unresolved_questions.push("需要明确验证方式或确认无需运行测试".into());
        }
    }
}

#[derive(Clone, Copy)]
enum ContextClause {
    Constraint,
    Verification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentSummary {
    pub id: String,
    pub ticket_id: String,
    pub parent_id: Option<String>,
    pub label: String,
    pub request: String,
    pub capability: String,
    pub state: AssignmentState,
    pub result: Option<String>,
    pub error: Option<String>,
    pub external_id: Option<String>,
    pub worktree_path: Option<String>,
    pub diff: Option<String>,
    pub merged_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub relative_path: String,
    pub capabilities: Vec<String>,
    pub instructions_digest: String,
    pub compatible: bool,
    pub compatibility_reason: Option<String>,
    pub enabled: bool,
    pub installed_at: String,
}

pub struct Storage {
    connection: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let from_version = schema_version(&connection)?;
        if from_version > CURRENT_SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchema {
                found: from_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        let backup_path = if from_version == 5 {
            backup_schema_five(&connection, path)?
        } else {
            None
        };
        let mut storage = Self { connection };
        storage.migrate(from_version, backup_path.as_deref())?;
        Ok(storage)
    }

    fn migrate(
        &mut self,
        from_version: i64,
        backup_path: Option<&Path>,
    ) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        transaction.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
              version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              absolute_path TEXT NOT NULL,
              relative_path TEXT,
              fingerprint TEXT NOT NULL UNIQUE,
              trusted INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS conversations (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
              title TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tickets (
              id TEXT PRIMARY KEY, conversation_id TEXT,
              project_id TEXT NOT NULL REFERENCES projects(id), request TEXT NOT NULL,
              title TEXT NOT NULL, state TEXT NOT NULL, policy_version INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS assignments (
              id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL REFERENCES tickets(id), parent_id TEXT,
              state TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS turns (
              id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL REFERENCES tickets(id), provider_id TEXT,
              model_id TEXT, state TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
              id TEXT PRIMARY KEY, turn_id TEXT, ticket_id TEXT NOT NULL REFERENCES tickets(id),
              role TEXT NOT NULL, content_json TEXT NOT NULL, ordinal INTEGER NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tool_calls (
              id TEXT PRIMARY KEY, turn_id TEXT, ticket_id TEXT NOT NULL REFERENCES tickets(id),
              tool_name TEXT NOT NULL, arguments_json TEXT NOT NULL, state TEXT NOT NULL,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS effects (
              id TEXT PRIMARY KEY, tool_call_id TEXT, ticket_id TEXT NOT NULL REFERENCES tickets(id),
              risk TEXT NOT NULL, state TEXT NOT NULL, before_digest TEXT, after_digest TEXT,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT, ticket_id TEXT,
              event_type TEXT NOT NULL, payload_json TEXT NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS deliveries (
              id TEXT PRIMARY KEY, ticket_id TEXT NOT NULL REFERENCES tickets(id),
              summary TEXT NOT NULL, evidence_json TEXT NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS skills (
              id TEXT PRIMARY KEY, version TEXT NOT NULL, signature TEXT, enabled INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS model_packs (
              id TEXT PRIMARY KEY, version TEXT NOT NULL, manifest_json TEXT NOT NULL,
              healthy INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY, value_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS external_refs (
              ticket_id TEXT NOT NULL REFERENCES tickets(id), adapter TEXT NOT NULL,
              external_id TEXT NOT NULL, PRIMARY KEY(ticket_id, adapter)
            );
            CREATE TABLE IF NOT EXISTS evidence_receipts (
              ticket_id TEXT PRIMARY KEY REFERENCES tickets(id) ON DELETE CASCADE,
              receipt_json TEXT NOT NULL, content_digest TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ticket_contexts (
              ticket_id TEXT PRIMARY KEY REFERENCES tickets(id) ON DELETE CASCADE,
              context_json TEXT NOT NULL, revision INTEGER NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )?;
        add_column_if_missing(
            &transaction,
            "tickets",
            "pinned",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(&transaction, "tickets", "archived_at", "TEXT")?;
        add_column_if_missing(&transaction, "tickets", "parent_ticket_id", "TEXT")?;
        add_column_if_missing(&transaction, "tickets", "branch_event_sequence", "INTEGER")?;
        add_column_if_missing(
            &transaction,
            "tickets",
            "search_text",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "assignments",
            "label",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "assignments",
            "request",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "assignments",
            "capability",
            "TEXT NOT NULL DEFAULT 'read'",
        )?;
        add_column_if_missing(&transaction, "assignments", "result", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "error", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "external_id", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "worktree_path", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "effect_json", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "diff_text", "TEXT")?;
        add_column_if_missing(&transaction, "assignments", "merged_at", "TEXT")?;
        add_column_if_missing(&transaction, "skills", "name", "TEXT NOT NULL DEFAULT ''")?;
        add_column_if_missing(
            &transaction,
            "skills",
            "description",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "skills",
            "relative_path",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "skills",
            "capabilities_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        add_column_if_missing(
            &transaction,
            "skills",
            "instructions_digest",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &transaction,
            "skills",
            "compatible",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        add_column_if_missing(&transaction, "skills", "compatibility_reason", "TEXT")?;
        add_column_if_missing(
            &transaction,
            "skills",
            "installed_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        transaction.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_tickets_sidebar ON tickets(pinned DESC, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_tickets_search ON tickets(search_text);
            CREATE INDEX IF NOT EXISTS idx_assignments_ticket ON assignments(ticket_id, created_at);
            CREATE TABLE IF NOT EXISTS terminal_audit (
              id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL,
              session_id TEXT NOT NULL, command TEXT NOT NULL, approved INTEGER NOT NULL,
              exit_code INTEGER, created_at TEXT NOT NULL
            );
            UPDATE tickets SET search_text=lower(title || ' ' || request) WHERE search_text='';
            "#,
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(2,?1)",
            [Utc::now().to_rfc3339()],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(3,?1)",
            [Utc::now().to_rfc3339()],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(4,?1)",
            [Utc::now().to_rfc3339()],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(5,?1)",
            [Utc::now().to_rfc3339()],
        )?;
        if from_version < 6 {
            migrate_superagent_schema_v6(&transaction)?;
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(6,?1)",
                [Utc::now().to_rfc3339()],
            )?;
            if from_version == 5 {
                transaction.execute(
                    "INSERT INTO schema_rollback_pairs(id,from_version,to_version,database_backup_path,app_version,created_at) VALUES(?1,5,6,?2,?3,?4)",
                    params![
                        format!("rollback-{}", Uuid::new_v4()),
                        backup_path.map(|path| path.display().to_string()),
                        env!("CARGO_PKG_VERSION"),
                        Utc::now().to_rfc3339(),
                    ],
                )?;
            }
        }
        if from_version < 7 {
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(7,?1)",
                [Utc::now().to_rfc3339()],
            )?;
        }
        if from_version < 8 {
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migrations(version,applied_at) VALUES(8,?1)",
                [Utc::now().to_rfc3339()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_project(
        &mut self,
        locator: &ProjectLocator,
        trusted: bool,
    ) -> Result<ProjectSummary, StorageError> {
        let now = Utc::now().to_rfc3339();
        let name = Path::new(&locator.absolute_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM projects WHERE fingerprint = ?1",
                [&locator.fingerprint],
                |row| row.get(0),
            )
            .optional()?;
        let id = existing.unwrap_or_else(|| format!("project-{}", Uuid::new_v4()));
        self.connection.execute(
            r#"INSERT INTO projects (id,name,absolute_path,relative_path,fingerprint,trusted,created_at,updated_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?7)
               ON CONFLICT(fingerprint) DO UPDATE SET name=excluded.name, absolute_path=excluded.absolute_path,
                 relative_path=excluded.relative_path, trusted=excluded.trusted, updated_at=excluded.updated_at"#,
            params![id, name, locator.absolute_path, locator.relative_path, locator.fingerprint, trusted, now],
        )?;
        self.append_event(None, "project.selected", json!({ "projectId": id }))?;
        Ok(ProjectSummary {
            id,
            name,
            path: locator.absolute_path.clone(),
            trusted,
        })
    }

    pub fn create_ticket(
        &mut self,
        project_id: &str,
        request: &str,
    ) -> Result<TicketSummary, StorageError> {
        let project_exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [project_id],
            |row| row.get(0),
        )?;
        if !project_exists {
            return Err(StorageError::ProjectNotFound(project_id.into()));
        }
        let now = Utc::now().to_rfc3339();
        let ticket = TicketSummary {
            id: format!("ticket-{}", Uuid::new_v4()),
            project_id: project_id.into(),
            title: request.chars().take(80).collect(),
            request: request.into(),
            state: TicketState::Open,
            created_at: now.clone(),
            updated_at: now.clone(),
            pinned: false,
            archived_at: None,
            parent_ticket_id: None,
            branch_event_sequence: None,
        };
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO tickets (id,project_id,request,title,state,created_at,updated_at,search_text) VALUES (?1,?2,?3,?4,'open',?5,?5,lower(?4 || ' ' || ?3))",
            params![ticket.id, ticket.project_id, ticket.request, ticket.title, now],
        )?;
        let context = TaskContext::from_request(request);
        upsert_task_context_tx(&transaction, &ticket.id, &context)?;
        append_event_tx(
            &transaction,
            Some(&ticket.id),
            "ticket.created",
            json!({ "request": ticket.request }),
        )?;
        append_event_tx(
            &transaction,
            Some(&ticket.id),
            "context.updated",
            serde_json::to_value(&context)?,
        )?;
        transaction.commit()?;
        Ok(ticket)
    }

    pub fn projects(&self) -> Result<Vec<ProjectSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id,name,absolute_path,trusted FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ProjectSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                trusted: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn reconcile_portable_projects(
        &mut self,
        portable: &crate::portable::PortableRoot,
    ) -> Result<usize, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id,relative_path,absolute_path FROM projects WHERE relative_path IS NOT NULL AND relative_path<>''",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let projects = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut updated = 0;
        for (id, relative, previous) in projects {
            let relative_path = Path::new(&relative);
            if relative_path.is_absolute()
                || relative_path.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                continue;
            }
            let candidate = portable.path().join(relative_path);
            if !candidate.is_dir() {
                continue;
            }
            let resolved = fs::canonicalize(candidate)?.display().to_string();
            if resolved.eq_ignore_ascii_case(&previous) {
                continue;
            }
            self.connection.execute(
                "UPDATE projects SET absolute_path=?2,updated_at=?3 WHERE id=?1",
                params![id, resolved, Utc::now().to_rfc3339()],
            )?;
            updated += 1;
        }
        if updated > 0 {
            self.append_event(
                None,
                "portable.projects_reconciled",
                json!({ "count": updated }),
            )?;
        }
        Ok(updated)
    }

    pub fn tickets(&self) -> Result<Vec<TicketSummary>, StorageError> {
        let mut statement = self.connection.prepare("SELECT id,project_id,title,request,state,created_at,updated_at,pinned,archived_at,parent_ticket_id,branch_event_sequence FROM tickets ORDER BY pinned DESC, updated_at DESC")?;
        let rows = statement.query_map([], |row| {
            let state: String = row.get(4)?;
            Ok(TicketSummary {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                request: row.get(3)?,
                state: parse_ticket_state(&state),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                pinned: row.get(7)?,
                archived_at: row.get(8)?,
                parent_ticket_id: row.get(9)?,
                branch_event_sequence: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn ticket(&self, id: &str) -> Result<TicketSummary, StorageError> {
        self.connection.query_row(
            "SELECT id,project_id,title,request,state,created_at,updated_at,pinned,archived_at,parent_ticket_id,branch_event_sequence FROM tickets WHERE id=?1",
            [id], |row| {
                let state: String = row.get(4)?;
                Ok(TicketSummary { id: row.get(0)?, project_id: row.get(1)?, title: row.get(2)?, request: row.get(3)?, state: parse_ticket_state(&state), created_at: row.get(5)?, updated_at: row.get(6)?, pinned: row.get(7)?, archived_at: row.get(8)?, parent_ticket_id: row.get(9)?, branch_event_sequence: row.get(10)? })
            },
        ).map_err(Into::into)
    }

    pub fn task_context(&self, ticket_id: &str) -> Result<TaskContext, StorageError> {
        let raw: Option<String> = self.connection.query_row(
            "SELECT context_json FROM ticket_contexts WHERE ticket_id=?1",
            [ticket_id],
            |row| row.get(0),
        ).optional()?;
        if let Some(raw) = raw {
            return serde_json::from_str(&raw).map_err(Into::into);
        }
        let ticket = self.ticket(ticket_id)?;
        Ok(TaskContext::from_request(&ticket.request))
    }

    pub fn update_task_context(&mut self, ticket_id: &str, message: &str) -> Result<TaskContext, StorageError> {
        let mut context = self.task_context(ticket_id)?;
        context.merge_message(message);
        let transaction = self.connection.transaction()?;
        upsert_task_context_tx(&transaction, ticket_id, &context)?;
        append_event_tx(
            &transaction,
            Some(ticket_id),
            "context.updated",
            serde_json::to_value(&context)?,
        )?;
        transaction.commit()?;
        Ok(context)
    }

    pub fn rename_ticket(&self, id: &str, title: &str) -> Result<TicketSummary, StorageError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(StorageError::TicketNotFound(id.into()));
        }
        let changed = self.connection.execute(
            "UPDATE tickets SET title=?2, search_text=lower(?2 || ' ' || request), updated_at=?3 WHERE id=?1",
            params![id, title.chars().take(120).collect::<String>(), Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            return Err(StorageError::TicketNotFound(id.into()));
        }
        self.ticket(id)
    }

    pub fn set_ticket_pinned(&self, id: &str, pinned: bool) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE tickets SET pinned=?2, updated_at=?3 WHERE id=?1",
            params![id, pinned, Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            return Err(StorageError::TicketNotFound(id.into()));
        }
        Ok(())
    }

    pub fn set_ticket_archived(&self, id: &str, archived: bool) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE tickets SET archived_at=?2, pinned=CASE WHEN ?2 IS NULL THEN pinned ELSE 0 END, updated_at=?3 WHERE id=?1",
            params![id, if archived { Some(Utc::now().to_rfc3339()) } else { None::<String> }, Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            return Err(StorageError::TicketNotFound(id.into()));
        }
        Ok(())
    }

    pub fn branch_ticket(
        &mut self,
        source_id: &str,
        through_sequence: i64,
    ) -> Result<TicketSummary, StorageError> {
        let source = self.ticket(source_id)?;
        let source_context = self.task_context(source_id).ok();
        let now = Utc::now().to_rfc3339();
        let ticket = TicketSummary {
            id: format!("ticket-{}", Uuid::new_v4()),
            project_id: source.project_id.clone(),
            title: format!("{} · 分支", source.title),
            request: source.request.clone(),
            state: TicketState::Completed,
            created_at: now.clone(),
            updated_at: now.clone(),
            pinned: false,
            archived_at: None,
            parent_ticket_id: Some(source_id.into()),
            branch_event_sequence: Some(through_sequence),
        };
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO tickets(id,project_id,request,title,state,created_at,updated_at,parent_ticket_id,branch_event_sequence,search_text) VALUES(?1,?2,?3,?4,'completed',?5,?5,?6,?7,lower(?4 || ' ' || ?3))",
            params![ticket.id, ticket.project_id, ticket.request, ticket.title, now, source_id, through_sequence],
        )?;
        if let Some(context) = source_context.as_ref() {
            upsert_task_context_tx(&tx, &ticket.id, context)?;
        }
        let mut statement = tx.prepare("SELECT event_type,payload_json,created_at FROM events WHERE ticket_id=?1 AND sequence<=?2 ORDER BY sequence")?;
        let events = statement
            .query_map(params![source_id, through_sequence], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (event_type, payload, created_at) in events {
            tx.execute("INSERT INTO events(ticket_id,event_type,payload_json,created_at) VALUES(?1,?2,?3,?4)", params![ticket.id, event_type, payload, created_at])?;
        }
        append_event_tx(
            &tx,
            Some(&ticket.id),
            "branch.created",
            json!({ "parentTicketId": source_id, "throughSequence": through_sequence }),
        )?;
        tx.commit()?;
        Ok(ticket)
    }

    pub fn create_assignment(
        &self,
        ticket_id: &str,
        parent_id: Option<&str>,
        label: &str,
        request: &str,
        capability: &str,
    ) -> Result<AssignmentSummary, StorageError> {
        let id = format!("assignment-{}", Uuid::new_v4());
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "INSERT INTO assignments(id,ticket_id,parent_id,state,label,request,capability,created_at,updated_at) VALUES(?1,?2,?3,'queued',?4,?5,?6,?7,?7)",
            params![id, ticket_id, parent_id, label, request, capability, now],
        )?;
        self.assignment(&id)
    }

    pub fn assignment(&self, id: &str) -> Result<AssignmentSummary, StorageError> {
        self.connection.query_row(
            "SELECT id,ticket_id,parent_id,label,request,capability,state,result,error,external_id,worktree_path,diff_text,merged_at,created_at,updated_at FROM assignments WHERE id=?1",
            [id],
            assignment_from_row,
        ).map_err(Into::into)
    }

    pub fn assignments(&self, ticket_id: &str) -> Result<Vec<AssignmentSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id,ticket_id,parent_id,label,request,capability,state,result,error,external_id,worktree_path,diff_text,merged_at,created_at,updated_at FROM assignments WHERE ticket_id=?1 ORDER BY created_at",
        )?;
        let rows = statement.query_map([ticket_id], assignment_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_assignment(
        &self,
        id: &str,
        state: AssignmentState,
        result: Option<&str>,
        error: Option<&str>,
        external_id: Option<&str>,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE assignments SET state=?2,result=COALESCE(?3,result),error=COALESCE(?4,error),external_id=COALESCE(?5,external_id),updated_at=?6 WHERE id=?1",
            params![id, assignment_state_str(state), result, error, external_id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_assignment_artifacts(
        &self,
        id: &str,
        worktree_path: Option<&str>,
        effect_json: Option<&str>,
        diff: Option<&str>,
        merged_at: Option<&str>,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE assignments SET worktree_path=COALESCE(?2,worktree_path),effect_json=COALESCE(?3,effect_json),diff_text=COALESCE(?4,diff_text),merged_at=COALESCE(?5,merged_at),updated_at=?6 WHERE id=?1",
            params![id, worktree_path, effect_json, diff, merged_at, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn assignment_effect(&self, id: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT effect_json FROM assignments WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn active_assignment_external_ids(
        &self,
        ticket_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id,external_id FROM assignments WHERE ticket_id=?1 AND state IN ('queued','running','waiting') AND external_id IS NOT NULL",
        )?;
        let rows = statement.query_map([ticket_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn cancel_assignments(&self, ticket_id: &str) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE assignments SET state='cancelled',updated_at=?2 WHERE ticket_id=?1 AND state IN ('queued','running','waiting')",
            params![ticket_id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn settings(&self) -> Result<AppSettings, StorageError> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key='app'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let settings = match value {
            Some(value) => serde_json::from_str(&value)?,
            None => AppSettings::default(),
        };
        Ok(settings.normalized())
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), StorageError> {
        let normalized = settings.clone().normalized();
        self.connection.execute("INSERT INTO settings(key,value_json) VALUES('app',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json", [serde_json::to_string(&normalized)?])?;
        Ok(())
    }

    pub fn onboarding(&self) -> Result<OnboardingState, StorageError> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key='onboarding'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut onboarding = match value {
            Some(value) => serde_json::from_str(&value).unwrap_or_default(),
            None => OnboardingState::default(),
        };
        onboarding.completed_steps.retain(|step| ONBOARDING_STEPS.contains(&step.as_str()));
        onboarding.skipped_steps.retain(|step| ONBOARDING_STEPS.contains(&step.as_str()));
        onboarding.completed_steps.sort();
        onboarding.completed_steps.dedup();
        onboarding.skipped_steps.sort();
        onboarding.skipped_steps.dedup();
        onboarding.dismissed_tips.sort();
        onboarding.dismissed_tips.dedup();
        Ok(onboarding)
    }

    pub fn save_onboarding(&self, onboarding: &OnboardingState) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO settings(key,value_json) VALUES('onboarding',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
            [serde_json::to_string(onboarding)?],
        )?;
        Ok(())
    }

    pub fn has_event_type(&self, event_type: &str) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE event_type=?1)",
                [event_type],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn has_terminal_audit(&self) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM terminal_audit WHERE approved=1)",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn has_completed_assignment(&self) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assignments WHERE state='completed')",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn claim_desktop_request(
        &self,
        client_request_id: &str,
        ticket_id: &str,
        content: &str,
    ) -> Result<bool, StorageError> {
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO idempotency_keys(namespace,key,target_kind,target_id,content_digest,created_at) VALUES('desktop_request',?1,'ticket',?2,?3,?4)",
            params![client_request_id, ticket_id, content, Utc::now().to_rfc3339()],
        )?;
        Ok(changed == 1)
    }

    pub fn skills(&self) -> Result<Vec<SkillSummary>, StorageError> {
        let mut statement = self.connection.prepare("SELECT id,name,version,description,relative_path,capabilities_json,instructions_digest,compatible,compatibility_reason,enabled,installed_at FROM skills ORDER BY name,id")?;
        let rows = statement.query_map([], |row| {
            let capabilities: String = row.get(5)?;
            Ok(SkillSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                description: row.get(3)?,
                relative_path: row.get(4)?,
                capabilities: serde_json::from_str(&capabilities).unwrap_or_default(),
                instructions_digest: row.get(6)?,
                compatible: row.get(7)?,
                compatibility_reason: row.get(8)?,
                enabled: row.get(9)?,
                installed_at: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_skill(&self, skill: &SkillSummary) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO skills(id,name,version,description,relative_path,capabilities_json,instructions_digest,compatible,compatibility_reason,enabled,installed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET name=excluded.name,version=excluded.version,description=excluded.description,relative_path=excluded.relative_path,capabilities_json=excluded.capabilities_json,instructions_digest=excluded.instructions_digest,compatible=excluded.compatible,compatibility_reason=excluded.compatibility_reason,enabled=excluded.enabled,installed_at=excluded.installed_at",
            params![skill.id,skill.name,skill.version,skill.description,skill.relative_path,serde_json::to_string(&skill.capabilities)?,skill.instructions_digest,skill.compatible,skill.compatibility_reason,skill.enabled,skill.installed_at],
        )?;
        Ok(())
    }

    pub fn set_skill_enabled(&self, id: &str, enabled: bool) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE skills SET enabled=?2 WHERE id=?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn remove_skill(&self, id: &str) -> Result<(), StorageError> {
        self.connection
            .execute("DELETE FROM skills WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn audit_terminal(
        &self,
        project_id: &str,
        session_id: &str,
        command: &str,
        approved: bool,
        exit_code: Option<i32>,
    ) -> Result<(), StorageError> {
        self.connection.execute("INSERT INTO terminal_audit(project_id,session_id,command,approved,exit_code,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![project_id, session_id, command, approved, exit_code, Utc::now().to_rfc3339()])?;
        Ok(())
    }

    pub fn delete_ticket(&mut self, id: &str) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM tickets WHERE id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::TicketNotFound(id.into()));
        }
        transaction.execute("DELETE FROM effects WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM tool_calls WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM messages WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM turns WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM assignments WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM deliveries WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM external_refs WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM events WHERE ticket_id=?1", [id])?;
        transaction.execute("DELETE FROM tickets WHERE id=?1", [id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn project(&self, id: &str) -> Result<ProjectSummary, StorageError> {
        self.connection
            .query_row(
                "SELECT id,name,absolute_path,trusted FROM projects WHERE id=?1",
                [id],
                |row| {
                    Ok(ProjectSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        path: row.get(2)?,
                        trusted: row.get(3)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn set_ticket_state(&self, id: &str, state: TicketState) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE tickets SET state=?2, updated_at=?3 WHERE id=?1",
            params![id, ticket_state_str(state), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_external_ref(
        &self,
        ticket_id: &str,
        adapter: &str,
        external_id: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO external_refs(ticket_id,adapter,external_id) VALUES(?1,?2,?3) ON CONFLICT(ticket_id,adapter) DO UPDATE SET external_id=excluded.external_id",
            params![ticket_id, adapter, external_id],
        )?;
        Ok(())
    }

    pub fn external_ref(
        &self,
        ticket_id: &str,
        adapter: &str,
    ) -> Result<Option<String>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT external_id FROM external_refs WHERE ticket_id=?1 AND adapter=?2",
                params![ticket_id, adapter],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn conversation_messages(
        &self,
        ticket_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT event_type,payload_json FROM events WHERE ticket_id=?1 AND event_type IN ('user.message','delivery.ready','provider.message') ORDER BY sequence ASC LIMIT 500",
        )?;
        let rows = statement.query_map([ticket_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (event_type, payload_json) = row?;
            let payload: serde_json::Value = serde_json::from_str(&payload_json)?;
            let Some(content) = payload.get("content").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if content.trim().is_empty() {
                continue;
            }
            let role = if event_type == "user.message" {
                "user"
            } else {
                "assistant"
            };
            messages.push((role.to_string(), content.to_string()));
        }
        Ok(messages)
    }

    pub fn last_delivery_route(&self, ticket_id: &str) -> Result<Option<String>, StorageError> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload_json FROM events WHERE ticket_id=?1 AND event_type='delivery.ready' ORDER BY sequence DESC LIMIT 1",
                [ticket_id],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|payload| {
                serde_json::from_str::<Value>(&payload).map(|value| {
                    value
                        .get("route")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
            })
            .transpose()
            .map(|value| value.flatten())
            .map_err(Into::into)
    }

    pub fn events(&self, after: i64) -> Result<Vec<RuntimeEvent>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT sequence,ticket_id,event_type,payload_json,created_at FROM events WHERE sequence>?1 ORDER BY sequence ASC LIMIT 1000",
        )?;
        let rows = statement.query_map([after], |row| {
            let payload: String = row.get(3)?;
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, payload, row.get(4)?))
        })?;
        rows.map(|row| {
            let (sequence, ticket_id, event_type, payload, created_at) = row?;
            Ok(RuntimeEvent {
                sequence,
                ticket_id,
                event_type,
                payload: serde_json::from_str(&payload)?,
                created_at,
            })
        })
        .collect()
    }

    pub fn last_sequence(&self) -> Result<i64, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM events",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn evidence_receipts(&self) -> Result<Vec<EvidenceReceipt>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT receipt_json FROM evidence_receipts ORDER BY updated_at DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn append_event(
        &self,
        ticket_id: Option<&str>,
        event_type: &str,
        payload: Value,
    ) -> Result<i64, StorageError> {
        append_event_connection(
            &self.connection,
            ticket_id,
            event_type,
            payload,
            &Utc::now().to_rfc3339(),
        )
    }

    pub fn append_event_at(
        &self,
        ticket_id: Option<&str>,
        event_type: &str,
        payload: Value,
        created_at: &str,
    ) -> Result<i64, StorageError> {
        append_event_connection(&self.connection, ticket_id, event_type, payload, created_at)
    }
}

fn append_event_connection(
    connection: &Connection,
    ticket_id: Option<&str>,
    event_type: &str,
    payload: Value,
    created_at: &str,
) -> Result<i64, StorageError> {
    connection.execute(
        "INSERT INTO events (ticket_id,event_type,payload_json,created_at) VALUES (?1,?2,?3,?4)",
        params![
            ticket_id,
            event_type,
            serde_json::to_string(&payload)?,
            created_at
        ],
    )?;
    let sequence = connection.last_insert_rowid();
    if let Some(ticket_id) = ticket_id {
        refresh_evidence_receipt(connection, ticket_id)?;
    }
    Ok(sequence)
}

fn refresh_evidence_receipt(connection: &Connection, ticket_id: &str) -> Result<(), StorageError> {
    let ticket = connection
        .query_row(
            "SELECT request,state FROM tickets WHERE id=?1",
            [ticket_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((request, state)) = ticket else {
        return Ok(());
    };
    let mut statement = connection.prepare(
        "SELECT event_type,payload_json,created_at FROM events WHERE ticket_id=?1 ORDER BY sequence ASC",
    )?;
    let rows = statement.query_map([ticket_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut route = None;
    let mut sources = Vec::new();
    let mut commands = Vec::new();
    let mut tests = Vec::new();
    let mut effects = Vec::new();
    let mut decisions = Vec::new();
    let mut last_event = String::new();
    let mut unresolved_effects = HashSet::new();
    for row in rows {
        let (event_type, payload_json, created_at) = row?;
        let payload: Value = serde_json::from_str(&payload_json)?;
        last_event = event_type.clone();
        match event_type.as_str() {
            "tool.completed" => {
                let tool = payload
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let item =
                    json!({ "tool": tool, "evidence": payload.get("evidence"), "at": created_at });
                if matches!(
                    tool,
                    "read_file" | "list_files" | "search" | "attachment_read"
                ) {
                    push_unique(&mut sources, item);
                } else if tool.contains("test") || tool == "pytest" {
                    push_unique(&mut tests, item);
                } else {
                    push_unique(&mut commands, item);
                }
            }
            "context.attached" => {
                for attachment in payload
                    .get("attachments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    push_unique(
                        &mut sources,
                        json!({ "attachment": attachment, "at": created_at }),
                    );
                }
            }
            "delivery.ready" => {
                route = payload
                    .get("route")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(route);
                for evidence in payload
                    .get("evidence")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    push_unique(&mut sources, evidence.clone());
                }
            }
            "effect.proposed" => {
                if let Some(effect_id) = payload
                    .get("effectId")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                {
                    unresolved_effects.insert(effect_id.to_string());
                }
                effects.push(json!({ "event": event_type, "detail": payload, "at": created_at }));
            }
            "effect.applied" | "effect.rolled_back" => {
                effects.push(json!({ "event": event_type, "detail": payload, "at": created_at }));
            }
            "effect.approved" | "effect.rejected" | "rollback.approved" | "rollback.refused" => {
                if matches!(event_type.as_str(), "effect.approved" | "effect.rejected") {
                    if let Some(effect_id) = payload
                        .get("effectId")
                        .or_else(|| payload.get("id"))
                        .and_then(Value::as_str)
                    {
                        unresolved_effects.remove(effect_id);
                    }
                }
                decisions.push(json!({ "event": event_type, "detail": payload, "at": created_at }));
            }
            "effect.recovery_required" => {
                if let Some(effect_id) = payload
                    .get("effectId")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                {
                    unresolved_effects.remove(effect_id);
                }
                effects.push(json!({ "event": event_type, "detail": payload, "at": created_at }));
            }
            _ => {}
        }
    }
    let outcome = match state.as_str() {
        "completed" => "completed",
        "failed" => "failed",
        "stopped" => "cancelled",
        "rolled_back" => "rolled_back",
        "waiting" if unresolved_effects.is_empty() => "waiting_input",
        "waiting" => "waiting_approval",
        _ => "in_progress",
    };
    let updated_at = Utc::now().to_rfc3339();
    let final_state = json!({ "ticketState": state, "lastConfirmedEvent": last_event });
    let body = json!({
        "ticketId": ticket_id, "request": request, "route": route, "outcome": outcome,
        "sources": sources, "commands": commands, "tests": tests, "effects": effects,
        "decisions": decisions, "finalState": final_state, "updatedAt": updated_at,
    });
    let encoded = serde_json::to_string(&body)?;
    let digest = format!("{:x}", Sha256::digest(encoded.as_bytes()));
    let receipt = EvidenceReceipt {
        ticket_id: ticket_id.to_string(),
        request,
        route,
        outcome: outcome.to_string(),
        sources,
        commands,
        tests,
        effects,
        decisions,
        final_state,
        content_digest: digest.clone(),
        updated_at: updated_at.clone(),
    };
    connection.execute(
        "INSERT INTO evidence_receipts(ticket_id,receipt_json,content_digest,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(ticket_id) DO UPDATE SET receipt_json=excluded.receipt_json,content_digest=excluded.content_digest,updated_at=excluded.updated_at",
        params![ticket_id, serde_json::to_string(&receipt)?, digest, updated_at],
    )?;
    Ok(())
}

fn push_unique(items: &mut Vec<Value>, value: Value) {
    if !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

fn upsert_task_context_tx(
    transaction: &Transaction<'_>,
    ticket_id: &str,
    context: &TaskContext,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT INTO ticket_contexts(ticket_id,context_json,revision,updated_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(ticket_id) DO UPDATE SET context_json=excluded.context_json,revision=excluded.revision,updated_at=excluded.updated_at",
        params![ticket_id, serde_json::to_string(context)?, context.revision, context.updated_at],
    )?;
    Ok(())
}

fn extend_unique(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !target.iter().any(|existing| existing.eq_ignore_ascii_case(&value)) {
            target.push(value);
        }
    }
}

fn extract_context_paths(value: &str) -> Vec<String> {
    const EXTENSIONS: &[&str] = &[
        "md", "txt", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "rs", "go", "java",
        "kt", "c", "h", "cpp", "hpp", "cs", "json", "yaml", "yml", "toml", "xml", "html",
        "css", "sql", "sh", "ps1", "bat", "cmd",
    ];
    let mut paths = Vec::new();
    for token in value.split(|character: char| {
        character.is_whitespace()
            || matches!(character, '`' | '\'' | '"' | '“' | '”' | '‘' | '’' | '，' | ',' | '。' | '；' | ';' | '：' | ':' | '（' | '(' | '）' | ')' | '、')
    }) {
        let candidate = token.trim_matches(|character: char| matches!(character, '.' | '!' | '?' | '！' | '？'));
        let extension = candidate.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase());
        if candidate.len() <= 260
            && !candidate.starts_with('.')
            && extension.as_deref().is_some_and(|extension| EXTENSIONS.contains(&extension))
            && !paths.iter().any(|path: &String| path.eq_ignore_ascii_case(candidate))
        {
            paths.push(candidate.replace('\\', "/"));
        }
    }
    paths
}

fn extract_context_clauses(value: &str, kind: ContextClause) -> Vec<String> {
    value
        .split(['。', '；', ';', '\n'])
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .filter(|clause| match kind {
            ContextClause::Constraint => contains_context_marker(
                clause,
                &["只", "仅", "不要", "不得", "不能", "必须", "保持", "限制", "only", "must", "do not", "without"],
            ),
            ContextClause::Verification => contains_context_marker(
                clause,
                &["测试", "验证", "pytest", "npm test", "cargo test", "check", "build", "test"],
            ),
        })
        .map(|clause| clause.chars().take(240).collect())
        .collect()
}

fn contains_context_marker(value: &str, markers: &[&str]) -> bool {
    let normalized = value.to_lowercase();
    markers.iter().any(|marker| normalized.contains(marker))
}

fn looks_like_mutation(value: &str) -> bool {
    contains_context_marker(
        value,
        &["修改", "修复", "新增", "添加", "删除", "重构", "实现", "创建", "写入", "fix", "modify", "edit", "add", "delete", "refactor", "implement", "create", "write"],
    )
}

fn schema_version(connection: &Connection) -> Result<i64, StorageError> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

fn backup_schema_five(
    connection: &Connection,
    database_path: &Path,
) -> Result<Option<std::path::PathBuf>, StorageError> {
    if !database_path.is_file() {
        return Ok(None);
    }
    let file_name = database_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("ucode.db");
    let backup_path = database_path.with_file_name(format!(
        "{file_name}.schema-v5-{}.bak",
        Utc::now().timestamp_millis()
    ));
    connection.execute("VACUUM INTO ?1", [backup_path.display().to_string()])?;
    Ok(Some(backup_path))
}

fn migrate_superagent_schema_v6(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS threads (
          key TEXT PRIMARY KEY,
          conversation_id TEXT UNIQUE,
          project_id TEXT NOT NULL REFERENCES projects(id),
          source TEXT NOT NULL,
          source_id TEXT NOT NULL,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS triggers (
          id TEXT PRIMARY KEY,
          client_request_id TEXT NOT NULL UNIQUE,
          source TEXT NOT NULL,
          actor_json TEXT NOT NULL,
          human INTEGER NOT NULL,
          addressed TEXT,
          at TEXT NOT NULL,
          text TEXT NOT NULL,
          meta_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS trigger_threads (
          trigger_id TEXT NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
          thread_key TEXT NOT NULL REFERENCES threads(key) ON DELETE CASCADE,
          ordinal INTEGER NOT NULL,
          PRIMARY KEY(trigger_id,thread_key)
        );
        CREATE TABLE IF NOT EXISTS trigger_dispositions (
          trigger_id TEXT PRIMARY KEY REFERENCES triggers(id) ON DELETE CASCADE,
          evaluation_id TEXT,
          decision_id TEXT,
          fanout_json TEXT NOT NULL DEFAULT '[]',
          read_by_json TEXT NOT NULL DEFAULT '[]',
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS evaluations (
          id TEXT PRIMARY KEY,
          thread_key TEXT NOT NULL REFERENCES threads(key),
          trigger_ids_json TEXT NOT NULL,
          strength TEXT NOT NULL,
          policy_version TEXT NOT NULL,
          state TEXT NOT NULL,
          started_at TEXT NOT NULL,
          finished_at TEXT
        );
        CREATE TABLE IF NOT EXISTS decisions (
          id TEXT PRIMARY KEY,
          evaluation_id TEXT NOT NULL,
          thread_key TEXT NOT NULL REFERENCES threads(key),
          trigger_ids_json TEXT NOT NULL,
          policy_version TEXT NOT NULL,
          kind TEXT NOT NULL,
          reason_code TEXT NOT NULL,
          ticket_id TEXT,
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mandates (
          ticket_id TEXT PRIMARY KEY REFERENCES tickets(id) ON DELETE CASCADE,
          strength TEXT NOT NULL,
          policy_version TEXT NOT NULL,
          basis_json TEXT NOT NULL,
          actor_json TEXT NOT NULL,
          project_id TEXT NOT NULL REFERENCES projects(id),
          project_fingerprint TEXT NOT NULL,
          capabilities_json TEXT NOT NULL,
          trigger_id TEXT NOT NULL REFERENCES triggers(id),
          thread_key TEXT NOT NULL REFERENCES threads(key),
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS inputs (
          id TEXT PRIMARY KEY,
          ticket_id TEXT NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
          kind TEXT NOT NULL,
          trigger_id TEXT REFERENCES triggers(id),
          text TEXT,
          human INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          delivered_to TEXT,
          delivered_at TEXT,
          read_at TEXT,
          undelivered INTEGER NOT NULL DEFAULT 1,
          UNIQUE(ticket_id,trigger_id)
        );
        CREATE TABLE IF NOT EXISTS reports (
          id TEXT PRIMARY KEY,
          assignment_id TEXT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
          state TEXT NOT NULL,
          summary TEXT NOT NULL,
          created_at TEXT NOT NULL,
          UNIQUE(assignment_id,created_at)
        );
        CREATE TABLE IF NOT EXISTS idempotency_keys (
          namespace TEXT NOT NULL,
          key TEXT NOT NULL,
          target_kind TEXT NOT NULL,
          target_id TEXT NOT NULL,
          content_digest TEXT NOT NULL,
          created_at TEXT NOT NULL,
          PRIMARY KEY(namespace,key)
        );
        CREATE TABLE IF NOT EXISTS schema_rollback_pairs (
          id TEXT PRIMARY KEY,
          from_version INTEGER NOT NULL,
          to_version INTEGER NOT NULL,
          database_backup_path TEXT,
          app_version TEXT NOT NULL,
          created_at TEXT NOT NULL
        );
        "#,
    )?;

    add_column_if_missing(transaction, "tickets", "thread_key", "TEXT")?;
    add_column_if_missing(transaction, "tickets", "superagent_decision_id", "TEXT")?;
    add_column_if_missing(transaction, "tickets", "ended_json", "TEXT")?;
    add_column_if_missing(transaction, "tickets", "attention_json", "TEXT")?;
    add_column_if_missing(transaction, "tickets", "origin_trigger_id", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "superagent_state", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "started_at", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "ended_at", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "ended_reason", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "delivered_at", "TEXT")?;
    add_column_if_missing(transaction, "assignments", "idle_at", "TEXT")?;
    add_column_if_missing(transaction, "effects", "effect_key", "TEXT")?;
    add_column_if_missing(transaction, "effects", "kind", "TEXT")?;
    add_column_if_missing(transaction, "effects", "payload_digest", "TEXT")?;
    add_column_if_missing(
        transaction,
        "effects",
        "attempts",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(transaction, "effects", "due_at", "TEXT")?;
    add_column_if_missing(transaction, "effects", "approval_json", "TEXT")?;
    add_column_if_missing(transaction, "effects", "result_digest", "TEXT")?;
    add_column_if_missing(transaction, "effects", "resolved_at", "TEXT")?;
    add_column_if_missing(transaction, "effects", "idempotency_key", "TEXT")?;

    transaction.execute_batch(
        r#"
        INSERT OR IGNORE INTO threads(key,conversation_id,project_id,source,source_id,title,created_at,updated_at)
        SELECT 'desktop:conversation:' || id,id,project_id,'desktop',id,title,created_at,updated_at FROM conversations;
        INSERT OR IGNORE INTO threads(key,conversation_id,project_id,source,source_id,title,created_at,updated_at)
        SELECT 'desktop:ticket:' || t.id,NULL,t.project_id,'desktop',t.id,t.title,t.created_at,t.updated_at
        FROM tickets t
        WHERE t.conversation_id IS NULL
           OR NOT EXISTS(SELECT 1 FROM conversations c WHERE c.id=t.conversation_id);
        UPDATE tickets
        SET thread_key=COALESCE(
          (SELECT th.key FROM threads th WHERE th.conversation_id=tickets.conversation_id),
          'desktop:ticket:' || tickets.id
        )
        WHERE thread_key IS NULL OR thread_key='';

        INSERT OR IGNORE INTO triggers(id,client_request_id,source,actor_json,human,addressed,at,text,meta_json)
        SELECT 'migration-trigger:' || id,'migration-ticket:' || id,'migration',
               '{"source":"desktop","id":"migrated-user"}',1,'direct',created_at,request,
               '{"schema":5,"kind":"ticket_request"}'
        FROM tickets;
        INSERT OR IGNORE INTO trigger_threads(trigger_id,thread_key,ordinal)
        SELECT 'migration-trigger:' || id,thread_key,0 FROM tickets;
        INSERT OR IGNORE INTO trigger_dispositions(trigger_id,decision_id,created_at)
        SELECT 'migration-trigger:' || id,'migration-decision:' || id,created_at FROM tickets;
        INSERT OR IGNORE INTO decisions(id,evaluation_id,thread_key,trigger_ids_json,policy_version,kind,reason_code,ticket_id,created_at)
        SELECT 'migration-decision:' || id,'migration-eval:' || id,thread_key,
               '["migration-trigger:' || id || '"]',CAST(policy_version AS TEXT),'accept','schema_v5_import',id,created_at
        FROM tickets;
        UPDATE tickets SET superagent_decision_id='migration-decision:' || id
        WHERE superagent_decision_id IS NULL OR superagent_decision_id='';
        INSERT OR IGNORE INTO mandates(ticket_id,strength,policy_version,basis_json,actor_json,project_id,project_fingerprint,capabilities_json,trigger_id,thread_key,created_at)
        SELECT t.id,'explicit',CAST(t.policy_version AS TEXT),'{"via":"migration"}',
               '{"source":"desktop","id":"migrated-user"}',t.project_id,p.fingerprint,
               '["chat","read","search","test","propose_write"]',
               'migration-trigger:' || t.id,t.thread_key,t.created_at
        FROM tickets t JOIN projects p ON p.id=t.project_id;
        INSERT OR IGNORE INTO inputs(id,ticket_id,kind,trigger_id,human,created_at)
        SELECT 'migration-input:' || id,id,'trigger','migration-trigger:' || id,1,created_at FROM tickets;
        UPDATE assignments
        SET superagent_state=CASE
          WHEN state IN ('queued','pending') THEN 'pending'
          WHEN state IN ('running','waiting') THEN 'running'
          ELSE 'ended' END
        WHERE superagent_state IS NULL OR superagent_state='';
        INSERT OR IGNORE INTO reports(id,assignment_id,state,summary,created_at)
        SELECT 'migration-report:' || id,id,
          CASE WHEN state='completed' THEN 'ready' WHEN state='failed' THEN 'failed'
               WHEN state='cancelled' THEN 'dropped' ELSE 'progress' END,
          COALESCE(result,error,''),updated_at
        FROM assignments
        WHERE result IS NOT NULL OR error IS NOT NULL OR state IN ('completed','failed','cancelled');
        INSERT OR IGNORE INTO idempotency_keys(namespace,key,target_kind,target_id,content_digest,created_at)
        SELECT 'trigger','migration-ticket:' || id,'trigger','migration-trigger:' || id,'legacy:' || id,created_at
        FROM tickets;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_effects_effect_key ON effects(effect_key) WHERE effect_key IS NOT NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_effects_idempotency ON effects(idempotency_key) WHERE idempotency_key IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_triggers_at ON triggers(at);
        CREATE INDEX IF NOT EXISTS idx_inputs_ticket ON inputs(ticket_id,created_at);
        CREATE INDEX IF NOT EXISTS idx_reports_assignment ON reports(assignment_id,created_at);
        CREATE TRIGGER IF NOT EXISTS mandates_immutable
        BEFORE UPDATE ON mandates
        BEGIN SELECT RAISE(ABORT,'mandate is immutable'); END;
        "#,
    )?;

    rebuild_turns_v6(transaction)?;
    rebuild_deliveries_v6(transaction)?;
    migrate_legacy_messages_v6(transaction)?;
    Ok(())
}

fn rebuild_turns_v6(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    if table_has_column(transaction, "turns", "thread_key")? {
        return Ok(());
    }
    transaction.execute_batch(
        r#"
        ALTER TABLE turns RENAME TO legacy_turns_v5;
        CREATE TABLE turns (
          id TEXT PRIMARY KEY,
          thread_key TEXT NOT NULL REFERENCES threads(key),
          ticket_id TEXT REFERENCES tickets(id) ON DELETE CASCADE,
          assignment_id TEXT REFERENCES assignments(id) ON DELETE CASCADE,
          idempotency_key TEXT NOT NULL UNIQUE,
          provider_id TEXT,
          model_id TEXT,
          provider_request_id TEXT,
          stream_sequence INTEGER NOT NULL DEFAULT 0,
          consumed_input_ids_json TEXT NOT NULL DEFAULT '[]',
          activity_json TEXT NOT NULL DEFAULT '[]',
          failure_class TEXT,
          state TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        INSERT INTO turns(id,thread_key,ticket_id,idempotency_key,provider_id,model_id,state,created_at,updated_at)
        SELECT old.id,t.thread_key,old.ticket_id,'migration-turn:' || old.id,
               old.provider_id,old.model_id,old.state,old.created_at,old.updated_at
        FROM legacy_turns_v5 old JOIN tickets t ON t.id=old.ticket_id;
        CREATE INDEX idx_turns_thread ON turns(thread_key,created_at);
        CREATE INDEX idx_turns_ticket ON turns(ticket_id,created_at);
        "#,
    )?;
    Ok(())
}

fn rebuild_deliveries_v6(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    if table_has_column(transaction, "deliveries", "thread_key")? {
        return Ok(());
    }
    transaction.execute_batch(
        r#"
        ALTER TABLE deliveries RENAME TO legacy_deliveries_v5;
        CREATE TABLE deliveries (
          id TEXT PRIMARY KEY,
          thread_key TEXT NOT NULL REFERENCES threads(key),
          ticket_id TEXT REFERENCES tickets(id) ON DELETE CASCADE,
          trigger_id TEXT REFERENCES triggers(id),
          turn_id TEXT REFERENCES turns(id),
          kind TEXT NOT NULL,
          summary TEXT NOT NULL,
          evidence_json TEXT NOT NULL,
          content_digest TEXT NOT NULL,
          idempotency_key TEXT NOT NULL UNIQUE,
          created_at TEXT NOT NULL
        );
        INSERT INTO deliveries(id,thread_key,ticket_id,kind,summary,evidence_json,content_digest,idempotency_key,created_at)
        SELECT old.id,t.thread_key,old.ticket_id,'legacy',old.summary,old.evidence_json,
               'legacy:' || old.id,'migration-delivery:' || old.id,old.created_at
        FROM legacy_deliveries_v5 old JOIN tickets t ON t.id=old.ticket_id;
        CREATE INDEX idx_deliveries_thread ON deliveries(thread_key,created_at);
        CREATE INDEX idx_deliveries_ticket ON deliveries(ticket_id,created_at);
        "#,
    )?;
    Ok(())
}

fn migrate_legacy_messages_v6(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute_batch(
        r#"
        INSERT OR IGNORE INTO triggers(id,client_request_id,source,actor_json,human,addressed,at,text,meta_json)
        SELECT 'migration-message-trigger:' || m.id,'migration-message:' || m.id,'migration',
               '{"source":"desktop","id":"migrated-user"}',1,'direct',m.created_at,m.content_json,
               '{"schema":5,"kind":"message"}'
        FROM messages m WHERE m.role='user';
        INSERT OR IGNORE INTO trigger_threads(trigger_id,thread_key,ordinal)
        SELECT 'migration-message-trigger:' || m.id,t.thread_key,m.ordinal
        FROM messages m JOIN tickets t ON t.id=m.ticket_id WHERE m.role='user';
        INSERT OR IGNORE INTO trigger_dispositions(trigger_id,created_at)
        SELECT 'migration-message-trigger:' || id,created_at FROM messages WHERE role='user';
        INSERT OR IGNORE INTO deliveries(id,thread_key,ticket_id,trigger_id,turn_id,kind,summary,evidence_json,content_digest,idempotency_key,created_at)
        SELECT 'migration-message-delivery:' || m.id,t.thread_key,m.ticket_id,NULL,
               CASE WHEN EXISTS(SELECT 1 FROM turns tr WHERE tr.id=m.turn_id) THEN m.turn_id ELSE NULL END,
               'legacy_message',m.content_json,'[]','legacy-message:' || m.id,
               'migration-message:' || m.id,m.created_at
        FROM messages m JOIN tickets t ON t.id=m.ticket_id WHERE m.role<>'user';
        "#,
    )?;
    Ok(())
}

fn table_has_column(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<bool, StorageError> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|value| value == column))
}

fn add_column_if_missing(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), StorageError> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|value| value == column) {
        transaction.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn append_event_tx(
    transaction: &Transaction<'_>,
    ticket_id: Option<&str>,
    event_type: &str,
    payload: Value,
) -> Result<i64, StorageError> {
    transaction.execute(
        "INSERT INTO events (ticket_id,event_type,payload_json,created_at) VALUES (?1,?2,?3,?4)",
        params![
            ticket_id,
            event_type,
            serde_json::to_string(&payload)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(transaction.last_insert_rowid())
}

fn parse_ticket_state(value: &str) -> TicketState {
    match value {
        "queued" => TicketState::Queued,
        "running" => TicketState::Running,
        "waiting" => TicketState::Waiting,
        "stopping" => TicketState::Stopping,
        "stopped" => TicketState::Stopped,
        "completed" => TicketState::Completed,
        "failed" => TicketState::Failed,
        "rolled_back" => TicketState::RolledBack,
        _ => TicketState::Open,
    }
}

fn assignment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AssignmentSummary> {
    let state: String = row.get(6)?;
    Ok(AssignmentSummary {
        id: row.get(0)?,
        ticket_id: row.get(1)?,
        parent_id: row.get(2)?,
        label: row.get(3)?,
        request: row.get(4)?,
        capability: row.get(5)?,
        state: parse_assignment_state(&state),
        result: row.get(7)?,
        error: row.get(8)?,
        external_id: row.get(9)?,
        worktree_path: row.get(10)?,
        diff: row.get(11)?,
        merged_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn parse_assignment_state(value: &str) -> AssignmentState {
    match value {
        "running" => AssignmentState::Running,
        "waiting" => AssignmentState::Waiting,
        "completed" => AssignmentState::Completed,
        "failed" => AssignmentState::Failed,
        "cancelled" => AssignmentState::Cancelled,
        _ => AssignmentState::Queued,
    }
}

fn assignment_state_str(state: AssignmentState) -> &'static str {
    match state {
        AssignmentState::Queued => "queued",
        AssignmentState::Running => "running",
        AssignmentState::Waiting => "waiting",
        AssignmentState::Completed => "completed",
        AssignmentState::Failed => "failed",
        AssignmentState::Cancelled => "cancelled",
    }
}

fn ticket_state_str(state: TicketState) -> &'static str {
    match state {
        TicketState::Open => "open",
        TicketState::Queued => "queued",
        TicketState::Running => "running",
        TicketState::Waiting => "waiting",
        TicketState::Stopping => "stopping",
        TicketState::Stopped => "stopped",
        TicketState::Completed => "completed",
        TicketState::Failed => "failed",
        TicketState::RolledBack => "rolled_back",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn events_are_monotonic_and_ticket_creation_is_atomic() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "fingerprint".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage.create_ticket(&project.id, "fix the test").unwrap();
        let events = storage.events(0).unwrap();

        assert_eq!(events.len(), 3);
        assert!(events.windows(2).all(|pair| pair[0].sequence < pair[1].sequence));
        assert_eq!(events[2].ticket_id.as_deref(), Some(ticket.id.as_str()));
        assert_eq!(events[2].event_type, "context.updated");
        assert_eq!(storage.tickets().unwrap()[0].state, TicketState::Open);
    }

    #[test]
    fn desktop_request_claim_is_idempotent() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        assert!(storage
            .claim_desktop_request("request-1", "ticket-1", "hello")
            .unwrap());
        assert!(!storage
            .claim_desktop_request("request-1", "ticket-1", "hello")
            .unwrap());
        assert!(storage
            .claim_desktop_request("request-2", "ticket-1", "hello")
            .unwrap());
    }

    #[test]
    fn tickets_can_be_renamed_and_deleted_atomically() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "rename-delete".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage
            .create_ticket(&project.id, "original title")
            .unwrap();
        let renamed = storage
            .rename_ticket(&ticket.id, "A precise title")
            .unwrap();
        assert_eq!(renamed.title, "A precise title");
        storage.delete_ticket(&ticket.id).unwrap();
        assert!(storage.tickets().unwrap().is_empty());
        assert!(storage
            .events(0)
            .unwrap()
            .iter()
            .all(|event| event.ticket_id.as_deref() != Some(&ticket.id)));
    }

    #[test]
    fn reselecting_a_project_moves_it_to_the_front_without_mixing_tickets() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let first_locator = ProjectLocator {
            absolute_path: directory.path().join("first").display().to_string(),
            relative_path: None,
            fingerprint: "project-first".into(),
        };
        let second_locator = ProjectLocator {
            absolute_path: directory.path().join("second").display().to_string(),
            relative_path: None,
            fingerprint: "project-second".into(),
        };
        let first = storage.upsert_project(&first_locator, true).unwrap();
        let first_ticket = storage
            .create_ticket(&first.id, "first project task")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = storage.upsert_project(&second_locator, true).unwrap();
        storage
            .create_ticket(&second.id, "second project task")
            .unwrap();
        assert_eq!(storage.projects().unwrap()[0].id, second.id);

        std::thread::sleep(std::time::Duration::from_millis(2));
        let selected = storage.upsert_project(&first_locator, true).unwrap();
        assert_eq!(selected.id, first.id);
        assert_eq!(storage.projects().unwrap()[0].id, first.id);
        assert_eq!(
            storage.ticket(&first_ticket.id).unwrap().project_id,
            first.id
        );
    }

    #[test]
    fn portable_projects_reconcile_after_the_capsule_root_moves() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("capsule-a");
        let second = directory.path().join("capsule-b");
        let project = first.join("workspaces/demo");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        let root = ProjectLocator {
            absolute_path: fs::canonicalize(&project).unwrap().display().to_string(),
            relative_path: Some("workspaces/demo".into()),
            fingerprint: "portable-move".into(),
        };
        let database = first.join("data/ucode.db");
        let mut storage = Storage::open(&database).unwrap();
        storage.upsert_project(&root, true).unwrap();
        drop(storage);
        fs::rename(&first, &second).unwrap();

        let portable = crate::portable::PortableRoot::from_path(&second).unwrap();
        let mut storage = Storage::open(&second.join("data/ucode.db")).unwrap();
        assert_eq!(storage.reconcile_portable_projects(&portable).unwrap(), 1);
        assert_eq!(
            storage.projects().unwrap()[0].path,
            fs::canonicalize(second.join("workspaces/demo"))
                .unwrap()
                .display()
                .to_string()
        );
    }

    #[test]
    fn conversation_metadata_branching_and_settings_persist() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "metadata".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage.create_ticket(&project.id, "branch me").unwrap();
        storage
            .append_event(
                Some(&ticket.id),
                "delivery.ready",
                json!({ "content": "answer" }),
            )
            .unwrap();
        let through = storage
            .append_event(
                Some(&ticket.id),
                "user.message",
                json!({ "content": "follow up" }),
            )
            .unwrap();
        storage.set_ticket_pinned(&ticket.id, true).unwrap();
        assert!(storage.ticket(&ticket.id).unwrap().pinned);
        storage.set_ticket_archived(&ticket.id, true).unwrap();
        let archived = storage.ticket(&ticket.id).unwrap();
        assert!(archived.archived_at.is_some());
        assert!(!archived.pinned);
        let branch = storage.branch_ticket(&ticket.id, through).unwrap();
        assert_eq!(branch.parent_ticket_id.as_deref(), Some(ticket.id.as_str()));
        assert!(storage
            .events(0)
            .unwrap()
            .iter()
            .any(
                |event| event.ticket_id.as_deref() == Some(branch.id.as_str())
                    && event.event_type == "delivery.ready"
            ));
        assert_eq!(
            storage.conversation_messages(&branch.id).unwrap(),
            vec![
                ("assistant".into(), "answer".into()),
                ("user".into(), "follow up".into())
            ]
        );
        let settings = AppSettings {
            reduce_motion: true,
            sound_volume: 200,
            ..AppSettings::default()
        };
        storage.save_settings(&settings).unwrap();
        assert!(storage.settings().unwrap().reduce_motion);
        assert!(storage.settings().unwrap().sound_enabled);
        assert!(storage.settings().unwrap().completion_notifications_enabled);
        assert_eq!(storage.settings().unwrap().sound_volume, 100);
        let unsafe_settings = AppSettings {
            theme: "neon".into(),
            terminal_shell: "bash.exe".into(),
            model_provider: "unknown".into(),
            ui_font_size: 1,
            code_font_size: 255,
            sidebar_width: 1,
            inspector_width: u16::MAX,
            ..AppSettings::default()
        };
        storage.save_settings(&unsafe_settings).unwrap();
        let normalized = storage.settings().unwrap();
        assert_eq!(normalized.theme, "dark");
        assert_eq!(normalized.terminal_shell, "powershell.exe");
        assert_eq!(normalized.model_provider, "bundled");
        assert_eq!(normalized.ui_font_size, 14);
        assert_eq!(normalized.code_font_size, 28);
        assert_eq!(normalized.sidebar_width, 210);
        assert_eq!(normalized.inspector_width, 760);
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(legacy.sound_enabled);
        assert!(legacy.completion_notifications_enabled);
        assert_eq!(legacy.sound_volume, 55);
        assert_eq!(legacy.ui_font_size, 18);
        assert_eq!(legacy.code_font_size, 14);
        let legacy_presets: AppSettings = serde_json::from_str(r#"{"uiFontScale":"large","codeFontScale":"comfortable"}"#).unwrap();
        assert_eq!(legacy_presets.ui_font_size, 18);
        assert_eq!(legacy_presets.code_font_size, 14);
    }

    #[test]
    fn onboarding_state_is_portable_versioned_and_recovers_from_corruption() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        let storage = Storage::open(&database).unwrap();
        let initial = storage.onboarding().unwrap();
        assert_eq!(initial.schema_version, 1);
        assert_eq!(initial.welcome_state, "new");
        assert!(initial.completed_steps.is_empty());

        let saved = OnboardingState {
            welcome_state: "active".into(),
            completed_steps: vec!["project".into(), "project".into(), "unknown".into()],
            skipped_steps: vec!["git".into()],
            dismissed_tips: vec!["diff".into(), "diff".into()],
            demo_project_id: Some("demo-project".into()),
            ..OnboardingState::default()
        };
        storage.save_onboarding(&saved).unwrap();
        let restored = storage.onboarding().unwrap();
        assert_eq!(restored.completed_steps, vec!["project"]);
        assert_eq!(restored.skipped_steps, vec!["git"]);
        assert_eq!(restored.dismissed_tips, vec!["diff"]);
        assert_eq!(restored.demo_project_id.as_deref(), Some("demo-project"));
        drop(storage);

        let reopened = Storage::open(&database).unwrap();
        assert_eq!(reopened.onboarding().unwrap().completed_steps, vec!["project"]);
        reopened.connection.execute(
            "UPDATE settings SET value_json='{broken' WHERE key='onboarding'",
            [],
        ).unwrap();
        assert_eq!(reopened.onboarding().unwrap().welcome_state, "new");
    }

    #[test]
    fn evidence_receipt_projects_factual_events_and_persists_digest() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        let mut storage = Storage::open(&database).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "evidence-receipt".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage
            .create_ticket(&project.id, "Inspect README")
            .unwrap();
        storage
            .append_event(
                Some(&ticket.id),
                "tool.completed",
                json!({
                    "tool": "read_file", "evidence": { "path": "README.md", "contentDigest": "abc" }
                }),
            )
            .unwrap();
        storage
            .set_ticket_state(&ticket.id, TicketState::Completed)
            .unwrap();
        storage
            .append_event(
                Some(&ticket.id),
                "delivery.ready",
                json!({
                    "content": "done", "route": "project_read", "evidence": []
                }),
            )
            .unwrap();
        drop(storage);

        let storage = Storage::open(&database).unwrap();
        let receipt = storage.evidence_receipts().unwrap().pop().unwrap();
        assert_eq!(receipt.ticket_id, ticket.id);
        assert_eq!(receipt.route.as_deref(), Some("project_read"));
        assert_eq!(receipt.outcome, "completed");
        assert_eq!(receipt.sources.len(), 1);
        assert_eq!(receipt.content_digest.len(), 64);
    }

    #[test]
    fn waiting_receipt_distinguishes_user_input_from_effect_approval() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        let mut storage = Storage::open(&database).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "waiting-receipt".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage.create_ticket(&project.id, "Diagnose project").unwrap();
        storage.set_ticket_state(&ticket.id, TicketState::Waiting).unwrap();
        storage.append_event(Some(&ticket.id), "assignment.report", json!({ "state": "waiting", "summary": "need a path" })).unwrap();
        let receipt = storage.evidence_receipts().unwrap().pop().unwrap();
        assert_eq!(receipt.outcome, "waiting_input");

        storage.append_event(Some(&ticket.id), "effect.proposed", json!({ "id": "effect", "path": "main.py" })).unwrap();
        let receipt = storage.evidence_receipts().unwrap().pop().unwrap();
        assert_eq!(receipt.outcome, "waiting_approval");

        storage.append_event(Some(&ticket.id), "effect.approved", json!({ "effectId": "effect", "selectedPaths": ["main.py"] })).unwrap();
        storage.append_event(Some(&ticket.id), "effect.applied", json!({ "effectId": "effect", "paths": ["main.py"] })).unwrap();
        storage.append_event(Some(&ticket.id), "tool.completed", json!({ "tool": "pytest", "evidence": { "exitCode": 1, "preview": "1 failed" } })).unwrap();
        storage.append_event(Some(&ticket.id), "assignment.report", json!({ "state": "waiting", "summary": "tests failed" })).unwrap();
        let receipt = storage.evidence_receipts().unwrap().pop().unwrap();
        assert_eq!(receipt.outcome, "waiting_input");
    }

    #[test]
    fn migration_is_versioned_and_backfills_search_text() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        let mut storage = Storage::open(&database).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "migration".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage
            .create_ticket(&project.id, "Searchable request")
            .unwrap();
        let version: i64 = storage
            .connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let search_text: String = storage
            .connection
            .query_row(
                "SELECT search_text FROM tickets WHERE id=?1",
                [&ticket.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 8);
        assert!(search_text.contains("searchable request"));
    }

    fn create_schema_five_fixture(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                r#"
                PRAGMA foreign_keys=ON;
                CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL);
                INSERT INTO schema_migrations VALUES(5,'2026-01-01T00:00:00Z');
                CREATE TABLE projects(
                  id TEXT PRIMARY KEY,name TEXT NOT NULL,absolute_path TEXT NOT NULL,relative_path TEXT,
                  fingerprint TEXT NOT NULL UNIQUE,trusted INTEGER NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE conversations(
                  id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),title TEXT NOT NULL,
                  archived INTEGER NOT NULL DEFAULT 0,created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE tickets(
                  id TEXT PRIMARY KEY,conversation_id TEXT,project_id TEXT NOT NULL REFERENCES projects(id),
                  request TEXT NOT NULL,title TEXT NOT NULL,state TEXT NOT NULL,policy_version INTEGER NOT NULL DEFAULT 1,
                  created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE assignments(
                  id TEXT PRIMARY KEY,ticket_id TEXT NOT NULL REFERENCES tickets(id),parent_id TEXT,state TEXT NOT NULL,
                  result TEXT,error TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE turns(
                  id TEXT PRIMARY KEY,ticket_id TEXT NOT NULL REFERENCES tickets(id),provider_id TEXT,model_id TEXT,
                  state TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE messages(
                  id TEXT PRIMARY KEY,turn_id TEXT,ticket_id TEXT NOT NULL REFERENCES tickets(id),role TEXT NOT NULL,
                  content_json TEXT NOT NULL,ordinal INTEGER NOT NULL,created_at TEXT NOT NULL
                );
                CREATE TABLE effects(
                  id TEXT PRIMARY KEY,tool_call_id TEXT,ticket_id TEXT NOT NULL REFERENCES tickets(id),risk TEXT NOT NULL,
                  state TEXT NOT NULL,before_digest TEXT,after_digest TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL
                );
                CREATE TABLE deliveries(
                  id TEXT PRIMARY KEY,ticket_id TEXT NOT NULL REFERENCES tickets(id),summary TEXT NOT NULL,
                  evidence_json TEXT NOT NULL,created_at TEXT NOT NULL
                );
                INSERT INTO projects VALUES('project','Fixture','C:/fixture',NULL,'fixture-fingerprint',1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');
                INSERT INTO conversations VALUES('conversation','project','Imported chat',0,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');
                INSERT INTO tickets VALUES('ticket','conversation','project','Fix the fixture','Fixture task','open',5,'2026-01-01T00:00:01Z','2026-01-01T00:00:01Z');
                INSERT INTO assignments VALUES('assignment','ticket',NULL,'completed','done',NULL,'2026-01-01T00:00:02Z','2026-01-01T00:00:03Z');
                INSERT INTO turns VALUES('turn','ticket','bundled','local','completed','2026-01-01T00:00:02Z','2026-01-01T00:00:03Z');
                INSERT INTO messages VALUES('user-message','turn','ticket','user','{"text":"hello"}',0,'2026-01-01T00:00:02Z');
                INSERT INTO messages VALUES('assistant-message','turn','ticket','assistant','{"text":"done"}',1,'2026-01-01T00:00:03Z');
                INSERT INTO effects VALUES('effect',NULL,'ticket','write','applied',NULL,'after','2026-01-01T00:00:02Z','2026-01-01T00:00:03Z');
                INSERT INTO deliveries VALUES('delivery','ticket','done','[]','2026-01-01T00:00:03Z');
                "#,
            )
            .unwrap();
    }

    #[test]
    fn schema_five_migrates_with_backup_and_normalized_history() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        create_schema_five_fixture(&database);

        let storage = Storage::open(&database).unwrap();
        assert_eq!(schema_version(&storage.connection).unwrap(), 8);
        let backup_count = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("schema-v5"))
            .count();
        assert_eq!(backup_count, 1);
        for (table, expected) in [
            ("threads", 1_i64),
            ("mandates", 1),
            ("inputs", 1),
            ("reports", 1),
            ("schema_rollback_pairs", 1),
        ] {
            let count: i64 = storage
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "unexpected row count for {table}");
        }
        let trigger_count: i64 = storage
            .connection
            .query_row("SELECT COUNT(*) FROM triggers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trigger_count, 2);
        let delivery_count: i64 = storage
            .connection
            .query_row("SELECT COUNT(*) FROM deliveries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(delivery_count, 2);
        assert!(storage
            .connection
            .execute(
                "UPDATE mandates SET strength='inferred' WHERE ticket_id='ticket'",
                []
            )
            .is_err());
    }

    #[test]
    fn failed_schema_six_transaction_leaves_schema_five_unchanged() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("ucode.db");
        create_schema_five_fixture(&database);
        let mut connection = Connection::open(&database).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        {
            let transaction = connection.transaction().unwrap();
            migrate_superagent_schema_v6(&transaction).unwrap();
            assert!(transaction
                .execute("UPDATE mandates SET strength='inferred'", [])
                .is_err());
            transaction.rollback().unwrap();
        }
        assert_eq!(schema_version(&connection).unwrap(), 5);
        let has_threads: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_threads);
        assert!(
            !table_has_column(&connection.transaction().unwrap(), "turns", "thread_key").unwrap()
        );
    }

    #[test]
    fn newer_database_schema_is_refused_without_mutation() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("future.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL); INSERT INTO schema_migrations VALUES(9,'future');",
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            Storage::open(&database),
            Err(StorageError::UnsupportedSchema {
                found: 9,
                supported: 8
            })
        ));
        let connection = Connection::open(&database).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 9);
    }

    #[test]
    fn subagent_assignments_persist_parent_state_and_result() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "subagents".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage.create_ticket(&project.id, "parent task").unwrap();
        let parent = storage
            .create_assignment(&ticket.id, None, "orchestrator", "coordinate", "read")
            .unwrap();
        let child = storage
            .create_assignment(
                &ticket.id,
                Some(&parent.id),
                "inspect tests",
                "analyze failures",
                "read",
            )
            .unwrap();
        storage
            .update_assignment(
                &child.id,
                AssignmentState::Running,
                None,
                None,
                Some("external-1"),
            )
            .unwrap();
        storage
            .set_assignment_artifacts(
                &child.id,
                Some("worktrees/child"),
                Some("{\"id\":\"effect-1\"}"),
                Some("diff --git a/a b/a"),
                None,
            )
            .unwrap();
        storage
            .update_assignment(
                &child.id,
                AssignmentState::Completed,
                Some("all tests pass"),
                None,
                None,
            )
            .unwrap();
        let assignments = storage.assignments(&ticket.id).unwrap();
        assert_eq!(assignments.len(), 2);
        assert_eq!(
            assignments[1].parent_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(assignments[1].state, AssignmentState::Completed);
        assert_eq!(assignments[1].result.as_deref(), Some("all tests pass"));
        assert_eq!(assignments[1].external_id.as_deref(), Some("external-1"));
        assert_eq!(
            assignments[1].worktree_path.as_deref(),
            Some("worktrees/child")
        );
        assert!(assignments[1]
            .diff
            .as_deref()
            .unwrap()
            .contains("diff --git"));
        assert!(storage
            .assignment_effect(&child.id)
            .unwrap()
            .unwrap()
            .contains("effect-1"));
        let cancellable = storage
            .create_assignment(&ticket.id, Some(&parent.id), "cancel me", "inspect", "read")
            .unwrap();
        storage
            .update_assignment(
                &cancellable.id,
                AssignmentState::Running,
                None,
                None,
                Some("external-2"),
            )
            .unwrap();
        assert_eq!(
            storage.active_assignment_external_ids(&ticket.id).unwrap(),
            vec![(cancellable.id.clone(), "external-2".into())]
        );
        storage.cancel_assignments(&ticket.id).unwrap();
        assert_eq!(
            storage.assignment(&cancellable.id).unwrap().state,
            AssignmentState::Cancelled
        );
    }

    #[test]
    fn append_event_at_preserves_the_timestamp_used_by_live_runtime_events() {
        let directory = tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let created_at = "2026-11-11T10:11:12.123456700+00:00";
        let sequence = storage
            .append_event_at(None, "provider.delta", json!({ "delta": "字" }), created_at)
            .unwrap();
        let event = storage
            .events(sequence - 1)
            .unwrap()
            .into_iter()
            .find(|event| event.sequence == sequence)
            .unwrap();
        assert_eq!(event.created_at, created_at);
    }

    #[test]
    fn task_context_survives_followups_and_user_corrections() {
        let directory = tempdir().unwrap();
        let mut storage = Storage::open(&directory.path().join("ucode.db")).unwrap();
        let locator = ProjectLocator {
            absolute_path: directory.path().display().to_string(),
            relative_path: None,
            fingerprint: "task-context".into(),
        };
        let project = storage.upsert_project(&locator, true).unwrap();
        let ticket = storage
            .create_ticket(&project.id, "修改 src/a.py，只改这个文件，并运行 pytest")
            .unwrap();
        let initial = storage.task_context(&ticket.id).unwrap();
        assert_eq!(initial.target_files, vec!["src/a.py"]);
        assert!(!initial.constraints.is_empty());
        assert!(!initial.verification.is_empty());

        storage
            .update_task_context(&ticket.id, "改为只修改 src/b.py，保持公开 API 不变")
            .unwrap();
        let corrected = storage.task_context(&ticket.id).unwrap();
        assert_eq!(corrected.target_files, vec!["src/b.py"]);
        assert!(corrected.objective.contains("src/b.py"));
        assert!(corrected.constraints.iter().any(|value| value.contains("公开 API")));
        assert_eq!(corrected.revision, 2);
    }

    #[test]
    fn task_context_marks_missing_action_scope_and_verification() {
        let context = TaskContext::from_request("帮我修复这个项目");
        assert_eq!(context.unresolved_questions.len(), 2);
        assert!(context.target_files.is_empty());
    }
}
