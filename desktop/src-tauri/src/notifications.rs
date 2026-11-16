use chrono::Utc;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::AppHandle;

pub const APP_USER_MODEL_ID: &str = "dev.ucode.portable";
pub const APP_DISPLAY_NAME: &str = "UCode";
pub const IDENTITY_REGISTRY_PATH: &str = r"SOFTWARE\Classes\AppUserModelId\dev.ucode.portable";
pub const PORTABLE_ICON_NAME: &str = "UCode.png";
pub const COMPLETION_TITLE: &str = "UCode · 任务完成";
pub const COMPLETION_BODY: &str = "任务已完成，点击返回 UCode";
const FILE_NOT_FOUND_HRESULT: i32 = 0x8007_0002u32 as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationIdentityHealth {
    Ready,
    Missing,
    Drifted,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationIdentityState {
    app_id: &'static str,
    display_name: &'static str,
    health: NotificationIdentityHealth,
    registered: bool,
    icon_uri: Option<String>,
    checked_at: String,
    last_error: Option<String>,
}

impl Default for NotificationIdentityState {
    fn default() -> Self {
        Self {
            app_id: APP_USER_MODEL_ID,
            display_name: APP_DISPLAY_NAME,
            health: NotificationIdentityHealth::Missing,
            registered: false,
            icon_uri: None,
            checked_at: now(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationTelemetry {
    attempts: u64,
    shown: u64,
    activated: u64,
    failed: u64,
    last_attempt_at: Option<String>,
    last_shown_at: Option<String>,
    last_activated_at: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct NotificationDiagnostics {
    identity: Arc<Mutex<NotificationIdentityState>>,
    telemetry: Arc<Mutex<NotificationTelemetry>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDiagnosticSnapshot {
    identity: NotificationIdentityState,
    telemetry: NotificationTelemetry,
}

impl NotificationDiagnostics {
    pub fn snapshot(&self) -> NotificationDiagnosticSnapshot {
        NotificationDiagnosticSnapshot {
            identity: self.identity_state(),
            telemetry: self
                .telemetry
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default(),
        }
    }

    pub fn identity_state(&self) -> NotificationIdentityState {
        self.identity
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| {
                identity_error_state(
                    NotificationIdentityHealth::Error,
                    false,
                    None,
                    "通知身份状态暂时不可用",
                )
            })
    }

    fn set_identity(&self, state: NotificationIdentityState) {
        if let Ok(mut current) = self.identity.lock() {
            *current = state;
        }
    }

    fn record_attempt(&self) {
        if let Ok(mut telemetry) = self.telemetry.lock() {
            telemetry.attempts += 1;
            telemetry.last_attempt_at = Some(now());
        }
    }

    fn record_shown(&self) {
        if let Ok(mut telemetry) = self.telemetry.lock() {
            telemetry.shown += 1;
            telemetry.last_shown_at = Some(now());
            telemetry.last_error = None;
        }
    }

    fn record_failed(&self, error: &str) {
        if let Ok(mut telemetry) = self.telemetry.lock() {
            telemetry.failed += 1;
            telemetry.last_error = Some(error.to_string());
        }
    }

    fn record_activated(&self) {
        if let Ok(mut telemetry) = self.telemetry.lock() {
            telemetry.activated += 1;
            telemetry.last_activated_at = Some(now());
        }
    }
}

#[derive(Debug)]
struct RegistryIdentityValues {
    display_name: Option<String>,
    icon_uri: Option<String>,
    icon_background_color: Option<String>,
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn identity_error_state(
    health: NotificationIdentityHealth,
    registered: bool,
    icon_uri: Option<String>,
    error: impl Into<String>,
) -> NotificationIdentityState {
    NotificationIdentityState {
        health,
        registered,
        icon_uri,
        checked_at: now(),
        last_error: Some(error.into()),
        ..NotificationIdentityState::default()
    }
}

fn classify_identity(
    values: Option<RegistryIdentityValues>,
    expected_icon: &Path,
    icon_exists: bool,
) -> NotificationIdentityState {
    let expected_icon = expected_icon.display().to_string();
    let Some(values) = values else {
        return NotificationIdentityState {
            icon_uri: Some(expected_icon),
            ..NotificationIdentityState::default()
        };
    };
    let values_match = values.display_name.as_deref() == Some(APP_DISPLAY_NAME)
        && values.icon_uri.as_deref() == Some(expected_icon.as_str())
        && values.icon_background_color.as_deref() == Some("0");
    if values_match && icon_exists {
        return NotificationIdentityState {
            health: NotificationIdentityHealth::Ready,
            registered: true,
            icon_uri: Some(expected_icon),
            checked_at: now(),
            last_error: None,
            ..NotificationIdentityState::default()
        };
    }
    let reason = if !icon_exists {
        "UCode.png 不存在或无法读取"
    } else {
        "Windows 中的 UCode 通知名称或图标路径已失效"
    };
    identity_error_state(
        NotificationIdentityHealth::Drifted,
        true,
        Some(expected_icon),
        reason,
    )
}

#[cfg(target_os = "windows")]
pub fn inspect_identity(
    portable_root: &Path,
    diagnostics: &NotificationDiagnostics,
) -> NotificationIdentityState {
    use windows_registry::CURRENT_USER;

    let icon_path = notification_icon_path(portable_root);
    let key = match CURRENT_USER.open(IDENTITY_REGISTRY_PATH) {
        Ok(key) => key,
        Err(error) if error.code().0 == FILE_NOT_FOUND_HRESULT => {
            let state = classify_identity(None, &icon_path, icon_path.is_file());
            diagnostics.set_identity(state.clone());
            return state;
        }
        Err(error) => {
            let state = identity_error_state(
                NotificationIdentityHealth::Error,
                false,
                Some(icon_path.display().to_string()),
                format!("无法读取 Windows 通知身份：{error}"),
            );
            diagnostics.set_identity(state.clone());
            return state;
        }
    };
    let values = RegistryIdentityValues {
        display_name: key.get_string("DisplayName").ok(),
        icon_uri: key.get_string("IconUri").ok(),
        icon_background_color: key.get_string("IconBackgroundColor").ok(),
    };
    let state = classify_identity(Some(values), &icon_path, icon_path.is_file());
    diagnostics.set_identity(state.clone());
    state
}

#[cfg(not(target_os = "windows"))]
pub fn inspect_identity(
    portable_root: &Path,
    diagnostics: &NotificationDiagnostics,
) -> NotificationIdentityState {
    let state = identity_error_state(
        NotificationIdentityHealth::Error,
        false,
        Some(notification_icon_path(portable_root).display().to_string()),
        "UCode 通知身份只支持 Windows",
    );
    diagnostics.set_identity(state.clone());
    state
}

#[cfg(target_os = "windows")]
pub fn register_identity(
    portable_root: &Path,
    diagnostics: &NotificationDiagnostics,
) -> Result<NotificationIdentityState, String> {
    use windows_registry::CURRENT_USER;

    let icon_path = notification_icon_path(portable_root);
    if !icon_path.is_file() {
        let message = format!("UCode 通知图标不存在：{}", icon_path.display());
        diagnostics.set_identity(identity_error_state(
            NotificationIdentityHealth::Drifted,
            false,
            Some(icon_path.display().to_string()),
            &message,
        ));
        return Err(message);
    }
    let result = (|| -> windows_registry::Result<()> {
        let key = CURRENT_USER.create(IDENTITY_REGISTRY_PATH)?;
        key.set_string("DisplayName", APP_DISPLAY_NAME)?;
        key.set_string("IconBackgroundColor", "0")?;
        key.set_hstring("IconUri", &icon_path.as_path().into())?;
        Ok(())
    })();
    if let Err(error) = result {
        let message = format!("无法注册 UCode 通知身份：{error}");
        diagnostics.set_identity(identity_error_state(
            NotificationIdentityHealth::Error,
            false,
            Some(icon_path.display().to_string()),
            &message,
        ));
        return Err(message);
    }
    let state = inspect_identity(portable_root, diagnostics);
    if state.health != NotificationIdentityHealth::Ready {
        return Err(state
            .last_error
            .clone()
            .unwrap_or_else(|| "UCode 通知身份校验失败".into()));
    }
    Ok(state)
}

#[cfg(not(target_os = "windows"))]
pub fn register_identity(
    portable_root: &Path,
    diagnostics: &NotificationDiagnostics,
) -> Result<NotificationIdentityState, String> {
    let state = inspect_identity(portable_root, diagnostics);
    Err(state.last_error.unwrap_or_else(|| "通知身份不可用".into()))
}

fn notification_icon_path(portable_root: &Path) -> PathBuf {
    let icon_path = portable_root.join(PORTABLE_ICON_NAME);
    let raw = icon_path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    icon_path
}

#[cfg(target_os = "windows")]
pub fn clear_identity(
    diagnostics: &NotificationDiagnostics,
) -> Result<NotificationIdentityState, String> {
    use windows_registry::CURRENT_USER;

    match CURRENT_USER.open(IDENTITY_REGISTRY_PATH) {
        Ok(_) => CURRENT_USER
            .remove_tree(IDENTITY_REGISTRY_PATH)
            .map_err(|error| format!("无法清除 UCode 通知身份：{error}"))?,
        Err(error) if error.code().0 == FILE_NOT_FOUND_HRESULT => {}
        Err(error) => return Err(format!("无法检查 UCode 通知身份：{error}")),
    }
    let state = NotificationIdentityState::default();
    diagnostics.set_identity(state.clone());
    Ok(state)
}

#[cfg(not(target_os = "windows"))]
pub fn clear_identity(
    diagnostics: &NotificationDiagnostics,
) -> Result<NotificationIdentityState, String> {
    let state = NotificationIdentityState::default();
    diagnostics.set_identity(state.clone());
    Ok(state)
}

#[cfg(target_os = "windows")]
pub fn show_completion(
    app: AppHandle,
    portable_root: PathBuf,
    diagnostics: NotificationDiagnostics,
) -> Result<(), String> {
    use notify_rust::{Notification, NotificationResponse};

    diagnostics.record_attempt();
    let identity = inspect_identity(&portable_root, &diagnostics);
    if identity.health != NotificationIdentityHealth::Ready {
        let error = identity
            .last_error
            .unwrap_or_else(|| "UCode 通知身份尚未就绪".into());
        diagnostics.record_failed(&error);
        return Err(error);
    }
    let handle = match Notification::new()
        .app_id(APP_USER_MODEL_ID)
        .summary(COMPLETION_TITLE)
        .body(COMPLETION_BODY)
        .show()
    {
        Ok(handle) => handle,
        Err(error) => {
            let error = format!("Windows 通知发送失败：{error}");
            diagnostics.record_failed(&error);
            return Err(error);
        }
    };
    diagnostics.record_shown();
    std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &NotificationResponse| {
            if response.is_default_action() {
                diagnostics.record_activated();
                crate::show_main_window(&app);
            }
        });
    });
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn show_completion(
    _app: AppHandle,
    _portable_root: PathBuf,
    diagnostics: NotificationDiagnostics,
) -> Result<(), String> {
    diagnostics.record_attempt();
    let error = "完成通知只支持 Windows".to_string();
    diagnostics.record_failed(&error);
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matching_values(icon: &Path) -> RegistryIdentityValues {
        RegistryIdentityValues {
            display_name: Some(APP_DISPLAY_NAME.into()),
            icon_uri: Some(icon.display().to_string()),
            icon_background_color: Some("0".into()),
        }
    }

    #[test]
    fn notification_identity_is_fixed_and_never_powershell() {
        assert_eq!(APP_USER_MODEL_ID, "dev.ucode.portable");
        assert_eq!(APP_DISPLAY_NAME, "UCode");
        assert_eq!(
            IDENTITY_REGISTRY_PATH,
            r"SOFTWARE\Classes\AppUserModelId\dev.ucode.portable"
        );
        assert!(!APP_USER_MODEL_ID
            .to_ascii_lowercase()
            .contains("powershell"));
    }

    #[test]
    fn identity_classification_is_truthful() {
        let icon = Path::new(r"E:\UCode\UCode.png");
        assert_eq!(
            classify_identity(None, icon, true).health,
            NotificationIdentityHealth::Missing
        );
        assert_eq!(
            classify_identity(Some(matching_values(icon)), icon, true).health,
            NotificationIdentityHealth::Ready
        );
        assert_eq!(
            classify_identity(Some(matching_values(icon)), icon, false).health,
            NotificationIdentityHealth::Drifted
        );
        let mut wrong = matching_values(icon);
        wrong.display_name = Some("Windows PowerShell".into());
        assert_eq!(
            classify_identity(Some(wrong), icon, true).health,
            NotificationIdentityHealth::Drifted
        );
    }

    #[test]
    fn icon_uri_uses_a_shell_compatible_absolute_path() {
        assert_eq!(
            notification_icon_path(Path::new(r"\\?\E:\UCode")),
            PathBuf::from(r"E:\UCode\UCode.png")
        );
        assert_eq!(
            notification_icon_path(Path::new(r"\\?\UNC\server\share\UCode")),
            PathBuf::from(r"\\server\share\UCode\UCode.png")
        );
    }

    #[test]
    fn diagnostics_count_failures_without_task_content() {
        let diagnostics = NotificationDiagnostics::default();
        diagnostics.record_attempt();
        diagnostics.record_failed("identity unavailable");
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.telemetry.attempts, 1);
        assert_eq!(snapshot.telemetry.failed, 1);
        assert_eq!(snapshot.telemetry.shown, 0);
        assert_eq!(
            snapshot.telemetry.last_error.as_deref(),
            Some("identity unavailable")
        );
    }

    #[test]
    fn completion_copy_is_fixed_and_private() {
        assert_eq!(COMPLETION_TITLE, "UCode · 任务完成");
        assert_eq!(COMPLETION_BODY, "任务已完成，点击返回 UCode");
        let text = format!("{COMPLETION_TITLE}{COMPLETION_BODY}").to_ascii_lowercase();
        for secret in ["project", "path", "prompt", "ticket"] {
            assert!(!text.contains(secret));
        }
    }
}
