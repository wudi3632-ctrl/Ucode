use crate::attachments::{AttachmentPayload, AttachmentStore, ContextAttachmentSelection};
use crate::domain::{AssignmentState, TicketState};
use crate::legacy::{LegacyAgentClient, LegacyAttachment, LegacyEffect, LegacyMessage, LegacyPlan};
use crate::notifications::{
    NotificationDiagnosticSnapshot, NotificationDiagnostics, NotificationIdentityState,
};
use crate::model_packs::ModelPackInfo;
use crate::portable::{CapsuleHealth, PortableRoot};
use crate::provider::{self, ProviderSecretStatus, ProviderTestResult};
use crate::skills::{SkillPreview, SkillRuntime};
use crate::storage::{
    AppSettings, AssignmentSummary, EvidenceReceipt, OnboardingState, ProjectSummary, RuntimeEvent,
    SkillSummary, Storage, TicketSummary, ONBOARDING_STEPS,
};
use crate::superagent::{
    ExecutorAttachment, ExecutorClient, ExecutorEvent, ExecutorEventPayload, ExecutorHistoryItem,
    ExecutorInput, ExecutorMode, ExecutorRequest, ExecutorWorkspace, HistoryRole, TicketInputKind,
    ReportState, WorkspaceFile, EXECUTOR_PROTOCOL_V1,
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
use std::collections::BTreeMap;
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
        Some(selection_id) => attachments.append(
            &selection_id,
            &project_id,
            Path::new(&project.path),
            paths,
        )?,
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

fn legacy_execution_context(storage: &Arc<Mutex<Storage>>, runtime: &SkillRuntime, ticket_id: &str) -> String {
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
    snapshot(&state)
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
        health
            .issues
            .push(format!("本地模型包未通过验证：{error}"));
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
pub fn create_onboarding_demo(
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
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
        demo_root.join(format!("ucode-starter-{}", Utc::now().format("%Y%m%d-%H%M%S")))
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
        "project" => !storage.projects().map_err(|error| error.to_string())?.is_empty(),
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
    if tip_id.len() > 64 || !tip_id.chars().all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-') {
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
        tauri::async_runtime::spawn_blocking(move || {
            supervisor.restart_with_profile(&profile_id)
        })
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
pub async fn list_model_packs(
    state: State<'_, AppState>,
) -> Result<Vec<ModelPackInfo>, String> {
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
                .append_event(None, "onboarding.git_observed", json!({ "projectId": project_id }))
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
    let attachments = Arc::clone(&state.attachments);
    let task_ticket_id = ticket_id.clone();
    let task_selection_id = selection_id.clone();
    let task_app = app.clone();
    let route = {
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let ticket = guard
            .ticket(&task_ticket_id)
            .map_err(|error| error.to_string())?;
        classify_desktop_route(&ticket.request, None)
    };
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
            let request = storage
                .lock()
                .map_err(|_| "storage lock was poisoned".to_string())?
                .ticket(&task_ticket_id)
                .map_err(|error| error.to_string())?
                .request;
            run_executor_ticket(
                &storage,
                &skills,
                &attachments,
                &legacy,
                &executor,
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
        let guard = storage
            .lock()
            .map_err(|_| "storage lock was poisoned".to_string())?;
        let previous = guard
            .last_delivery_route(&ticket_id)
            .map_err(|error| error.to_string())?;
        classify_desktop_route(&request, previous.as_deref())
    };
    if route != DesktopRoute::Action {
        let skills = Arc::clone(&state.skills);
        let legacy = state.legacy.clone();
        let executor = state.executor.clone();
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

#[tauri::command]
pub async fn resolve_effect(
    ticket_id: String,
    effect_id: String,
    approved: bool,
    selected_paths: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<BootstrapSnapshot, String> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopRoute {
    Chat,
    CodeGenerate,
    ProjectRead,
    Action,
}

impl DesktopRoute {
    fn protocol_mode(self) -> ExecutorMode {
        match self {
            Self::Chat | Self::CodeGenerate => ExecutorMode::Chat,
            Self::ProjectRead => ExecutorMode::ProjectRead,
            Self::Action => ExecutorMode::Action,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::CodeGenerate => "code_generate",
            Self::ProjectRead => "project_read",
            Self::Action => "action",
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
            _ => {}
        }
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
    skills: &SkillRuntime,
    attachments: &Arc<Mutex<AttachmentStore>>,
    legacy: &LegacyAgentClient,
    executor: &ExecutorClient,
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
    provider::configure(&settings, legacy, None)?;
    let attachment_payloads = attachments
        .lock()
        .map_err(|_| "attachment store lock was poisoned".to_string())?
        .resolve(selection_id, &ticket.project_id)?;
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
    let request = ExecutorRequest {
        version: EXECUTOR_PROTOCOL_V1.into(),
        request_id: request_id.clone(),
        turn_id,
        thread_key: format!("desktop:ticket:{ticket_id}"),
        mode: route.protocol_mode(),
        assignment: None,
        mandate: None,
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
        if followup {
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
    append_runtime_event(
        storage,
        ticket_id,
        "provider.started",
        json!({ "provider": settings.model_provider }),
        app,
    );
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
                if !preserve_waiting {
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
        assert!(preserve_waiting_after_executor_complete(TicketState::Waiting));
        assert!(!preserve_waiting_after_executor_complete(TicketState::Running));
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
}
