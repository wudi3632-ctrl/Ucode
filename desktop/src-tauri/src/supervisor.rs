use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CONTROLLER_WARMUP_SECONDS: i64 = 420;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    pub id: String,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub health_url: String,
    pub startup_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeServiceDiagnostic {
    pub id: String,
    pub label: String,
    pub status: ServiceStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOverallStatus {
    Ready,
    Starting,
    Degraded,
    Stopped,
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnosticSnapshot {
    pub overall: RuntimeOverallStatus,
    pub services: Vec<RuntimeServiceDiagnostic>,
    pub runtime_root: Option<String>,
    pub owned: bool,
    pub starting: bool,
    pub restart_attempts: u8,
    pub model_profile: String,
    pub checked_at: String,
    pub last_error: Option<String>,
}

pub trait ProcessSupervisor: Send + Sync {
    fn status(&self, service_id: &str) -> ServiceStatus;
    fn start(&self, spec: &ServiceSpec) -> Result<(), String>;
    fn stop_tree(&self, service_id: &str) -> Result<(), String>;
}

/// Owns a compatibility runtime started by this capsule, including one safely
/// adopted from the capsule's persisted runtime state after a desktop crash.
#[derive(Clone)]
pub struct CompatibilitySupervisor {
    portable_root: PathBuf,
    runtime_root: Option<PathBuf>,
    logs_root: PathBuf,
    runtime_state_path: PathBuf,
    starting: Arc<AtomicBool>,
    owned: Arc<AtomicBool>,
    monitor_started: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    controller_cleanup_attempted: Arc<AtomicBool>,
    restart_attempts: Arc<AtomicU8>,
    model_profile: Arc<Mutex<String>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl CompatibilitySupervisor {
    pub fn discover(portable_root: &Path, model_profile: impl Into<String>) -> Self {
        Self {
            portable_root: portable_root.to_path_buf(),
            runtime_root: discover_runtime_root(portable_root),
            logs_root: portable_root.join("logs"),
            runtime_state_path: portable_root.join("data/runtime.json"),
            starting: Arc::new(AtomicBool::new(false)),
            owned: Arc::new(AtomicBool::new(false)),
            monitor_started: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            controller_cleanup_attempted: Arc::new(AtomicBool::new(false)),
            restart_attempts: Arc::new(AtomicU8::new(0)),
            model_profile: Arc::new(Mutex::new(model_profile.into())),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn runtime_root(&self) -> Option<&Path> {
        self.runtime_root.as_deref()
    }

    pub fn is_owned(&self) -> bool {
        self.owned.load(Ordering::SeqCst)
    }

    pub fn ensure_started(&self) {
        self.stop_requested.store(false, Ordering::SeqCst);
        let agent_status = self.status("agent");
        let adoptable = agent_status == ServiceStatus::Ready && self.can_adopt_capsule_runtime();
        let requires_refresh = agent_status == ServiceStatus::Ready
            && !adoptable
            && self.runtime_state_requires_refresh();
        self.trace_lifecycle(
            "ensure_started",
            &format!(
                "agent={agent_status:?} adoptable={adoptable} requires_refresh={requires_refresh}"
            ),
        );
        if agent_status == ServiceStatus::Ready {
            if adoptable {
                self.owned.store(true, Ordering::SeqCst);
                self.set_last_error(None);
            } else if requires_refresh {
                if self.starting.swap(true, Ordering::SeqCst) {
                    return;
                }
                let supervisor = self.clone();
                thread::spawn(move || {
                    let result = supervisor.refresh_stale_runtime();
                    let ready = result.is_ok();
                    supervisor.owned.store(ready, Ordering::SeqCst);
                    supervisor.set_last_error(
                        result.err().map(|error| {
                            format!("升级后本地 Agent 未能刷新到当前胶囊版本：{error}")
                        }),
                    );
                    supervisor.starting.store(false, Ordering::SeqCst);
                });
            } else {
                self.owned.store(false, Ordering::SeqCst);
                self.set_last_error(Some(
                    "端口 3082 已被另一份 UCode 运行时占用；当前胶囊拒绝连接到外部 Agent。请关闭旧 UCode 后重试。"
                        .into(),
                ));
            }
            return;
        }
        if self.starting.swap(true, Ordering::SeqCst) {
            return;
        }
        let supervisor = self.clone();
        thread::spawn(move || {
            let result = supervisor.start_current_capsule_runtime();
            if result.is_ok() {
                supervisor.owned.store(true, Ordering::SeqCst);
                supervisor.set_last_error(None);
            } else if let Err(error) = result {
                supervisor.set_last_error(Some(error));
            }
            supervisor.starting.store(false, Ordering::SeqCst);
        });
    }

    pub fn diagnostics(&self) -> RuntimeDiagnosticSnapshot {
        const SERVICES: [(&str, &str); 4] = [
            ("agent", "Agent"),
            ("controller", "Controller"),
            ("coder", "Coder"),
            ("llama", "llama runtime"),
        ];
        let services = thread::scope(|scope| {
            let handles = SERVICES.map(|(id, label)| {
                scope.spawn(move || RuntimeServiceDiagnostic {
                    id: id.into(),
                    label: label.into(),
                    status: self.status(id),
                })
            });
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| RuntimeServiceDiagnostic {
                        id: "unknown".into(),
                        label: "Unknown service".into(),
                        status: ServiceStatus::Failed,
                    })
                })
                .collect::<Vec<_>>()
        });
        let starting = self.starting.load(Ordering::SeqCst);
        RuntimeDiagnosticSnapshot {
            overall: classify_runtime(self.runtime_root.is_some(), starting, &services),
            services,
            runtime_root: self
                .runtime_root
                .as_deref()
                .map(powershell_path)
                .map(|path| path.display().to_string()),
            owned: self.owned.load(Ordering::SeqCst),
            starting,
            restart_attempts: self.restart_attempts.load(Ordering::SeqCst),
            model_profile: self
                .model_profile
                .lock()
                .map(|value| value.clone())
                .unwrap_or_else(|_| "unknown".into()),
            checked_at: chrono::Utc::now().to_rfc3339(),
            last_error: self.last_error.lock().ok().and_then(|value| value.clone()),
        }
    }

    pub fn recover(&self) -> Result<RuntimeDiagnosticSnapshot, String> {
        if self.runtime_root.is_none() {
            let error = "本地运行时包不在 UCode 旁边，无法启动。".to_string();
            self.set_last_error(Some(error.clone()));
            return Err(error);
        }
        let before = self.diagnostics();
        if core_services_ready(&before.services) {
            if !self.can_adopt_capsule_runtime() {
                if self.runtime_state_requires_refresh() {
                    if self.starting.swap(true, Ordering::SeqCst) {
                        return Err("本地模型服务已经在刷新，请稍候。".into());
                    }
                    let result = self.refresh_stale_runtime();
                    let ready = result.is_ok();
                    self.starting.store(false, Ordering::SeqCst);
                    self.owned.store(ready, Ordering::SeqCst);
                    if ready {
                        self.set_last_error(None);
                        return Ok(self.diagnostics());
                    }
                    let error = format!(
                        "升级后本地 Agent 未能刷新到当前胶囊版本：{}",
                        result.err().unwrap_or_else(|| "未知错误".into())
                    );
                    self.set_last_error(Some(error.clone()));
                    return Err(error);
                }
                return Err("检测到不属于当前胶囊的本地 Agent，已拒绝连接。".into());
            }
            self.owned.store(true, Ordering::SeqCst);
            let controller = before
                .services
                .iter()
                .find(|service| service.id == "controller")
                .map(|service| service.status.clone())
                .unwrap_or(ServiceStatus::Stopped);
            if controller == ServiceStatus::Ready || controller == ServiceStatus::Starting {
                self.set_last_error(None);
                return Ok(before);
            }
            self.restart_controller()?;
            self.controller_cleanup_attempted
                .store(false, Ordering::SeqCst);
            self.restart_attempts.store(0, Ordering::SeqCst);
            self.set_last_error(None);
            return Ok(self.diagnostics());
        }
        if self.starting.swap(true, Ordering::SeqCst) {
            return Err("本地模型服务已经在启动，请稍候。".into());
        }
        self.stop_requested.store(false, Ordering::SeqCst);
        let result = (|| {
            if self.owned.load(Ordering::SeqCst) {
                self.stop_compatibility_runtime()?;
                self.owned.store(false, Ordering::SeqCst);
            }
            self.restart_attempts.store(0, Ordering::SeqCst);
            self.start_compatibility_runtime()?;
            let health = self.diagnostics();
            if core_services_ready(&health.services) {
                self.owned.store(true, Ordering::SeqCst);
                Ok(())
            } else {
                Err("本地运行时启动完成，但 Agent 或 Coder 仍未通过健康检查。".to_string())
            }
        })();
        self.starting.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => {
                self.set_last_error(None);
                Ok(self.diagnostics())
            }
            Err(error) => {
                self.set_last_error(Some(error.clone()));
                Err(error)
            }
        }
    }

    /// Applies an already verified stable model profile and restarts only the
    /// runtime owned by this capsule. A failed switch attempts to restore the
    /// previous profile so settings never claim an unobserved model is active.
    pub fn restart_with_profile(
        &self,
        profile_id: &str,
    ) -> Result<RuntimeDiagnosticSnapshot, String> {
        if self.starting.swap(true, Ordering::SeqCst) {
            return Err("本地模型服务已经在启动，请稍候。".into());
        }
        self.stop_requested.store(false, Ordering::SeqCst);
        if !self.owned.load(Ordering::SeqCst)
            && self.status("agent") == ServiceStatus::Ready
            && self.can_adopt_capsule_runtime()
        {
            self.owned.store(true, Ordering::SeqCst);
        }
        if !self.owned.load(Ordering::SeqCst)
            && self
                .diagnostics()
                .services
                .iter()
                .any(|service| service.status == ServiceStatus::Ready)
        {
            self.starting.store(false, Ordering::SeqCst);
            return Err("检测到不属于当前胶囊的本地服务，未自动停止或替换。".into());
        }
        let previous = self
            .model_profile
            .lock()
            .map_err(|_| "模型档位状态不可用".to_string())?
            .clone();
        let had_owned_runtime = self.owned.load(Ordering::SeqCst);
        let switched: Result<(), String> = (|| {
            if had_owned_runtime {
                self.stop_compatibility_runtime()?;
                self.owned.store(false, Ordering::SeqCst);
            }
            *self
                .model_profile
                .lock()
                .map_err(|_| "模型档位状态不可用".to_string())? = profile_id.to_string();
            self.start_compatibility_runtime()?;
            if self.status("agent") != ServiceStatus::Ready {
                return Err("新模型运行时启动后未通过健康检查。".into());
            }
            self.owned.store(true, Ordering::SeqCst);
            Ok(())
        })();
        let result = match switched {
            Ok(()) => {
                self.restart_attempts.store(0, Ordering::SeqCst);
                self.set_last_error(None);
                Ok(self.diagnostics())
            }
            Err(switch_error) => {
                *self
                    .model_profile
                    .lock()
                    .map_err(|_| "模型档位状态不可用".to_string())? = previous;
                let restored = self.start_compatibility_runtime().is_ok()
                    && self.status("agent") == ServiceStatus::Ready;
                self.owned.store(restored, Ordering::SeqCst);
                let error = if restored {
                    format!("模型切换失败，已恢复原档位：{switch_error}")
                } else {
                    format!("模型切换失败，原档位也未能恢复：{switch_error}")
                };
                self.set_last_error(Some(error.clone()));
                Err(error)
            }
        };
        self.starting.store(false, Ordering::SeqCst);
        result
    }

    fn set_last_error(&self, error: Option<String>) {
        if let Ok(mut value) = self.last_error.lock() {
            *value = error;
        }
    }

    fn trace_lifecycle(&self, event: &str, detail: &str) {
        if fs::create_dir_all(&self.logs_root).is_err() {
            return;
        }
        let path = self.logs_root.join("supervisor-lifecycle.log");
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{} {event} {detail}", chrono::Utc::now().to_rfc3339());
        }
    }

    fn can_adopt_capsule_runtime(&self) -> bool {
        let Ok(content) = fs::read(&self.runtime_state_path) else {
            return false;
        };
        let Ok(state) = serde_json::from_slice::<RuntimeState>(json_without_utf8_bom(&content))
        else {
            return false;
        };
        runtime_state_is_adoptable(
            &state,
            self.status("agent") == ServiceStatus::Ready,
            self.status("coder") == ServiceStatus::Ready,
            self.agent_capsule_matches(&state),
        )
    }

    fn runtime_state_requires_refresh(&self) -> bool {
        let Ok(content) = fs::read(&self.runtime_state_path) else {
            return false;
        };
        let Ok(state) =
            serde_json::from_slice::<RuntimeAgentState>(json_without_utf8_bom(&content))
        else {
            return false;
        };
        self.portable_root.join("ucode-manifest.json").is_file()
            && state.agent_pid > 0
            && !self.agent_capsule_matches_pid(state.agent_pid)
    }

    fn start_current_capsule_runtime(&self) -> Result<(), String> {
        self.start_compatibility_runtime()?;
        if self.status("agent") != ServiceStatus::Ready {
            return Err("本地 Agent 启动后未通过健康检查。".into());
        }
        if self.can_adopt_capsule_runtime() {
            return Ok(());
        }
        if self.runtime_state_requires_refresh() {
            return self.refresh_stale_runtime();
        }
        Err("端口 3082 上的 Agent 不属于当前胶囊，已拒绝收养。".into())
    }

    fn refresh_stale_runtime(&self) -> Result<(), String> {
        self.trace_lifecycle("refresh_stale_runtime", "stopping old capsule runtime");
        self.stop_compatibility_runtime()
            .map_err(|error| format!("停止旧 Agent 失败：{error}"))?;
        self.trace_lifecycle("refresh_stale_runtime", "starting current capsule runtime");
        self.start_compatibility_runtime()
            .map_err(|error| format!("启动当前 Agent 失败：{error}"))?;
        if self.status("agent") != ServiceStatus::Ready {
            return Err("当前 Agent 启动后未通过健康检查。".into());
        }
        if !self.can_adopt_capsule_runtime() {
            return Err("当前 Agent 的胶囊身份仍未通过校验。".into());
        }
        self.trace_lifecycle("refresh_stale_runtime", "current capsule runtime adopted");
        Ok(())
    }

    fn agent_capsule_matches(&self, state: &RuntimeState) -> bool {
        self.agent_capsule_matches_pid(state.agent_pid)
    }

    fn agent_capsule_matches_pid(&self, agent_pid: u32) -> bool {
        let manifest = self.portable_root.join("ucode-manifest.json");
        if !manifest.is_file() {
            return true;
        }
        let Some(expected) = sha256_file(&manifest) else {
            return false;
        };
        let identity_path = self.portable_root.join("data/agent-runtime.json");
        let Ok(content) = fs::read(identity_path) else {
            return false;
        };
        let Ok(identity) = serde_json::from_slice::<AgentRuntimeIdentity>(&content) else {
            return false;
        };
        identity.pid == agent_pid && identity.capsule_digest.as_deref() == Some(&expected)
    }

    fn controller_is_warming_up(&self) -> bool {
        let Ok(content) = fs::read(&self.runtime_state_path) else {
            return false;
        };
        let Ok(state) = serde_json::from_slice::<RuntimeState>(json_without_utf8_bom(&content))
        else {
            return false;
        };
        let Some(started_at) = state.controller_started_at else {
            return false;
        };
        let elapsed = chrono::Utc::now()
            .signed_duration_since(started_at)
            .num_seconds();
        state.model_pid > 0 && (-30..=CONTROLLER_WARMUP_SECONDS).contains(&elapsed)
    }

    fn controller_has_expired(&self) -> bool {
        let Ok(content) = fs::read(&self.runtime_state_path) else {
            return false;
        };
        let Ok(state) = serde_json::from_slice::<RuntimeState>(json_without_utf8_bom(&content))
        else {
            return false;
        };
        let Some(started_at) = state.controller_started_at else {
            return false;
        };
        state.model_pid > 0
            && chrono::Utc::now()
                .signed_duration_since(started_at)
                .num_seconds()
                > CONTROLLER_WARMUP_SECONDS
    }

    /// Watches the runtime owned by this desktop instance. Two consecutive
    /// failed health checks trigger one compatibility-script restart. A single
    /// retry avoids an endless crash loop on low-memory portable machines.
    pub fn start_watchdog(&self) {
        if self.monitor_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let supervisor = self.clone();
        thread::spawn(move || {
            let mut consecutive_failures = 0_u8;
            loop {
                thread::sleep(Duration::from_secs(2));
                if supervisor.stop_requested.load(Ordering::SeqCst) {
                    break;
                }
                if !supervisor.owned.load(Ordering::SeqCst) {
                    // Agent may become ready before Controller/Coder/llama on
                    // a cold USB start. Retry safe adoption after all persisted
                    // capsule services converge instead of permanently losing
                    // watchdog ownership because of that brief race.
                    if watchdog_may_adopt(
                        supervisor.owned.load(Ordering::SeqCst),
                        supervisor.starting.load(Ordering::SeqCst),
                        supervisor.status("agent") == ServiceStatus::Ready,
                    ) {
                        if supervisor.can_adopt_capsule_runtime() {
                            supervisor.owned.store(true, Ordering::SeqCst);
                            supervisor.restart_attempts.store(0, Ordering::SeqCst);
                            supervisor.set_last_error(None);
                        } else if supervisor.runtime_state_requires_refresh()
                            && !supervisor.starting.swap(true, Ordering::SeqCst)
                        {
                            let result = supervisor.refresh_stale_runtime();
                            let ready = result.is_ok();
                            supervisor.owned.store(ready, Ordering::SeqCst);
                            supervisor.set_last_error(result.err().map(|error| {
                                format!("升级后本地 Agent 未能刷新到当前胶囊版本：{error}")
                            }));
                            supervisor.starting.store(false, Ordering::SeqCst);
                        }
                    }
                    consecutive_failures = 0;
                    continue;
                }
                if supervisor.starting.load(Ordering::SeqCst) {
                    consecutive_failures = 0;
                    continue;
                }
                if supervisor.status("controller") != ServiceStatus::Ready
                    && supervisor.controller_has_expired()
                    && !supervisor
                        .controller_cleanup_attempted
                        .swap(true, Ordering::SeqCst)
                {
                    match supervisor.stop_controller_after_timeout() {
                        Ok(()) => supervisor.set_last_error(Some(
                            "复杂规划模型启动超时，已仅停止 Controller；普通对话和明确文件任务仍可继续。"
                                .into(),
                        )),
                        Err(error) => supervisor.set_last_error(Some(format!(
                            "复杂规划模型启动超时，但未能安全停止：{error}"
                        ))),
                    }
                }
                if supervisor.status("agent") == ServiceStatus::Ready {
                    consecutive_failures = 0;
                    continue;
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
                if consecutive_failures < 2
                    || !watchdog_may_restart(
                        supervisor.owned.load(Ordering::SeqCst),
                        supervisor.stop_requested.load(Ordering::SeqCst),
                        supervisor.starting.load(Ordering::SeqCst),
                        supervisor.restart_attempts.load(Ordering::SeqCst),
                    )
                {
                    continue;
                }
                if supervisor.starting.swap(true, Ordering::SeqCst) {
                    continue;
                }
                supervisor.restart_attempts.fetch_add(1, Ordering::SeqCst);
                // The compatibility script launches all four services. Stop
                // the surviving processes from this owned runtime first so a
                // single Agent crash cannot duplicate Controller/Coder/llama.
                let _ = supervisor.stop_compatibility_runtime();
                let restarted = supervisor.start_compatibility_runtime().is_ok()
                    && supervisor.status("agent") == ServiceStatus::Ready;
                supervisor.owned.store(restarted, Ordering::SeqCst);
                if restarted {
                    supervisor.set_last_error(None);
                } else {
                    supervisor
                        .set_last_error(Some("自动恢复后，本地 Agent 仍未通过健康检查。".into()));
                }
                supervisor.starting.store(false, Ordering::SeqCst);
                consecutive_failures = 0;
            }
            supervisor.monitor_started.store(false, Ordering::SeqCst);
        });
    }

    pub fn stop_owned(&self) -> Result<(), String> {
        self.stop_requested.store(true, Ordering::SeqCst);
        if !self.owned.load(Ordering::SeqCst) && !self.starting.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.stop_compatibility_runtime()?;
        self.owned.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn stop_compatibility_runtime(&self) -> Result<(), String> {
        let root = self
            .runtime_root
            .as_ref()
            .ok_or_else(|| "local runtime root is unavailable".to_string())?;
        let script = root.join("Stop-PocketMind.ps1");
        if !script.is_file() {
            return Err(format!(
                "runtime stop script is missing: {}",
                script.display()
            ));
        }
        let script = powershell_path(&script);
        let status = hidden_powershell()
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .status()
            .map_err(|error| format!("failed to stop local runtime: {error}"))?;
        if !status.success() {
            return Err(format!("runtime stop script exited with {status}"));
        }
        Ok(())
    }

    fn start_compatibility_runtime(&self) -> Result<(), String> {
        let root = self
            .runtime_root
            .as_ref()
            .ok_or_else(|| "local runtime pack was not found beside UCode".to_string())?;
        let script = root.join("Start-PocketMind-Research.ps1");
        if !script.is_file() {
            return Err(format!(
                "runtime start script is missing: {}",
                script.display()
            ));
        }
        let script = powershell_path(&script);
        let working_root = powershell_path(root);
        let profile_id = self
            .model_profile
            .lock()
            .map_err(|_| "model profile state is unavailable".to_string())?
            .clone();
        crate::model_packs::verify_selected(&self.portable_root, &profile_id)?;
        let logs = &self.logs_root;
        fs::create_dir_all(&logs).map_err(|error| error.to_string())?;
        let stdout =
            File::create(logs.join("desktop-supervisor.log")).map_err(|error| error.to_string())?;
        let stderr = File::create(logs.join("desktop-supervisor.error.log"))
            .map_err(|error| error.to_string())?;
        let status = hidden_powershell()
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .arg("-Device")
            .arg("cpu")
            .arg("-NoBrowser")
            .arg("-ModelProfile")
            .arg(&profile_id)
            .current_dir(&working_root)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .status()
            .map_err(|error| format!("failed to start local runtime: {error}"))?;
        let result = status
            .success()
            .then_some(())
            .ok_or_else(|| format!("runtime start script exited with {status}"));
        if result.is_ok() {
            self.controller_cleanup_attempted
                .store(false, Ordering::SeqCst);
        }
        result
    }

    fn restart_controller(&self) -> Result<(), String> {
        self.run_controller_script("Restart-PocketMind-Controller.ps1", &[])
    }

    fn stop_controller_after_timeout(&self) -> Result<(), String> {
        self.run_controller_script(
            "Stop-PocketMind-Controller.ps1",
            &[
                "-Reason",
                "Controller exceeded its bounded 420-second startup budget.",
            ],
        )
    }

    fn run_controller_script(&self, script_name: &str, arguments: &[&str]) -> Result<(), String> {
        let root = self
            .runtime_root
            .as_ref()
            .ok_or_else(|| "local runtime root is unavailable".to_string())?;
        let script = root.join(script_name);
        if !script.is_file() {
            return Err(format!(
                "runtime Controller script is missing: {}",
                script.display()
            ));
        }
        let script = powershell_path(&script);
        let status = hidden_powershell()
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .args(arguments)
            .current_dir(powershell_path(root))
            .status()
            .map_err(|error| format!("failed to run {script_name}: {error}"))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("{script_name} exited with {status}"))
    }
}

fn classify_runtime(
    runtime_present: bool,
    starting: bool,
    services: &[RuntimeServiceDiagnostic],
) -> RuntimeOverallStatus {
    if !runtime_present {
        return RuntimeOverallStatus::Missing;
    }
    if !services.is_empty()
        && services
            .iter()
            .all(|service| service.status == ServiceStatus::Ready)
    {
        return RuntimeOverallStatus::Ready;
    }
    if starting
        || services
            .iter()
            .any(|service| service.status == ServiceStatus::Starting)
    {
        return RuntimeOverallStatus::Starting;
    }
    if services
        .iter()
        .any(|service| service.status == ServiceStatus::Ready)
    {
        RuntimeOverallStatus::Degraded
    } else {
        RuntimeOverallStatus::Stopped
    }
}

fn core_services_ready(services: &[RuntimeServiceDiagnostic]) -> bool {
    ["agent", "coder"].into_iter().all(|required| {
        services
            .iter()
            .any(|service| service.id == required && service.status == ServiceStatus::Ready)
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeState {
    model_pid: u32,
    coder_pid: u32,
    llama_pid: Option<u32>,
    agent_pid: u32,
    #[serde(default)]
    controller_started_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeIdentity {
    pid: u32,
    capsule_digest: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeAgentState {
    agent_pid: u32,
}

fn runtime_state_is_adoptable(
    state: &RuntimeState,
    agent_ready: bool,
    coder_ready: bool,
    capsule_matches: bool,
) -> bool {
    state.agent_pid > 0
        && state.coder_pid > 0
        && state.llama_pid.is_none_or(|pid| pid > 0)
        && agent_ready
        && coder_ready
        && capsule_matches
}

fn sha256_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn json_without_utf8_bom(content: &[u8]) -> &[u8] {
    content.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(content)
}

fn watchdog_may_restart(owned: bool, stop_requested: bool, starting: bool, attempts: u8) -> bool {
    owned && !stop_requested && !starting && attempts < 1
}

fn watchdog_may_adopt(owned: bool, starting: bool, agent_ready: bool) -> bool {
    !owned && !starting && agent_ready
}

impl ProcessSupervisor for CompatibilitySupervisor {
    fn status(&self, service_id: &str) -> ServiceStatus {
        let url = match service_id {
            "controller" => "http://127.0.0.1:3083/health",
            "coder" => "http://127.0.0.1:3284/health",
            "llama" => "http://127.0.0.1:3285/health",
            _ => "http://127.0.0.1:3082/ready",
        };
        if health_ready(url) {
            ServiceStatus::Ready
        } else if service_id == "controller" && self.controller_is_warming_up() {
            ServiceStatus::Starting
        } else if self.starting.load(Ordering::SeqCst) {
            ServiceStatus::Starting
        } else {
            ServiceStatus::Stopped
        }
    }

    fn start(&self, _spec: &ServiceSpec) -> Result<(), String> {
        self.ensure_started();
        Ok(())
    }

    fn stop_tree(&self, _service_id: &str) -> Result<(), String> {
        self.stop_owned()
    }
}

fn discover_runtime_root(portable_root: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("UCODE_RUNTIME_ROOT") {
        candidates.push(PathBuf::from(value));
    }
    candidates.push(portable_root.join("runtime-pack"));
    if let Some(parent) = portable_root.parent() {
        candidates.push(parent.join("PocketMind-Research-CPU-Windows11"));
    }
    portable_root
        .ancestors()
        .for_each(|parent| candidates.push(parent.to_path_buf()));
    candidates.into_iter().find(|candidate| {
        candidate.join("Start-PocketMind-Research.ps1").is_file()
            && candidate.join("runtime/node/node.exe").is_file()
            && candidate.join("runtime/python/python.exe").is_file()
    })
}

fn health_ready(url: &str) -> bool {
    reqwest::blocking::Client::builder()
        // A first loopback request can exceed 700 ms while the bundled CPU
        // models are faulting pages in from a portable drive. Keep the probe
        // bounded, but do not misclassify a healthy runtime as permanently
        // starting on slower Windows machines.
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|client| client.get(url).send())
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// PowerShell 5.1 does not populate `$PSScriptRoot` when `-File` receives an
/// extended-length `\\?\` path. Tauri canonicalizes the portable root into
/// that form on Windows, so convert only the shell boundary back to a regular
/// Win32 path while keeping canonical paths internally.
fn powershell_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

fn hidden_powershell() -> Command {
    let mut command = Command::new("powershell.exe");
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discovers_a_sibling_runtime_pack_without_absolute_paths() {
        let root = tempdir().unwrap();
        let portable = root.path().join("UCode-Portable-Prototype");
        let runtime = root.path().join("PocketMind-Research-CPU-Windows11");
        fs::create_dir_all(&portable).unwrap();
        fs::create_dir_all(runtime.join("runtime/node")).unwrap();
        fs::create_dir_all(runtime.join("runtime/python")).unwrap();
        fs::write(runtime.join("Start-PocketMind-Research.ps1"), "").unwrap();
        fs::write(runtime.join("runtime/node/node.exe"), "").unwrap();
        fs::write(runtime.join("runtime/python/python.exe"), "").unwrap();
        assert_eq!(discover_runtime_root(&portable), Some(runtime));
        let supervisor =
            CompatibilitySupervisor::discover(&portable, crate::model_packs::DEFAULT_MODEL_PROFILE);
        assert_eq!(supervisor.logs_root, portable.join("logs"));
    }

    #[test]
    fn converts_extended_windows_paths_before_invoking_powershell() {
        let extended = PathBuf::from(r"\\?\E:\windows-export\Start-PocketMind-Research.ps1");
        assert_eq!(
            powershell_path(&extended),
            PathBuf::from(r"E:\windows-export\Start-PocketMind-Research.ps1")
        );
        let extended_unc = PathBuf::from(r"\\?\UNC\server\share\Start.ps1");
        assert_eq!(
            powershell_path(&extended_unc),
            PathBuf::from(r"\\server\share\Start.ps1")
        );
    }

    #[test]
    fn watchdog_restarts_owned_runtime_once_but_never_after_exit() {
        assert!(watchdog_may_restart(true, false, false, 0));
        assert!(!watchdog_may_restart(true, false, false, 1));
        assert!(!watchdog_may_restart(true, true, false, 0));
        assert!(!watchdog_may_restart(false, false, false, 0));
        assert!(!watchdog_may_restart(true, false, true, 0));
        assert!(watchdog_may_adopt(false, false, true));
        assert!(!watchdog_may_adopt(true, false, true));
        assert!(!watchdog_may_adopt(false, true, true));
        assert!(!watchdog_may_adopt(false, false, false));
    }

    #[test]
    fn runtime_diagnostics_distinguish_ready_degraded_stopped_and_missing() {
        let service = |id: &str, status| RuntimeServiceDiagnostic {
            id: id.into(),
            label: id.into(),
            status,
        };
        let ready = vec![
            service("agent", ServiceStatus::Ready),
            service("controller", ServiceStatus::Ready),
        ];
        assert_eq!(
            classify_runtime(true, false, &ready),
            RuntimeOverallStatus::Ready
        );
        assert_eq!(
            classify_runtime(
                true,
                false,
                &[
                    service("agent", ServiceStatus::Ready),
                    service("coder", ServiceStatus::Stopped),
                ],
            ),
            RuntimeOverallStatus::Degraded
        );
        assert_eq!(
            classify_runtime(
                true,
                false,
                &[
                    service("agent", ServiceStatus::Ready),
                    service("controller", ServiceStatus::Starting),
                    service("coder", ServiceStatus::Ready),
                ],
            ),
            RuntimeOverallStatus::Starting
        );
        assert_eq!(
            classify_runtime(
                true,
                false,
                &[
                    service("agent", ServiceStatus::Stopped),
                    service("coder", ServiceStatus::Stopped),
                ],
            ),
            RuntimeOverallStatus::Stopped
        );
        assert_eq!(
            classify_runtime(true, true, &[]),
            RuntimeOverallStatus::Starting
        );
        assert_eq!(
            classify_runtime(false, true, &ready),
            RuntimeOverallStatus::Missing
        );
    }

    #[test]
    fn adopts_a_core_ready_runtime_while_controller_is_still_loading() {
        let state: RuntimeState = serde_json::from_str(
            r#"{
                "modelPid": 101,
                "coderPid": 102,
                "llamaPid": 103,
                "agentPid": 104,
                "controllerStartedAt": "2026-11-12T12:00:00Z",
                "mode": "research-cpu"
            }"#,
        )
        .unwrap();

        assert!(runtime_state_is_adoptable(&state, true, true, true));
        assert!(!runtime_state_is_adoptable(&state, false, true, true));
        assert!(!runtime_state_is_adoptable(&state, true, false, true));
        assert!(!runtime_state_is_adoptable(&state, true, true, false));
    }

    #[test]
    fn rejects_missing_or_zero_runtime_processes() {
        let without_llama = RuntimeState {
            model_pid: 201,
            coder_pid: 202,
            llama_pid: None,
            agent_pid: 203,
            controller_started_at: None,
        };
        assert!(runtime_state_is_adoptable(&without_llama, true, true, true));

        for state in [
            RuntimeState {
                agent_pid: 0,
                ..without_llama
            },
            RuntimeState {
                coder_pid: 0,
                ..without_llama
            },
            RuntimeState {
                llama_pid: Some(0),
                ..without_llama
            },
        ] {
            assert!(!runtime_state_is_adoptable(&state, true, true, true));
        }

        let controller_timed_out = RuntimeState {
            model_pid: 0,
            ..without_llama
        };
        assert!(runtime_state_is_adoptable(
            &controller_timed_out,
            true,
            true,
            true
        ));
    }

    #[test]
    fn core_readiness_requires_agent_and_coder_but_not_controller() {
        let service = |id: &str, status| RuntimeServiceDiagnostic {
            id: id.into(),
            label: id.into(),
            status,
        };
        assert!(core_services_ready(&[
            service("agent", ServiceStatus::Ready),
            service("controller", ServiceStatus::Starting),
            service("coder", ServiceStatus::Ready),
            service("llama", ServiceStatus::Ready),
        ]));
        assert!(!core_services_ready(&[
            service("agent", ServiceStatus::Ready),
            service("coder", ServiceStatus::Stopped),
        ]));
    }

    #[test]
    fn stale_capsule_identity_requires_refresh_even_before_health_converges() {
        let root = tempdir().unwrap();
        let portable = root.path().join("capsule");
        let runtime = portable.join("runtime-pack");
        fs::create_dir_all(portable.join("data")).unwrap();
        fs::create_dir_all(runtime.join("runtime/node")).unwrap();
        fs::create_dir_all(runtime.join("runtime/python")).unwrap();
        fs::write(runtime.join("Start-PocketMind-Research.ps1"), "").unwrap();
        fs::write(runtime.join("runtime/node/node.exe"), "").unwrap();
        fs::write(runtime.join("runtime/python/python.exe"), "").unwrap();
        fs::write(portable.join("ucode-manifest.json"), "current manifest").unwrap();
        fs::write(
            portable.join("data/runtime.json"),
            b"\xef\xbb\xbf{\"modelPid\":1,\"coderPid\":2,\"llamaPid\":3,\"agentPid\":4}",
        )
        .unwrap();
        fs::write(
            portable.join("data/agent-runtime.json"),
            r#"{"pid":4,"capsuleDigest":"old"}"#,
        )
        .unwrap();

        let supervisor = CompatibilitySupervisor::discover(&portable, "profile");
        assert!(supervisor.runtime_state_requires_refresh());

        let digest = sha256_file(&portable.join("ucode-manifest.json")).unwrap();
        fs::write(
            portable.join("data/agent-runtime.json"),
            serde_json::json!({ "pid": 4, "capsuleDigest": digest }).to_string(),
        )
        .unwrap();
        assert!(!supervisor.runtime_state_requires_refresh());
    }
}
