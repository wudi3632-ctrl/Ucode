use crate::attachments::{AttachmentPayload, AttachmentStore, ContextAttachmentSelection};
use crate::domain::{AssignmentState, TicketState};
use crate::legacy::{LegacyAgentClient, LegacyAttachment, LegacyEffect, LegacyMessage, LegacyPlan};
use crate::model_packs::ModelPackInfo;
use crate::notifications::{
    NotificationDiagnosticSnapshot, NotificationDiagnostics, NotificationIdentityState,
};
use crate::portable::{CapsuleHealth, PortableRoot};
use crate::provider::{self, ProviderSecretStatus, ProviderTestResult};
use crate::race::{
    judge, project_capsule, Candidate, CandidateState, CandidateStrategy, CapsuleCandidate,
    HardFailure, Race, RaceState, RaceTarget, ValidationFact, ValidationOutcome,
    RACE_SCHEMA_VERSION,
};
use crate::race_verifier::{acceptance_is_supported, verify_candidate, CandidateProposal};
use crate::race_workspace::{
    create_candidate_workspace, digest_tree, load_candidate_workspace, remove_candidate_workspace,
    workspace_inventory,
};
use crate::rescue::{
    compact_report as compact_rescue_report, diagnose as diagnose_rescue, is_rescue_request,
};
use crate::rescue_probe::{
    requested_probe_description, NetworkProbeService, ProbeStatus, R1ProbeRecord,
};
use crate::rescue_repair::{RepairRecord, RepairService, RepairStatus};
use crate::skills::{SkillPreview, SkillRuntime};
use crate::storage::{
    AppSettings, AssignmentSummary, EvidenceReceipt, OnboardingState, ProjectSummary, RuntimeEvent,
    SkillSummary, Storage, TaskContextScope, TicketSummary, ONBOARDING_STEPS,
};
use crate::superagent::{
    Capability, ExecutorAssignment, ExecutorAttachment, ExecutorCandidate, ExecutorCandidateTarget,
    ExecutorClient, ExecutorEvent, ExecutorEventPayload, ExecutorHistoryItem, ExecutorInput,
    ExecutorMandate, ExecutorMode, ExecutorRequest, ExecutorWorkspace, HistoryRole, ReportState,
    Strength, TicketInputKind, WorkspaceFile, EXECUTOR_PROTOCOL_V1,
};
use crate::supervisor::{
    CompatibilitySupervisor, ProcessSupervisor, RuntimeDiagnosticSnapshot, ServiceStatus,
};
use crate::terminal::{
    validate_terminal_command, TerminalCommandAccepted, TerminalInterruptResult, TerminalManager,
    TerminalReadResult, TerminalStarted,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

pub struct AppState {
    pub portable: PortableRoot,
    pub storage: Arc<Mutex<Storage>>,
    pub skills: Arc<SkillRuntime>,
    pub legacy: LegacyAgentClient,
    pub executor: ExecutorClient,
    pub terminals: Arc<TerminalManager>,
    pub supervisor: Arc<CompatibilitySupervisor>,
    pub attachments: Arc<Mutex<AttachmentStore>>,
    pub capsule_health: Arc<Mutex<CapsuleHealth>>,
    pub rescue_repairs: Arc<RepairService>,
    pub rescue_probes: Arc<NetworkProbeService>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MainWindowActivity {
    focused: bool,
    visible: bool,
    minimized: bool,
}

#[tauri::command]
pub fn main_window_activity(app: AppHandle) -> Result<MainWindowActivity, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    Ok(MainWindowActivity {
        focused: window.is_focused().map_err(|error| error.to_string())?,
        visible: window.is_visible().map_err(|error| error.to_string())?,
        minimized: window.is_minimized().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
pub fn show_completion_notification(app: AppHandle) -> Result<(), String> {
    let diagnostics = app.state::<NotificationDiagnostics>().inner().clone();
    let portable_root = app.state::<AppState>().portable.path().to_path_buf();
    crate::notifications::show_completion(app, portable_root, diagnostics)
}

#[tauri::command]
pub fn completion_notification_diagnostics(
    state: State<'_, NotificationDiagnostics>,
) -> NotificationDiagnosticSnapshot {
    state.snapshot()
}

#[tauri::command]
pub fn notification_identity_status(
    state: State<'_, AppState>,
    diagnostics: State<'_, NotificationDiagnostics>,
) -> NotificationIdentityState {
    crate::notifications::inspect_identity(state.portable.path(), diagnostics.inner())
}

#[tauri::command]
pub fn repair_notification_identity(
    state: State<'_, AppState>,
    diagnostics: State<'_, NotificationDiagnostics>,
) -> Result<NotificationIdentityState, String> {
    crate::notifications::register_identity(state.portable.path(), diagnostics.inner())
}

#[tauri::command]
pub fn clear_notification_identity(
    state: State<'_, AppState>,
    diagnostics: State<'_, NotificationDiagnostics>,
) -> Result<BootstrapSnapshot, String> {
    {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let mut settings = storage.settings().map_err(|error| error.to_string())?;
        settings.completion_notifications_enabled = false;
        storage
            .save_settings(&settings)
            .map_err(|error| error.to_string())?;
    }
    crate::notifications::clear_identity(diagnostics.inner())?;
    snapshot(&state)
}

#[tauri::command]
pub fn restore_main_window(app: AppHandle) {
    crate::show_main_window(&app);
}

#[tauri::command]
pub fn hide_main_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn choose_context_attachments(
    project_id: String,
    selection_id: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ContextAttachmentSelection>, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let selected = app
        .dialog()
        .file()
        .set_title("添加本轮只读上下文文件")
        .blocking_pick_files();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let paths = selected
        .into_iter()
        .map(|value| value.into_path().map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut attachments = state
        .attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?;
    let selection = match selection_id {
        Some(selection_id) => {
            attachments.append(&selection_id, &project_id, Path::new(&project.path), paths)?
        }
        None => attachments.prepare(&project_id, Path::new(&project.path), paths),
    };
    Ok(Some(selection))
}

#[tauri::command]
pub fn remove_context_attachment(
    selection_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .remove_attachment(&selection_id, &attachment_id)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    portable_root: String,
    projects: Vec<ProjectSummary>,
    tickets: Vec<TicketSummary>,
    events: Vec<RuntimeEvent>,
    receipts: Vec<EvidenceReceipt>,
    last_sequence: i64,
    provider_status: &'static str,
    settings: AppSettings,
    onboarding: OnboardingState,
    capsule_health: CapsuleHealth,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceBundle {
    race: Race,
    candidates: Vec<CapsuleCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaceEffectChange {
    path: String,
    operation: String,
    before: String,
    after: String,
    before_digest: Option<String>,
    after_digest: Option<String>,
}

fn race_bundle(race: Race, candidates: Vec<Candidate>) -> RaceBundle {
    let public_candidates = project_capsule(&race, &candidates).candidates;
    RaceBundle {
        race,
        candidates: public_candidates,
    }
}

fn snapshot(state: &AppState) -> Result<BootstrapSnapshot, String> {
    let capsule_health = state
        .capsule_health
        .lock()
        .map_err(|_| "capsule health lock was poisoned".to_string())?
        .clone();
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let last_sequence = storage.last_sequence().map_err(|error| error.to_string())?;
    // `Storage::events` is intentionally bounded. Reading from zero returned
    // the oldest 1000 rows, which made every newer reply disappear once a
    // portable profile accumulated enough history.
    let event_floor = last_sequence.saturating_sub(1_000);
    Ok(BootstrapSnapshot {
        portable_root: git_compatible_path(state.portable.path())
            .display()
            .to_string(),
        projects: storage.projects().map_err(|error| error.to_string())?,
        tickets: storage.tickets().map_err(|error| error.to_string())?,
        events: storage
            .events(event_floor)
            .map_err(|error| error.to_string())?,
        receipts: storage
            .evidence_receipts()
            .map_err(|error| error.to_string())?,
        last_sequence,
        provider_status: if !capsule_health.can_start_runtime {
            "unavailable"
        } else if state.supervisor.is_owned() && state.legacy.health() {
            "ready"
        } else if state.supervisor.status("agent") == ServiceStatus::Starting {
            "offline"
        } else {
            "unavailable"
        },
        settings: storage.settings().map_err(|error| error.to_string())?,
        onboarding: storage.onboarding().map_err(|error| error.to_string())?,
        capsule_health,
    })
}

fn ensure_capsule_writable(state: &AppState) -> Result<(), String> {
    state
        .portable
        .ensure_side_effects_allowed()
        .map_err(|error| error.to_string())
}

fn enabled_skill_context(storage: &Arc<Mutex<Storage>>, runtime: &SkillRuntime) -> String {
    let Ok(guard) = storage.lock() else {
        return String::new();
    };
    let Ok(skills) = guard.skills() else {
        return String::new();
    };
    let mut contexts = Vec::new();
    for skill in skills.into_iter().filter(|skill| skill.enabled).take(3) {
        match runtime.instructions(&skill.relative_path, &skill.instructions_digest) {
            Ok(instructions) => contexts.push(format!(
                "## {} ({})\n{}",
                skill.name, skill.version, instructions
            )),
            Err(error) => {
                let _ = guard.set_skill_enabled(&skill.id, false);
                let _ = guard.append_event(
                    None,
                    "skill.integrity_failed",
                    json!({ "skillId": skill.id, "error": error.to_string() }),
                );
            }
        }
    }
    contexts.join("\n\n")
}

fn legacy_execution_context(
    storage: &Arc<Mutex<Storage>>,
    runtime: &SkillRuntime,
    ticket_id: &str,
) -> String {
    let skill_context = enabled_skill_context(storage, runtime);
    let task_context = storage
        .lock()
        .ok()
        .and_then(|guard| guard.task_context(ticket_id).ok())
        .and_then(|context| serde_json::to_string_pretty(&context).ok())
        .unwrap_or_default();
    match (skill_context.is_empty(), task_context.is_empty()) {
        (true, true) => String::new(),
        (false, true) => skill_context,
        (true, false) => format!("## UCode 持久化任务上下文\n{task_context}"),
        (false, false) => format!("{skill_context}\n\n## UCode 持久化任务上下文\n{task_context}"),
    }
}

#[tauri::command]
pub fn bootstrap(state: State<'_, AppState>) -> Result<BootstrapSnapshot, String> {
    invalidate_legacy_recoverable_races(&state)?;
    reconcile_terminal_race_states(&state)?;
    snapshot(&state)
}

fn race_targets_match_allowed_paths(race: &Race) -> bool {
    if race.targets.is_empty() || race.targets.len() != race.allowed_paths.len() {
        return false;
    }
    let targets = race
        .targets
        .iter()
        .map(|target| target.path.as_str())
        .collect::<BTreeSet<_>>();
    let allowed = race
        .allowed_paths
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    targets.len() == race.targets.len()
        && allowed.len() == race.allowed_paths.len()
        && targets == allowed
}

fn terminal_ticket_state_for_race(state: RaceState) -> Option<TicketState> {
    match state {
        RaceState::NoWinner | RaceState::Stale => Some(TicketState::Failed),
        RaceState::Cancelled => Some(TicketState::Stopped),
        _ => None,
    }
}

fn invalidate_legacy_recoverable_races(state: &AppState) -> Result<(), String> {
    let legacy_races = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .recoverable_races()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|race| {
            race.schema_version != RACE_SCHEMA_VERSION || !race_targets_match_allowed_paths(race)
        })
        .collect::<Vec<_>>();
    if legacy_races.is_empty() {
        return Ok(());
    }
    let managed_root = state.portable.worktrees_dir().join("races");
    for mut race in legacy_races {
        let candidates = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .candidates_for_race(&race.id)
            .map_err(|error| error.to_string())?;
        for candidate in &candidates {
            if let Some(request_id) = candidate.executor_request_id.as_deref() {
                let _ = state.executor.cancel(request_id);
            }
        }
        race.transition(RaceState::Stale, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        {
            let storage = state
                .storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            storage
                .save_race(&race)
                .map_err(|error| error.to_string())?;
            storage
                .set_ticket_state(&race.ticket_id, TicketState::Failed)
                .map_err(|error| error.to_string())?;
            storage
                .append_event(
                    Some(&race.ticket_id),
                    "race.legacy_stopped",
                    json!({
                        "raceId": race.id,
                        "reason": "legacy_protocol_missing_targets",
                        "summary": "该候选任务来自旧协议，缺少逐文件操作约束，已在写入前安全终止；请重新提交任务。",
                        "originalProjectChanged": false,
                    }),
                )
                .map_err(|error| error.to_string())?;
        }
        cleanup_race_workspaces(&managed_root, &race.id, &state.storage);
    }
    Ok(())
}

fn reconcile_terminal_race_states(state: &AppState) -> Result<(), String> {
    let corrections = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let tickets = storage
            .tickets()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|ticket| (ticket.id, ticket.state))
            .collect::<BTreeMap<_, _>>();
        storage
            .races()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter_map(|race| {
                let expected = terminal_ticket_state_for_race(race.state)?;
                (tickets.get(&race.ticket_id).copied() != Some(expected))
                    .then_some((race, expected))
            })
            .collect::<Vec<_>>()
    };
    for (race, expected) in corrections {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .set_ticket_state(&race.ticket_id, expected)
            .map_err(|error| error.to_string())?;
        storage
            .append_event(
                Some(&race.ticket_id),
                "race.terminal_reconciled",
                json!({
                    "raceId": race.id,
                    "raceState": race.state,
                    "ticketState": expected,
                    "summary": "候选竞赛终态已与任务状态对账；失败或失效的候选不会再显示为已完成。",
                    "originalProjectChanged": matches!(race.state, RaceState::Applied | RaceState::RolledBack),
                }),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn list_ticket_races(
    ticket_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<RaceBundle>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .races_for_ticket(&ticket_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|race| {
            let candidates = storage
                .candidates_for_race(&race.id)
                .map_err(|error| error.to_string())?;
            Ok(race_bundle(race, candidates))
        })
        .collect()
}

#[tauri::command]
pub fn start_candidate_race(
    ticket_id: String,
    targets: Vec<RaceTarget>,
    acceptance: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RaceBundle, String> {
    ensure_capsule_writable(&state)?;
    let targets = normalize_race_targets(targets)?;
    let allowed_paths: Vec<String> = targets.iter().map(|target| target.path.clone()).collect();
    if acceptance.is_empty()
        || acceptance.len() > 16
        || acceptance
            .iter()
            .any(|value| !acceptance_is_supported(value))
    {
        return Err(
            "候选竞赛需要 1-16 个受支持的机械验收命令（pytest、cargo test 或受限 npm 脚本）".into(),
        );
    }
    let (ticket, project, settings) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = storage
            .ticket(&ticket_id)
            .map_err(|error| error.to_string())?;
        if storage
            .races_for_ticket(&ticket_id)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|race| race.state.is_recoverable())
        {
            return Err("该任务已有尚未结束的候选竞赛".into());
        }
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let settings = storage.settings().map_err(|error| error.to_string())?;
        (ticket, project, settings)
    };
    let now = Utc::now().to_rfc3339();
    let race_id = format!("race-{}", Uuid::new_v4());
    let project_digest =
        digest_tree(Path::new(&project.path)).map_err(|error| error.to_string())?;
    let race = Race {
        schema_version: RACE_SCHEMA_VERSION,
        id: race_id.clone(),
        ticket_id: ticket.id.clone(),
        project_fingerprint: project_digest,
        objective: ticket.request.clone(),
        state: RaceState::Prepared,
        targets,
        allowed_paths: allowed_paths.clone(),
        acceptance: acceptance.clone(),
        verdict: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let model = if settings.model_provider == "bundled" {
        settings.bundled_model_profile.clone()
    } else {
        settings.provider_model.clone()
    };
    let candidates = [
        CandidateStrategy::MinimalChange,
        CandidateStrategy::CompatibilityFirst,
    ]
    .into_iter()
    .map(|strategy| Candidate {
        schema_version: RACE_SCHEMA_VERSION,
        id: format!("candidate-{}", Uuid::new_v4()),
        race_id: race.id.clone(),
        strategy,
        state: CandidateState::Queued,
        provider: settings.model_provider.clone(),
        model: model.clone(),
        executor_request_id: None,
        workspace_path: None,
        diff: None,
        base_digest: String::new(),
        result_digest: None,
        facts: Vec::new(),
        metrics: Default::default(),
        generation_attempts: 0,
        created_at: now.clone(),
        updated_at: now.clone(),
    })
    .collect::<Vec<_>>();
    {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .save_race(&race)
            .map_err(|error| error.to_string())?;
        storage
            .set_ticket_state(&ticket.id, TicketState::Running)
            .map_err(|error| error.to_string())?;
        for candidate in &candidates {
            storage
                .save_candidate(candidate)
                .map_err(|error| error.to_string())?;
        }
        storage
            .append_event(
                Some(&ticket.id),
                "race.prepared",
                json!({ "raceId": race.id, "candidateIds": candidates.iter().map(|candidate| &candidate.id).collect::<Vec<_>>() }),
            )
            .map_err(|error| error.to_string())?;
    }
    let storage = Arc::clone(&state.storage);
    let executor = state.executor.clone();
    let managed_root = state.portable.worktrees_dir().join("races");
    let task_race_id = race.id.clone();
    let task_project_id = project.id.clone();
    let task_project_path = project.path.clone();
    std::thread::spawn(move || {
        if let Err(error) = run_candidate_race_task(
            &storage,
            &executor,
            &app,
            &task_race_id,
            &task_project_id,
            Path::new(&task_project_path),
            &managed_root,
        ) {
            cleanup_race_workspaces(&managed_root, &task_race_id, &storage);
            fail_race(&storage, &app, &task_race_id, &error);
        }
    });
    Ok(race_bundle(race, candidates))
}

#[tauri::command]
pub fn cancel_candidate_race(
    race_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RaceBundle, String> {
    let mut race = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .race(&race_id)
        .map_err(|error| error.to_string())?;
    if !matches!(
        race.state,
        RaceState::Prepared | RaceState::Generating | RaceState::Validating
    ) {
        return Err("只有运行中的候选竞赛可以取消".into());
    }
    race.transition(RaceState::Cancelled, Utc::now().to_rfc3339())
        .map_err(|error| error.to_string())?;
    let mut candidates = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .save_race(&race)
            .map_err(|error| error.to_string())?;
        storage
            .set_ticket_state(&race.ticket_id, TicketState::Stopped)
            .map_err(|error| error.to_string())?;
        storage
            .candidates_for_race(&race_id)
            .map_err(|error| error.to_string())?
    };
    for candidate in &mut candidates {
        if let Some(request_id) = candidate.executor_request_id.as_deref() {
            let _ = state.executor.cancel(request_id);
        }
        if candidate.state.can_transition_to(CandidateState::Cancelled) {
            candidate
                .transition(CandidateState::Cancelled, Utc::now().to_rfc3339())
                .map_err(|error| error.to_string())?;
            state
                .storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?
                .save_candidate(candidate)
                .map_err(|error| error.to_string())?;
        }
    }
    append_runtime_event(
        &state.storage,
        &race.ticket_id,
        "race.cancelled",
        json!({ "raceId": race.id }),
        &app,
    );
    Ok(race_bundle(race, candidates))
}

#[tauri::command]
pub fn recheck_capsule(state: State<'_, AppState>) -> Result<BootstrapSnapshot, String> {
    let mut health = state.portable.inspect_capsule();
    let selected_profile = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .settings()
        .map_err(|error| error.to_string())?
        .bundled_model_profile;
    if let Err(error) =
        crate::model_packs::verify_selected(state.portable.path(), &selected_profile)
    {
        health.status = "blocked".into();
        health.can_start_runtime = false;
        health.side_effects_allowed = false;
        health.issues.push(format!("本地模型包未通过验证：{error}"));
    }
    if health.can_start_runtime {
        state.supervisor.ensure_started();
        state.supervisor.start_watchdog();
    } else {
        let _ = state.supervisor.stop_owned();
    }
    *state
        .capsule_health
        .lock()
        .map_err(|_| "capsule health lock was poisoned".to_string())? = health;
    snapshot(&state)
}

#[tauri::command]
pub async fn runtime_diagnostics(
    state: State<'_, AppState>,
) -> Result<RuntimeDiagnosticSnapshot, String> {
    let supervisor = Arc::clone(&state.supervisor);
    tauri::async_runtime::spawn_blocking(move || supervisor.diagnostics())
        .await
        .map_err(|error| format!("无法检查本地运行时：{error}"))
}

#[tauri::command]
pub async fn recover_local_runtime(
    state: State<'_, AppState>,
) -> Result<RuntimeDiagnosticSnapshot, String> {
    let health = state
        .capsule_health
        .lock()
        .map_err(|_| "capsule health lock was poisoned".to_string())?
        .clone();
    if !health.can_start_runtime {
        return Err("便携胶囊未通过启动检查，不能恢复本地模型服务。".into());
    }
    let supervisor = Arc::clone(&state.supervisor);
    tauri::async_runtime::spawn_blocking(move || supervisor.recover())
        .await
        .map_err(|error| format!("本地运行时恢复任务失败：{error}"))?
}

#[tauri::command]
pub fn frontend_ready(location: String, state: State<'_, AppState>) -> Result<i64, String> {
    if location.len() > 2_048 {
        return Err("frontend location is too long".into());
    }
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .append_event(
            None,
            "desktop.frontend.ready",
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "build": if cfg!(debug_assertions) { "debug" } else { "release" },
                "location": location,
            }),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    validate_external_url(&url)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("explorer.exe")
            .arg(&url)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("无法用系统浏览器打开链接：{error}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("当前系统不支持打开外部链接".into())
    }
}

fn validate_external_url(value: &str) -> Result<(), String> {
    if value.len() > 2_048
        || value
            .chars()
            .any(|character| character == '\r' || character == '\n')
    {
        return Err("链接格式无效".into());
    }
    let parsed = tauri::Url::parse(value).map_err(|_| "链接格式无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return Err("仅允许打开 http、https 或 mailto 链接".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("不允许打开包含凭据的链接".into());
    }
    Ok(())
}

#[tauri::command]
pub fn select_project(
    path: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    let locator = state
        .portable
        .locate_project(path)
        .map_err(|error| error.to_string())?;
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .upsert_project(&locator, true)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub fn create_onboarding_demo(state: State<'_, AppState>) -> Result<BootstrapSnapshot, String> {
    ensure_capsule_writable(&state)?;
    let demo_root = state.portable.data_dir().join("demo-projects");
    fs::create_dir_all(&demo_root).map_err(|error| error.to_string())?;
    let expected = [
        (
            "README.md",
            "# UCode Starter\n\n这是一个可安全修改和删除的入门项目。\n",
        ),
        (
            "calculator.py",
            "def add(a, b):\n    \"\"\"Return the sum of two numbers.\"\"\"\n    return a + b\n",
        ),
        (
            "test_calculator.py",
            "from calculator import add\n\n\ndef test_add():\n    assert add(2, 3) == 5\n",
        ),
    ];
    let base = demo_root.join("ucode-starter");
    let reusable = base.is_dir()
        && expected.iter().all(|(name, content)| {
            fs::read_to_string(base.join(name)).is_ok_and(|value| value == *content)
        });
    let target = if !base.exists() || reusable {
        base
    } else {
        demo_root.join(format!(
            "ucode-starter-{}",
            Utc::now().format("%Y%m%d-%H%M%S")
        ))
    };
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    for (name, content) in expected {
        let file = target.join(name);
        if !file.exists() {
            fs::write(file, content).map_err(|error| error.to_string())?;
        }
    }
    let locator = state
        .portable
        .locate_project(&target)
        .map_err(|error| error.to_string())?;
    {
        let mut storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let project = storage
            .upsert_project(&locator, true)
            .map_err(|error| error.to_string())?;
        let mut onboarding = storage.onboarding().map_err(|error| error.to_string())?;
        onboarding.welcome_state = "active".into();
        onboarding.demo_project_id = Some(project.id);
        complete_onboarding_value(&mut onboarding, "project");
        storage
            .save_onboarding(&onboarding)
            .map_err(|error| error.to_string())?;
    }
    snapshot(&state)
}

#[tauri::command]
pub fn set_onboarding_welcome(
    welcome_state: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if !matches!(welcome_state.as_str(), "active" | "dismissed") {
        return Err("不支持的引导状态".into());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let mut onboarding = storage.onboarding().map_err(|error| error.to_string())?;
    onboarding.welcome_state = welcome_state;
    onboarding.updated_at = Utc::now().to_rfc3339();
    storage
        .save_onboarding(&onboarding)
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(&state)
}

#[tauri::command]
pub fn complete_onboarding_step(
    step_id: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if !ONBOARDING_STEPS.contains(&step_id.as_str()) {
        return Err("未知的引导任务".into());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let verified = match step_id.as_str() {
        "project" => !storage
            .projects()
            .map_err(|error| error.to_string())?
            .is_empty(),
        "conversation" => storage
            .has_event_type("delivery.ready")
            .map_err(|error| error.to_string())?,
        "review_change" => storage
            .has_event_type("effect.applied")
            .map_err(|error| error.to_string())?,
        "terminal" => storage
            .has_terminal_audit()
            .map_err(|error| error.to_string())?,
        "git" => storage
            .has_event_type("onboarding.git_observed")
            .map_err(|error| error.to_string())?,
        "agent" => storage
            .has_completed_assignment()
            .map_err(|error| error.to_string())?,
        "evidence" => !storage
            .evidence_receipts()
            .map_err(|error| error.to_string())?
            .is_empty(),
        _ => false,
    };
    if !verified {
        return Err("该能力尚未产生可验证的完成结果".into());
    }
    let mut onboarding = storage.onboarding().map_err(|error| error.to_string())?;
    complete_onboarding_value(&mut onboarding, &step_id);
    storage
        .save_onboarding(&onboarding)
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(&state)
}

#[tauri::command]
pub fn skip_onboarding_step(
    step_id: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if !ONBOARDING_STEPS.contains(&step_id.as_str()) {
        return Err("未知的引导任务".into());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let mut onboarding = storage.onboarding().map_err(|error| error.to_string())?;
    if !onboarding.skipped_steps.contains(&step_id) {
        onboarding.skipped_steps.push(step_id.clone());
    }
    onboarding.completed_steps.retain(|value| value != &step_id);
    if onboarding.welcome_state == "new" || onboarding.welcome_state == "dismissed" {
        onboarding.welcome_state = "active".into();
    }
    finish_onboarding_if_ready(&mut onboarding);
    storage
        .save_onboarding(&onboarding)
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(&state)
}

#[tauri::command]
pub fn dismiss_onboarding_tip(
    tip_id: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if tip_id.len() > 64
        || !tip_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-')
    {
        return Err("无效的引导提示".into());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let mut onboarding = storage.onboarding().map_err(|error| error.to_string())?;
    if !onboarding.dismissed_tips.contains(&tip_id) {
        onboarding.dismissed_tips.push(tip_id);
    }
    onboarding.updated_at = Utc::now().to_rfc3339();
    storage
        .save_onboarding(&onboarding)
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(&state)
}

#[tauri::command]
pub fn reset_onboarding(state: State<'_, AppState>) -> Result<BootstrapSnapshot, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .save_onboarding(&OnboardingState::default())
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(&state)
}

fn complete_onboarding_value(onboarding: &mut OnboardingState, step_id: &str) {
    let step = step_id.to_string();
    if !onboarding.completed_steps.contains(&step) {
        onboarding.completed_steps.push(step.clone());
    }
    onboarding.skipped_steps.retain(|value| value != &step);
    if onboarding.welcome_state == "new" || onboarding.welcome_state == "dismissed" {
        onboarding.welcome_state = "active".into();
    }
    finish_onboarding_if_ready(onboarding);
}

fn finish_onboarding_if_ready(onboarding: &mut OnboardingState) {
    let resolved = ONBOARDING_STEPS.iter().all(|step| {
        onboarding.completed_steps.iter().any(|value| value == step)
            || onboarding.skipped_steps.iter().any(|value| value == step)
    });
    if resolved {
        onboarding.welcome_state = "completed".into();
    }
    onboarding.updated_at = Utc::now().to_rfc3339();
}

#[tauri::command]
pub fn create_ticket(
    project_id: String,
    request: String,
    state: State<'_, AppState>,
) -> Result<TicketSummary, String> {
    let request = request.trim();
    if request.is_empty() {
        return Err("task request is required".into());
    }
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .create_ticket(&project_id, request)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn rename_ticket(
    ticket_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if title.trim().is_empty() {
        return Err("conversation title is required".into());
    }
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .rename_ticket(&ticket_id, &title)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub fn delete_ticket(
    ticket_id: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .delete_ticket(&ticket_id)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub fn pin_ticket(
    ticket_id: String,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .set_ticket_pinned(&ticket_id, pinned)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub fn archive_ticket(
    ticket_id: String,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .set_ticket_archived(&ticket_id, archived)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub fn branch_ticket(
    ticket_id: String,
    through_sequence: i64,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .branch_ticket(&ticket_id, through_sequence)
        .map_err(|error| error.to_string())?;
    snapshot(&state)
}

#[tauri::command]
pub async fn save_settings(
    settings: AppSettings,
    state: State<'_, AppState>,
    diagnostics: State<'_, NotificationDiagnostics>,
) -> Result<BootstrapSnapshot, String> {
    if !["dark", "light", "system"].contains(&settings.theme.as_str()) {
        return Err("unsupported theme".into());
    }
    if !(14..=32).contains(&settings.ui_font_size) {
        return Err("UI font size is outside the supported range".into());
    }
    if !(12..=28).contains(&settings.code_font_size) {
        return Err("code font size is outside the supported range".into());
    }
    let shell_name = Path::new(settings.terminal_shell.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ![
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "cmd",
        "cmd.exe",
    ]
    .contains(&shell_name.as_str())
    {
        return Err("unsupported terminal shell".into());
    }
    if settings.git_user_name.len() > 200 || settings.git_user_email.len() > 320 {
        return Err("Git identity is too long".into());
    }
    if !(210..=420).contains(&settings.sidebar_width)
        || !(320..=760).contains(&settings.inspector_width)
    {
        return Err("panel width is outside the supported range".into());
    }
    provider::validate_settings(&settings)?;
    let previous = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .settings()
        .map_err(|error| error.to_string())?;
    let portable_root = state.portable.path().to_path_buf();
    let profile_id = settings.bundled_model_profile.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::model_packs::verify_selected(&portable_root, &profile_id)
    })
    .await
    .map_err(|error| format!("模型包验证任务失败：{error}"))??;
    let profile_changed = previous.bundled_model_profile != settings.bundled_model_profile;
    if profile_changed {
        let supervisor = Arc::clone(&state.supervisor);
        let profile_id = settings.bundled_model_profile.clone();
        tauri::async_runtime::spawn_blocking(move || supervisor.restart_with_profile(&profile_id))
            .await
            .map_err(|error| format!("模型切换任务失败：{error}"))??;
    }
    let save_result = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .save_settings(&settings)
            .map_err(|error| error.to_string())
    };
    if let Err(error) = save_result {
        if profile_changed {
            let supervisor = Arc::clone(&state.supervisor);
            let previous_profile = previous.bundled_model_profile.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                supervisor.restart_with_profile(&previous_profile)
            })
            .await;
        }
        return Err(error);
    }
    if settings.completion_notifications_enabled {
        // Saving remains successful when Windows registration is unavailable;
        // the native notification send path will stay closed and retry on the
        // next launch or settings save.
        let _ = crate::notifications::register_identity(state.portable.path(), diagnostics.inner());
    }
    snapshot(&state)
}

#[tauri::command]
pub async fn list_model_packs(state: State<'_, AppState>) -> Result<Vec<ModelPackInfo>, String> {
    let portable_root = state.portable.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || crate::model_packs::inventory(&portable_root))
        .await
        .map_err(|error| format!("模型包扫描失败：{error}"))
}

#[tauri::command]
pub fn provider_secret_status() -> Result<ProviderSecretStatus, String> {
    provider::secret_status()
}

#[tauri::command]
pub fn save_provider_secret(secret: String) -> Result<ProviderSecretStatus, String> {
    provider::save_secret(&secret)
}

#[tauri::command]
pub fn clear_provider_secret() -> Result<ProviderSecretStatus, String> {
    provider::clear_secret()
}

#[tauri::command]
pub async fn test_model_provider(
    settings: AppSettings,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<ProviderTestResult, String> {
    let legacy = state.legacy.clone();
    tauri::async_runtime::spawn_blocking(move || {
        provider::configure(&settings, &legacy, api_key.as_deref())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>) -> Result<Vec<SkillSummary>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .skills()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn preview_skill(path: String, state: State<'_, AppState>) -> Result<SkillPreview, String> {
    state
        .skills
        .preview(Path::new(&path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn install_skill(
    preview: SkillPreview,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Vec<SkillSummary>, String> {
    if !approved {
        return Err("安装技能需要明确批准".into());
    }
    ensure_capsule_writable(&state)?;
    state
        .skills
        .install(&preview)
        .map_err(|error| error.to_string())?;
    let record = SkillSummary {
        id: preview.id.clone(),
        name: preview.name,
        version: preview.version,
        description: preview.description,
        relative_path: preview.id,
        capabilities: preview.capabilities,
        instructions_digest: preview.instructions_digest,
        compatible: preview.compatible,
        compatibility_reason: preview.compatibility_reason,
        enabled: false,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .upsert_skill(&record)
        .map_err(|error| error.to_string())?;
    storage
        .append_event(
            None,
            "skill.installed",
            json!({ "skillId": record.id, "compatible": record.compatible }),
        )
        .map_err(|error| error.to_string())?;
    storage.skills().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_skill_enabled(
    skill_id: String,
    enabled: bool,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Vec<SkillSummary>, String> {
    if !approved {
        return Err("启停技能需要明确批准".into());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let skills = storage.skills().map_err(|error| error.to_string())?;
    let skill = skills
        .iter()
        .find(|skill| skill.id == skill_id)
        .ok_or_else(|| "技能不存在".to_string())?;
    if enabled {
        if !skill.compatible {
            return Err(skill
                .compatibility_reason
                .clone()
                .unwrap_or_else(|| "技能与当前运行时不兼容".into()));
        }
        if skills
            .iter()
            .filter(|value| value.enabled && value.id != skill_id)
            .count()
            >= 3
        {
            return Err("最多同时启用 3 个技能".into());
        }
        state
            .skills
            .instructions(&skill.relative_path, &skill.instructions_digest)
            .map_err(|error| error.to_string())?;
    }
    storage
        .set_skill_enabled(&skill_id, enabled)
        .map_err(|error| error.to_string())?;
    storage
        .append_event(
            None,
            if enabled {
                "skill.enabled"
            } else {
                "skill.disabled"
            },
            json!({ "skillId": skill_id }),
        )
        .map_err(|error| error.to_string())?;
    storage.skills().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn uninstall_skill(
    skill_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Vec<SkillSummary>, String> {
    if !approved {
        return Err("卸载技能需要明确批准".into());
    }
    ensure_capsule_writable(&state)?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let skill = storage
        .skills()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|skill| skill.id == skill_id)
        .ok_or_else(|| "技能不存在".to_string())?;
    let trash = state
        .skills
        .uninstall(&skill.relative_path)
        .map_err(|error| error.to_string())?;
    storage
        .remove_skill(&skill_id)
        .map_err(|error| error.to_string())?;
    storage
        .append_event(
            None,
            "skill.uninstalled",
            json!({ "skillId": skill_id, "recoverablePath": trash.display().to_string() }),
        )
        .map_err(|error| error.to_string())?;
    storage.skills().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn export_markdown(path: String, content: String) -> Result<(), String> {
    let target = PathBuf::from(path);
    if target
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("md"))
        != Some(true)
    {
        return Err("导出文件必须使用 .md 扩展名".into());
    }
    let parent = target.parent().ok_or_else(|| "导出路径无效".to_string())?;
    if !parent.is_dir() {
        return Err("导出目录不存在".into());
    }
    fs::write(target, content).map_err(|error| format!("导出对话失败: {error}"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileEntry {
    path: String,
    name: String,
    is_directory: bool,
    depth: usize,
    size: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    path: String,
    content: String,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    is_repository: bool,
    branch: String,
    clean: bool,
    lines: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDetails {
    is_repository: bool,
    branch: String,
    clean: bool,
    lines: Vec<String>,
    diff: String,
    staged_diff: String,
    history: Vec<String>,
    branches: Vec<String>,
    changes: Vec<GitChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    status: String,
    path: String,
    original_path: Option<String>,
    staged: bool,
    unstaged: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    path: String,
    branch: String,
    head: String,
    is_main: bool,
    locked: bool,
    prunable: bool,
    dirty: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRequest {
    label: String,
    request: String,
    capability: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResult {
    command: String,
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[tauri::command]
pub fn list_project_files(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectFileEntry>, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let root = PathBuf::from(project.path);
    let mut entries = Vec::new();
    collect_project_files(&root, &root, 0, &mut entries)?;
    Ok(entries)
}

#[tauri::command]
pub fn read_project_file(
    project_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<FilePreview, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let root = fs::canonicalize(&project.path).map_err(|error| error.to_string())?;
    let relative = Path::new(&path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("file path must stay inside the selected project".into());
    }
    let target = fs::canonicalize(root.join(relative)).map_err(|error| error.to_string())?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("file path must stay inside the selected project".into());
    }
    let bytes = fs::read(&target).map_err(|error| error.to_string())?;
    let full = String::from_utf8_lossy(&bytes);
    let content: String = full.chars().take(200_000).collect();
    let truncated = content.len() < full.len();
    Ok(FilePreview {
        path,
        content,
        truncated,
    })
}

#[tauri::command]
pub fn git_status(project_id: String, state: State<'_, AppState>) -> Result<GitSnapshot, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let output = Command::new("git")
        .args(["-C", &project.path, "status", "--short", "--branch"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(GitSnapshot {
            is_repository: false,
            branch: String::new(),
            clean: false,
            lines: Vec::new(),
        });
    }
    let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let branch = lines
        .first()
        .cloned()
        .unwrap_or_default()
        .trim_start_matches("## ")
        .to_string();
    Ok(GitSnapshot {
        is_repository: true,
        branch,
        clean: lines.len() <= 1,
        lines: lines.into_iter().skip(1).collect(),
    })
}

#[tauri::command]
pub fn git_details(project_id: String, state: State<'_, AppState>) -> Result<GitDetails, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let details = git_details_for(&project.path)?;
    if details.is_repository {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        if !storage
            .has_event_type("onboarding.git_observed")
            .map_err(|error| error.to_string())?
        {
            storage
                .append_event(
                    None,
                    "onboarding.git_observed",
                    json!({ "projectId": project_id }),
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(details)
}

#[tauri::command]
pub fn git_operation(
    project_id: String,
    operation: String,
    paths: Vec<String>,
    message: String,
    branch: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<GitDetails, String> {
    if !approved {
        return Err("Git 写操作尚未批准".into());
    }
    ensure_capsule_writable(&state)?;
    let (project, settings) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        (
            storage
                .project(&project_id)
                .map_err(|error| error.to_string())?,
            storage.settings().map_err(|error| error.to_string())?,
        )
    };
    let mut args: Vec<String> = vec!["-C".into(), project.path.clone()];
    match operation.as_str() {
        "stage" | "unstage" => {
            if paths.is_empty() {
                return Err("请选择文件".into());
            }
            for value in &paths {
                validate_relative(value)?;
            }
            if operation == "stage" {
                args.push("add".into());
                args.push("--".into());
            } else {
                args.extend(["restore".into(), "--staged".into(), "--".into()]);
            }
            args.extend(paths);
        }
        "commit" => {
            let value = message.trim();
            if value.is_empty() {
                return Err("提交信息不能为空".into());
            }
            args.extend([
                "commit".into(),
                "-m".into(),
                value.chars().take(240).collect(),
            ]);
        }
        "create_branch" => {
            validate_branch(&branch)?;
            args.extend(["switch".into(), "-c".into(), branch]);
        }
        "switch_branch" => {
            validate_branch(&branch)?;
            args.extend(["switch".into(), branch]);
        }
        _ => return Err("不支持的 Git 操作".into()),
    }
    let mut process = Command::new("git");
    process.args(&args);
    if operation == "commit" {
        if !settings.git_user_name.trim().is_empty() {
            process
                .env("GIT_AUTHOR_NAME", settings.git_user_name.trim())
                .env("GIT_COMMITTER_NAME", settings.git_user_name.trim());
        }
        if !settings.git_user_email.trim().is_empty() {
            process
                .env("GIT_AUTHOR_EMAIL", settings.git_user_email.trim())
                .env("GIT_COMMITTER_EMAIL", settings.git_user_email.trim());
        }
    }
    let output = process.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    git_details_for(&project.path)
}

#[tauri::command]
pub fn list_worktrees(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeEntry>, String> {
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    list_worktrees_for(&project.path)
}

#[tauri::command]
pub fn create_worktree(
    project_id: String,
    name: String,
    branch: String,
    base: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeEntry>, String> {
    if !approved {
        return Err("Worktree 创建尚未批准".into());
    }
    ensure_capsule_writable(&state)?;
    validate_worktree_name(&name)?;
    validate_branch(&branch)?;
    let base = if base.trim().is_empty() {
        "HEAD"
    } else {
        base.trim()
    };
    validate_revision(base)?;
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    ensure_git_repository(&project.path)?;

    let parent = state.portable.worktrees_dir().join(&project_id);
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let parent = fs::canonicalize(&parent).map_err(|error| error.to_string())?;
    let target = parent.join(&name);
    if target.exists() {
        return Err("同名 Worktree 目录已存在".into());
    }

    add_worktree_for(&project.path, &target, &branch, base)?;
    list_worktrees_for(&project.path)
}

#[tauri::command]
pub fn remove_worktree(
    project_id: String,
    path: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeEntry>, String> {
    if !approved {
        return Err("Worktree 移除尚未批准".into());
    }
    ensure_capsule_writable(&state)?;
    let project = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .project(&project_id)
        .map_err(|error| error.to_string())?;
    let root = state.portable.worktrees_dir();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    remove_worktree_for(&project.path, Path::new(&path), &root)?;
    list_worktrees_for(&project.path)
}

#[tauri::command]
pub fn list_subagents(
    ticket_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<AssignmentSummary>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .assignments(&ticket_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn start_subagents(
    ticket_id: String,
    requests: Vec<SubagentRequest>,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AssignmentSummary>, String> {
    if !approved {
        return Err("Sub-agent 委派尚未批准".into());
    }
    if requests.is_empty() || requests.len() > 3 {
        return Err("每批必须包含 1 到 3 个子任务".into());
    }
    for request in &requests {
        validate_subagent_request(request)?;
    }
    let (ticket, project, root, children) = {
        let guard = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = guard
            .ticket(&ticket_id)
            .map_err(|error| error.to_string())?;
        if matches!(ticket.state, TicketState::Running | TicketState::Stopping) {
            return Err("主 Agent 正在运行，请等待本轮结束后再委派子任务".into());
        }
        if guard
            .assignments(&ticket_id)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|assignment| {
                matches!(
                    assignment.state,
                    AssignmentState::Queued | AssignmentState::Running | AssignmentState::Waiting
                )
            })
        {
            return Err("当前已有 Sub-agent 正在运行".into());
        }
        let project = guard
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        if requests.iter().any(|request| request.capability == "edit") {
            ensure_git_repository(&project.path)?;
        }
        let root = guard
            .create_assignment(
                &ticket_id,
                None,
                "协调子任务",
                &ticket.request,
                "orchestrate",
            )
            .map_err(|error| error.to_string())?;
        let mut children = Vec::new();
        for request in &requests {
            children.push(
                guard
                    .create_assignment(
                        &ticket_id,
                        Some(&root.id),
                        request.label.trim(),
                        request.request.trim(),
                        &request.capability,
                    )
                    .map_err(|error| error.to_string())?,
            );
        }
        (ticket, project, root, children)
    };
    let storage = Arc::clone(&state.storage);
    let skills = Arc::clone(&state.skills);
    let legacy = state.legacy.clone();
    let executor = state.executor.clone();
    let task_app = app.clone();
    let worktrees_root = state.portable.worktrees_dir().join(&project.id);
    tauri::async_runtime::spawn_blocking(move || {
        run_subagent_batch(
            storage,
            legacy,
            executor,
            skills,
            task_app,
            ticket.id,
            project.path,
            worktrees_root,
            root,
            children,
        );
    });
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .assignments(&ticket_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_subagents(
    ticket_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AssignmentSummary>, String> {
    let (pending, project_path, managed_root) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = storage
            .ticket(&ticket_id)
            .map_err(|error| error.to_string())?;
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        (
            storage
                .assignments(&ticket_id)
                .map_err(|error| error.to_string())?,
            project.path,
            state.portable.worktrees_dir().join(&ticket.project_id),
        )
    };
    for assignment in pending
        .iter()
        .filter(|assignment| assignment.state == AssignmentState::Waiting)
    {
        let effect = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .assignment_effect(&assignment.id)
            .map_err(|error| error.to_string())?;
        if let (Some(effect), Some(external_id)) = (effect, assignment.external_id.as_deref()) {
            if let Ok(effect) = serde_json::from_str::<LegacyEffect>(&effect) {
                let _ = state.legacy.approve(external_id, &effect.id, false, None);
            }
        }
    }
    let references = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .active_assignment_external_ids(&ticket_id)
        .map_err(|error| error.to_string())?;
    for (_, external_id) in &references {
        let _ = state.legacy.cancel(external_id);
    }
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .cancel_assignments(&ticket_id)
        .map_err(|error| error.to_string())?;
    for assignment in pending
        .iter()
        .filter(|assignment| assignment.capability == "edit")
    {
        if let Some(path) = assignment.worktree_path.as_deref() {
            let _ = remove_worktree_for(&project_path, Path::new(path), &managed_root);
        }
    }
    append_runtime_event(
        &state.storage,
        &ticket_id,
        "subagents.cancelled",
        json!({ "count": references.len() }),
        &app,
    );
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .assignments(&ticket_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn resolve_subagent_effect(
    assignment_id: String,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AssignmentSummary>, String> {
    if approved {
        ensure_capsule_writable(&state)?;
    }
    let (assignment, effect, external_id, worktree_path, project_path, managed_root) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let assignment = storage
            .assignment(&assignment_id)
            .map_err(|error| error.to_string())?;
        if assignment.capability != "edit" || assignment.state != AssignmentState::Waiting {
            return Err("该编辑子任务当前没有待审批修改".into());
        }
        let effect_json = storage
            .assignment_effect(&assignment_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "待审批修改已丢失".to_string())?;
        let effect: LegacyEffect =
            serde_json::from_str(&effect_json).map_err(|error| error.to_string())?;
        let external_id = assignment
            .external_id
            .clone()
            .ok_or_else(|| "Sub-agent 外部引用缺失".to_string())?;
        let worktree_path = assignment
            .worktree_path
            .clone()
            .ok_or_else(|| "隔离 Worktree 路径缺失".to_string())?;
        let ticket = storage
            .ticket(&assignment.ticket_id)
            .map_err(|error| error.to_string())?;
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let managed_root = state.portable.worktrees_dir().join(&ticket.project_id);
        (
            assignment,
            effect,
            external_id,
            worktree_path,
            project.path,
            managed_root,
        )
    };
    ensure_managed_assignment_worktree(&project_path, &worktree_path, &managed_root)?;
    state
        .legacy
        .approve(&external_id, &effect.id, approved, None)
        .map_err(|error| error.to_string())?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let runtime_event = if approved {
        let diff = match git_diff_for(&worktree_path) {
            Ok(diff) if !diff.trim().is_empty() => diff,
            Ok(_) => {
                let message = "批准后 Worktree 没有产生可合并的 Git Diff";
                storage
                    .update_assignment(
                        &assignment_id,
                        AssignmentState::Failed,
                        None,
                        Some(message),
                        None,
                    )
                    .map_err(|error| error.to_string())?;
                drop(storage);
                append_runtime_event(
                    &state.storage,
                    &assignment.ticket_id,
                    "subagent.failed",
                    json!({ "assignmentId": assignment_id, "error": message }),
                    &app,
                );
                recompute_subagent_root(&state.storage, &assignment)?;
                return Err(message.into());
            }
            Err(error) => {
                storage
                    .update_assignment(
                        &assignment_id,
                        AssignmentState::Failed,
                        None,
                        Some(&error),
                        None,
                    )
                    .map_err(|storage_error| storage_error.to_string())?;
                drop(storage);
                append_runtime_event(
                    &state.storage,
                    &assignment.ticket_id,
                    "subagent.failed",
                    json!({ "assignmentId": assignment_id, "error": error }),
                    &app,
                );
                recompute_subagent_root(&state.storage, &assignment)?;
                return Err(error);
            }
        };
        storage
            .set_assignment_artifacts(&assignment_id, None, None, Some(&diff), None)
            .map_err(|error| error.to_string())?;
        storage
            .update_assignment(
                &assignment_id,
                AssignmentState::Completed,
                Some("隔离修改已批准，可审查并合并到主工作区"),
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
        (
            "subagent.change_ready",
            json!({ "assignmentId": assignment_id, "diff": diff, "worktreePath": worktree_path }),
        )
    } else {
        storage
            .update_assignment(
                &assignment_id,
                AssignmentState::Cancelled,
                Some("修改已拒绝，主工作区未改变"),
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
        (
            "subagent.change_rejected",
            json!({ "assignmentId": assignment_id }),
        )
    };
    drop(storage);
    append_runtime_event(
        &state.storage,
        &assignment.ticket_id,
        runtime_event.0,
        runtime_event.1,
        &app,
    );
    if !approved {
        let _ = remove_worktree_for(&project_path, Path::new(&worktree_path), &managed_root);
    }
    recompute_subagent_root(&state.storage, &assignment)?;
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .assignments(&assignment.ticket_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn merge_subagent_change(
    assignment_id: String,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AssignmentSummary>, String> {
    if !approved {
        return Err("合并 Sub-agent 修改需要明确批准".into());
    }
    ensure_capsule_writable(&state)?;
    let (assignment, project_path, managed_root, expected_diff) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let assignment = storage
            .assignment(&assignment_id)
            .map_err(|error| error.to_string())?;
        if assignment.capability != "edit" || assignment.state != AssignmentState::Completed {
            return Err("该子任务还没有可合并的修改".into());
        }
        if assignment.merged_at.is_some() {
            return Err("该 Sub-agent 修改已经合并".into());
        }
        let ticket = storage
            .ticket(&assignment.ticket_id)
            .map_err(|error| error.to_string())?;
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let expected_diff = assignment
            .diff
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Sub-agent Diff 缺失".to_string())?;
        let managed_root = state.portable.worktrees_dir().join(&ticket.project_id);
        (assignment, project.path, managed_root, expected_diff)
    };
    let worktree_path = assignment
        .worktree_path
        .as_deref()
        .ok_or_else(|| "隔离 Worktree 路径缺失".to_string())?;
    ensure_managed_assignment_worktree(&project_path, worktree_path, &managed_root)?;
    ensure_same_worktree_base(&project_path, worktree_path)?;
    let actual_diff = git_diff_for(worktree_path)?;
    if actual_diff != expected_diff {
        return Err("Worktree 在审批后发生变化，请重新审查，已阻止合并".into());
    }
    apply_git_patch(&project_path, &actual_diff)?;
    let merged_at = chrono::Utc::now().to_rfc3339();
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .set_assignment_artifacts(&assignment_id, None, None, None, Some(&merged_at))
        .map_err(|error| error.to_string())?;
    storage
        .update_assignment(
            &assignment_id,
            AssignmentState::Completed,
            Some("修改已合并到主工作区，尚未暂存或提交"),
            None,
            None,
        )
        .map_err(|error| error.to_string())?;
    drop(storage);
    append_runtime_event(
        &state.storage,
        &assignment.ticket_id,
        "subagent.change_merged",
        json!({ "assignmentId": assignment_id, "worktreePath": worktree_path }),
        &app,
    );
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .assignments(&assignment.ticket_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn terminal_command(
    project_id: String,
    command: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<TerminalResult, String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("命令不能为空".into());
    }
    if !approved {
        return Err("终端命令尚未批准".into());
    }
    ensure_capsule_writable(&state)?;
    validate_terminal_command(&command)?;
    let (project, settings) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        (
            storage
                .project(&project_id)
                .map_err(|error| error.to_string())?,
            storage.settings().map_err(|error| error.to_string())?,
        )
    };
    let shell = if settings.terminal_shell.trim().is_empty() {
        "powershell.exe"
    } else {
        settings.terminal_shell.trim()
    };
    let output = if shell.to_ascii_lowercase().contains("powershell")
        || shell.to_ascii_lowercase().contains("pwsh")
    {
        Command::new(shell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &command,
            ])
            .current_dir(&project.path)
            .output()
    } else {
        Command::new(shell)
            .args(["/D", "/S", "/C", &command])
            .current_dir(&project.path)
            .output()
    }
    .map_err(|error| error.to_string())?;
    let result = TerminalResult {
        command: command.clone(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    };
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .audit_terminal(
            &project_id,
            "command",
            &command,
            true,
            Some(result.exit_code),
        )
        .map_err(|error| error.to_string())?;
    Ok(result)
}

#[tauri::command]
pub fn terminal_start(
    project_id: String,
    rows: u16,
    cols: u16,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalStarted, String> {
    let (project, settings) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        (
            storage
                .project(&project_id)
                .map_err(|error| error.to_string())?,
            storage.settings().map_err(|error| error.to_string())?,
        )
    };
    let root = fs::canonicalize(&project.path).map_err(|error| error.to_string())?;
    state
        .terminals
        .start(app, &settings.terminal_shell, &root, rows, cols)
}

#[tauri::command]
pub fn terminal_write(
    project_id: String,
    session_id: String,
    command: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<TerminalCommandAccepted, String> {
    if !approved {
        return Err("终端命令尚未批准".into());
    }
    ensure_capsule_writable(&state)?;
    validate_terminal_command(&command)?;
    let accepted = state.terminals.write_command(&session_id, &command)?;
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .audit_terminal(&project_id, &session_id, &command, true, None)
        .map_err(|error| error.to_string())?;
    Ok(accepted)
}

#[tauri::command]
pub fn terminal_read(
    session_id: String,
    after_sequence: Option<u64>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<TerminalReadResult, String> {
    state.terminals.read(&session_id, after_sequence, limit)
}

#[tauri::command]
pub fn terminal_resize(
    session_id: String,
    rows: u16,
    cols: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.terminals.resize(&session_id, rows, cols)
}

#[tauri::command]
pub fn terminal_interrupt(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<TerminalInterruptResult, String> {
    state.terminals.interrupt(&session_id)
}

#[tauri::command]
pub fn terminal_close(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.terminals.close(&session_id)
}

#[tauri::command]
pub fn cancel_ticket(
    ticket_id: String,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if let Ok(Some(request_id)) = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .external_ref(&ticket_id, "executor_v1")
    {
        let _ = state.executor.cancel(&request_id);
    }
    if let Ok(Some(external_id)) = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .external_ref(&ticket_id, "pocketmind")
    {
        let _ = state.legacy.cancel(&external_id);
    }
    let child_refs = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .active_assignment_external_ids(&ticket_id)
        .map_err(|error| error.to_string())?;
    for (_, external_id) in &child_refs {
        let _ = state.legacy.cancel(external_id);
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .cancel_assignments(&ticket_id)
        .map_err(|error| error.to_string())?;
    if !child_refs.is_empty() {
        storage
            .append_event(
                Some(&ticket_id),
                "subagents.cancelled",
                json!({ "count": child_refs.len() }),
            )
            .map_err(|error| error.to_string())?;
    }
    if storage
        .ticket(&ticket_id)
        .map_err(|error| error.to_string())?
        .state
        != TicketState::Stopped
    {
        storage
            .set_ticket_state(&ticket_id, TicketState::Stopped)
            .map_err(|error| error.to_string())?;
        storage
            .append_event(Some(&ticket_id), "ticket.cancelled", json!({}))
            .map_err(|error| error.to_string())?;
    }
    drop(storage);
    snapshot(&state)
}

fn run_subagent_batch(
    storage: Arc<Mutex<Storage>>,
    legacy: LegacyAgentClient,
    executor: ExecutorClient,
    skills: Arc<SkillRuntime>,
    app: AppHandle,
    ticket_id: String,
    project_path: String,
    worktrees_root: PathBuf,
    root: AssignmentSummary,
    children: Vec<AssignmentSummary>,
) {
    if let Ok(guard) = storage.lock() {
        let _ = guard.update_assignment(&root.id, AssignmentState::Running, None, None, None);
    }
    append_runtime_event(
        &storage,
        &ticket_id,
        "subagents.started",
        json!({ "assignmentId": root.id, "count": children.len() }),
        &app,
    );
    let provider_settings = match storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())
        .and_then(|guard| guard.settings().map_err(|error| error.to_string()))
    {
        Ok(settings) => settings,
        Err(error) => {
            append_runtime_event(
                &storage,
                &ticket_id,
                "subagents.failed",
                json!({ "error": error }),
                &app,
            );
            return;
        }
    };
    if let Err(error) = provider::configure(&provider_settings, &legacy, None) {
        if let Ok(guard) = storage.lock() {
            let _ = guard.update_assignment(
                &root.id,
                AssignmentState::Failed,
                None,
                Some(&error),
                None,
            );
            for child in &children {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Failed,
                    None,
                    Some(&error),
                    None,
                );
            }
        }
        append_runtime_event(
            &storage,
            &ticket_id,
            "subagents.failed",
            json!({ "error": error }),
            &app,
        );
        return;
    }
    if let Err(error) = legacy.select_project(&project_path) {
        let message = error.to_string();
        if let Ok(guard) = storage.lock() {
            let _ = guard.update_assignment(
                &root.id,
                AssignmentState::Failed,
                None,
                Some(&message),
                None,
            );
            for child in &children {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Failed,
                    None,
                    Some(&message),
                    None,
                );
            }
        }
        append_runtime_event(
            &storage,
            &ticket_id,
            "subagents.failed",
            json!({ "error": message }),
            &app,
        );
        return;
    }

    let mut runnable = Vec::new();
    let skill_context = enabled_skill_context(&storage, &skills);
    for child in children {
        if child.capability == "read" {
            let external_id = format!("executor-subagent-{}", Uuid::new_v4());
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Running,
                    None,
                    None,
                    Some(&external_id),
                );
            }
            append_runtime_event(
                &storage,
                &ticket_id,
                "subagent.started",
                json!({ "assignmentId": child.id, "label": child.label, "capability": child.capability }),
                &app,
            );
            runnable.push(SubagentRunnable::ReadOnly(child));
            continue;
        }
        let child_root = if child.capability == "edit" {
            let suffix = child.id.rsplit('-').next().unwrap_or("task");
            let target = worktrees_root.join(format!("agent-{suffix}"));
            let external_target = git_compatible_path(&target);
            let branch = format!("ucode/agent-{suffix}");
            if let Err(error) = fs::create_dir_all(&worktrees_root)
                .map_err(|error| error.to_string())
                .and_then(|_| add_worktree_for(&project_path, &external_target, &branch, "HEAD"))
            {
                if let Ok(guard) = storage.lock() {
                    let _ = guard.update_assignment(
                        &child.id,
                        AssignmentState::Failed,
                        None,
                        Some(&error),
                        None,
                    );
                }
                append_runtime_event(
                    &storage,
                    &ticket_id,
                    "subagent.failed",
                    json!({ "assignmentId": child.id, "label": child.label, "error": error }),
                    &app,
                );
                continue;
            }
            let path = external_target.display().to_string();
            if let Ok(guard) = storage.lock() {
                let _ = guard.set_assignment_artifacts(&child.id, Some(&path), None, None, None);
            }
            path
        } else {
            project_path.clone()
        };
        if let Err(error) = legacy.select_project(&child_root) {
            let message = error.to_string();
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Failed,
                    None,
                    Some(&message),
                    None,
                );
            }
            append_runtime_event(
                &storage,
                &ticket_id,
                "subagent.failed",
                json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                &app,
            );
            continue;
        }
        let prompt = if child.capability == "edit" {
            format!("[隔离编辑子任务：{}]\n你正在独立 Git Worktree 中工作。请完成以下修改：{}\n必须先读取真实文件，只提出一次最小 write_file 修改；不要改动任务之外的文件。写入仍需用户批准。", child.label, child.request)
        } else {
            format!("[只读子任务：{}]\n请先总结本项目，再重点回答以下分析问题：{}\n请给出简洁结论并引用实际文件；信息不足时明确说明。", child.label, child.request)
        };
        match legacy.create_ticket(&prompt) {
            Ok(external_id) => {
                if let Err(error) = legacy.set_skill_context(&external_id, &skill_context) {
                    let message = error.to_string();
                    if let Ok(guard) = storage.lock() {
                        let _ = guard.update_assignment(
                            &child.id,
                            AssignmentState::Failed,
                            None,
                            Some(&message),
                            Some(&external_id),
                        );
                    }
                    append_runtime_event(
                        &storage,
                        &ticket_id,
                        "subagent.failed",
                        json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                        &app,
                    );
                    continue;
                }
                if let Ok(guard) = storage.lock() {
                    let _ = guard.update_assignment(
                        &child.id,
                        AssignmentState::Running,
                        None,
                        None,
                        Some(&external_id),
                    );
                }
                append_runtime_event(
                    &storage,
                    &ticket_id,
                    "subagent.started",
                    json!({ "assignmentId": child.id, "label": child.label, "capability": child.capability, "worktreePath": if child.capability == "edit" { Some(child_root) } else { None } }),
                    &app,
                );
                runnable.push(SubagentRunnable::Legacy(child, external_id));
            }
            Err(error) => {
                let message = error.to_string();
                if let Ok(guard) = storage.lock() {
                    let _ = guard.update_assignment(
                        &child.id,
                        AssignmentState::Failed,
                        None,
                        Some(&message),
                        None,
                    );
                }
                append_runtime_event(
                    &storage,
                    &ticket_id,
                    "subagent.failed",
                    json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                    &app,
                );
            }
        }
    }

    let handles: Vec<_> = runnable
        .into_iter()
        .map(|runtime| {
            let child_storage = Arc::clone(&storage);
            let child_legacy = legacy.clone();
            let child_executor = executor.clone();
            let child_app = app.clone();
            let child_ticket_id = ticket_id.clone();
            let child_project_path = project_path.clone();
            let child_skill_context = skill_context.clone();
            std::thread::spawn(move || match runtime {
                SubagentRunnable::Legacy(child, external_id) => run_subagent_child(
                    child_storage,
                    child_legacy,
                    child_app,
                    child_ticket_id,
                    child,
                    external_id,
                ),
                SubagentRunnable::ReadOnly(child) => run_readonly_subagent_child(
                    child_storage,
                    child_executor,
                    child_app,
                    child_ticket_id,
                    child_project_path,
                    child_skill_context,
                    child,
                ),
            })
        })
        .collect();
    for handle in handles {
        let _ = handle.join();
    }

    let (state, aggregate, error) = match storage.lock() {
        Ok(guard) => {
            let assignments = guard.assignments(&ticket_id).unwrap_or_default();
            let child_assignments: Vec<_> = assignments
                .into_iter()
                .filter(|assignment| assignment.parent_id.as_deref() == Some(root.id.as_str()))
                .collect();
            let aggregate = child_assignments
                .iter()
                .filter_map(|assignment| {
                    assignment
                        .result
                        .as_ref()
                        .map(|result| format!("{}：{}", assignment.label, result))
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if child_assignments
                .iter()
                .any(|assignment| assignment.state == AssignmentState::Waiting)
            {
                (AssignmentState::Waiting, aggregate, None)
            } else if child_assignments
                .iter()
                .any(|assignment| assignment.state == AssignmentState::Cancelled)
            {
                (AssignmentState::Cancelled, aggregate, None)
            } else if child_assignments
                .iter()
                .any(|assignment| assignment.state == AssignmentState::Failed)
            {
                (
                    AssignmentState::Failed,
                    aggregate,
                    Some("部分 Sub-agent 执行失败".to_string()),
                )
            } else {
                (AssignmentState::Completed, aggregate, None)
            }
        }
        Err(_) => (
            AssignmentState::Failed,
            String::new(),
            Some("storage lock was poisoned".into()),
        ),
    };
    if let Ok(guard) = storage.lock() {
        let _ = guard.update_assignment(
            &root.id,
            state,
            (!aggregate.is_empty()).then_some(aggregate.as_str()),
            error.as_deref(),
            None,
        );
    }
    append_runtime_event(
        &storage,
        &ticket_id,
        if state == AssignmentState::Completed {
            "subagents.completed"
        } else if state == AssignmentState::Cancelled {
            "subagents.cancelled"
        } else {
            "subagents.failed"
        },
        json!({ "assignmentId": root.id, "result": aggregate, "error": error }),
        &app,
    );
}

enum SubagentRunnable {
    Legacy(AssignmentSummary, String),
    ReadOnly(AssignmentSummary),
}

fn run_readonly_subagent_child(
    storage: Arc<Mutex<Storage>>,
    executor: ExecutorClient,
    app: AppHandle,
    ticket_id: String,
    project_path: String,
    skill_context: String,
    child: AssignmentSummary,
) {
    let mut entries = Vec::new();
    let file_result = collect_project_files(
        Path::new(&project_path),
        Path::new(&project_path),
        0,
        &mut entries,
    );
    if let Err(error) = file_result {
        complete_readonly_subagent(&storage, &app, &ticket_id, &child, Err(error));
        return;
    }
    let files = entries
        .into_iter()
        .filter(|entry| !entry.is_directory)
        .map(|entry| WorkspaceFile {
            path: entry.path,
            size_bytes: entry.size,
            content_digest: None,
        })
        .collect::<Vec<_>>();
    if let Some(content) = deterministic_file_inventory(&child.request, &files) {
        complete_readonly_subagent(&storage, &app, &ticket_id, &child, Ok(content));
        return;
    }
    let request_id = format!("executor-subagent-{}", Uuid::new_v4());
    let request = ExecutorRequest {
        version: EXECUTOR_PROTOCOL_V1.into(),
        request_id: request_id.clone(),
        turn_id: format!("turn-{}", Uuid::new_v4()),
        thread_key: format!("desktop:subagent:{}", child.id),
        mode: ExecutorMode::ProjectRead,
        assignment: None,
        mandate: None,
        candidate: None,
        inputs: vec![ExecutorInput {
            id: format!("input-{}", Uuid::new_v4()),
            kind: TicketInputKind::Trigger,
            trigger_id: Some(format!("trigger-{}", Uuid::new_v4())),
            text: format!(
                "只读分析任务：{}\n{}\n请基于实际项目文件给出简洁结论，不要提出写入操作。",
                child.label, child.request
            ),
            human: true,
        }],
        history: Vec::new(),
        workspace: ExecutorWorkspace {
            canonical_root: project_path,
            fingerprint: format!("subagent:{}", child.id),
            files,
        },
        attachment_token: None,
        attachments: Vec::new(),
        metadata: json!({ "locale": "zh-CN", "skillContext": skill_context, "assignmentId": child.id }),
    };
    let event_storage = Arc::clone(&storage);
    let event_app = app.clone();
    let event_ticket_id = ticket_id.clone();
    let assignment_id = child.id.clone();
    let terminal = executor.run(&request, move |event| {
        if let ExecutorEventPayload::TextDelta { delta } = &event.payload {
            append_runtime_event(
                &event_storage,
                &event_ticket_id,
                "subagent.delta",
                json!({ "assignmentId": assignment_id, "delta": delta }),
                &event_app,
            );
        }
    });
    let result = match terminal {
        Ok(ExecutorEvent {
            payload: ExecutorEventPayload::Complete { content, .. },
            ..
        }) if !content.trim().is_empty() => Ok(content),
        Ok(ExecutorEvent {
            payload: ExecutorEventPayload::Failure { message, .. },
            ..
        }) => Err(message),
        Ok(ExecutorEvent {
            payload: ExecutorEventPayload::Cancelled,
            ..
        }) => Err("只读 Agent 已取消".into()),
        Ok(_) => Err("只读 Agent 未返回可用结论".into()),
        Err(error) => Err(error.to_string()),
    };
    complete_readonly_subagent(&storage, &app, &ticket_id, &child, result);
}

fn complete_readonly_subagent(
    storage: &Arc<Mutex<Storage>>,
    app: &AppHandle,
    ticket_id: &str,
    child: &AssignmentSummary,
    result: Result<String, String>,
) {
    match result {
        Ok(content) => {
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Completed,
                    Some(content.trim()),
                    None,
                    None,
                );
            }
            append_runtime_event(
                storage,
                ticket_id,
                "subagent.completed",
                json!({ "assignmentId": child.id, "label": child.label, "result": content }),
                app,
            );
        }
        Err(message) => {
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Failed,
                    None,
                    Some(&message),
                    None,
                );
            }
            append_runtime_event(
                storage,
                ticket_id,
                "subagent.failed",
                json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                app,
            );
        }
    }
}

fn deterministic_file_inventory(request: &str, files: &[WorkspaceFile]) -> Option<String> {
    let normalized = request.to_lowercase();
    if ![
        "文件类型",
        "项目结构",
        "目录结构",
        "有哪些文件",
        "file type",
        "project structure",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return None;
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for file in files {
        let extension = Path::new(&file.path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let label = match extension.as_str() {
            "py" => "Python".to_string(),
            "md" | "markdown" => "Markdown".to_string(),
            "html" | "htm" => "HTML".to_string(),
            "css" | "scss" => "CSS".to_string(),
            "js" | "mjs" | "cjs" => "JavaScript".to_string(),
            "ts" | "tsx" => "TypeScript".to_string(),
            "rs" => "Rust".to_string(),
            "go" => "Go".to_string(),
            "json" => "JSON".to_string(),
            "yaml" | "yml" => "YAML".to_string(),
            "toml" => "TOML".to_string(),
            "" => "无扩展名".to_string(),
            value => format!(".{}", value),
        };
        *counts.entry(label).or_default() += 1;
    }
    let summary = counts
        .iter()
        .map(|(label, count)| format!("{count} 个 {label} 文件"))
        .collect::<Vec<_>>()
        .join("、");
    let sources = files
        .iter()
        .take(12)
        .map(|file| format!("- `{}`", file.path))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "项目索引包含 {} 个文件：{}。\n\n来源\n{}",
        files.len(),
        if summary.is_empty() {
            "没有可分类文件"
        } else {
            &summary
        },
        if sources.is_empty() {
            "- 无"
        } else {
            &sources
        },
    ))
}

fn run_subagent_child(
    storage: Arc<Mutex<Storage>>,
    legacy: LegacyAgentClient,
    app: AppHandle,
    ticket_id: String,
    child: AssignmentSummary,
    external_id: String,
) {
    let event_storage = Arc::clone(&storage);
    let event_app = app.clone();
    let event_ticket_id = ticket_id.clone();
    let assignment_id = child.id.clone();
    let plan = legacy.plan_stream(&external_id, move |event| {
        let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else { return; };
        match kind {
            "text_delta" => append_runtime_event(&event_storage, &event_ticket_id, "subagent.delta", json!({ "assignmentId": assignment_id, "delta": event.get("delta").and_then(serde_json::Value::as_str).unwrap_or("") }), &event_app),
            "route" => append_runtime_event(&event_storage, &event_ticket_id, "subagent.route", json!({ "assignmentId": assignment_id, "intent": event.get("intent") }), &event_app),
            _ => {}
        }
    });
    if storage
        .lock()
        .ok()
        .and_then(|guard| guard.assignment(&child.id).ok())
        .is_some_and(|assignment| assignment.state == AssignmentState::Cancelled)
    {
        return;
    }
    match plan {
        Ok(plan) if plan.kind == "cancelled" => {
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Cancelled,
                    None,
                    None,
                    None,
                );
            }
        }
        Ok(plan) if plan.effect.is_some() => {
            let effect = plan.effect.expect("effect checked above");
            if child.capability == "edit" {
                let effect_json = serde_json::to_string(&effect).unwrap_or_default();
                let diff = effect_preview_diff(&effect);
                if let Ok(guard) = storage.lock() {
                    let _ = guard.set_assignment_artifacts(
                        &child.id,
                        None,
                        Some(&effect_json),
                        Some(&diff),
                        None,
                    );
                    let _ = guard.update_assignment(
                        &child.id,
                        AssignmentState::Waiting,
                        Some("修改已生成，等待审批"),
                        None,
                        None,
                    );
                }
                append_runtime_event(
                    &storage,
                    &ticket_id,
                    "subagent.effect_proposed",
                    json!({ "assignmentId": child.id, "label": child.label, "path": effect.path, "diff": diff }),
                    &app,
                );
            } else {
                let _ = legacy.approve(&external_id, &effect.id, false, None);
                let message = "Sub-agent 请求了写操作；只读委派已安全拒绝";
                if let Ok(guard) = storage.lock() {
                    let _ = guard.update_assignment(
                        &child.id,
                        AssignmentState::Failed,
                        None,
                        Some(message),
                        None,
                    );
                }
                append_runtime_event(
                    &storage,
                    &ticket_id,
                    "subagent.failed",
                    json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                    &app,
                );
            }
        }
        Ok(plan) => {
            let content = if plan.content.trim().is_empty() {
                "子任务完成，但没有返回文本结论。"
            } else {
                plan.content.trim()
            };
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Completed,
                    Some(content),
                    None,
                    None,
                );
            }
            append_runtime_event(
                &storage,
                &ticket_id,
                "subagent.completed",
                json!({ "assignmentId": child.id, "label": child.label, "result": content, "evidence": plan.evidence }),
                &app,
            );
        }
        Err(error) => {
            let message = error.to_string();
            if let Ok(guard) = storage.lock() {
                let _ = guard.update_assignment(
                    &child.id,
                    AssignmentState::Failed,
                    None,
                    Some(&message),
                    None,
                );
            }
            append_runtime_event(
                &storage,
                &ticket_id,
                "subagent.failed",
                json!({ "assignmentId": child.id, "label": child.label, "error": message }),
                &app,
            );
        }
    }
}

fn effect_preview_diff(effect: &LegacyEffect) -> String {
    similar::TextDiff::from_lines(effect.before.as_deref().unwrap_or(""), &effect.content)
        .unified_diff()
        .header(&format!("a/{}", effect.path), &format!("b/{}", effect.path))
        .to_string()
}

fn git_diff_for(path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["-C", path, "diff", "--binary", "--no-ext-diff", "--"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_head(path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["-C", path, "rev-parse", "HEAD"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ensure_same_worktree_base(project_path: &str, worktree_path: &str) -> Result<(), String> {
    if git_head(project_path)? != git_head(worktree_path)? {
        return Err(
            "主工作区基准提交在审查期间发生变化，请重新运行编辑 Agent，已阻止陈旧 Diff 合并".into(),
        );
    }
    Ok(())
}

fn ensure_managed_assignment_worktree(
    project_path: &str,
    worktree_path: &str,
    managed_root: &Path,
) -> Result<(), String> {
    let target =
        fs::canonicalize(worktree_path).map_err(|_| "隔离 Worktree 已不存在".to_string())?;
    let root =
        fs::canonicalize(managed_root).map_err(|_| "UCode Worktree 管理目录不存在".to_string())?;
    if !target.starts_with(&root) {
        return Err("Sub-agent Worktree 不在 UCode 管理目录内".into());
    }
    let registered = list_worktrees_for(project_path)?.into_iter().any(|entry| {
        !entry.is_main
            && fs::canonicalize(entry.path)
                .ok()
                .is_some_and(|path| path == target)
    });
    if !registered {
        return Err("隔离 Worktree 未在当前 Git 项目中注册".into());
    }
    Ok(())
}

fn validate_patch_paths(patch: &str) -> Result<(), String> {
    let mut found = false;
    for line in patch
        .lines()
        .filter(|line| line.starts_with("diff --git a/"))
    {
        found = true;
        let Some((left, right)) = line.trim_start_matches("diff --git ").split_once(" b/") else {
            return Err("Diff 文件头无效".into());
        };
        for value in [left.trim_start_matches("a/"), right] {
            let path = Path::new(value);
            if path.is_absolute()
                || path.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err("Diff 包含越界路径，已阻止合并".into());
            }
        }
    }
    if !found {
        return Err("Diff 不包含可合并文件".into());
    }
    Ok(())
}

fn apply_git_patch(project_path: &str, patch: &str) -> Result<(), String> {
    validate_patch_paths(patch)?;
    for check in [true, false] {
        let mut command = Command::new("git");
        command.args(["-C", project_path, "apply"]);
        if check {
            command.arg("--check");
        }
        command
            .args(["--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法打开 Git patch 输入".to_string())?
            .write_all(patch.as_bytes())
            .map_err(|error| error.to_string())?;
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if check {
                format!("主工作区无法无冲突应用该 Diff：{detail}")
            } else {
                format!("合并 Diff 失败：{detail}")
            });
        }
    }
    Ok(())
}

fn recompute_subagent_root(
    storage: &Arc<Mutex<Storage>>,
    child: &AssignmentSummary,
) -> Result<(), String> {
    let Some(root_id) = child.parent_id.as_deref() else {
        return Ok(());
    };
    let guard = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let children: Vec<_> = guard
        .assignments(&child.ticket_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|assignment| assignment.parent_id.as_deref() == Some(root_id))
        .collect();
    let state = if children.iter().any(|assignment| {
        matches!(
            assignment.state,
            AssignmentState::Queued | AssignmentState::Running | AssignmentState::Waiting
        )
    }) {
        AssignmentState::Waiting
    } else if children
        .iter()
        .any(|assignment| assignment.state == AssignmentState::Failed)
    {
        AssignmentState::Failed
    } else if children
        .iter()
        .all(|assignment| assignment.state == AssignmentState::Cancelled)
    {
        AssignmentState::Cancelled
    } else {
        AssignmentState::Completed
    };
    let aggregate = children
        .iter()
        .filter_map(|assignment| {
            assignment
                .result
                .as_ref()
                .map(|result| format!("{}：{}", assignment.label, result))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    guard
        .update_assignment(
            root_id,
            state,
            (!aggregate.is_empty()).then_some(aggregate.as_str()),
            None,
            None,
        )
        .map_err(|error| error.to_string())
}

fn append_runtime_event(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    app: &AppHandle,
) {
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if let Ok(guard) = storage.lock() {
        if let Ok(sequence) =
            guard.append_event_at(Some(ticket_id), event_type, payload.clone(), &created_at)
        {
            let runtime_event = RuntimeEvent {
                sequence,
                ticket_id: Some(ticket_id.to_string()),
                event_type: event_type.to_string(),
                payload,
                created_at,
            };
            drop(guard);
            let _ = app.emit("runtime://event", runtime_event);
        }
    }
}

fn git_details_for(path: &str) -> Result<GitDetails, String> {
    let repository = Command::new("git")
        .args(["-C", path, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|error| error.to_string())?;
    if !repository.status.success() {
        return Ok(GitDetails {
            is_repository: false,
            branch: String::new(),
            clean: false,
            lines: vec![],
            diff: String::new(),
            staged_diff: String::new(),
            history: vec![],
            branches: vec![],
            changes: vec![],
        });
    }
    let output = |args: &[&str]| -> Result<String, String> {
        let value = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        Ok(String::from_utf8_lossy(&value.stdout).to_string())
    };
    let status = Command::new("git")
        .args(["-C", path, "status", "--porcelain=v1", "-z"])
        .output()
        .map_err(|error| error.to_string())?;
    if !status.status.success() {
        return Err(String::from_utf8_lossy(&status.stderr).trim().to_string());
    }
    let changes = parse_porcelain_changes(&status.stdout);
    let branch = output(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .unwrap_or_else(|_| "HEAD".into())
        .trim()
        .to_string();
    let lines = changes
        .iter()
        .map(|change| format!("{} {}", change.status, change.path))
        .collect();
    Ok(GitDetails {
        is_repository: true,
        branch: if branch.is_empty() {
            "HEAD".into()
        } else {
            branch
        },
        clean: changes.is_empty(),
        lines,
        diff: output(&["diff", "--no-ext-diff", "--"])?,
        staged_diff: output(&["diff", "--cached", "--no-ext-diff", "--"])?,
        history: output(&["log", "-n", "20", "--pretty=format:%h %s"])?
            .lines()
            .map(str::to_string)
            .collect(),
        branches: output(&["branch", "--list"])?
            .lines()
            .map(str::to_string)
            .collect(),
        changes,
    })
}

fn ensure_git_repository(path: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(["-C", path, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err("当前项目不是 Git 仓库".into())
    }
}

fn list_worktrees_for(path: &str) -> Result<Vec<WorktreeEntry>, String> {
    ensure_git_repository(path)?;
    let output = Command::new("git")
        .args(["-C", path, "worktree", "list", "--porcelain"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let mut entries = Vec::new();
    for block in text.split("\n\n").filter(|value| !value.trim().is_empty()) {
        let mut worktree_path = String::new();
        let mut branch = String::new();
        let mut head = String::new();
        let mut locked = false;
        let mut prunable = false;
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                worktree_path = value.trim_end_matches('\r').to_string();
            } else if let Some(value) = line.strip_prefix("HEAD ") {
                head = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("branch ") {
                branch = value.trim().trim_start_matches("refs/heads/").to_string();
            } else if line == "detached" {
                branch = "detached HEAD".into();
            } else if line.starts_with("locked") {
                locked = true;
            } else if line.starts_with("prunable") {
                prunable = true;
            }
        }
        if worktree_path.is_empty() {
            continue;
        }
        let dirty = if Path::new(&worktree_path).is_dir() {
            let status = Command::new("git")
                .args(["-C", &worktree_path, "status", "--porcelain"])
                .output()
                .map_err(|error| error.to_string())?;
            status.status.success() && !status.stdout.is_empty()
        } else {
            false
        };
        entries.push(WorktreeEntry {
            path: worktree_path,
            branch: if branch.is_empty() {
                "HEAD".into()
            } else {
                branch
            },
            head,
            is_main: entries.is_empty(),
            locked,
            prunable,
            dirty,
        });
    }
    Ok(entries)
}

fn add_worktree_for(
    project_path: &str,
    target: &Path,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    let project_path = git_compatible_path(Path::new(project_path));
    let target = git_compatible_path(target);
    let base_check = Command::new("git")
        .arg("-C")
        .arg(&project_path)
        .args(["rev-parse", "--verify", &format!("{base}^{{commit}}")])
        .output()
        .map_err(|error| error.to_string())?;
    if !base_check.status.success() {
        return Err("Worktree 基准提交不存在".into());
    }
    let branch_ref = format!("refs/heads/{branch}");
    let existing_branch = Command::new("git")
        .arg("-C")
        .arg(&project_path)
        .args(["show-ref", "--verify", "--quiet", &branch_ref])
        .status()
        .map_err(|error| error.to_string())?
        .success();
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&project_path)
        .args(["worktree", "add"]);
    if existing_branch {
        command.arg(target).arg(branch);
    } else {
        command.args(["-b", branch]).arg(target).arg(base);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Git for Windows treats an extended-length path as a malformed `//?/`
/// network path. Keep canonical paths for policy checks, but convert only at
/// the process boundary used by Git and by the child Agent.
fn git_compatible_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

fn remove_worktree_for(
    project_path: &str,
    requested: &Path,
    managed_root: &Path,
) -> Result<(), String> {
    let worktrees = list_worktrees_for(project_path)?;
    let target = fs::canonicalize(requested).map_err(|_| "Worktree 路径不存在".to_string())?;
    let root = fs::canonicalize(managed_root).map_err(|error| error.to_string())?;
    if !target.starts_with(&root) {
        return Err("只能移除 UCode 便携目录内的 Worktree".into());
    }
    let entry = worktrees
        .iter()
        .find(|entry| fs::canonicalize(&entry.path).ok().as_ref() == Some(&target))
        .ok_or_else(|| "该目录不是当前项目的 Worktree".to_string())?;
    if entry.is_main {
        return Err("不能移除主工作区".into());
    }
    if entry.locked {
        return Err("Worktree 已锁定，无法移除".into());
    }
    if entry.dirty {
        return Err("Worktree 含有未提交改动，请先提交或清理后再移除".into());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["worktree", "remove"])
        .arg(&target)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn parse_porcelain_changes(bytes: &[u8]) -> Vec<GitChange> {
    let values: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let value = String::from_utf8_lossy(values[index]);
        if value.len() < 4 {
            index += 1;
            continue;
        }
        let status = value[..2].to_string();
        let path = value[3..].to_string();
        let renamed = matches!(status.as_bytes().first(), Some(b'R') | Some(b'C'));
        let original_path = if renamed && index + 1 < values.len() {
            index += 1;
            Some(String::from_utf8_lossy(values[index]).to_string())
        } else {
            None
        };
        let staged = status.as_bytes()[0] != b' ' && status.as_bytes()[0] != b'?';
        let unstaged = status.as_bytes()[1] != b' ' || status == "??";
        changes.push(GitChange {
            status,
            path,
            original_path,
            staged,
            unstaged,
        });
        index += 1;
    }
    changes
}

fn validate_relative(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Git 路径必须位于项目内".into());
    }
    Ok(())
}

fn validate_branch(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 120
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-_/ .".contains(ch))
        || value.contains("..")
        || value.contains(' ')
    {
        return Err("分支名称无效".into());
    }
    Ok(())
}

fn validate_worktree_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("Worktree 名称只能包含字母、数字、点、短横线和下划线".into());
    }
    Ok(())
}

fn validate_revision(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || value.starts_with('-')
        || value.contains("..")
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.'))
    {
        return Err("Worktree 基准引用无效".into());
    }
    Ok(())
}

fn validate_subagent_request(value: &SubagentRequest) -> Result<(), String> {
    let label = value.label.trim();
    let request = value.request.trim();
    if label.is_empty() || label.chars().count() > 80 {
        return Err("Sub-agent 名称必须为 1 到 80 个字符".into());
    }
    if request.is_empty() || request.chars().count() > 4_000 {
        return Err("Sub-agent 任务必须为 1 到 4000 个字符".into());
    }
    if !["read", "edit"].contains(&value.capability.as_str()) {
        return Err("Sub-agent 权限必须为 read 或 edit".into());
    }
    Ok(())
}

fn collect_project_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    entries: &mut Vec<ProjectFileEntry>,
) -> Result<(), String> {
    if depth > 6 || entries.len() >= 600 {
        return Ok(());
    }
    let mut children: Vec<_> = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect();
    children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for child in children {
        if entries.len() >= 600 {
            break;
        }
        let name = child.file_name().to_string_lossy().to_string();
        if [
            ".git",
            ".idea",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
            ".vscode",
            "node_modules",
            "target",
            "dist",
            ".venv",
            "__pycache__",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        let file_type = child.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        let relative = child
            .path()
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let size = if file_type.is_file() {
            child.metadata().map(|value| value.len()).unwrap_or(0)
        } else {
            0
        };
        entries.push(ProjectFileEntry {
            path: relative,
            name: name.clone(),
            is_directory: file_type.is_dir(),
            depth,
            size,
        });
        if file_type.is_dir() {
            collect_project_files(root, &child.path(), depth + 1, entries)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn list_events(after: i64, state: State<'_, AppState>) -> Result<Vec<RuntimeEvent>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .events(after)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn run_ticket(
    ticket_id: String,
    selection_id: Option<String>,
    client_request_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if client_request_id.trim().is_empty() {
        return Err("client request id is required".into());
    }
    {
        let guard = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let request = guard
            .ticket(&ticket_id)
            .map_err(|error| error.to_string())?
            .request;
        if !guard
            .claim_desktop_request(&client_request_id, &ticket_id, &request)
            .map_err(|error| error.to_string())?
        {
            drop(guard);
            return snapshot(&state);
        }
    }
    let storage = Arc::clone(&state.storage);
    let skills = Arc::clone(&state.skills);
    let legacy = state.legacy.clone();
    let executor = state.executor.clone();
    let rescue_repairs = Arc::clone(&state.rescue_repairs);
    let rescue_probes = Arc::clone(&state.rescue_probes);
    let attachments = Arc::clone(&state.attachments);
    let task_ticket_id = ticket_id.clone();
    let task_selection_id = selection_id.clone();
    let task_app = app.clone();
    let route = {
        let mut guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = guard
            .ticket(&task_ticket_id)
            .map_err(|error| error.to_string())?;
        let route = classify_desktop_route(&ticket.request, None);
        guard
            .set_task_context_scope(&task_ticket_id, route.task_context_scope())
            .map_err(|error| error.to_string())?;
        route
    };
    let request = {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .ticket(&task_ticket_id)
            .map_err(|error| error.to_string())?
            .request
    };
    if let Some(content) = low_information_clarification(&request) {
        record_deterministic_clarification(
            &storage,
            &task_ticket_id,
            &request,
            &content,
            false,
            &app,
        )?;
        return snapshot(&state);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        if route == DesktopRoute::Action {
            run_legacy_ticket(
                &storage,
                &skills,
                &attachments,
                &legacy,
                &task_ticket_id,
                task_selection_id.as_deref(),
                &task_app,
            )
        } else {
            run_executor_ticket(
                &storage,
                &skills,
                &attachments,
                &legacy,
                &executor,
                &rescue_repairs,
                &rescue_probes,
                &task_ticket_id,
                &request,
                &client_request_id,
                task_selection_id.as_deref(),
                route,
                false,
                &task_app,
            )
        }
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = result {
        record_failure(&state.storage, &ticket_id, &error);
        return Err(error);
    }
    state
        .attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .complete(selection_id.as_deref());
    snapshot(&state)
}

#[tauri::command]
pub async fn continue_ticket(
    ticket_id: String,
    request: String,
    selection_id: Option<String>,
    client_request_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    let request = request.trim().to_string();
    if request.is_empty() {
        return Err("message is required".into());
    }
    if client_request_id.trim().is_empty() {
        return Err("client request id is required".into());
    }
    {
        let mut guard = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        if !guard
            .claim_desktop_request(&client_request_id, &ticket_id, &request)
            .map_err(|error| error.to_string())?
        {
            drop(guard);
            return snapshot(&state);
        }
        guard
            .update_task_context(&ticket_id, &request)
            .map_err(|error| error.to_string())?;
    }
    let storage = Arc::clone(&state.storage);
    let route = {
        let mut guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let previous = guard
            .last_delivery_route(&ticket_id)
            .map_err(|error| error.to_string())?;
        let route = classify_desktop_route(&request, previous.as_deref());
        guard
            .set_task_context_scope(&ticket_id, route.task_context_scope())
            .map_err(|error| error.to_string())?;
        route
    };
    if let Some(content) = low_information_clarification(&request) {
        record_deterministic_clarification(&storage, &ticket_id, &request, &content, true, &app)?;
        state
            .attachments
            .lock()
            .map_err(|_| "attachment store lock was poisoned".to_string())?
            .complete(selection_id.as_deref());
        return snapshot(&state);
    }
    if route != DesktopRoute::Action {
        let skills = Arc::clone(&state.skills);
        let legacy = state.legacy.clone();
        let executor = state.executor.clone();
        let rescue_repairs = Arc::clone(&state.rescue_repairs);
        let rescue_probes = Arc::clone(&state.rescue_probes);
        let attachments = Arc::clone(&state.attachments);
        let task_ticket_id = ticket_id.clone();
        let task_request = request.clone();
        let task_client_request_id = client_request_id.clone();
        let task_selection_id = selection_id.clone();
        let task_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            run_executor_ticket(
                &storage,
                &skills,
                &attachments,
                &legacy,
                &executor,
                &rescue_repairs,
                &rescue_probes,
                &task_ticket_id,
                &task_request,
                &task_client_request_id,
                task_selection_id.as_deref(),
                route,
                true,
                &task_app,
            )
        })
        .await
        .map_err(|error| error.to_string())?;
        if let Err(error) = result {
            record_failure(&state.storage, &ticket_id, &error);
            return Err(error);
        }
        state
            .attachments
            .lock()
            .map_err(|_| "attachment store lock was poisoned".to_string())?
            .complete(selection_id.as_deref());
        return snapshot(&state);
    }
    let skills = Arc::clone(&state.skills);
    let legacy = state.legacy.clone();
    let attachments = Arc::clone(&state.attachments);
    let task_ticket_id = ticket_id.clone();
    let task_selection_id = selection_id.clone();
    let task_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (ticket_project_id, attachment_payloads) = {
            let guard = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            let ticket = guard
                .ticket(&task_ticket_id)
                .map_err(|error| error.to_string())?;
            let project_id = ticket.project_id.clone();
            drop(guard);
            let payloads = attachments
                .lock()
                .map_err(|_| "attachment store lock was poisoned".to_string())?
                .resolve(task_selection_id.as_deref(), &project_id)?;
            (project_id, payloads)
        };
        let (external_id, stream_enabled, provider_mode) = {
            let guard = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            let existing = guard
                .external_ref(&task_ticket_id, "pocketmind")
                .map_err(|error| error.to_string())?;
            let ticket = guard
                .ticket(&task_ticket_id)
                .map_err(|error| error.to_string())?;
            let project = guard
                .project(&ticket.project_id)
                .map_err(|error| error.to_string())?;
            let settings = guard.settings().map_err(|error| error.to_string())?;
            let history = if existing.is_none() {
                guard
                    .conversation_messages(&task_ticket_id)
                    .map_err(|error| error.to_string())?
            } else {
                Vec::new()
            };
            drop(guard);
            provider::configure(&settings, &legacy, None)?;
            let external_id = if let Some(external_id) = existing {
                external_id
            } else {
                legacy
                    .select_project(&project.path)
                    .map_err(|error| error.to_string())?;
                let external_id = legacy
                    .create_ticket(&ticket.request)
                    .map_err(|error| error.to_string())?;
                let messages: Vec<LegacyMessage> = history
                    .into_iter()
                    .map(|(role, content)| LegacyMessage { role, content })
                    .collect();
                legacy
                    .hydrate_ticket(&external_id, &messages)
                    .map_err(|error| error.to_string())?;
                storage
                    .lock()
                    .map_err(|_| "storage lock was poisoned".to_string())?
                    .set_external_ref(&task_ticket_id, "pocketmind", &external_id)
                    .map_err(|error| error.to_string())?;
                external_id
            };
            (
                external_id,
                settings.stream_enabled,
                settings.model_provider,
            )
        };
        let skill_context = legacy_execution_context(&storage, &skills, &task_ticket_id);
        legacy
            .set_skill_context(&external_id, &skill_context)
            .map_err(|error| error.to_string())?;
        let legacy_attachments = legacy_attachments(&attachment_payloads);
        legacy
            .set_attachment_context(&external_id, &legacy_attachments)
            .map_err(|error| error.to_string())?;
        legacy
            .continue_ticket(&external_id, &request)
            .map_err(|error| error.to_string())?;
        {
            let guard = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            guard
                .set_ticket_state(&task_ticket_id, TicketState::Running)
                .map_err(|error| error.to_string())?;
            guard
                .append_event(
                    Some(&task_ticket_id),
                    "user.message",
                    json!({ "content": request, "clientRequestId": client_request_id }),
                )
                .map_err(|error| error.to_string())?;
            if !attachment_payloads.is_empty() {
                guard
                    .append_event(
                        Some(&task_ticket_id),
                        "context.attached",
                        attachment_event_payload(&attachment_payloads, &ticket_project_id),
                    )
                    .map_err(|error| error.to_string())?;
            }
            guard
                .append_event(
                    Some(&task_ticket_id),
                    "assignment.started",
                    json!({ "adapter": "pocketmind", "followUp": true }),
                )
                .map_err(|error| error.to_string())?;
            guard
                .append_event(
                    Some(&task_ticket_id),
                    "provider.started",
                    json!({ "provider": provider_mode }),
                )
                .map_err(|error| error.to_string())?;
        }
        let plan = if stream_enabled {
            let event_storage = Arc::clone(&storage);
            let event_ticket_id = task_ticket_id.clone();
            let event_app = task_app.clone();
            legacy
                .plan_stream(&external_id, move |event| {
                    persist_stream_event(&event_storage, &event_ticket_id, event, &event_app);
                })
                .map_err(|error| error.to_string())?
        } else {
            legacy
                .plan(&external_id)
                .map_err(|error| error.to_string())?
        };
        persist_plan(&storage, &task_ticket_id, plan)
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = result {
        record_failure(&state.storage, &ticket_id, &error);
        return Err(error);
    }
    state
        .attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .complete(selection_id.as_deref());
    snapshot(&state)
}

fn path_has_reparse_point(path: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0);
    }
    #[cfg(not(windows))]
    Ok(false)
}

fn safe_race_target(project_path: &Path, relative: &str) -> Result<PathBuf, String> {
    normalize_race_paths(vec![relative.to_string()])?;
    let root = fs::canonicalize(project_path).map_err(|error| error.to_string())?;
    let mut current = root.clone();
    for component in Path::new(relative).components() {
        let Component::Normal(segment) = component else {
            return Err(format!("候选路径无效：{relative}"));
        };
        current.push(segment);
        if !current.exists() {
            continue;
        }
        if path_has_reparse_point(&current)? {
            return Err(format!("候选路径包含链接或重解析点：{relative}"));
        }
        let resolved = fs::canonicalize(&current).map_err(|error| error.to_string())?;
        if !resolved.starts_with(&root) {
            return Err(format!("候选路径越过项目边界：{relative}"));
        }
    }
    Ok(root.join(relative))
}

fn current_file_digest(path: &Path) -> Result<Option<String>, String> {
    match fs::read(path) {
        Ok(content) => Ok(Some(digest_content(&content))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn apply_race_changes(
    project_path: &Path,
    changes: &[RaceEffectChange],
    forward: bool,
) -> Result<Vec<String>, String> {
    struct Entry {
        target: PathBuf,
        staged: Option<PathBuf>,
        backup: Option<PathBuf>,
        original_existed: bool,
    }

    let transaction_id = Uuid::new_v4().to_string();
    let mut entries = Vec::with_capacity(changes.len());
    for (index, change) in changes.iter().enumerate() {
        let target = safe_race_target(project_path, &change.path)?;
        let expected = if forward {
            change.before_digest.as_deref()
        } else {
            change.after_digest.as_deref()
        };
        let current = current_file_digest(&target)?;
        if current.as_deref() != expected {
            return Err(format!(
                "文件 {} 在审查后发生变化，已拒绝整批操作",
                change.path
            ));
        }
        let desired = if forward {
            change
                .after_digest
                .as_ref()
                .map(|_| change.after.as_bytes())
        } else {
            change
                .before_digest
                .as_ref()
                .map(|_| change.before.as_bytes())
        };
        let parent = target
            .parent()
            .ok_or_else(|| format!("无法解析目标目录：{}", change.path))?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let staged = if let Some(content) = desired {
            let path = parent.join(format!(".ucode-{transaction_id}-{index}.stage"));
            fs::write(&path, content).map_err(|error| error.to_string())?;
            Some(path)
        } else {
            None
        };
        entries.push(Entry {
            original_existed: current.is_some(),
            target,
            staged,
            backup: None,
        });
    }

    let apply_result = (|| -> Result<(), String> {
        for (index, entry) in entries.iter_mut().enumerate() {
            if entry.target.exists() {
                let parent = entry
                    .target
                    .parent()
                    .ok_or_else(|| "目标目录无效".to_string())?;
                let backup = parent.join(format!(".ucode-{transaction_id}-{index}.backup"));
                fs::rename(&entry.target, &backup).map_err(|error| error.to_string())?;
                entry.backup = Some(backup);
            }
            if let Some(staged) = entry.staged.as_ref() {
                fs::rename(staged, &entry.target).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })();

    if let Err(error) = apply_result {
        let mut recovery_errors = Vec::new();
        for entry in entries.iter().rev() {
            if entry.backup.as_ref().is_some_and(|path| path.exists()) {
                if entry.target.exists() {
                    if let Err(restore_error) = fs::remove_file(&entry.target) {
                        recovery_errors.push(restore_error.to_string());
                    }
                }
                if let Some(backup) = entry.backup.as_ref() {
                    if let Err(restore_error) = fs::rename(backup, &entry.target) {
                        recovery_errors.push(restore_error.to_string());
                    }
                }
            } else if !entry.original_existed && entry.target.exists() {
                if let Err(restore_error) = fs::remove_file(&entry.target) {
                    recovery_errors.push(restore_error.to_string());
                }
            }
            if let Some(staged) = entry.staged.as_ref() {
                let _ = fs::remove_file(staged);
            }
        }
        return if recovery_errors.is_empty() {
            Err(format!("批量写入失败，已恢复原文件：{error}"))
        } else {
            Err(format!(
                "batch_recovery_failed: {error}; {}",
                recovery_errors.join("; ")
            ))
        };
    }

    for entry in &entries {
        if let Some(backup) = entry.backup.as_ref() {
            fs::remove_file(backup).map_err(|error| error.to_string())?;
        }
    }
    Ok(changes.iter().map(|change| change.path.clone()).collect())
}

fn race_effect_changes_from_event(
    storage: &Storage,
    ticket_id: &str,
    effect_id: &str,
) -> Result<Vec<RaceEffectChange>, String> {
    let payload = storage
        .effect_proposal(ticket_id, effect_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "候选 Effect 已不存在".to_string())?;
    serde_json::from_value(
        payload
            .get("changes")
            .cloned()
            .ok_or_else(|| "候选 Effect 缺少文件变化".to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn resolve_race_effect(
    ticket_id: &str,
    effect_id: &str,
    approved: bool,
    selected_paths: Option<&[String]>,
    state: &AppState,
) -> Result<BootstrapSnapshot, String> {
    let race_id = effect_id
        .strip_prefix("race-effect-")
        .ok_or_else(|| "候选 Effect 标识无效".to_string())?;
    let (mut race, project, changes) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let race = storage.race(race_id).map_err(|error| error.to_string())?;
        if race.ticket_id != ticket_id {
            return Err("候选 Effect 与当前任务不匹配".into());
        }
        let ticket = storage
            .ticket(ticket_id)
            .map_err(|error| error.to_string())?;
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let changes = race_effect_changes_from_event(&storage, ticket_id, effect_id)?;
        (race, project, changes)
    };
    if race.state != RaceState::AwaitingReview {
        return Err("该候选竞赛已不在等待审查状态".into());
    }
    let expected_paths = changes
        .iter()
        .map(|change| change.path.as_str())
        .collect::<BTreeSet<_>>();
    if approved {
        let selected = selected_paths
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if selected != expected_paths {
            return Err("候选方案经过整体验证，只能整批批准，不能拆分文件".into());
        }
        let current_digest =
            digest_tree(Path::new(&project.path)).map_err(|error| error.to_string())?;
        if current_digest != race.project_fingerprint {
            race.transition(RaceState::Stale, Utc::now().to_rfc3339())
                .map_err(|error| error.to_string())?;
            let storage = state
                .storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            storage
                .save_race(&race)
                .map_err(|error| error.to_string())?;
            storage
                .append_event(
                    Some(ticket_id),
                    "effect.apply_refused",
                    json!({ "effectId": effect_id, "message": "workspace_changed" }),
                )
                .map_err(|error| error.to_string())?;
            drop(storage);
            cleanup_race_workspaces(
                &state.portable.worktrees_dir().join("races"),
                race_id,
                &state.storage,
            );
            return Err("项目在候选验证后发生了变化，旧方案已失效；没有写入任何文件。".into());
        }
        let paths = apply_race_changes(Path::new(&project.path), &changes, true)?;
        race.transition(RaceState::Applied, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .save_race(&race)
            .map_err(|error| error.to_string())?;
        storage
            .set_ticket_state(ticket_id, TicketState::Completed)
            .map_err(|error| error.to_string())?;
        storage
            .append_event(
                Some(ticket_id),
                "effect.approved",
                json!({ "effectId": effect_id, "selectedPaths": paths }),
            )
            .map_err(|error| error.to_string())?;
        storage
            .append_event(Some(ticket_id), "effect.applied", json!({ "effectId": effect_id, "result": { "paths": paths, "source": "candidate_race" } }))
            .map_err(|error| error.to_string())?;
        storage
            .append_event(Some(ticket_id), "delivery.ready", json!({ "content": "推荐方案已整批应用。候选通过了本地机械验证，文件摘要在写入前再次核对。" }))
            .map_err(|error| error.to_string())?;
    } else {
        race.transition(RaceState::Rejected, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        storage
            .save_race(&race)
            .map_err(|error| error.to_string())?;
        storage
            .set_ticket_state(ticket_id, TicketState::Completed)
            .map_err(|error| error.to_string())?;
        storage
            .append_event(
                Some(ticket_id),
                "effect.rejected",
                json!({ "effectId": effect_id }),
            )
            .map_err(|error| error.to_string())?;
        storage
            .append_event(
                Some(ticket_id),
                "delivery.ready",
                json!({ "content": "已拒绝候选方案，原项目没有变化。" }),
            )
            .map_err(|error| error.to_string())?;
    }
    cleanup_race_workspaces(
        &state.portable.worktrees_dir().join("races"),
        race_id,
        &state.storage,
    );
    snapshot(state)
}

#[tauri::command]
pub async fn resolve_effect(
    ticket_id: String,
    effect_id: String,
    approved: bool,
    selected_paths: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if effect_id.starts_with("race-effect-") {
        if approved {
            ensure_capsule_writable(&state)?;
        }
        return resolve_race_effect(
            &ticket_id,
            &effect_id,
            approved,
            selected_paths.as_deref(),
            &state,
        );
    }
    if approved {
        ensure_capsule_writable(&state)?;
    }
    let storage = Arc::clone(&state.storage);
    let legacy = state.legacy.clone();
    let task_ticket_id = ticket_id.clone();
    let task_effect_id = effect_id.clone();
    let task_selected_paths = selected_paths.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let settings = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .settings()
            .map_err(|error| error.to_string())?;
        provider::configure(&settings, &legacy, None)?;
        let external_id = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .external_ref(&task_ticket_id, "pocketmind")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "compatibility Ticket reference is missing".to_string())?;
        let effect_result = legacy
            .approve(
                &external_id,
                &task_effect_id,
                approved,
                task_selected_paths.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        {
            let guard = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            guard
                .append_event(
                    Some(&task_ticket_id),
                    if approved {
                        "effect.approved"
                    } else {
                        "effect.rejected"
                    },
                    json!({ "effectId": task_effect_id, "selectedPaths": task_selected_paths }),
                )
                .map_err(|error| error.to_string())?;
            if approved {
                guard
                    .append_event(
                        Some(&task_ticket_id),
                        "effect.applied",
                        json!({ "effectId": task_effect_id, "result": effect_result }),
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        let plan = legacy
            .plan(&external_id)
            .map_err(|error| error.to_string())?;
        persist_plan(&storage, &task_ticket_id, plan)?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = result {
        let recovery_required = is_effect_recovery_failure(&error);
        if let Ok(guard) = state.storage.lock() {
            let _ = guard.set_ticket_state(
                &ticket_id,
                if recovery_required {
                    TicketState::Failed
                } else {
                    TicketState::Waiting
                },
            );
            let _ = guard.append_event(
                Some(&ticket_id),
                if recovery_required {
                    "effect.recovery_required"
                } else {
                    "effect.apply_refused"
                },
                json!({ "effectId": effect_id, "message": error }),
            );
        }
        if recovery_required {
            return Err("批量写入失败，且 UCode 无法验证所有文件已自动恢复。任务已停止；请在 Diff 和项目目录中检查保留的 .bak 文件，不要直接重试审批。".into());
        }
        if error.contains("File changed after approval") || error.contains("workspace_changed") {
            return Err("审批后至少一个文件发生了变化，UCode 已拒绝整个批次，未写入任何文件。请重新生成并审查 Diff。".into());
        }
        return Err(error);
    }
    snapshot(&state)
}

fn is_effect_recovery_failure(message: &str) -> bool {
    message.contains("automatic recovery could not be verified")
        || message.contains("batch_recovery_failed")
}

fn rollback_race_effect(
    ticket_id: &str,
    effect_id: &str,
    state: &AppState,
) -> Result<BootstrapSnapshot, String> {
    let race_id = effect_id
        .strip_prefix("race-effect-")
        .ok_or_else(|| "候选 Effect 标识无效".to_string())?;
    let (mut race, project, changes) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let race = storage.race(race_id).map_err(|error| error.to_string())?;
        if race.ticket_id != ticket_id || race.state != RaceState::Applied {
            return Err("只有已应用且尚未回滚的候选方案可以回滚".into());
        }
        let ticket = storage
            .ticket(ticket_id)
            .map_err(|error| error.to_string())?;
        let project = storage
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let changes = race_effect_changes_from_event(&storage, ticket_id, effect_id)?;
        (race, project, changes)
    };
    let paths = apply_race_changes(Path::new(&project.path), &changes, false)?;
    race.transition(RaceState::RolledBack, Utc::now().to_rfc3339())
        .map_err(|error| error.to_string())?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    storage
        .save_race(&race)
        .map_err(|error| error.to_string())?;
    storage
        .set_ticket_state(ticket_id, TicketState::RolledBack)
        .map_err(|error| error.to_string())?;
    storage
        .append_event(
            Some(ticket_id),
            "effect.rolled_back",
            json!({ "effectId": effect_id, "result": { "paths": paths, "source": "candidate_race" } }),
        )
        .map_err(|error| error.to_string())?;
    drop(storage);
    snapshot(state)
}

#[tauri::command]
pub async fn rollback_effect(
    ticket_id: String,
    effect_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if !approved {
        return Err("回滚必须经过明确确认".into());
    }
    ensure_capsule_writable(&state)?;
    if effect_id.starts_with("race-effect-") {
        return rollback_race_effect(&ticket_id, &effect_id, &state);
    }
    let storage = Arc::clone(&state.storage);
    let legacy = state.legacy.clone();
    let task_ticket_id = ticket_id.clone();
    let task_effect_id = effect_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let external_id = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .external_ref(&task_ticket_id, "pocketmind")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "compatibility Ticket reference is missing".to_string())?;
        {
            let guard = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?;
            guard
                .append_event(
                    Some(&task_ticket_id),
                    "rollback.approved",
                    json!({ "effectId": task_effect_id }),
                )
                .map_err(|error| error.to_string())?;
        }
        let rollback = legacy
            .rollback(&external_id, &task_effect_id)
            .map_err(|error| error.to_string())?;
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_ticket_state(&task_ticket_id, TicketState::RolledBack)
            .map_err(|error| error.to_string())?;
        guard
            .append_event(
                Some(&task_ticket_id),
                "effect.rolled_back",
                json!({ "effectId": task_effect_id, "result": rollback }),
            )
            .map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = result {
        if let Ok(guard) = state.storage.lock() {
            let _ = guard.append_event(
                Some(&ticket_id),
                "rollback.refused",
                json!({ "effectId": effect_id, "message": error }),
            );
        }
        if error.contains("Refusing to overwrite") || error.contains("workspace_changed") {
            return Err("批次中至少一个文件在应用后又发生了变化，UCode 已拒绝整批回滚，未覆盖当前内容。请先审查当前 Diff，再决定如何恢复。".into());
        }
        return Err(error);
    }
    snapshot(&state)
}

#[tauri::command]
pub fn resolve_rescue_repair(
    ticket_id: String,
    proposal_id: String,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if approved {
        ensure_capsule_writable(&state)?;
    }
    let record = if approved {
        state.rescue_repairs.apply(&proposal_id, &ticket_id)?
    } else {
        state.rescue_repairs.reject(&proposal_id, &ticket_id)?
    };
    record_rescue_repair_event(&state.storage, &ticket_id, &record, approved, false, &app)?;
    snapshot(&state)
}

#[tauri::command]
pub fn rollback_rescue_repair(
    ticket_id: String,
    proposal_id: String,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if !approved {
        return Err("Rescue 回滚必须经过明确确认".into());
    }
    ensure_capsule_writable(&state)?;
    let record = state.rescue_repairs.rollback(&proposal_id, &ticket_id)?;
    record_rescue_repair_event(&state.storage, &ticket_id, &record, true, true, &app)?;
    snapshot(&state)
}

#[tauri::command]
pub fn resolve_rescue_probe(
    ticket_id: String,
    proposal_id: String,
    approved: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
    if approved {
        ensure_capsule_writable(&state)?;
    }
    let record = if approved {
        state.rescue_probes.execute(&proposal_id, &ticket_id)?
    } else {
        state.rescue_probes.reject(&proposal_id, &ticket_id)?
    };
    record_rescue_probe_event(&state.storage, &ticket_id, &record, approved, &app)?;
    snapshot(&state)
}

fn record_rescue_probe_event(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    record: &R1ProbeRecord,
    approved: bool,
    app: &AppHandle,
) -> Result<(), String> {
    let event_type = if !approved {
        "rescue.network_probe.rejected"
    } else {
        match record.proposal.status {
            ProbeStatus::Observed => "rescue.network_probe.observed",
            ProbeStatus::Unavailable => "rescue.network_probe.unavailable",
            _ => "rescue.network_probe.safe_stop",
        }
    };
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_ticket_state(ticket_id, TicketState::Waiting)
            .map_err(|error| error.to_string())?;
    }
    if let Some(evidence) = &record.evidence {
        append_runtime_event(
            storage,
            ticket_id,
            "rescue.probe.completed",
            json!({
                "evidenceId": evidence.id,
                "toolId": evidence.tool_id,
                "state": evidence.state,
                "summary": evidence.summary,
                "contentDigest": evidence.content_digest,
                "elapsedMs": evidence.elapsed_ms,
                "risk": "r1",
                "approved": true,
                "target": record.proposal.target,
            }),
            app,
        );
    }
    append_runtime_event(
        storage,
        ticket_id,
        event_type,
        json!({ "proposalId": record.proposal.id, "toolId": record.proposal.tool_id, "status": record.proposal.status, "detail": record.detail, "record": record }),
        app,
    );
    if approved && record.evidence.is_some() {
        let content = format!(
            "## 本次联网探测结果\n\n- **目标：** `{}`\n- **探针：** `{}`\n- **结果：** {}\n- **证据：** `{}`\n\n本次只执行了你批准的单个 DNS 或 TCP 探针，没有发送 HTTP 内容，也没有修改 DNS、代理、路由、防火墙或服务。",
            record.proposal.target,
            record.proposal.tool_id,
            record.detail,
            record
                .evidence
                .as_ref()
                .map(|value| value.id.as_str())
                .unwrap_or("evidence-unavailable")
        );
        append_runtime_event(
            storage,
            ticket_id,
            "provider.message",
            json!({ "content": content, "route": "rescue", "hostGrounded": true }),
            app,
        );
    }
    Ok(())
}

fn record_rescue_repair_event(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    record: &RepairRecord,
    approved: bool,
    rollback: bool,
    app: &AppHandle,
) -> Result<(), String> {
    let event_type = if rollback {
        match record.proposal.status {
            RepairStatus::RolledBack => "rescue.repair.rolled_back",
            RepairStatus::RollbackFailed => "rescue.repair.rollback_failed",
            _ => "rescue.repair.safe_stop",
        }
    } else if !approved {
        "rescue.repair.rejected"
    } else {
        match record.proposal.status {
            RepairStatus::Verified => "rescue.repair.verified",
            RepairStatus::RepairedUnverified => "rescue.repair.unverified",
            _ => "rescue.repair.safe_stop",
        }
    };
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_ticket_state(ticket_id, TicketState::Waiting)
            .map_err(|error| error.to_string())?;
    }
    append_runtime_event(
        storage,
        ticket_id,
        event_type,
        json!({ "proposalId": record.proposal.id, "operationId": record.proposal.operation_id, "status": record.proposal.status, "detail": record.detail, "record": record }),
        app,
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopRoute {
    Chat,
    CodeGenerate,
    ProjectRead,
    Rescue,
    Action,
}

impl DesktopRoute {
    fn protocol_mode(self) -> ExecutorMode {
        match self {
            Self::Chat | Self::CodeGenerate => ExecutorMode::Chat,
            Self::ProjectRead => ExecutorMode::ProjectRead,
            Self::Rescue => ExecutorMode::Rescue,
            Self::Action => ExecutorMode::Action,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::CodeGenerate => "code_generate",
            Self::ProjectRead => "project_read",
            Self::Rescue => "rescue",
            Self::Action => "action",
        }
    }

    fn task_context_scope(self) -> TaskContextScope {
        match self {
            Self::Rescue => TaskContextScope::System,
            Self::Chat => TaskContextScope::Conversation,
            Self::ProjectRead | Self::CodeGenerate | Self::Action => TaskContextScope::Project,
        }
    }
}

fn classify_desktop_route(request: &str, previous: Option<&str>) -> DesktopRoute {
    let normalized = request.trim().to_lowercase();
    let actionable = without_negated_desktop_actions(&normalized);
    let short_followup = normalized.chars().count() <= 16
        && [
            "继续",
            "接着",
            "然后呢",
            "为什么",
            "详细一点",
            "再说说",
            "continue",
            "why",
            "more",
        ]
        .iter()
        .any(|value| normalized.starts_with(value));
    if short_followup {
        match previous.unwrap_or_default() {
            "project_read" | "project_question" | "read_file" | "list_files"
            | "summarize_project" => return DesktopRoute::ProjectRead,
            "action" | "tool_task" => return DesktopRoute::Action,
            "rescue" => return DesktopRoute::Rescue,
            _ => {}
        }
    }
    // A URL contains slash-delimited text that the project-file heuristic can
    // otherwise mistake for an absolute workspace path. An explicit bounded
    // network target is stronger Rescue evidence than that generic heuristic.
    if is_rescue_request(&normalized)
        && (requested_probe_description(&normalized).is_some()
            || crate::rescue_filesystem::is_filesystem_request(&normalized)
            || !mentions_project_file(&actionable))
    {
        return DesktopRoute::Rescue;
    }
    let conversational_code_request = contains_any(
        &actionable,
        &[
            "写一个",
            "写一段",
            "写份",
            "给我一个",
            "给我一段",
            "帮我做一个",
            "帮我写一个",
            "生成一段",
            "示例代码",
            "代码示例",
            "code example",
            "sample code",
            "write code",
            "generate code",
        ],
    ) && contains_any(
        &actionable,
        &[
            "代码",
            "程序",
            "脚本",
            "python",
            "javascript",
            "typescript",
            "rust",
            "java",
            "golang",
            "c++",
        ],
    );
    let explicit_persistence = contains_any(
        &actionable,
        &[
            "写入",
            "保存为",
            "保存成",
            "保存到",
            "存为",
            "存成",
            "写到",
            "创建文件",
            "新建文件",
            "生成文件",
        ],
    );
    if conversational_code_request && !explicit_persistence && !mentions_project_file(&actionable) {
        return DesktopRoute::CodeGenerate;
    }
    if contains_any(
        &actionable,
        &[
            "修复",
            "修改",
            "改成",
            "实现",
            "新增",
            "添加",
            "增加",
            "插入",
            "删除",
            "重构",
            "写入",
            "保存为",
            "保存成",
            "保存到",
            "存为",
            "存成",
            "写到",
            "创建文件",
            "新建文件",
            "生成文件",
            "运行测试",
            "执行测试",
            "pytest",
            "fix ",
            "modify",
            "implement",
            "refactor",
            "write ",
            "run tests",
            "run the tests",
        ],
    ) {
        return DesktopRoute::Action;
    }
    if contains_any(
        &normalized,
        &[
            "你能做什么",
            "有什么能力",
            "支持什么",
            "如何帮助",
            "怎么帮助",
            "怎样帮助",
        ],
    ) {
        return DesktopRoute::Chat;
    }
    if mentions_project_file(&actionable)
        || contains_any(
            &actionable,
            &[
                "项目",
                "仓库",
                "代码",
                "源码",
                "文件",
                "目录",
                "函数",
                "模块",
                "报错",
                "错误",
                "测试",
                "readme",
                "cargo.toml",
                "package.json",
                "project",
                "repository",
                "code",
                "file",
                "function",
                "error",
                "list files",
                "what files",
            ],
        )
    {
        return DesktopRoute::ProjectRead;
    }
    DesktopRoute::Chat
}

fn low_information_clarification(request: &str) -> Option<String> {
    let value = request.trim();
    if value.is_empty() {
        return None;
    }
    let numeric = value.chars().all(|character| character.is_ascii_digit()) && value.len() <= 12;
    let punctuation = value.chars().all(|character| {
        character.is_ascii_punctuation()
            || matches!(
                character,
                '。' | '，' | '？' | '！' | '；' | '：' | '、' | '…'
            )
    });
    if numeric {
        return Some(format!(
            "你希望我用 {value} 做什么？例如把它作为函数参数、查看第 {value} 行，或继续某个编号选项。请补充一个动作，我就能准确执行。"
        ));
    }
    punctuation.then(|| {
        "我还不知道你希望我执行什么。请补充一句具体目标，例如“解释当前文件”或“运行测试”。"
            .to_string()
    })
}

fn record_deterministic_clarification(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    request: &str,
    content: &str,
    followup: bool,
    app: &AppHandle,
) -> Result<(), String> {
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_ticket_state(ticket_id, TicketState::Waiting)
            .map_err(|error| error.to_string())?;
        if followup {
            guard
                .append_event(
                    Some(ticket_id),
                    "user.message",
                    json!({ "content": request }),
                )
                .map_err(|error| error.to_string())?;
        }
    }
    append_runtime_event(
        storage,
        ticket_id,
        "delivery.ready",
        json!({ "content": content, "route": "clarification", "evidence": [] }),
        app,
    );
    append_runtime_event(
        storage,
        ticket_id,
        "ticket.needs_clarification",
        json!({}),
        app,
    );
    Ok(())
}

fn without_negated_desktop_actions(value: &str) -> String {
    const NEGATIONS: &[&str] = &[
        "不要",
        "不用",
        "无需",
        "不需要",
        "禁止",
        "别",
        "不应",
        "不可",
        "不能",
        "do not",
        "don't",
        "never",
        "without",
    ];
    const ACTIONS: &[&str] = &[
        "读取", "打开", "查看", "修复", "修改", "改成", "实现", "新增", "添加", "增加", "删除",
        "重构", "写入", "创建", "新建", "保存", "运行", "执行", "read", "open", "inspect",
        "modify", "edit", "write", "delete", "create", "save", "run", "execute",
    ];
    value
        .split(|character| {
            matches!(
                character,
                '，' | ',' | '。' | '；' | ';' | '！' | '!' | '？' | '?' | '\n'
            )
        })
        .map(|clause| {
            let cutoff = NEGATIONS
                .iter()
                .filter_map(|negation| clause.find(negation))
                .filter(|index| {
                    ACTIONS
                        .iter()
                        .any(|action| clause[*index..].contains(action))
                })
                .min();
            cutoff.map_or(clause, |index| &clause[..index]).trim()
        })
        .filter(|clause| !clause.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn requests_rescue_repair(value: &str) -> bool {
    let actionable = without_negated_desktop_actions(&value.to_lowercase());
    [
        "修复",
        "解决",
        "恢复",
        "纠正",
        "规范化",
        "清理",
        "repair",
        "fix",
        "restore",
        "normalize",
    ]
    .iter()
    .any(|action| actionable.contains(action))
}

fn mentions_project_file(value: &str) -> bool {
    [
        ".md", ".txt", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".py", ".rs", ".go", ".java",
        ".kt", ".c", ".h", ".cpp", ".hpp", ".cs", ".json", ".yaml", ".yml", ".toml", ".xml",
        ".html", ".css", ".sql", ".sh", ".ps1", ".bat", ".cmd",
    ]
    .iter()
    .any(|extension| value.contains(extension))
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[allow(clippy::too_many_arguments)]
fn run_executor_ticket(
    storage: &Arc<Mutex<Storage>>,
    skills: &Arc<SkillRuntime>,
    attachments: &Arc<Mutex<AttachmentStore>>,
    legacy: &LegacyAgentClient,
    executor: &ExecutorClient,
    rescue_repairs: &RepairService,
    rescue_probes: &NetworkProbeService,
    ticket_id: &str,
    request_text: &str,
    client_request_id: &str,
    selection_id: Option<&str>,
    route: DesktopRoute,
    followup: bool,
    app: &AppHandle,
) -> Result<(), String> {
    let (ticket, project, settings, history, task_context) = {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = guard.ticket(ticket_id).map_err(|error| error.to_string())?;
        let project = guard
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let settings = guard.settings().map_err(|error| error.to_string())?;
        let history = guard
            .conversation_messages(ticket_id)
            .map_err(|error| error.to_string())?;
        let task_context = guard
            .task_context(ticket_id)
            .map_err(|error| error.to_string())?;
        (ticket, project, settings, history, task_context)
    };
    if route != DesktopRoute::Rescue {
        provider::configure(&settings, legacy, None)?;
    }
    let attachment_payloads = attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .resolve(selection_id, &ticket.project_id)?;
    if followup && route == DesktopRoute::Rescue {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .append_event(
                Some(ticket_id),
                "user.message",
                json!({ "content": request_text, "clientRequestId": client_request_id }),
            )
            .map_err(|error| error.to_string())?;
    }
    let mut entries = Vec::new();
    collect_project_files(
        Path::new(&project.path),
        Path::new(&project.path),
        0,
        &mut entries,
    )?;
    let files = entries
        .into_iter()
        .filter(|entry| !entry.is_directory)
        .map(|entry| WorkspaceFile {
            path: entry.path,
            size_bytes: entry.size,
            content_digest: None,
        })
        .collect::<Vec<_>>();
    let mut rescue_report = (route == DesktopRoute::Rescue).then(|| diagnose_rescue(request_text));
    if let Some(report) = rescue_report.as_mut() {
        if report.domain == "network" && report.next_probe.is_some() {
            report.next_probe = Some(requested_probe_description(request_text).unwrap_or_else(
                || "请明确一个受影响域名，或一个主机与端口；UCode 不会自行选择外部目标".into(),
            ));
        }
        for evidence in &report.evidence {
            append_runtime_event(
                storage,
                ticket_id,
                "rescue.probe.completed",
                json!({
                    "evidenceId": evidence.id,
                    "toolId": evidence.tool_id,
                    "state": evidence.state,
                    "summary": evidence.summary,
                    "contentDigest": evidence.content_digest,
                    "elapsedMs": evidence.elapsed_ms,
                }),
                app,
            );
        }
        append_runtime_event(
            storage,
            ticket_id,
            "rescue.diagnosis.ready",
            json!({
                "outcome": report.outcome,
                "domain": report.domain,
                "hypotheses": report.hypotheses,
                "nextProbe": report.next_probe,
                "mutationPerformed": false,
            }),
            app,
        );
        if report.domain == "system" && requests_rescue_repair(request_text) {
            let evidence_ids = report
                .evidence
                .iter()
                .filter(|item| item.tool_id == "system.environment.inspect")
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            match rescue_repairs.propose_user_path_normalization(ticket_id, evidence_ids) {
                Ok(Some(proposal)) => append_runtime_event(
                    storage,
                    ticket_id,
                    "rescue.repair.proposed",
                    json!({ "proposal": proposal }),
                    app,
                ),
                Ok(None) => {}
                Err(error) => append_runtime_event(
                    storage,
                    ticket_id,
                    "rescue.repair.unavailable",
                    json!({ "operationId": "system.user_path.normalize", "error": error }),
                    app,
                ),
            }
        } else if report.domain == "network" && report.next_probe.is_some() {
            let evidence_ids = report
                .evidence
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            match rescue_probes.propose_for_request(ticket_id, request_text, evidence_ids) {
                Ok(Some(proposal)) => append_runtime_event(
                    storage,
                    ticket_id,
                    "rescue.network_probe.proposed",
                    json!({ "proposal": proposal }),
                    app,
                ),
                Ok(None) => append_runtime_event(
                    storage,
                    ticket_id,
                    "rescue.network_probe.target_required",
                    json!({ "detail": "需要明确一个受影响域名，或一个主机与端口；UCode 不会自行选择外部目标" }),
                    app,
                ),
                Err(error) => append_runtime_event(
                    storage,
                    ticket_id,
                    "rescue.network_probe.unavailable",
                    json!({ "error": error }),
                    app,
                ),
            }
        }
    }
    let scout_context = if route == DesktopRoute::Rescue {
        None
    } else {
        run_automatic_scouts(
            storage,
            skills,
            legacy,
            executor,
            ticket_id,
            &project.path,
            request_text,
            route,
            files.len(),
            app,
        )
    };
    let mut history = history;
    if followup
        && !history
            .iter()
            .any(|(role, content)| role == "user" && content == &ticket.request)
    {
        history.insert(0, ("user".to_string(), ticket.request.clone()));
    }
    let history_budget = if route == DesktopRoute::ProjectRead {
        500
    } else {
        3_000
    };
    let mut remaining = history_budget;
    let mut bounded_history = history
        .into_iter()
        .rev()
        .take(12)
        .filter_map(|(role, content)| {
            let role = match role.as_str() {
                "user" => HistoryRole::User,
                "assistant" => HistoryRole::Assistant,
                _ => return None,
            };
            if remaining == 0 {
                return None;
            }
            let content = content.chars().take(remaining).collect::<String>();
            remaining = remaining.saturating_sub(content.chars().count());
            Some(ExecutorHistoryItem { role, content })
        })
        .collect::<Vec<_>>();
    bounded_history.reverse();
    let executor_attachments = attachment_payloads
        .iter()
        .map(|item| ExecutorAttachment {
            id: item.id.clone(),
            display_name: item.display_name.clone(),
            source: item.source.clone(),
            relative_path: item.relative_path.clone(),
            content_digest: item.content_sha256.clone(),
            content: item.content.clone(),
        })
        .collect::<Vec<_>>();
    let request_id = format!("executor-request-{}", Uuid::new_v4());
    let turn_id = format!("turn-{}", Uuid::new_v4());
    let mut task_context = serde_json::to_value(task_context).map_err(|error| error.to_string())?;
    if let (Some(findings), Some(object)) = (scout_context, task_context.as_object_mut()) {
        object.insert("scoutFindings".into(), json!(findings));
    }
    if let (Some(report), Some(object)) = (rescue_report.as_ref(), task_context.as_object_mut()) {
        object.insert("rescueEvidence".into(), compact_rescue_report(report));
    }
    let request = ExecutorRequest {
        version: EXECUTOR_PROTOCOL_V1.into(),
        request_id: request_id.clone(),
        turn_id,
        thread_key: format!("desktop:ticket:{ticket_id}"),
        mode: route.protocol_mode(),
        assignment: None,
        mandate: None,
        candidate: None,
        inputs: vec![ExecutorInput {
            id: format!("input-{}", Uuid::new_v4()),
            kind: TicketInputKind::Trigger,
            trigger_id: Some(format!("trigger-{}", Uuid::new_v4())),
            text: request_text.to_string(),
            human: true,
        }],
        history: bounded_history,
        workspace: ExecutorWorkspace {
            canonical_root: project.path.clone(),
            fingerprint: ticket.project_id.clone(),
            files,
        },
        attachment_token: if executor_attachments.is_empty() {
            None
        } else {
            selection_id.map(str::to_string)
        },
        attachments: executor_attachments,
        metadata: json!({
            "locale": "zh-CN",
            "skillContext": enabled_skill_context(storage, skills),
            "taskContext": task_context,
        }),
    };
    request.validate().map_err(|error| error.to_string())?;
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_external_ref(ticket_id, "executor_v1", &request_id)
            .map_err(|error| error.to_string())?;
        guard
            .set_ticket_state(ticket_id, TicketState::Running)
            .map_err(|error| error.to_string())?;
        if followup && route != DesktopRoute::Rescue {
            guard
                .append_event(
                    Some(ticket_id),
                    "user.message",
                    json!({ "content": request_text, "clientRequestId": client_request_id }),
                )
                .map_err(|error| error.to_string())?;
        }
        if !attachment_payloads.is_empty() {
            guard
                .append_event(
                    Some(ticket_id),
                    "context.attached",
                    attachment_event_payload(&attachment_payloads, &ticket.project_id),
                )
                .map_err(|error| error.to_string())?;
        }
    }
    if route != DesktopRoute::Rescue {
        append_runtime_event(
            storage,
            ticket_id,
            "provider.started",
            json!({ "provider": settings.model_provider }),
            app,
        );
    }
    append_runtime_event(
        storage,
        ticket_id,
        "provider.stream.started",
        json!({ "requestId": request_id }),
        app,
    );
    let event_storage = Arc::clone(storage);
    let event_ticket_id = ticket_id.to_string();
    let event_app = app.clone();
    let terminal = executor
        .run(&request, move |event| {
            persist_executor_event(&event_storage, &event_ticket_id, route, event, &event_app);
        })
        .map_err(|error| error.to_string())?;
    if let ExecutorEventPayload::Failure { message, .. } = terminal.payload {
        return Err(message);
    }
    Ok(())
}

fn normalize_race_paths(values: Vec<String>) -> Result<Vec<String>, String> {
    if values.is_empty() || values.len() > 20 {
        return Err("候选竞赛需要 1-20 个明确的允许路径".into());
    }
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim().replace('\\', "/");
        let path = Path::new(&value);
        if value.is_empty()
            || value.len() > 260
            || path.is_absolute()
            || value.split('/').any(|segment| segment.is_empty())
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!("候选允许路径无效：{value}"));
        }
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn normalize_race_targets(values: Vec<RaceTarget>) -> Result<Vec<RaceTarget>, String> {
    let normalized_paths =
        normalize_race_paths(values.iter().map(|target| target.path.clone()).collect())?;
    if normalized_paths.len() != values.len() {
        return Err("候选目标路径不能重复".into());
    }
    Ok(values
        .into_iter()
        .zip(normalized_paths)
        .map(|(mut target, path)| {
            target.path = path;
            target
        })
        .collect())
}

fn digest_content(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn race_effect_changes(
    project_path: &Path,
    candidate_path: &Path,
    targets: &[RaceTarget],
) -> Result<Vec<RaceEffectChange>, String> {
    if targets.is_empty() {
        return Err("获胜候选缺少可验证的目标操作，不能创建审批批次".into());
    }
    let mut changes = Vec::new();
    for target in targets {
        let relative = &target.path;
        let original_path = project_path.join(relative);
        let candidate_file = candidate_path.join(relative);
        let before_bytes = match fs::read(&original_path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("无法读取原文件 {relative}：{error}")),
        };
        let after_bytes = match fs::read(&candidate_file) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("无法读取候选文件 {relative}：{error}")),
        };
        if before_bytes == after_bytes {
            return Err(format!("获胜候选未完成目标文件变化：{relative}"));
        }
        let operation = match (&before_bytes, &after_bytes) {
            (None, Some(_)) => "create",
            (Some(_), None) => "delete",
            (Some(_), Some(_)) => "modify",
            (None, None) => continue,
        };
        let expected_operation = match target.operation {
            crate::race::RaceTargetOperation::Create => "create",
            crate::race::RaceTargetOperation::Modify => "modify",
            crate::race::RaceTargetOperation::Delete => "delete",
        };
        if operation != expected_operation {
            return Err(format!(
                "获胜候选对 {relative} 的实际操作为 {operation}，与请求的 {expected_operation} 不一致"
            ));
        }
        let before = before_bytes
            .as_deref()
            .map(|content| std::str::from_utf8(content).map(str::to_owned))
            .transpose()
            .map_err(|_| format!("原文件 {relative} 不是 UTF-8 文本"))?
            .unwrap_or_default();
        let after = after_bytes
            .as_deref()
            .map(|content| std::str::from_utf8(content).map(str::to_owned))
            .transpose()
            .map_err(|_| format!("候选文件 {relative} 不是 UTF-8 文本"))?
            .unwrap_or_default();
        changes.push(RaceEffectChange {
            path: relative.clone(),
            operation: operation.into(),
            before,
            after,
            before_digest: before_bytes.as_deref().map(digest_content),
            after_digest: after_bytes.as_deref().map(digest_content),
        });
    }
    if changes.len() != targets.len() {
        return Err("获胜候选未完整覆盖全部目标文件".into());
    }
    Ok(changes)
}

fn run_candidate_race_task(
    storage: &Arc<Mutex<Storage>>,
    executor: &ExecutorClient,
    app: &AppHandle,
    race_id: &str,
    project_id: &str,
    project_path: &Path,
    managed_root: &Path,
) -> Result<(), String> {
    let mut race = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .race(race_id)
        .map_err(|error| error.to_string())?;
    race.transition(RaceState::Generating, Utc::now().to_rfc3339())
        .map_err(|error| error.to_string())?;
    storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .save_race(&race)
        .map_err(|error| error.to_string())?;
    append_runtime_event(
        storage,
        &race.ticket_id,
        "race.generating",
        json!({ "raceId": race.id }),
        app,
    );

    let candidates = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .candidates_for_race(race_id)
        .map_err(|error| error.to_string())?;
    for mut candidate in candidates {
        if race_cancelled(storage, race_id)? {
            cleanup_race_workspaces(managed_root, race_id, storage);
            return Ok(());
        }
        candidate
            .transition(CandidateState::Generating, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        candidate.generation_attempts = 1;
        let workspace =
            match create_candidate_workspace(project_path, managed_root, race_id, &candidate.id) {
                Ok(workspace) => workspace,
                Err(error) => {
                    mark_candidate_failed(&mut candidate, format!("隔离工作区创建失败：{error}"));
                    storage
                        .lock()
                        .map_err(|_| "storage lock was poisoned".to_string())?
                        .save_candidate(&candidate)
                        .map_err(|error| error.to_string())?;
                    continue;
                }
            };
        candidate.workspace_path = Some(workspace.path.clone());
        candidate.base_digest = workspace.base_digest.clone();
        let request_id = format!("executor-race-{}", Uuid::new_v4());
        candidate.executor_request_id = Some(request_id.clone());
        storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .save_candidate(&candidate)
            .map_err(|error| error.to_string())?;

        let inventory = match workspace_inventory(Path::new(&workspace.path), 2_000) {
            Ok(inventory) => inventory,
            Err(error) => {
                mark_candidate_failed(&mut candidate, format!("候选索引失败：{error}"));
                storage
                    .lock()
                    .map_err(|_| "storage lock was poisoned".to_string())?
                    .save_candidate(&candidate)
                    .map_err(|error| error.to_string())?;
                let _ = remove_candidate_workspace(&workspace);
                continue;
            }
        };
        let thread_key = format!("desktop:race:{race_id}");
        let trigger_id = format!("trigger-{}", Uuid::new_v4());
        let capabilities = [
            Capability::Read,
            Capability::Search,
            Capability::Test,
            Capability::ProposeWrite,
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let strategy = candidate_strategy_id(candidate.strategy).to_string();
        let request = ExecutorRequest {
            version: EXECUTOR_PROTOCOL_V1.into(),
            request_id: request_id.clone(),
            turn_id: format!("turn-{}", Uuid::new_v4()),
            thread_key: thread_key.clone(),
            mode: ExecutorMode::Candidate,
            assignment: Some(ExecutorAssignment {
                ticket_id: race.ticket_id.clone(),
                assignment_id: candidate.id.clone(),
            }),
            mandate: Some(ExecutorMandate {
                strength: Strength::Explicit,
                policy_version: "race-v1".into(),
                project_id: project_id.into(),
                project_fingerprint: workspace.base_digest.clone(),
                capabilities,
                trigger_id: trigger_id.clone(),
                thread_key,
            }),
            candidate: Some(ExecutorCandidate {
                race_id: race_id.into(),
                candidate_id: candidate.id.clone(),
                strategy,
                allowed_paths: race.allowed_paths.clone(),
                targets: race
                    .targets
                    .iter()
                    .map(|target| ExecutorCandidateTarget {
                        path: target.path.clone(),
                        operation: match target.operation {
                            crate::race::RaceTargetOperation::Create => "create",
                            crate::race::RaceTargetOperation::Modify => "modify",
                            crate::race::RaceTargetOperation::Delete => "delete",
                        }
                        .into(),
                    })
                    .collect(),
                acceptance: race.acceptance.clone(),
                generation_attempt: 1,
                feedback: None,
            }),
            inputs: vec![ExecutorInput {
                id: format!("input-{}", Uuid::new_v4()),
                kind: TicketInputKind::Trigger,
                trigger_id: Some(trigger_id),
                text: race.objective.clone(),
                human: true,
            }],
            history: Vec::new(),
            workspace: ExecutorWorkspace {
                canonical_root: workspace.path.clone(),
                fingerprint: workspace.base_digest.clone(),
                files: inventory
                    .into_iter()
                    .map(|file| WorkspaceFile {
                        path: file.path,
                        size_bytes: file.size_bytes,
                        content_digest: Some(file.content_digest),
                    })
                    .collect(),
            },
            attachment_token: None,
            attachments: Vec::new(),
            metadata: json!({ "locale": "zh-CN", "raceId": race_id }),
        };
        let event_storage = Arc::clone(storage);
        let event_app = app.clone();
        let event_ticket_id = race.ticket_id.clone();
        let event_candidate_id = candidate.id.clone();
        let terminal = executor.run(&request, move |event| {
            if let ExecutorEventPayload::Activity {
                kind,
                title,
                status,
                evidence,
                ..
            } = &event.payload
            {
                append_runtime_event(
                    &event_storage,
                    &event_ticket_id,
                    "race.candidate_activity",
                    json!({
                        "candidateId": event_candidate_id,
                        "kind": kind,
                        "title": title,
                        "status": status,
                        "evidence": evidence,
                    }),
                    &event_app,
                );
            }
        });
        let current = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .candidates_for_race(race_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|value| value.id == candidate.id)
            .ok_or_else(|| "candidate disappeared during generation".to_string())?;
        if current.state == CandidateState::Cancelled || race_cancelled(storage, race_id)? {
            let _ = remove_candidate_workspace(&workspace);
            cleanup_race_workspaces(managed_root, race_id, storage);
            return Ok(());
        }
        match terminal {
            Ok(ExecutorEvent {
                payload: ExecutorEventPayload::Complete { content, .. },
                ..
            }) => match serde_json::from_str::<CandidateProposal>(&content) {
                Ok(proposal)
                    if proposal.race_id == race.id
                        && proposal.candidate_id == candidate.id
                        && proposal.strategy == candidate_strategy_id(candidate.strategy) =>
                {
                    candidate
                        .transition(CandidateState::Proposed, Utc::now().to_rfc3339())
                        .map_err(|error| error.to_string())?;
                    let proposal_path = Path::new(&workspace.path).join(".ucode-candidate.json");
                    fs::write(&proposal_path, content).map_err(|error| error.to_string())?;
                }
                Ok(_) => {
                    mark_candidate_failed(&mut candidate, "候选响应身份或策略与请求不匹配".into())
                }
                Err(error) => {
                    mark_candidate_failed(&mut candidate, format!("候选响应无法解析：{error}"))
                }
            },
            Ok(ExecutorEvent {
                payload: ExecutorEventPayload::Cancelled,
                ..
            }) => {
                candidate
                    .transition(CandidateState::Cancelled, Utc::now().to_rfc3339())
                    .map_err(|error| error.to_string())?;
            }
            Ok(ExecutorEvent {
                payload: ExecutorEventPayload::Failure { message, .. },
                ..
            }) => mark_candidate_failed(&mut candidate, message),
            Ok(_) => mark_candidate_failed(&mut candidate, "候选生成没有终态结果".into()),
            Err(error) => mark_candidate_failed(&mut candidate, error.to_string()),
        }
        storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .save_candidate(&candidate)
            .map_err(|error| error.to_string())?;
        if candidate.state != CandidateState::Proposed {
            let _ = remove_candidate_workspace(&workspace);
            candidate.workspace_path = None;
            storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?
                .save_candidate(&candidate)
                .map_err(|error| error.to_string())?;
        }
    }

    if race_cancelled(storage, race_id)? {
        cleanup_race_workspaces(managed_root, race_id, storage);
        return Ok(());
    }
    race = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .race(race_id)
        .map_err(|error| error.to_string())?;
    race.transition(RaceState::Validating, Utc::now().to_rfc3339())
        .map_err(|error| error.to_string())?;
    storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .save_race(&race)
        .map_err(|error| error.to_string())?;
    append_runtime_event(
        storage,
        &race.ticket_id,
        "race.validating",
        json!({ "raceId": race.id }),
        app,
    );

    let candidates = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .candidates_for_race(race_id)
        .map_err(|error| error.to_string())?;
    for mut candidate in candidates {
        if candidate.state != CandidateState::Proposed {
            continue;
        }
        candidate
            .transition(CandidateState::Validating, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        let workspace = load_candidate_workspace(managed_root, race_id, &candidate.id)
            .map_err(|error| error.to_string())?;
        let proposal_path = Path::new(&workspace.path).join(".ucode-candidate.json");
        let proposal: CandidateProposal =
            serde_json::from_slice(&fs::read(&proposal_path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&proposal_path);
        match verify_candidate(
            &workspace,
            &proposal,
            &race.allowed_paths,
            &race.targets,
            &race.acceptance,
        ) {
            Ok(result) => {
                candidate.diff = Some(result.diff);
                candidate.facts = result.facts;
                candidate.metrics = result.metrics;
                candidate.result_digest = Some(result.result_digest);
                let passed = candidate
                    .facts
                    .iter()
                    .all(|fact| fact.hard_failure.is_none())
                    && candidate
                        .facts
                        .iter()
                        .all(|fact| !fact.required || fact.outcome == ValidationOutcome::Passed);
                candidate
                    .transition(
                        if passed {
                            CandidateState::Passed
                        } else {
                            CandidateState::Failed
                        },
                        Utc::now().to_rfc3339(),
                    )
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                mark_candidate_failed(&mut candidate, format!("候选验证失败：{error}"));
            }
        }
        storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?
            .save_candidate(&candidate)
            .map_err(|error| error.to_string())?;
        if candidate.state != CandidateState::Passed {
            let _ = remove_candidate_workspace(&workspace);
            candidate.workspace_path = None;
            storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?
                .save_candidate(&candidate)
                .map_err(|error| error.to_string())?;
        }
    }

    let candidates = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .candidates_for_race(race_id)
        .map_err(|error| error.to_string())?;
    race.verdict = Some(judge(&candidates, Utc::now().to_rfc3339()));
    let mut proposed_effect = None;
    if race
        .verdict
        .as_ref()
        .and_then(|verdict| verdict.winner_id.as_ref())
        .is_some()
    {
        race.transition(RaceState::Decided, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        race.transition(RaceState::AwaitingReview, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        let current_digest = digest_tree(project_path).map_err(|error| error.to_string())?;
        if current_digest != race.project_fingerprint {
            race.transition(RaceState::Stale, Utc::now().to_rfc3339())
                .map_err(|error| error.to_string())?;
            cleanup_race_workspaces(managed_root, race_id, storage);
        } else if let Some(winner) = race
            .verdict
            .as_ref()
            .and_then(|verdict| verdict.winner_id.as_deref())
        {
            for candidate in &candidates {
                if candidate.id != winner {
                    cleanup_candidate_workspace_record(
                        managed_root,
                        race_id,
                        &candidate.id,
                        storage,
                    );
                }
            }
            let winner_candidate = candidates
                .iter()
                .find(|candidate| candidate.id == winner)
                .ok_or_else(|| "winner candidate is missing".to_string())?;
            let winner_path = winner_candidate
                .workspace_path
                .as_deref()
                .ok_or_else(|| "winner workspace is missing".to_string())?;
            let changes = race_effect_changes(project_path, Path::new(winner_path), &race.targets)?;
            proposed_effect = Some(json!({
                "id": format!("race-effect-{}", race.id),
                "risk": "write",
                "raceId": race.id,
                "atomic": true,
                "changes": changes,
            }));
        }
    } else {
        race.transition(RaceState::NoWinner, Utc::now().to_rfc3339())
            .map_err(|error| error.to_string())?;
        cleanup_race_workspaces(managed_root, race_id, storage);
    }
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard.save_race(&race).map_err(|error| error.to_string())?;
        if let Some(ticket_state) = terminal_ticket_state_for_race(race.state) {
            guard
                .set_ticket_state(&race.ticket_id, ticket_state)
                .map_err(|error| error.to_string())?;
        }
        if race.state == RaceState::NoWinner {
            guard
                .append_event(
                    Some(&race.ticket_id),
                    "delivery.ready",
                    json!({
                        "content": "两个隔离方案都没有通过全部必要验证，因此任务未完成，也没有推荐或写入任何修改。可查看候选事实后改写任务并重试。",
                        "route": "candidate_race",
                        "originalProjectChanged": false,
                    }),
                )
                .map_err(|error| error.to_string())?;
        }
    }
    if let Some(effect) = proposed_effect {
        append_runtime_event(storage, &race.ticket_id, "effect.proposed", effect, app);
    }
    let public_candidates = project_capsule(&race, &candidates).candidates;
    let tests_executed = public_candidates
        .iter()
        .flat_map(|candidate| candidate.facts.iter())
        .any(|fact| fact.command.is_some());
    append_runtime_event(
        storage,
        &race.ticket_id,
        "race.decided",
        json!({
            "race": race,
            "candidates": public_candidates,
            "testsExecuted": tests_executed,
            "testSkippedReason": if race.state == RaceState::NoWinner && !tests_executed {
                Some("两个候选均未完整覆盖任务目标，机械验收未执行，已在写入前安全停止。")
            } else {
                None
            },
            "originalProjectChanged": false,
        }),
        app,
    );
    Ok(())
}

fn candidate_strategy_id(strategy: CandidateStrategy) -> &'static str {
    match strategy {
        CandidateStrategy::MinimalChange => "minimal_change",
        CandidateStrategy::CompatibilityFirst => "compatibility_first",
    }
}

fn mark_candidate_failed(candidate: &mut Candidate, summary: String) {
    candidate.facts.push(ValidationFact {
        check_id: "candidate-generation".into(),
        outcome: ValidationOutcome::Failed,
        summary,
        required: true,
        hard_failure: Some(HardFailure::NonReplayable),
        command: None,
        exit_code: None,
        evidence_digests: Vec::new(),
    });
    if candidate.state.can_transition_to(CandidateState::Failed) {
        let _ = candidate.transition(CandidateState::Failed, Utc::now().to_rfc3339());
    }
}

fn race_cancelled(storage: &Arc<Mutex<Storage>>, race_id: &str) -> Result<bool, String> {
    Ok(storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .race(race_id)
        .map_err(|error| error.to_string())?
        .state
        == RaceState::Cancelled)
}

fn cleanup_race_workspaces(managed_root: &Path, race_id: &str, storage: &Arc<Mutex<Storage>>) {
    let candidates = storage
        .lock()
        .ok()
        .and_then(|guard| guard.candidates_for_race(race_id).ok())
        .unwrap_or_default();
    for candidate in candidates {
        cleanup_candidate_workspace_record(managed_root, race_id, &candidate.id, storage);
    }
}

fn cleanup_candidate_workspace_record(
    managed_root: &Path,
    race_id: &str,
    candidate_id: &str,
    storage: &Arc<Mutex<Storage>>,
) {
    let mut candidate = storage
        .lock()
        .ok()
        .and_then(|guard| guard.candidates_for_race(race_id).ok())
        .and_then(|values| values.into_iter().find(|value| value.id == candidate_id));
    let Some(candidate) = candidate.as_mut() else {
        return;
    };
    if let Ok(workspace) = load_candidate_workspace(managed_root, race_id, candidate_id) {
        if remove_candidate_workspace(&workspace).is_ok() {
            candidate.workspace_path = None;
            if let Ok(guard) = storage.lock() {
                let _ = guard.save_candidate(candidate);
            }
        }
    } else if candidate.workspace_path.is_some() {
        candidate.workspace_path = None;
        if let Ok(guard) = storage.lock() {
            let _ = guard.save_candidate(candidate);
        }
    }
}

fn fail_race(storage: &Arc<Mutex<Storage>>, app: &AppHandle, race_id: &str, error: &str) {
    let Ok(guard) = storage.lock() else {
        return;
    };
    let Ok(mut race) = guard.race(race_id) else {
        return;
    };
    if race.state == RaceState::Cancelled {
        return;
    }
    if race.state == RaceState::Prepared {
        let _ = race.transition(RaceState::Generating, Utc::now().to_rfc3339());
    }
    if race.state == RaceState::Generating {
        let _ = race.transition(RaceState::Validating, Utc::now().to_rfc3339());
    }
    let candidates = guard.candidates_for_race(race_id).unwrap_or_default();
    race.verdict = Some(judge(&candidates, Utc::now().to_rfc3339()));
    if race.state == RaceState::Validating {
        let _ = race.transition(RaceState::NoWinner, Utc::now().to_rfc3339());
    }
    let ticket_id = race.ticket_id.clone();
    let _ = guard.save_race(&race);
    let _ = guard.set_ticket_state(&ticket_id, TicketState::Failed);
    drop(guard);
    append_runtime_event(
        storage,
        &ticket_id,
        "race.failed",
        json!({ "raceId": race_id, "message": error }),
        app,
    );
}

fn automatic_scout_requests(
    request: &str,
    route: DesktopRoute,
    file_count: usize,
) -> Vec<SubagentRequest> {
    if route != DesktopRoute::ProjectRead {
        return Vec::new();
    }
    let normalized = request.to_lowercase();
    let broad = contains_any(
        &normalized,
        &[
            "整个项目",
            "全面",
            "架构",
            "跨模块",
            "重构",
            "安全",
            "性能",
            "整体",
            "多模块",
            "同时",
            "分别",
            "architecture",
            "security",
            "performance",
            "cross-module",
        ],
    );
    let named_files = [
        ".md", ".js", ".ts", ".tsx", ".py", ".rs", ".go", ".java", ".json", ".toml", ".yaml",
        ".yml",
    ]
    .iter()
    .map(|extension| normalized.matches(extension).count())
    .sum::<usize>();
    if !broad && named_files < 2 && !(file_count >= 40 && normalized.contains("项目")) {
        return Vec::new();
    }
    let requests = vec![
        SubagentRequest {
            label: "实现定位".into(),
            request: format!(
                "围绕主任务“{request}”，定位入口、依赖关系和最相关的实现文件，并引用实际路径。"
            ),
            capability: "read".into(),
        },
        SubagentRequest {
            label: "验证与风险".into(),
            request: format!(
                "围绕主任务“{request}”，检查现有测试、失败模式和可能的回归风险，并引用实际路径。"
            ),
            capability: "read".into(),
        },
    ];
    requests
}

#[allow(clippy::too_many_arguments)]
fn run_automatic_scouts(
    storage: &Arc<Mutex<Storage>>,
    skills: &Arc<SkillRuntime>,
    legacy: &LegacyAgentClient,
    executor: &ExecutorClient,
    ticket_id: &str,
    project_path: &str,
    request: &str,
    route: DesktopRoute,
    file_count: usize,
    app: &AppHandle,
) -> Option<String> {
    let requests = automatic_scout_requests(request, route, file_count);
    if requests.is_empty() {
        return None;
    }
    let (root, children) = {
        let guard = storage.lock().ok()?;
        let root = guard
            .create_assignment(ticket_id, None, "自动并行分析", request, "orchestrate")
            .ok()?;
        let children = requests
            .iter()
            .filter_map(|item| {
                guard
                    .create_assignment(
                        ticket_id,
                        Some(&root.id),
                        &item.label,
                        &item.request,
                        "read",
                    )
                    .ok()
            })
            .collect::<Vec<_>>();
        (root, children)
    };
    if children.is_empty() {
        return None;
    }
    run_subagent_batch(
        Arc::clone(storage),
        legacy.clone(),
        executor.clone(),
        Arc::clone(skills),
        app.clone(),
        ticket_id.to_string(),
        project_path.to_string(),
        PathBuf::new(),
        root.clone(),
        children,
    );
    storage
        .lock()
        .ok()
        .and_then(|guard| guard.assignments(ticket_id).ok())
        .and_then(|assignments| assignments.into_iter().find(|item| item.id == root.id))
        .and_then(|assignment| assignment.result)
        .map(|result| result.chars().take(4_000).collect())
}

fn persist_executor_event(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    route: DesktopRoute,
    event: &ExecutorEvent,
    app: &AppHandle,
) {
    match &event.payload {
        ExecutorEventPayload::Start => {}
        ExecutorEventPayload::TextDelta { delta } => append_runtime_event(
            storage,
            ticket_id,
            "provider.delta",
            json!({ "delta": delta, "requestId": event.request_id, "streamSequence": event.sequence }),
            app,
        ),
        ExecutorEventPayload::Activity {
            kind,
            title,
            status,
            evidence,
            ..
        } => append_runtime_event(
            storage,
            ticket_id,
            "tool.completed",
            json!({ "tool": kind, "title": title, "status": status, "evidence": evidence }),
            app,
        ),
        ExecutorEventPayload::EffectProposal {
            effect_key,
            kind,
            payload,
            ..
        } => append_runtime_event(
            storage,
            ticket_id,
            "effect.proposed",
            json!({
                "id": effect_key,
                "path": payload.get("path"),
                "before": "",
                "after": payload.get("content"),
                "risk": kind,
                "content": "修改提案已生成，等待审批。",
            }),
            app,
        ),
        ExecutorEventPayload::Report { state, summary } => {
            if matches!(state, ReportState::Waiting | ReportState::Blocked) {
                if let Ok(guard) = storage.lock() {
                    let _ = guard.set_ticket_state(ticket_id, TicketState::Waiting);
                }
            }
            append_runtime_event(
                storage,
                ticket_id,
                "assignment.report",
                json!({ "state": state, "summary": summary }),
                app,
            );
        }
        ExecutorEventPayload::Complete { content, .. } => {
            if let Ok(guard) = storage.lock() {
                let preserve_waiting = guard
                    .ticket(ticket_id)
                    .is_ok_and(|ticket| preserve_waiting_after_executor_complete(ticket.state));
                if route == DesktopRoute::Rescue {
                    let _ = guard.set_ticket_state(ticket_id, TicketState::Waiting);
                } else if !preserve_waiting {
                    let _ = guard.set_ticket_state(ticket_id, TicketState::Completed);
                }
            }
            append_runtime_event(
                storage,
                ticket_id,
                "delivery.ready",
                json!({ "content": content, "route": route.label(), "protocol": EXECUTOR_PROTOCOL_V1 }),
                app,
            );
        }
        ExecutorEventPayload::Failure {
            code,
            message,
            retryable,
        } => append_runtime_event(
            storage,
            ticket_id,
            "provider.error",
            json!({ "error": { "code": code, "message": message, "retryable": retryable } }),
            app,
        ),
        ExecutorEventPayload::Cancelled => {
            if let Ok(guard) = storage.lock() {
                let _ = guard.set_ticket_state(ticket_id, TicketState::Stopped);
            }
            append_runtime_event(storage, ticket_id, "ticket.cancelled", json!({}), app);
        }
    }
}

fn preserve_waiting_after_executor_complete(state: TicketState) -> bool {
    state == TicketState::Waiting
}

fn run_legacy_ticket(
    storage: &Arc<Mutex<Storage>>,
    skills: &SkillRuntime,
    attachments: &Arc<Mutex<AttachmentStore>>,
    legacy: &LegacyAgentClient,
    ticket_id: &str,
    selection_id: Option<&str>,
    app: &AppHandle,
) -> Result<(), String> {
    let (ticket, project, settings) = {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = guard.ticket(ticket_id).map_err(|error| error.to_string())?;
        let project = guard
            .project(&ticket.project_id)
            .map_err(|error| error.to_string())?;
        let settings = guard.settings().map_err(|error| error.to_string())?;
        (ticket, project, settings)
    };
    let attachment_payloads = attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .resolve(selection_id, &ticket.project_id)?;
    provider::configure(&settings, legacy, None)?;
    legacy
        .select_project(&project.path)
        .map_err(|error| error.to_string())?;
    let external_id = legacy
        .create_ticket(&ticket.request)
        .map_err(|error| error.to_string())?;
    let skill_context = legacy_execution_context(storage, skills, ticket_id);
    legacy
        .set_skill_context(&external_id, &skill_context)
        .map_err(|error| error.to_string())?;
    legacy
        .set_attachment_context(&external_id, &legacy_attachments(&attachment_payloads))
        .map_err(|error| error.to_string())?;
    {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        guard
            .set_ticket_state(ticket_id, TicketState::Running)
            .map_err(|error| error.to_string())?;
        if !attachment_payloads.is_empty() {
            guard
                .append_event(
                    Some(ticket_id),
                    "context.attached",
                    attachment_event_payload(&attachment_payloads, &ticket.project_id),
                )
                .map_err(|error| error.to_string())?;
        }
        guard
            .append_event(
                Some(ticket_id),
                "assignment.started",
                json!({ "adapter": "pocketmind" }),
            )
            .map_err(|error| error.to_string())?;
        guard
            .append_event(
                Some(ticket_id),
                "workspace.ready",
                json!({ "project": project.name, "attachmentCount": attachment_payloads.len() }),
            )
            .map_err(|error| error.to_string())?;
    }
    storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .set_external_ref(ticket_id, "pocketmind", &external_id)
        .map_err(|error| error.to_string())?;
    storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?
        .append_event(
            Some(ticket_id),
            "provider.started",
            json!({ "provider": settings.model_provider }),
        )
        .map_err(|error| error.to_string())?;
    let plan = if settings.stream_enabled {
        let event_storage = Arc::clone(storage);
        let event_ticket_id = ticket_id.to_string();
        let event_app = app.clone();
        legacy
            .plan_stream(&external_id, move |event| {
                persist_stream_event(&event_storage, &event_ticket_id, event, &event_app);
            })
            .map_err(|error| error.to_string())?
    } else {
        legacy
            .plan(&external_id)
            .map_err(|error| error.to_string())?
    };
    persist_plan(storage, ticket_id, plan)
}

fn legacy_attachments(payloads: &[AttachmentPayload]) -> Vec<LegacyAttachment> {
    payloads
        .iter()
        .map(|item| LegacyAttachment {
            id: item.id.clone(),
            display_name: item.display_name.clone(),
            source: item.source.clone(),
            relative_path: item.relative_path.clone(),
            size_bytes: item.size_bytes,
            included_bytes: item.included_bytes,
            truncated: item.truncated,
            sha256: item.sha256.clone(),
            content_sha256: item.content_sha256.clone(),
            content: item.content.clone(),
        })
        .collect()
}

fn attachment_event_payload(payloads: &[AttachmentPayload], project_id: &str) -> serde_json::Value {
    json!({
        "projectId": project_id,
        "attachments": payloads.iter().map(|item| json!({
            "id": item.id,
            "displayName": item.display_name,
            "source": item.source,
            "relativePath": item.relative_path,
            "sizeBytes": item.size_bytes,
            "includedBytes": item.included_bytes,
            "truncated": item.truncated,
            "sha256": item.sha256,
        })).collect::<Vec<_>>()
    })
}

fn persist_stream_event(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    event: &serde_json::Value,
    app: &AppHandle,
) {
    let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    let (event_type, payload) = match kind {
        "start" => (
            "provider.stream.started",
            json!({ "requestId": event.get("requestId") }),
        ),
        "route" => ("route.selected", json!({ "intent": event.get("intent") })),
        "text_delta" => (
            "provider.delta",
            json!({ "delta": event.get("delta").and_then(serde_json::Value::as_str).unwrap_or(""), "requestId": event.get("requestId"), "streamSequence": event.get("sequence") }),
        ),
        "tool_completed" => (
            "tool.completed",
            json!({ "tool": event.get("tool"), "evidence": event.get("evidence") }),
        ),
        "cancelled" => (
            "provider.cancelled",
            json!({ "requestId": event.get("requestId") }),
        ),
        "reconnecting" => (
            "provider.reconnecting",
            json!({ "requestId": event.get("requestId") }),
        ),
        "reconnected" => (
            "provider.reconnected",
            json!({ "requestId": event.get("requestId") }),
        ),
        "error" => ("provider.error", json!({ "error": event.get("error") })),
        _ => return,
    };
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if let Ok(guard) = storage.lock() {
        if let Ok(sequence) =
            guard.append_event_at(Some(ticket_id), event_type, payload.clone(), &created_at)
        {
            let runtime_event = RuntimeEvent {
                sequence,
                ticket_id: Some(ticket_id.to_string()),
                event_type: event_type.to_string(),
                payload,
                created_at,
            };
            drop(guard);
            let _ = app.emit("runtime://event", runtime_event);
        }
    }
}

fn persist_plan(
    storage: &Arc<Mutex<Storage>>,
    ticket_id: &str,
    plan: LegacyPlan,
) -> Result<(), String> {
    let guard = storage
        .lock()
        .map_err(|_| "storage lock was poisoned".to_string())?;
    let current_state = guard
        .ticket(ticket_id)
        .map_err(|error| error.to_string())?
        .state;
    if current_state == TicketState::Stopped {
        return Ok(());
    }
    if plan.kind == "cancelled" {
        guard
            .set_ticket_state(ticket_id, TicketState::Stopped)
            .map_err(|error| error.to_string())?;
        guard
            .append_event(Some(ticket_id), "ticket.cancelled", json!({}))
            .map_err(|error| error.to_string())?;
    } else if let Some(effect) = plan.effect {
        let changes = if effect.changes.is_empty() {
            vec![json!({
                "path": effect.path,
                "operation": if effect.before.is_some() { "modify" } else { "create" },
                "before": effect.before,
                "after": effect.content,
                "beforeDigest": effect.before_digest,
                "afterDigest": effect.after_digest,
            })]
        } else {
            effect
                .changes
                .iter()
                .map(|change| {
                    json!({
                        "path": change.path,
                        "operation": change.operation,
                        "before": change.before,
                        "after": change.content,
                        "beforeDigest": change.before_digest,
                        "afterDigest": change.after_digest,
                    })
                })
                .collect::<Vec<_>>()
        };
        let first = changes.first().cloned().unwrap_or_else(|| json!({}));
        guard
            .set_ticket_state(ticket_id, TicketState::Waiting)
            .map_err(|error| error.to_string())?;
        guard.append_event(Some(ticket_id), "effect.proposed", json!({
            "id": effect.id,
            "path": first.get("path").and_then(serde_json::Value::as_str).unwrap_or(""),
            "before": first.get("before").and_then(serde_json::Value::as_str).unwrap_or(""),
            "after": first.get("after").and_then(serde_json::Value::as_str).unwrap_or(""),
            "risk": effect.risk, "content": plan.content,
            "beforeDigest": first.get("beforeDigest"), "afterDigest": first.get("afterDigest"),
            "changes": changes,
        })).map_err(|error| error.to_string())?;
    } else if plan.kind == "needs_clarification" {
        guard
            .set_ticket_state(ticket_id, TicketState::Waiting)
            .map_err(|error| error.to_string())?;
        guard
            .append_event(
                Some(ticket_id),
                "delivery.ready",
                json!({ "content": plan.content, "route": plan.route, "evidence": plan.evidence, "recovery": plan.recovery }),
            )
            .map_err(|error| error.to_string())?;
        guard
            .append_event(Some(ticket_id), "ticket.needs_clarification", json!({}))
            .map_err(|error| error.to_string())?;
    } else if plan.kind == "final" {
        guard
            .set_ticket_state(ticket_id, TicketState::Completed)
            .map_err(|error| error.to_string())?;
        guard
            .append_event(
                Some(ticket_id),
                "delivery.ready",
                json!({ "content": plan.content, "route": plan.route, "evidence": plan.evidence }),
            )
            .map_err(|error| error.to_string())?;
        guard
            .append_event(Some(ticket_id), "ticket.completed", json!({}))
            .map_err(|error| error.to_string())?;
    } else {
        guard
            .append_event(
                Some(ticket_id),
                "provider.message",
                json!({ "type": plan.kind, "content": plan.content }),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn record_failure(storage: &Arc<Mutex<Storage>>, ticket_id: &str, message: &str) {
    if let Ok(guard) = storage.lock() {
        if guard
            .ticket(ticket_id)
            .map(|ticket| ticket.state == TicketState::Stopped)
            .unwrap_or(false)
        {
            return;
        }
        let _ = guard.set_ticket_state(ticket_id, TicketState::Failed);
        let _ = guard.append_event(
            Some(ticket_id),
            "ticket.failed",
            json!({ "message": message }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn onboarding_finishes_only_when_every_step_is_resolved() {
        let mut onboarding = OnboardingState::default();
        for step in ONBOARDING_STEPS.iter().take(6) {
            complete_onboarding_value(&mut onboarding, step);
        }
        assert_eq!(onboarding.welcome_state, "active");
        onboarding.skipped_steps.push("evidence".into());
        finish_onboarding_if_ready(&mut onboarding);
        assert_eq!(onboarding.welcome_state, "completed");
        assert!(!onboarding.completed_steps.contains(&"evidence".into()));
    }

    #[test]
    fn executor_complete_does_not_erase_a_waiting_for_user_report() {
        assert!(preserve_waiting_after_executor_complete(
            TicketState::Waiting
        ));
        assert!(!preserve_waiting_after_executor_complete(
            TicketState::Running
        ));
        assert!(!preserve_waiting_after_executor_complete(TicketState::Open));
    }

    #[test]
    fn effect_recovery_failure_is_distinct_from_an_ordinary_apply_refusal() {
        assert!(is_effect_recovery_failure(
            "File batch failed and automatic recovery could not be verified: a.py"
        ));
        assert!(is_effect_recovery_failure("batch_recovery_failed"));
        assert!(!is_effect_recovery_failure(
            "File changed after approval was requested: a.py"
        ));
    }

    #[test]
    fn project_inventory_is_deterministic_and_ignores_generated_caches() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("main.py"), "print('ok')\n").unwrap();
        fs::write(directory.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(directory.path().join(".pytest_cache")).unwrap();
        fs::write(
            directory.path().join(".pytest_cache/CACHEDIR.TAG"),
            "cache\n",
        )
        .unwrap();
        let mut entries = Vec::new();
        collect_project_files(directory.path(), directory.path(), 0, &mut entries).unwrap();
        let files = entries
            .into_iter()
            .filter(|entry| !entry.is_directory)
            .map(|entry| WorkspaceFile {
                path: entry.path,
                size_bytes: entry.size,
                content_digest: None,
            })
            .collect::<Vec<_>>();
        let result = deterministic_file_inventory("说明项目结构和主要文件类型", &files).unwrap();
        assert!(result.contains("2 个文件"));
        assert!(result.contains("1 个 Python 文件"));
        assert!(result.contains("1 个 Markdown 文件"));
        assert!(result.contains("`main.py`"));
        assert!(!result.contains("pytest_cache"));
        assert!(deterministic_file_inventory("分析业务逻辑", &files).is_none());
    }

    #[test]
    fn git_validation_rejects_escaping_paths_and_invalid_branches() {
        assert!(validate_relative("src/main.rs").is_ok());
        assert!(validate_relative("../outside.txt").is_err());
        assert!(validate_relative("C:\\outside.txt").is_err());
        assert!(validate_branch("feature/terminal-ui").is_ok());
        assert!(validate_branch("bad branch").is_err());
        assert!(validate_branch("bad..branch").is_err());
        let directory = tempdir().unwrap();
        let markdown = directory.path().join("conversation.md");
        export_markdown(markdown.display().to_string(), "# UCode\n".into()).unwrap();
        assert_eq!(fs::read_to_string(markdown).unwrap(), "# UCode\n");
        assert!(export_markdown(
            directory
                .path()
                .join("conversation.txt")
                .display()
                .to_string(),
            "no".into()
        )
        .is_err());
    }

    #[test]
    fn external_links_allow_web_and_mail_but_reject_unsafe_schemes_and_credentials() {
        assert!(validate_external_url("https://example.com/docs?q=ucode").is_ok());
        assert!(validate_external_url("http://127.0.0.1:3082/ready").is_ok());
        assert!(validate_external_url("mailto:hello@example.com").is_ok());
        assert!(validate_external_url("javascript:alert(1)").is_err());
        assert!(validate_external_url("file:///C:/Windows/win.ini").is_err());
        assert!(validate_external_url("https://user:secret@example.com/").is_err());
        assert!(validate_external_url("https://example.com/\nsecond").is_err());
    }

    #[test]
    fn desktop_route_keeps_chat_reading_and_actions_separate() {
        assert_eq!(
            classify_desktop_route("你好，你是谁？", None),
            DesktopRoute::Chat
        );
        assert_eq!(
            classify_desktop_route("你好，请用一句话说明你能如何帮助这个项目。", None),
            DesktopRoute::Chat
        );
        assert_eq!(
            classify_desktop_route("请总结这个项目", None),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("解释 README.md", None),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("解释 build_refs.py 是做什么的", None),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("修改 calculator.py 并运行测试", None),
            DesktopRoute::Action
        );
        assert_eq!(
            classify_desktop_route("在 knapsack.py 顶部增加一行注释", None),
            DesktopRoute::Action
        );
        assert_eq!(
            classify_desktop_route("写一个背包问题的 Python 代码", None),
            DesktopRoute::CodeGenerate
        );
        assert_eq!(
            classify_desktop_route("写一个背包问题的 Python 代码，不要读取或修改项目文件", None),
            DesktopRoute::CodeGenerate
        );
        assert_eq!(
            classify_desktop_route(
                "请只用一句话说明 Python list 和 tuple 的主要区别，不要读取或修改项目文件。",
                None
            ),
            DesktopRoute::Chat
        );
        assert_eq!(
            classify_desktop_route(
                "Please explain Python lists without reading or modifying project files.",
                None
            ),
            DesktopRoute::Chat
        );
        assert_eq!(
            classify_desktop_route("不要修改 calculator.py，只解释它为什么报错", None),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("不要先读取文件，直接修改 calculator.py", None),
            DesktopRoute::Action
        );
        assert_eq!(
            classify_desktop_route("帮我做一个背包问题的 Python 代码", None),
            DesktopRoute::CodeGenerate
        );
        assert_eq!(
            classify_desktop_route("Windows 电脑无法上网，请帮我诊断", None),
            DesktopRoute::Rescue
        );
        assert_eq!(
            classify_desktop_route(
                "https://example.com 无法打开，请根据真实系统证据诊断；联网探测前必须让我确认。",
                None
            ),
            DesktopRoute::Rescue
        );
        assert_eq!(
            classify_desktop_route("example.com:443 端口连接失败，请排查", None),
            DesktopRoute::Rescue
        );
        assert_eq!(
            classify_desktop_route(
                "检查 `C:\\Windows\\System32\\notepad.exe` 的 SHA256 和 ACL 权限",
                None
            ),
            DesktopRoute::Rescue
        );
        assert_eq!(
            classify_desktop_route("检查系统服务为什么异常", None),
            DesktopRoute::Rescue
        );
        assert_eq!(
            classify_desktop_route("解释 network_service.py 为什么报错", None),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("给我一段 Python 示例代码", None),
            DesktopRoute::CodeGenerate
        );
        assert_eq!(
            classify_desktop_route("把它保存为 knapsack.py", Some("code_generate")),
            DesktopRoute::Action
        );
        assert_eq!(
            classify_desktop_route("为什么？", Some("project_read")),
            DesktopRoute::ProjectRead
        );
        assert_eq!(
            classify_desktop_route("继续", Some("action")),
            DesktopRoute::Action
        );
    }

    #[test]
    fn rescue_repair_requires_positive_non_negated_user_intent() {
        assert!(requests_rescue_repair(
            "Windows PATH 异常，请检查并生成可回滚修复方案"
        ));
        assert!(requests_rescue_repair("PATH 坏了，请帮我解决"));
        assert!(!requests_rescue_repair(
            "检查事件日志，只读取有界元数据，不要修改系统"
        ));
        assert!(!requests_rescue_repair("不要修复 PATH，只检查并解释问题"));
    }

    #[test]
    fn low_information_input_is_clarified_before_model_routing() {
        assert!(low_information_clarification("15")
            .unwrap()
            .contains("用 15 做什么"));
        assert!(low_information_clarification("？")
            .unwrap()
            .contains("具体目标"));
        assert!(low_information_clarification("解释第 15 行").is_none());
        assert!(low_information_clarification("继续").is_none());
    }

    #[test]
    fn automatic_scouts_only_activate_for_complex_project_work() {
        assert!(
            automatic_scout_requests("解释 README.md", DesktopRoute::ProjectRead, 80).is_empty()
        );
        assert!(automatic_scout_requests("你好", DesktopRoute::Chat, 80).is_empty());
        assert_eq!(
            automatic_scout_requests(
                "全面分析整个项目架构和测试风险",
                DesktopRoute::ProjectRead,
                80
            )
            .len(),
            2
        );
        assert_eq!(
            automatic_scout_requests(
                "分析整个项目的安全、性能和构建配置",
                DesktopRoute::ProjectRead,
                80
            )
            .len(),
            2
        );
        assert_eq!(
            automatic_scout_requests("比较 src/a.py 和 src/b.py", DesktopRoute::ProjectRead, 8)
                .len(),
            2
        );
    }

    #[test]
    fn git_details_reads_disposable_repository_changes() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init"]);
        git(&["config", "user.name", "UCode Test"]);
        git(&["config", "user.email", "ucode@example.invalid"]);
        fs::write(root.join("sample.txt"), "one\n").unwrap();
        git(&["add", "--", "sample.txt"]);
        git(&["commit", "-m", "initial"]);
        fs::write(root.join("sample.txt"), "one\ntwo\n").unwrap();
        let details = git_details_for(&root.display().to_string()).unwrap();
        assert!(details.is_repository);
        assert!(!details.clean);
        assert!(details
            .lines
            .iter()
            .any(|line| line.ends_with("sample.txt")));
        assert!(details.diff.contains("+two"));
        assert!(details.history.iter().any(|line| line.contains("initial")));
        assert!(details
            .changes
            .iter()
            .any(|change| change.path == "sample.txt" && change.unstaged));

        fs::write(root.join("file with space.txt"), "space\n").unwrap();
        git(&["add", "--", "file with space.txt"]);
        let details = git_details_for(&root.display().to_string()).unwrap();
        assert!(details
            .changes
            .iter()
            .any(|change| change.path == "file with space.txt" && change.staged));

        git(&["mv", "sample.txt", "renamed sample.txt"]);
        let details = git_details_for(&root.display().to_string()).unwrap();
        assert!(details.changes.iter().any(|change| {
            change.path == "renamed sample.txt"
                && change.original_path.as_deref() == Some("sample.txt")
        }));
    }

    #[cfg(windows)]
    #[test]
    fn git_process_paths_drop_windows_extended_length_prefixes() {
        assert_eq!(
            git_compatible_path(Path::new(r"\\?\E:\portable\worktrees\agent")),
            PathBuf::from(r"E:\portable\worktrees\agent")
        );
        assert_eq!(
            git_compatible_path(Path::new(r"\\?\UNC\server\share\agent")),
            PathBuf::from(r"\\server\share\agent")
        );
    }

    #[test]
    fn worktrees_are_created_listed_and_removed_with_safety_checks() {
        let directory = tempdir().unwrap();
        let managed = directory.path().join("worktrees");
        let root = managed.join("main");
        fs::create_dir_all(&root).unwrap();
        let git = |path: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&root, &["init"]);
        git(&root, &["config", "user.name", "UCode Test"]);
        git(&root, &["config", "user.email", "ucode@example.invalid"]);
        fs::write(root.join("README.md"), "UCode\n").unwrap();
        git(&root, &["add", "--", "README.md"]);
        git(&root, &["commit", "-m", "initial"]);

        assert!(validate_worktree_name("feature-one").is_ok());
        assert!(validate_worktree_name("../escape").is_err());
        assert!(validate_revision("HEAD").is_ok());
        assert!(validate_revision("--help").is_err());
        let feature = managed.join("feature-one");
        add_worktree_for(&root.display().to_string(), &feature, "feature/one", "HEAD").unwrap();
        let listed = list_worktrees_for(&root.display().to_string()).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].is_main);
        assert_eq!(listed[1].branch, "feature/one");
        assert!(!listed[1].dirty);
        assert!(
            remove_worktree_for(&root.display().to_string(), &root, &managed)
                .unwrap_err()
                .contains("主工作区")
        );

        fs::write(feature.join("dirty.txt"), "not committed\n").unwrap();
        assert!(
            remove_worktree_for(&root.display().to_string(), &feature, &managed)
                .unwrap_err()
                .contains("未提交改动")
        );
        fs::remove_file(feature.join("dirty.txt")).unwrap();
        remove_worktree_for(&root.display().to_string(), &feature, &managed).unwrap();
        assert_eq!(
            list_worktrees_for(&root.display().to_string())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn isolated_subagent_diff_requires_safe_paths_and_applies_only_after_check() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("main");
        let worktree = directory.path().join("agent");
        fs::create_dir_all(&root).unwrap();
        let git = |path: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&root, &["init"]);
        git(&root, &["config", "user.name", "UCode Test"]);
        git(&root, &["config", "user.email", "ucode@example.invalid"]);
        fs::write(root.join("app.txt"), "before\n").unwrap();
        git(&root, &["add", "--", "app.txt"]);
        git(&root, &["commit", "-m", "initial"]);
        add_worktree_for(
            &root.display().to_string(),
            &worktree,
            "ucode/agent-test",
            "HEAD",
        )
        .unwrap();
        fs::write(worktree.join("app.txt"), "after\n").unwrap();
        let diff = git_diff_for(&worktree.display().to_string()).unwrap();
        assert!(diff.contains("+after"));
        ensure_same_worktree_base(&root.display().to_string(), &worktree.display().to_string())
            .unwrap();
        ensure_managed_assignment_worktree(
            &root.display().to_string(),
            &worktree.display().to_string(),
            directory.path(),
        )
        .unwrap();
        apply_git_patch(&root.display().to_string(), &diff).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("app.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "after\n"
        );
        git(&root, &["add", "--", "app.txt"]);
        git(&root, &["commit", "-m", "main advanced"]);
        assert!(ensure_same_worktree_base(
            &root.display().to_string(),
            &worktree.display().to_string()
        )
        .unwrap_err()
        .contains("基准提交"));
        assert!(validate_patch_paths("diff --git a/../escape b/../escape\n").is_err());
    }

    #[test]
    fn candidate_race_scope_requires_explicit_project_relative_paths() {
        assert_eq!(
            normalize_race_paths(vec!["src/app.py".into(), "src/app.py".into()]).unwrap(),
            vec!["src/app.py"]
        );
        for value in [
            "../secret.txt",
            "C:/secret.txt",
            "/root/secret.txt",
            "src//app.py",
        ] {
            assert!(
                normalize_race_paths(vec![value.into()]).is_err(),
                "accepted {value}"
            );
        }
        assert!(normalize_race_paths(Vec::new()).is_err());
    }

    #[test]
    fn candidate_race_terminal_states_do_not_report_no_winner_as_completed() {
        assert_eq!(
            terminal_ticket_state_for_race(RaceState::NoWinner),
            Some(TicketState::Failed)
        );
        assert_eq!(
            terminal_ticket_state_for_race(RaceState::Stale),
            Some(TicketState::Failed)
        );
        assert_eq!(
            terminal_ticket_state_for_race(RaceState::Cancelled),
            Some(TicketState::Stopped)
        );
        assert_eq!(
            terminal_ticket_state_for_race(RaceState::AwaitingReview),
            None
        );
    }

    #[test]
    fn legacy_candidate_race_requires_complete_unique_target_contract() {
        let race = Race {
            schema_version: RACE_SCHEMA_VERSION,
            id: "race-legacy".into(),
            ticket_id: "ticket-legacy".into(),
            project_fingerprint: "project".into(),
            objective: "modify files".into(),
            state: RaceState::Generating,
            targets: vec![RaceTarget {
                path: "src/app.py".into(),
                operation: crate::race::RaceTargetOperation::Modify,
            }],
            allowed_paths: vec!["src/app.py".into()],
            acceptance: vec!["pytest".into()],
            verdict: None,
            created_at: "created".into(),
            updated_at: "updated".into(),
        };
        assert!(race_targets_match_allowed_paths(&race));
        assert!(!race_targets_match_allowed_paths(&Race {
            targets: Vec::new(),
            ..race.clone()
        }));
        assert!(!race_targets_match_allowed_paths(&Race {
            allowed_paths: vec!["src/other.py".into()],
            ..race
        }));
    }

    #[test]
    fn race_effect_batch_applies_rolls_back_and_refuses_stale_files() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let candidate = directory.path().join("candidate");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(project.join("app.txt"), "before\n").unwrap();
        fs::write(project.join("remove.txt"), "remove me\n").unwrap();
        fs::write(candidate.join("app.txt"), "after\n").unwrap();
        fs::write(candidate.join("created.txt"), "created\n").unwrap();
        let changes = race_effect_changes(
            &project,
            &candidate,
            &[
                RaceTarget {
                    path: "app.txt".into(),
                    operation: crate::race::RaceTargetOperation::Modify,
                },
                RaceTarget {
                    path: "created.txt".into(),
                    operation: crate::race::RaceTargetOperation::Create,
                },
                RaceTarget {
                    path: "remove.txt".into(),
                    operation: crate::race::RaceTargetOperation::Delete,
                },
            ],
        )
        .unwrap();

        let paths = apply_race_changes(&project, &changes, true).unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(
            fs::read_to_string(project.join("app.txt")).unwrap(),
            "after\n"
        );
        assert_eq!(
            fs::read_to_string(project.join("created.txt")).unwrap(),
            "created\n"
        );
        assert!(!project.join("remove.txt").exists());

        apply_race_changes(&project, &changes, false).unwrap();
        assert_eq!(
            fs::read_to_string(project.join("app.txt")).unwrap(),
            "before\n"
        );
        assert!(!project.join("created.txt").exists());
        assert_eq!(
            fs::read_to_string(project.join("remove.txt")).unwrap(),
            "remove me\n"
        );

        fs::write(project.join("app.txt"), "user changed it\n").unwrap();
        assert!(apply_race_changes(&project, &changes, true)
            .unwrap_err()
            .contains("审查后发生变化"));
        assert_eq!(
            fs::read_to_string(project.join("app.txt")).unwrap(),
            "user changed it\n"
        );
        assert!(!project.join("created.txt").exists());
    }

    #[test]
    fn race_effect_refuses_partial_or_wrong_operation_winners() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let candidate = directory.path().join("candidate");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(project.join("a.txt"), "before\n").unwrap();
        fs::write(project.join("b.txt"), "before\n").unwrap();
        fs::write(candidate.join("a.txt"), "after\n").unwrap();
        fs::write(candidate.join("b.txt"), "before\n").unwrap();

        let incomplete = race_effect_changes(
            &project,
            &candidate,
            &[
                RaceTarget {
                    path: "a.txt".into(),
                    operation: crate::race::RaceTargetOperation::Modify,
                },
                RaceTarget {
                    path: "b.txt".into(),
                    operation: crate::race::RaceTargetOperation::Modify,
                },
            ],
        )
        .unwrap_err();
        assert!(incomplete.contains("未完成目标文件变化"));

        fs::write(candidate.join("b.txt"), "after\n").unwrap();
        let wrong_operation = race_effect_changes(
            &project,
            &candidate,
            &[RaceTarget {
                path: "b.txt".into(),
                operation: crate::race::RaceTargetOperation::Delete,
            }],
        )
        .unwrap_err();
        assert!(wrong_operation.contains("与请求的 delete 不一致"));
    }
}
