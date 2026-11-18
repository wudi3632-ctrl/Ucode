use crate::rescue_repair::{RepairProposal, UserPathState};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration as StdDuration, Instant};
use uuid::Uuid;

pub const ELEVATED_SCHEMA_VERSION: u16 = 1;
pub const MACHINE_PATH_OPERATION: &str = "system.machine_path.normalize";
pub const MACHINE_PATH_OPERATION_VERSION: u16 = 1;
pub const MACHINE_PATH_TARGET: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment\Path";
const CREDENTIAL_SERVICE: &str = "ucode-rescue-elevated-v1";
const HELPER_ARGUMENT: &str = "--ucode-rescue-elevated-helper";
const RESPONSE_WAIT: StdDuration = StdDuration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedAction {
    Apply,
    Rollback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevatedRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub ticket_id: String,
    pub proposal_id: String,
    pub proposal_digest: String,
    pub operation_id: String,
    pub operation_version: u16,
    pub action: ElevatedAction,
    pub target: String,
    pub expected_current: UserPathState,
    pub expected_current_digest: String,
    pub desired: UserPathState,
    pub desired_digest: String,
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub hmac: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedResponseStatus {
    Verified,
    SafeStop,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevatedResponse {
    pub schema_version: u16,
    pub request_id: String,
    pub proposal_id: String,
    pub nonce: String,
    pub status: ElevatedResponseStatus,
    pub detail: String,
    pub observed: Option<UserPathState>,
    pub observed_digest: Option<String>,
    pub completed_at: DateTime<Utc>,
    pub hmac: String,
}

pub trait MachinePathHost {
    fn read_machine_path(&self) -> Result<UserPathState, String>;
    fn write_machine_path(&self, state: &UserPathState) -> Result<(), String>;
}

pub trait ElevatedRepairExecutor: Send + Sync {
    fn read_machine_path(&self) -> Result<UserPathState, String>;
    fn mutate_machine_path(
        &self,
        root: &Path,
        proposal: &RepairProposal,
        expected_current: &UserPathState,
        desired: &UserPathState,
        action: ElevatedAction,
    ) -> Result<UserPathState, String>;
}

pub struct WindowsElevatedRepairExecutor;

impl ElevatedRepairExecutor for WindowsElevatedRepairExecutor {
    fn read_machine_path(&self) -> Result<UserPathState, String> {
        WindowsMachinePathHost.read_machine_path()
    }

    fn mutate_machine_path(
        &self,
        root: &Path,
        proposal: &RepairProposal,
        expected_current: &UserPathState,
        desired: &UserPathState,
        action: ElevatedAction,
    ) -> Result<UserPathState, String> {
        execute_via_uac(root, proposal, expected_current, desired, action)
    }
}

pub struct DisabledElevatedRepairExecutor;

impl ElevatedRepairExecutor for DisabledElevatedRepairExecutor {
    fn read_machine_path(&self) -> Result<UserPathState, String> {
        Err("R3 elevated helper is not configured for this host".into())
    }

    fn mutate_machine_path(
        &self,
        _root: &Path,
        _proposal: &RepairProposal,
        _expected_current: &UserPathState,
        _desired: &UserPathState,
        _action: ElevatedAction,
    ) -> Result<UserPathState, String> {
        Err("R3 elevated helper is not configured for this host".into())
    }
}

pub struct WindowsMachinePathHost;

#[cfg(target_os = "windows")]
impl MachinePathHost for WindowsMachinePathHost {
    fn read_machine_path(&self) -> Result<UserPathState, String> {
        use windows_registry::{Type, LOCAL_MACHINE};
        let key = LOCAL_MACHINE
            .open(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
            .map_err(|error| format!("无法打开机器环境变量：{error}"))?;
        match key.get_string("Path") {
            Ok(value) => {
                let expandable = key
                    .get_type("Path")
                    .map(|value| value == Type::ExpandString)
                    .unwrap_or(false);
                Ok(UserPathState {
                    present: true,
                    value,
                    expandable,
                })
            }
            Err(error) if error.code().0 == -2147024894 => Ok(UserPathState {
                present: false,
                value: String::new(),
                expandable: true,
            }),
            Err(error) => Err(format!("无法读取机器 PATH：{error}")),
        }
    }

    fn write_machine_path(&self, state: &UserPathState) -> Result<(), String> {
        use windows_registry::LOCAL_MACHINE;
        let key = LOCAL_MACHINE
            .create(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
            .map_err(|error| format!("无法以管理员权限打开机器环境变量：{error}"))?;
        if !state.present {
            return key
                .remove_value("Path")
                .map_err(|error| format!("无法恢复机器 PATH：{error}"));
        }
        if state.expandable {
            key.set_expand_string("Path", &state.value)
        } else {
            key.set_string("Path", &state.value)
        }
        .map_err(|error| format!("无法写入机器 PATH：{error}"))
    }
}

#[cfg(not(target_os = "windows"))]
impl MachinePathHost for WindowsMachinePathHost {
    fn read_machine_path(&self) -> Result<UserPathState, String> {
        Err("机器 PATH 只支持 Windows".into())
    }

    fn write_machine_path(&self, _state: &UserPathState) -> Result<(), String> {
        Err("机器 PATH 只支持 Windows".into())
    }
}

pub fn maybe_run_helper(arguments: &[String]) -> Option<i32> {
    let position = arguments
        .iter()
        .position(|value| value == HELPER_ARGUMENT)?;
    let request = arguments.get(position + 1).map(PathBuf::from);
    let response = arguments.get(position + 2).map(PathBuf::from);
    Some(match (request, response) {
        (Some(request), Some(response)) => {
            match execute_helper_files(&request, &response, &WindowsMachinePathHost) {
                Ok(()) => 0,
                Err(_) => 2,
            }
        }
        _ => 2,
    })
}

fn execute_via_uac(
    root: &Path,
    proposal: &RepairProposal,
    expected_current: &UserPathState,
    desired: &UserPathState,
    action: ElevatedAction,
) -> Result<UserPathState, String> {
    if proposal.operation_id != MACHINE_PATH_OPERATION
        || proposal.operation_version != MACHINE_PATH_OPERATION_VERSION
        || proposal.risk != "r3"
        || proposal.target != MACHINE_PATH_TARGET
    {
        return Err("R3 helper 拒绝未知操作或目标".into());
    }
    let directory = root.join("elevated");
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建 R3 请求目录：{error}"))?;
    let request_id = format!("elevated-{}", Uuid::new_v4());
    let nonce = Uuid::new_v4().to_string();
    let secret = one_time_secret();
    let now = Utc::now();
    let mut request = ElevatedRequest {
        schema_version: ELEVATED_SCHEMA_VERSION,
        request_id: request_id.clone(),
        ticket_id: proposal.ticket_id.clone(),
        proposal_id: proposal.id.clone(),
        proposal_digest: proposal.proposal_digest.clone(),
        operation_id: proposal.operation_id.clone(),
        operation_version: proposal.operation_version,
        action,
        target: proposal.target.clone(),
        expected_current: expected_current.clone(),
        expected_current_digest: digest_json(expected_current),
        desired: desired.clone(),
        desired_digest: digest_json(desired),
        nonce: nonce.clone(),
        created_at: now,
        expires_at: now + Duration::minutes(3),
        hmac: String::new(),
    };
    request.hmac = request_hmac(&request, &secret);
    let request_path = directory.join(format!("{request_id}.request.json"));
    let response_path = directory.join(format!("{request_id}.response.json"));
    save_atomic(&request_path, &request)?;
    if let Err(error) = save_secret(&nonce, &secret) {
        let _ = fs::remove_file(&request_path);
        return Err(error);
    }
    let result = (|| {
        launch_elevated(&request_path, &response_path)?;
        let started = Instant::now();
        while started.elapsed() < RESPONSE_WAIT {
            if response_path.is_file() {
                let response: ElevatedResponse = serde_json::from_slice(
                    &fs::read(&response_path)
                        .map_err(|error| format!("无法读取 R3 helper 响应：{error}"))?,
                )
                .map_err(|_| "R3 helper 响应格式无效".to_string())?;
                validate_response(&request, &response, &secret)?;
                return match response.status {
                    ElevatedResponseStatus::Verified => response
                        .observed
                        .ok_or_else(|| "R3 helper 未返回验证后的状态".to_string()),
                    ElevatedResponseStatus::SafeStop | ElevatedResponseStatus::Failed => {
                        Err(response.detail)
                    }
                };
            }
            thread::sleep(StdDuration::from_millis(100));
        }
        Err("未收到 R3 helper 响应；UAC 可能被取消或 helper 已超时，未声明系统已修改".into())
    })();
    delete_secret(&nonce);
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(request_path.with_extension("claimed"));
    let _ = fs::remove_file(&response_path);
    result
}

fn execute_helper_files(
    request_path: &Path,
    response_path: &Path,
    host: &dyn MachinePathHost,
) -> Result<(), String> {
    validate_helper_paths(request_path, response_path)?;
    let claimed_path = request_path.with_extension("claimed");
    fs::rename(request_path, &claimed_path)
        .map_err(|_| "R3 helper 请求已经被领取、重放或不存在".to_string())?;
    let request: ElevatedRequest = serde_json::from_slice(
        &fs::read(&claimed_path).map_err(|error| format!("无法读取 R3 请求：{error}"))?,
    )
    .map_err(|_| "R3 请求格式无效".to_string())?;
    let secret = load_secret(&request.nonce)?;
    let result = execute_authenticated(&request, &secret, host);
    let mut response = match result {
        Ok(observed) => ElevatedResponse {
            schema_version: ELEVATED_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            proposal_id: request.proposal_id.clone(),
            nonce: request.nonce.clone(),
            status: ElevatedResponseStatus::Verified,
            detail: "管理员 helper 已写入并重读验证机器 PATH".into(),
            observed_digest: Some(digest_json(&observed)),
            observed: Some(observed),
            completed_at: Utc::now(),
            hmac: String::new(),
        },
        Err(error) => ElevatedResponse {
            schema_version: ELEVATED_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            proposal_id: request.proposal_id.clone(),
            nonce: request.nonce.clone(),
            status: ElevatedResponseStatus::SafeStop,
            detail: error,
            observed_digest: None,
            observed: None,
            completed_at: Utc::now(),
            hmac: String::new(),
        },
    };
    response.hmac = response_hmac(&response, &secret);
    save_atomic(response_path, &response)?;
    delete_secret(&request.nonce);
    let _ = fs::remove_file(&claimed_path);
    Ok(())
}

fn execute_authenticated(
    request: &ElevatedRequest,
    secret: &[u8],
    host: &dyn MachinePathHost,
) -> Result<UserPathState, String> {
    validate_request(request, secret)?;
    let current = host.read_machine_path()?;
    if digest_json(&current) != request.expected_current_digest
        || current != request.expected_current
    {
        return Err("机器 PATH 在审批后发生变化，R3 helper 已安全停止".into());
    }
    if digest_json(&request.desired) != request.desired_digest {
        return Err("R3 helper 的目标状态摘要无效".into());
    }
    host.write_machine_path(&request.desired)?;
    let observed = host.read_machine_path()?;
    if observed != request.desired || digest_json(&observed) != request.desired_digest {
        let restored = host.write_machine_path(&current).is_ok()
            && host
                .read_machine_path()
                .map(|value| value == current)
                .unwrap_or(false);
        return Err(format!(
            "机器 PATH 写入后验证失败；快照恢复{}",
            if restored { "成功" } else { "失败" }
        ));
    }
    Ok(observed)
}

fn validate_request(request: &ElevatedRequest, secret: &[u8]) -> Result<(), String> {
    if request.schema_version != ELEVATED_SCHEMA_VERSION
        || request.operation_id != MACHINE_PATH_OPERATION
        || request.operation_version != MACHINE_PATH_OPERATION_VERSION
        || request.target != MACHINE_PATH_TARGET
        || request.request_id.is_empty()
        || request.proposal_id.is_empty()
        || request.ticket_id.is_empty()
        || request.nonce.is_empty()
    {
        return Err("R3 helper 请求包含未知操作、版本或目标".into());
    }
    if Utc::now() > request.expires_at || request.expires_at <= request.created_at {
        return Err("R3 helper 请求已过期".into());
    }
    if !constant_time_equal(&request.hmac, &request_hmac(request, secret)) {
        return Err("R3 helper 请求认证失败".into());
    }
    Ok(())
}

fn validate_response(
    request: &ElevatedRequest,
    response: &ElevatedResponse,
    secret: &[u8],
) -> Result<(), String> {
    if response.schema_version != ELEVATED_SCHEMA_VERSION
        || response.request_id != request.request_id
        || response.proposal_id != request.proposal_id
        || response.nonce != request.nonce
        || !constant_time_equal(&response.hmac, &response_hmac(response, secret))
    {
        return Err("R3 helper 响应认证或绑定信息无效".into());
    }
    if response.status == ElevatedResponseStatus::Verified
        && response.observed_digest.as_deref() != Some(request.desired_digest.as_str())
    {
        return Err("R3 helper 响应未证明目标状态".into());
    }
    if response.status == ElevatedResponseStatus::Verified
        && response.observed.as_ref().map(digest_json).as_deref()
            != Some(request.desired_digest.as_str())
    {
        return Err("R3 helper 响应中的状态与摘要不一致".into());
    }
    Ok(())
}

fn validate_helper_paths(request: &Path, response: &Path) -> Result<(), String> {
    if !request.is_absolute() || !response.is_absolute() {
        return Err("R3 helper 只接受绝对请求与响应路径".into());
    }
    let request_parent = request.parent().ok_or("R3 请求目录无效")?;
    let response_parent = response.parent().ok_or("R3 响应目录无效")?;
    if request_parent != response_parent
        || request.extension().and_then(|value| value.to_str()) != Some("json")
        || response.extension().and_then(|value| value.to_str()) != Some("json")
    {
        return Err("R3 helper 请求与响应必须位于同一目录并使用 JSON".into());
    }
    Ok(())
}

fn request_hmac(request: &ElevatedRequest, secret: &[u8]) -> String {
    let mut value = serde_json::to_value(request).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("hmac".into(), Value::String(String::new()));
    }
    hmac_hex(secret, &serde_json::to_vec(&value).unwrap_or_default())
}

fn response_hmac(response: &ElevatedResponse, secret: &[u8]) -> String {
    let mut value = serde_json::to_value(response).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("hmac".into(), Value::String(String::new()));
    }
    hmac_hex(secret, &serde_json::to_vec(&value).unwrap_or_default())
}

fn hmac_hex(secret: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut key = if secret.len() > BLOCK {
        Sha256::digest(secret).to_vec()
    } else {
        secret.to_vec()
    };
    key.resize(BLOCK, 0);
    let mut inner_pad = vec![0x36; BLOCK];
    let mut outer_pad = vec![0x5c; BLOCK];
    for (index, value) in key.iter().enumerate() {
        inner_pad[index] ^= value;
        outer_pad[index] ^= value;
    }
    let mut inner = Sha256::new();
    inner.update(&inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(&outer_pad);
    outer.update(inner.finalize());
    hex_encode(&outer.finalize())
}

fn one_time_secret() -> Vec<u8> {
    let mut value = Vec::with_capacity(32);
    value.extend_from_slice(Uuid::new_v4().as_bytes());
    value.extend_from_slice(Uuid::new_v4().as_bytes());
    value
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("R3 helper secret encoding is invalid".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "R3 helper secret encoding is invalid".into())
        })
        .collect()
}

fn save_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("R3 helper 文件目录无效")?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 R3 helper 目录：{error}"))?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("无法写入 R3 helper 临时文件：{error}"))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("无法提交 R3 helper 文件：{error}")
    })
}

#[cfg(target_os = "windows")]
fn credential_entry(nonce: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, nonce)
        .map_err(|error| format!("无法打开 R3 一次性凭据：{error}"))
}

#[cfg(target_os = "windows")]
fn save_secret(nonce: &str, secret: &[u8]) -> Result<(), String> {
    credential_entry(nonce)?
        .set_password(&hex_encode(secret))
        .map_err(|error| format!("无法保存 R3 一次性凭据：{error}"))
}

#[cfg(not(target_os = "windows"))]
fn save_secret(_nonce: &str, _secret: &[u8]) -> Result<(), String> {
    Err("R3 elevated helper only supports Windows".into())
}

#[cfg(target_os = "windows")]
fn load_secret(nonce: &str) -> Result<Vec<u8>, String> {
    let value = credential_entry(nonce)?
        .get_password()
        .map_err(|_| "R3 一次性凭据不存在或已使用".to_string())?;
    hex_decode(&value)
}

#[cfg(not(target_os = "windows"))]
fn load_secret(_nonce: &str) -> Result<Vec<u8>, String> {
    Err("R3 elevated helper only supports Windows".into())
}

#[cfg(target_os = "windows")]
fn delete_secret(nonce: &str) {
    if let Ok(entry) = credential_entry(nonce) {
        let _ = entry.delete_credential();
    }
}

#[cfg(not(target_os = "windows"))]
fn delete_secret(_nonce: &str) {}

#[cfg(target_os = "windows")]
fn launch_elevated(request: &Path, response: &Path) -> Result<(), String> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_command: i32,
        ) -> isize;
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(once(0)).collect()
    }

    let executable =
        std::env::current_exe().map_err(|error| format!("无法定位 UCode helper：{error}"))?;
    let operation = wide(std::ffi::OsStr::new("runas"));
    let file = wide(executable.as_os_str());
    let parameters = format!(
        "{HELPER_ARGUMENT} \"{}\" \"{}\"",
        request.display(),
        response.display()
    );
    let parameters = wide(std::ffi::OsStr::new(&parameters));
    let result = unsafe {
        ShellExecuteW(
            0,
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            null(),
            0,
        )
    };
    if result <= 32 {
        Err(format!(
            "Windows 未启动 R3 helper（ShellExecuteW={result}）；UAC 可能被取消"
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn launch_elevated(_request: &Path, _response: &Path) -> Result<(), String> {
    Err("R3 elevated helper only supports Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeMachineHost {
        state: Mutex<UserPathState>,
        corrupt_after_write: bool,
    }

    impl MachinePathHost for FakeMachineHost {
        fn read_machine_path(&self) -> Result<UserPathState, String> {
            Ok(self.state.lock().unwrap().clone())
        }

        fn write_machine_path(&self, state: &UserPathState) -> Result<(), String> {
            let mut value = state.clone();
            if self.corrupt_after_write {
                value.value.push_str(";C:\\unexpected");
            }
            *self.state.lock().unwrap() = value;
            Ok(())
        }
    }

    fn signed_request(
        before: &UserPathState,
        after: &UserPathState,
        secret: &[u8],
    ) -> ElevatedRequest {
        let now = Utc::now();
        let mut request = ElevatedRequest {
            schema_version: ELEVATED_SCHEMA_VERSION,
            request_id: "request-1".into(),
            ticket_id: "ticket-1".into(),
            proposal_id: "proposal-1".into(),
            proposal_digest: "a".repeat(64),
            operation_id: MACHINE_PATH_OPERATION.into(),
            operation_version: MACHINE_PATH_OPERATION_VERSION,
            action: ElevatedAction::Apply,
            target: MACHINE_PATH_TARGET.into(),
            expected_current: before.clone(),
            expected_current_digest: digest_json(before),
            desired: after.clone(),
            desired_digest: digest_json(after),
            nonce: "nonce-1".into(),
            created_at: now,
            expires_at: now + Duration::minutes(2),
            hmac: String::new(),
        };
        request.hmac = request_hmac(&request, secret);
        request
    }

    #[test]
    fn authenticated_request_writes_only_the_declared_machine_path_state() {
        let before = UserPathState {
            present: true,
            value: r"C:\Windows;C:\Windows".into(),
            expandable: true,
        };
        let after = UserPathState {
            present: true,
            value: r"C:\Windows".into(),
            expandable: true,
        };
        let secret = b"one-time-test-secret";
        let request = signed_request(&before, &after, secret);
        let host = FakeMachineHost {
            state: Mutex::new(before),
            corrupt_after_write: false,
        };
        let observed = execute_authenticated(&request, secret, &host).unwrap();
        assert_eq!(observed, after);
    }

    #[test]
    fn tampering_expiry_and_unknown_targets_are_rejected_without_writes() {
        let before = UserPathState {
            present: true,
            value: r"C:\Windows;C:\Windows".into(),
            expandable: true,
        };
        let after = UserPathState {
            present: true,
            value: r"C:\Windows".into(),
            expandable: true,
        };
        let secret = b"one-time-test-secret";
        for mutation in 0..3 {
            let mut request = signed_request(&before, &after, secret);
            match mutation {
                0 => request.desired.value = r"C:\tampered".into(),
                1 => request.expires_at = Utc::now() - Duration::seconds(1),
                _ => request.target = r"HKLM\Software\Other".into(),
            }
            let host = FakeMachineHost {
                state: Mutex::new(before.clone()),
                corrupt_after_write: false,
            };
            assert!(execute_authenticated(&request, secret, &host).is_err());
            assert_eq!(host.read_machine_path().unwrap(), before);
        }
    }

    #[test]
    fn stale_state_safe_stops_and_failed_verification_restores_snapshot() {
        let before = UserPathState {
            present: true,
            value: r"C:\Windows;C:\Windows".into(),
            expandable: true,
        };
        let after = UserPathState {
            present: true,
            value: r"C:\Windows".into(),
            expandable: true,
        };
        let secret = b"one-time-test-secret";
        let request = signed_request(&before, &after, secret);
        let stale = UserPathState {
            value: r"C:\Windows;D:\later".into(),
            ..before.clone()
        };
        let host = FakeMachineHost {
            state: Mutex::new(stale.clone()),
            corrupt_after_write: false,
        };
        assert!(execute_authenticated(&request, secret, &host).is_err());
        assert_eq!(host.read_machine_path().unwrap(), stale);

        let host = FakeMachineHost {
            state: Mutex::new(before.clone()),
            corrupt_after_write: true,
        };
        let error = execute_authenticated(&request, secret, &host).unwrap_err();
        assert!(error.contains("快照恢复"));
    }

    #[test]
    fn response_must_bind_request_nonce_proposal_and_desired_digest() {
        let before = UserPathState {
            present: true,
            value: r"C:\Windows;C:\Windows".into(),
            expandable: true,
        };
        let after = UserPathState {
            present: true,
            value: r"C:\Windows".into(),
            expandable: true,
        };
        let secret = b"one-time-test-secret";
        let request = signed_request(&before, &after, secret);
        let mut response = ElevatedResponse {
            schema_version: 1,
            request_id: request.request_id.clone(),
            proposal_id: request.proposal_id.clone(),
            nonce: request.nonce.clone(),
            status: ElevatedResponseStatus::Verified,
            detail: "ok".into(),
            observed: Some(after.clone()),
            observed_digest: Some(digest_json(&after)),
            completed_at: Utc::now(),
            hmac: String::new(),
        };
        response.hmac = response_hmac(&response, secret);
        validate_response(&request, &response, secret).unwrap();
        response.nonce = "other".into();
        assert!(validate_response(&request, &response, secret).is_err());
    }

    #[test]
    fn helper_argument_mode_never_matches_normal_desktop_startup() {
        assert_eq!(maybe_run_helper(&["UCode.exe".into()]), None);
        assert_eq!(
            maybe_run_helper(&["UCode.exe".into(), HELPER_ARGUMENT.into()]),
            Some(2)
        );
    }

    #[test]
    fn hmac_implementation_matches_rfc_4231_sha256_vector() {
        let key = vec![0x0b; 20];
        assert_eq!(
            hmac_hex(&key, b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
