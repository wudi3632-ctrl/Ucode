pub mod attachments;
mod commands;
pub mod domain;
pub mod legacy;
pub mod model_packs;
pub mod notifications;
pub mod portable;
pub mod provider;
pub mod race;
pub mod race_verifier;
pub mod race_workspace;
pub mod rescue;
pub mod rescue_elevated;
pub mod rescue_filesystem;
pub mod rescue_probe;
pub mod rescue_repair;
pub mod skills;
pub mod storage;
pub mod superagent;
pub mod supervisor;
pub mod terminal;

use attachments::AttachmentStore;
use commands::AppState;
use legacy::LegacyAgentClient;
use notifications::NotificationDiagnostics;
use portable::PortableRoot;
use skills::SkillRuntime;
use std::sync::{Arc, Mutex};
use storage::Storage;
use superagent::ExecutorClient;
use supervisor::CompatibilitySupervisor;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use terminal::TerminalManager;

#[cfg(any(not(debug_assertions), test))]
const BUNDLED_FRONTEND_URL: &str = "http://tauri.localhost/";

pub fn prime_integrity_cache() -> Result<(), String> {
    let portable = PortableRoot::discover().map_err(|error| error.to_string())?;
    let result = portable
        .prime_integrity_cache()
        .map_err(|error| error.to_string())?;
    if !matches!(result.as_str(), "verified" | "cached") {
        return Err(format!("unexpected integrity result: {result}"));
    }
    model_packs::verify_selected(portable.path(), model_packs::DEFAULT_MODEL_PROFILE)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("ucode-navigation-guard")
                .on_navigation(|_, url| is_internal_frontend_url(url))
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| {
                show_main_window(app);
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // A development build may have left 127.0.0.1:1420 in WebView2's
            // navigation history. Release builds must never depend on that
            // server, even when the cached browser session is restored.
            #[cfg(not(debug_assertions))]
            if let Some(window) = app.get_webview_window("main") {
                window.navigate(
                    BUNDLED_FRONTEND_URL
                        .parse()
                        .expect("valid bundled frontend URL"),
                )?;
            }
            let portable = PortableRoot::discover()?;
            let mut capsule_health = portable.inspect_capsule();
            let mut storage = Storage::open(&portable.data_dir().join("ucode.db"))?;
            storage.reconcile_portable_projects(&portable)?;
            let settings = storage.settings()?;
            if let Err(error) =
                model_packs::verify_selected(portable.path(), &settings.bundled_model_profile)
            {
                capsule_health.status = "blocked".into();
                capsule_health.can_start_runtime = false;
                capsule_health.side_effects_allowed = false;
                capsule_health
                    .issues
                    .push(format!("本地模型包未通过验证：{error}"));
            }
            let notification_diagnostics = NotificationDiagnostics::default();
            if settings.completion_notifications_enabled {
                // Notification identity is best-effort and must never block the
                // portable app or its task runtime.
                let _ =
                    notifications::register_identity(portable.path(), &notification_diagnostics);
            }
            let skills = Arc::new(SkillRuntime::new(portable.skills_dir())?);
            let rescue_repairs = Arc::new(rescue_repair::RepairService::new(
                portable.data_dir().join("evidence").join("repairs"),
            )?);
            let rescue_probes = Arc::new(rescue_probe::NetworkProbeService::new(
                portable.data_dir().join("evidence").join("probes"),
            )?);
            let legacy_url = std::env::var("UCODE_LEGACY_AGENT_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3082".into());
            let legacy = LegacyAgentClient::new(legacy_url)?;
            let executor_url = std::env::var("UCODE_EXECUTOR_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3082".into());
            let executor = ExecutorClient::new(executor_url)?;
            let supervisor = Arc::new(CompatibilitySupervisor::discover(
                portable.path(),
                settings.bundled_model_profile,
            ));
            if capsule_health.can_start_runtime {
                supervisor.ensure_started();
                supervisor.start_watchdog();
            }
            app.manage(AppState {
                portable,
                storage: Arc::new(Mutex::new(storage)),
                skills,
                legacy,
                executor,
                terminals: Arc::new(TerminalManager::default()),
                supervisor,
                attachments: Arc::new(Mutex::new(AttachmentStore::default())),
                capsule_health: Arc::new(Mutex::new(capsule_health)),
                rescue_repairs,
                rescue_probes,
            });
            app.manage(notification_diagnostics);
            let open_item = MenuItem::with_id(app, "open", "打开 UCode", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出 UCode", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &quit_item])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("UCode Portable")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "quit" => {
                        if let Some(state) = app.try_state::<AppState>() {
                            let _ = state.supervisor.stop_owned();
                        }
                        app.exit(0)
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::recheck_capsule,
            commands::runtime_diagnostics,
            commands::recover_local_runtime,
            commands::frontend_ready,
            commands::main_window_activity,
            commands::show_completion_notification,
            commands::completion_notification_diagnostics,
            commands::notification_identity_status,
            commands::repair_notification_identity,
            commands::clear_notification_identity,
            commands::restore_main_window,
            commands::hide_main_window,
            commands::choose_context_attachments,
            commands::remove_context_attachment,
            commands::open_external_url,
            commands::select_project,
            commands::create_onboarding_demo,
            commands::set_onboarding_welcome,
            commands::complete_onboarding_step,
            commands::skip_onboarding_step,
            commands::dismiss_onboarding_tip,
            commands::reset_onboarding,
            commands::create_ticket,
            commands::rename_ticket,
            commands::delete_ticket,
            commands::pin_ticket,
            commands::archive_ticket,
            commands::branch_ticket,
            commands::save_settings,
            commands::list_model_packs,
            commands::provider_secret_status,
            commands::save_provider_secret,
            commands::clear_provider_secret,
            commands::test_model_provider,
            commands::list_skills,
            commands::preview_skill,
            commands::install_skill,
            commands::set_skill_enabled,
            commands::uninstall_skill,
            commands::export_markdown,
            commands::list_events,
            commands::list_ticket_races,
            commands::start_candidate_race,
            commands::cancel_candidate_race,
            commands::run_ticket,
            commands::continue_ticket,
            commands::resolve_effect,
            commands::rollback_effect,
            commands::resolve_rescue_repair,
            commands::rollback_rescue_repair,
            commands::resolve_rescue_probe,
            commands::list_project_files,
            commands::read_project_file,
            commands::git_status,
            commands::git_details,
            commands::git_operation,
            commands::list_worktrees,
            commands::create_worktree,
            commands::remove_worktree,
            commands::list_subagents,
            commands::start_subagents,
            commands::cancel_subagents,
            commands::resolve_subagent_effect,
            commands::merge_subagent_change,
            commands::terminal_command,
            commands::terminal_start,
            commands::terminal_write,
            commands::terminal_read,
            commands::terminal_resize,
            commands::terminal_interrupt,
            commands::terminal_close,
            commands::cancel_ticket,
        ])
        .run(tauri::generate_context!())
        .expect("error while running UCode Portable");
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window
            .url()
            .map(|url| !is_internal_frontend_url(&url))
            .unwrap_or(true)
        {
            #[cfg(not(debug_assertions))]
            let _ = window.navigate(
                BUNDLED_FRONTEND_URL
                    .parse()
                    .expect("valid bundled frontend URL"),
            );
        }
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn is_internal_frontend_url(url: &tauri::Url) -> bool {
    if url.scheme() == "tauri"
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
    {
        return true;
    }
    cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420)
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_frontend_uses_tauri_custom_protocol_origin() {
        assert_eq!(super::BUNDLED_FRONTEND_URL, "http://tauri.localhost/");
        assert!(!super::BUNDLED_FRONTEND_URL.contains("127.0.0.1"));
    }

    #[test]
    fn navigation_guard_rejects_external_and_stale_development_urls() {
        assert!(super::is_internal_frontend_url(
            &"http://tauri.localhost/".parse().unwrap()
        ));
        assert!(!super::is_internal_frontend_url(
            &"https://example.com/".parse().unwrap()
        ));
        assert!(!super::is_internal_frontend_url(
            &"http://127.0.0.1/".parse().unwrap()
        ));
        if !cfg!(debug_assertions) {
            assert!(!super::is_internal_frontend_url(
                &"http://127.0.0.1:1420/".parse().unwrap()
            ));
        }
    }
}
