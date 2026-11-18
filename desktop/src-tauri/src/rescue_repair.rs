use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const REPAIR_SCHEMA_VERSION: u16 = 1;
pub const USER_PATH_OPERATION: &str = "system.user_path.normalize";
const USER_PATH_OPERATION_VERSION: u16 = 1;
const USER_ENVIRONMENT_TARGET: &str = r"HKCU\Environment\Path";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairOperationDefinition {
    pub id: &'static str,
    pub version: u16,
    pub risk: &'static str,
    pub authority: &'static str,
    pub enabled: bool,
}

pub fn operation_registry() -> &'static [RepairOperationDefinition] {
    static OPERATIONS: [RepairOperationDefinition; 4] = [
        RepairOperationDefinition {
            id: USER_PATH_OPERATION,
            version: 1,
            risk: "r2",
            authority: "current_user_registry",
            enabled: true,
        },
        RepairOperationDefinition {
            id: "file.utf8.restore",
            version: 1,
            risk: "r2",
            authority: "explicit_file",
            enabled: false,
        },
        RepairOperationDefinition {
            id: "network.winhttp.reset",
            version: 1,
            risk: "r3",
            authority: "elevated_helper",
            enabled: false,
        },
        RepairOperationDefinition {
            id: "system.machine_path.normalize",
            version: 1,
            risk: "r3",
            authority: "elevated_helper",
            enabled: false,
        },
    ];
    &OPERATIONS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairStatus {
    AwaitingApproval,
    Applying,
    RepairedUnverified,
    Verified,
    SafeStop,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairProposal {
    pub schema_version: u16,
    pub id: String,
    pub ticket_id: String,
    pub operation_id: String,
    pub operation_version: u16,
    pub risk: String,
    pub target: String,
    pub normalized_arguments: Value,
    pub evidence_ids: Vec<String>,
    pub prerequisites: Vec<String>,
    pub snapshot_plan: String,
    pub expected_postcondition: String,
    pub verification_probe: String,
    pub rollback_contract: String,
    pub before_digest: String,
    pub expected_after_digest: String,
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: RepairStatus,
    pub proposal_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairSnapshot {
    pub schema_version: u16,
    pub proposal_id: String,
    pub operation_id: String,
    pub operation_version: u16,
    pub target: String,
    pub before_state: Value,
    pub before_digest: String,
    pub after_state: Value,
    pub after_digest: String,
    pub snapshot_digest: String,
    pub applied_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairRecord {
    pub proposal: RepairProposal,
    pub snapshot: Option<RepairSnapshot>,
    pub detail: String,
    pub updated_at: DateTime<Utc>,
}

pub trait RepairHost: Send + Sync {
    fn read_user_path(&self) -> Result<UserPathState, String>;
    fn write_user_path(&self, state: &UserPathState) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPathState {
    pub present: bool,
    pub value: String,
    pub expandable: bool,
}

pub struct WindowsRepairHost;

#[cfg(target_os = "windows")]
impl RepairHost for WindowsRepairHost {
    fn read_user_path(&self) -> Result<UserPathState, String> {
        use windows_registry::{Type, CURRENT_USER};
        let key = CURRENT_USER
            .create("Environment")
            .map_err(|error| format!("无法打开当前用户环境变量：{error}"))?;
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
            Err(error) => Err(format!("无法读取当前用户 PATH：{error}")),
        }
    }

    fn write_user_path(&self, state: &UserPathState) -> Result<(), String> {
        use windows_registry::CURRENT_USER;
        let key = CURRENT_USER
            .create("Environment")
            .map_err(|error| format!("无法打开当前用户环境变量：{error}"))?;
        if !state.present {
            return key
                .remove_value("Path")
                .map_err(|error| format!("无法恢复当前用户 PATH：{error}"));
        }
        if state.expandable {
            key.set_expand_string("Path", &state.value)
        } else {
            key.set_string("Path", &state.value)
        }
        .map_err(|error| format!("无法写入当前用户 PATH：{error}"))
    }
}

#[cfg(not(target_os = "windows"))]
impl RepairHost for WindowsRepairHost {
    fn read_user_path(&self) -> Result<UserPathState, String> {
        Err("用户 PATH 修复只支持 Windows".into())
    }
    fn write_user_path(&self, _state: &UserPathState) -> Result<(), String> {
        Err("用户 PATH 修复只支持 Windows".into())
    }
}

pub struct RepairService<H: RepairHost = WindowsRepairHost> {
    root: PathBuf,
    host: H,
}

impl RepairService<WindowsRepairHost> {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        Self::with_host(root, WindowsRepairHost)
    }
}

impl<H: RepairHost> RepairService<H> {
    pub fn with_host(root: PathBuf, host: H) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| format!("无法创建修复证据目录：{error}"))?;
        Ok(Self { root, host })
    }

    pub fn propose_user_path_normalization(
        &self,
        ticket_id: &str,
        evidence_ids: Vec<String>,
    ) -> Result<Option<RepairProposal>, String> {
        let before = self.host.read_user_path()?;
        let (after, removed) = normalize_user_path(&before);
        if before == after {
            return Ok(None);
        }
        let now = Utc::now();
        let mut proposal = RepairProposal {
            schema_version: REPAIR_SCHEMA_VERSION,
            id: format!("repair-{}", Uuid::new_v4()),
            ticket_id: ticket_id.into(),
            operation_id: USER_PATH_OPERATION.into(),
            operation_version: USER_PATH_OPERATION_VERSION,
            risk: "r2".into(),
            target: USER_ENVIRONMENT_TARGET.into(),
            normalized_arguments: json!({ "after": after, "removedEntries": removed }),
            evidence_ids,
            prerequisites: vec![
                "当前用户 PATH 与诊断时摘要一致".into(),
                "不需要管理员权限".into(),
            ],
            snapshot_plan: "在便携数据目录保存原值、注册表值类型和 SHA-256".into(),
            expected_postcondition: "用户 PATH 不含空项、重复项和明确不存在的绝对目录".into(),
            verification_probe: "重新读取 HKCU\\Environment\\Path 并核对精确摘要".into(),
            rollback_contract: "仅当当前值仍等于本次应用后摘要时恢复原值".into(),
            before_digest: digest_json(&before),
            expected_after_digest: digest_json(&after),
            nonce: Uuid::new_v4().to_string(),
            created_at: now,
            expires_at: now + Duration::minutes(10),
            status: RepairStatus::AwaitingApproval,
            proposal_digest: String::new(),
        };
        proposal.proposal_digest = proposal_digest(&proposal);
        self.save_record(&RepairRecord {
            proposal: proposal.clone(),
            snapshot: None,
            detail: "等待用户审批".into(),
            updated_at: now,
        })?;
        Ok(Some(proposal))
    }

    pub fn reject(&self, proposal_id: &str, ticket_id: &str) -> Result<RepairRecord, String> {
        let mut record = self.load_record(proposal_id)?;
        self.validate_identity(&record.proposal, ticket_id)?;
        if record.proposal.status != RepairStatus::AwaitingApproval {
            return Err("修复方案已经处理，不能重复拒绝".into());
        }
        record.proposal.status = RepairStatus::SafeStop;
        record.detail = "用户选择暂不修复；系统没有发生变化".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    pub fn apply(&self, proposal_id: &str, ticket_id: &str) -> Result<RepairRecord, String> {
        let mut record = self.load_record(proposal_id)?;
        self.validate_for_apply(&record.proposal, ticket_id)?;
        let before = self.host.read_user_path()?;
        if digest_json(&before) != record.proposal.before_digest {
            return self.safe_stop(record, "审批前目标状态已经变化，已拒绝应用");
        }
        let after: UserPathState = serde_json::from_value(
            record
                .proposal
                .normalized_arguments
                .get("after")
                .cloned()
                .ok_or("修复参数缺少 after")?,
        )
        .map_err(|_| "修复参数格式无效".to_string())?;
        if digest_json(&after) != record.proposal.expected_after_digest {
            return self.safe_stop(record, "修复参数摘要不匹配，已拒绝应用");
        }

        record.proposal.status = RepairStatus::Applying;
        record.detail = "正在应用已批准修复".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        let applied_at = Utc::now();
        let mut snapshot = RepairSnapshot {
            schema_version: REPAIR_SCHEMA_VERSION,
            proposal_id: record.proposal.id.clone(),
            operation_id: record.proposal.operation_id.clone(),
            operation_version: record.proposal.operation_version,
            target: record.proposal.target.clone(),
            before_state: serde_json::to_value(&before).map_err(|error| error.to_string())?,
            before_digest: digest_json(&before),
            after_state: serde_json::to_value(&after).map_err(|error| error.to_string())?,
            after_digest: digest_json(&after),
            snapshot_digest: String::new(),
            applied_at,
            verified_at: None,
        };
        snapshot.snapshot_digest = snapshot_digest(&snapshot);
        record.snapshot = Some(snapshot.clone());
        self.save_record(&record)?;

        if let Err(error) = self.host.write_user_path(&after) {
            let rollback = self.host.write_user_path(&before);
            return self.safe_stop(
                record,
                &format!(
                    "应用失败：{error}；快照恢复{}",
                    if rollback.is_ok() { "成功" } else { "失败" }
                ),
            );
        }
        record.proposal.status = RepairStatus::RepairedUnverified;
        record.detail = "修复已写入，正在独立重读验证".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        let observed = self.host.read_user_path()?;
        if digest_json(&observed) != snapshot.after_digest {
            record.detail = "修复已写入，但后置验证失败；不能声明完成".into();
            record.updated_at = Utc::now();
            self.save_record(&record)?;
            return Ok(record);
        }
        if let Some(value) = record.snapshot.as_mut() {
            value.verified_at = Some(Utc::now());
            value.snapshot_digest = snapshot_digest(value);
        }
        record.proposal.status = RepairStatus::Verified;
        record.detail = "修复已通过目标重读验证，可安全回滚".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    pub fn rollback(&self, proposal_id: &str, ticket_id: &str) -> Result<RepairRecord, String> {
        let mut record = self.load_record(proposal_id)?;
        self.validate_identity(&record.proposal, ticket_id)?;
        if record.proposal.status != RepairStatus::Verified {
            return Err("只有已验证的修复可以回滚".into());
        }
        let snapshot = record.snapshot.clone().ok_or("修复快照不存在")?;
        if snapshot.snapshot_digest != snapshot_digest(&snapshot) {
            return Err("修复快照摘要无效，已拒绝回滚".into());
        }
        let current = self.host.read_user_path()?;
        if digest_json(&current) != snapshot.after_digest {
            return self.safe_stop(record, "应用后目标又被修改，已拒绝覆盖后续用户更改");
        }
        let before: UserPathState = serde_json::from_value(snapshot.before_state)
            .map_err(|_| "快照 before state 无效".to_string())?;
        if let Err(error) = self.host.write_user_path(&before) {
            record.proposal.status = RepairStatus::RollbackFailed;
            record.detail = format!("回滚失败：{error}");
            record.updated_at = Utc::now();
            self.save_record(&record)?;
            return Ok(record);
        }
        if digest_json(&self.host.read_user_path()?) != snapshot.before_digest {
            record.proposal.status = RepairStatus::RollbackFailed;
            record.detail = "回滚写入后摘要与原始快照不一致".into();
        } else {
            record.proposal.status = RepairStatus::RolledBack;
            record.detail = "已恢复审批前的精确用户 PATH".into();
        }
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    fn safe_stop(&self, mut record: RepairRecord, detail: &str) -> Result<RepairRecord, String> {
        record.proposal.status = RepairStatus::SafeStop;
        record.detail = detail.into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    fn validate_identity(&self, proposal: &RepairProposal, ticket_id: &str) -> Result<(), String> {
        if proposal.ticket_id != ticket_id {
            return Err("修复方案不属于当前任务".into());
        }
        let registered = operation_registry().iter().find(|operation| {
            operation.id == proposal.operation_id
                && operation.version == proposal.operation_version
                && operation.enabled
                && operation.risk == "r2"
        });
        if proposal.schema_version != REPAIR_SCHEMA_VERSION
            || registered.is_none()
            || proposal.operation_id != USER_PATH_OPERATION
            || proposal.operation_version != USER_PATH_OPERATION_VERSION
            || proposal.risk != "r2"
            || proposal.target != USER_ENVIRONMENT_TARGET
        {
            return Err("未知、过期或非 R2 修复操作，已拒绝执行".into());
        }
        if proposal.proposal_digest != proposal_digest(proposal) {
            return Err("修复方案摘要无效，已拒绝执行".into());
        }
        Ok(())
    }

    fn validate_for_apply(&self, proposal: &RepairProposal, ticket_id: &str) -> Result<(), String> {
        self.validate_identity(proposal, ticket_id)?;
        if proposal.status != RepairStatus::AwaitingApproval {
            return Err("修复方案已经执行或拒绝，不能重放".into());
        }
        if Utc::now() > proposal.expires_at {
            return Err("修复方案已过期，请重新诊断".into());
        }
        Ok(())
    }

    fn record_path(&self, proposal_id: &str) -> Result<PathBuf, String> {
        if proposal_id.is_empty()
            || !proposal_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return Err("修复方案 ID 无效".into());
        }
        Ok(self.root.join(format!("{proposal_id}.json")))
    }

    fn load_record(&self, proposal_id: &str) -> Result<RepairRecord, String> {
        let path = self.record_path(proposal_id)?;
        serde_json::from_slice(&fs::read(path).map_err(|_| "修复方案不存在".to_string())?)
            .map_err(|_| "修复方案记录损坏".to_string())
    }

    fn save_record(&self, record: &RepairRecord) -> Result<(), String> {
        let path = self.record_path(&record.proposal.id)?;
        let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
        fs::write(&temporary, bytes).map_err(|error| format!("无法保存修复记录：{error}"))?;
        fs::rename(&temporary, &path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("无法提交修复记录：{error}")
        })
    }
}

fn normalize_user_path(before: &UserPathState) -> (UserPathState, Vec<String>) {
    if !before.present {
        return (before.clone(), Vec::new());
    }
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for raw in before.value.split(';') {
        let value = raw.trim().trim_matches('"').trim();
        if value.is_empty() {
            removed.push("（空项）".into());
            continue;
        }
        let key = value
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        if !seen.insert(key) {
            removed.push(format!("{value}（重复）"));
            continue;
        }
        let path = Path::new(value);
        let has_variable = value.contains('%');
        if path.is_absolute() && !has_variable && !path.is_dir() {
            removed.push(format!("{value}（目录不存在）"));
            continue;
        }
        kept.push(value.to_string());
    }
    (
        UserPathState {
            present: true,
            value: kept.join(";"),
            expandable: before.expandable,
        },
        removed,
    )
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn proposal_digest(proposal: &RepairProposal) -> String {
    let mut value = serde_json::to_value(proposal).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("proposalDigest".into(), Value::String(String::new()));
        object.insert("status".into(), Value::String("awaiting_approval".into()));
    }
    digest_json(&value)
}

fn snapshot_digest(snapshot: &RepairSnapshot) -> String {
    let mut value = serde_json::to_value(snapshot).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("snapshotDigest".into(), Value::String(String::new()));
    }
    digest_json(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct FakeHost {
        state: Mutex<UserPathState>,
        fail_writes_remaining: Mutex<usize>,
        corrupt_after_write: Mutex<bool>,
        corrupt_next_read: Mutex<bool>,
    }
    impl FakeHost {
        fn new(value: &str) -> Self {
            Self {
                state: Mutex::new(UserPathState {
                    present: true,
                    value: value.into(),
                    expandable: true,
                }),
                fail_writes_remaining: Mutex::new(0),
                corrupt_after_write: Mutex::new(false),
                corrupt_next_read: Mutex::new(false),
            }
        }
    }
    impl RepairHost for FakeHost {
        fn read_user_path(&self) -> Result<UserPathState, String> {
            let state = self.state.lock().unwrap().clone();
            let mut corrupt = self.corrupt_next_read.lock().unwrap();
            if *corrupt {
                *corrupt = false;
                return Ok(UserPathState {
                    value: format!("{};D:\\unexpected", state.value),
                    ..state
                });
            }
            Ok(state)
        }
        fn write_user_path(&self, state: &UserPathState) -> Result<(), String> {
            let mut remaining = self.fail_writes_remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err("injected".into());
            }
            drop(remaining);
            *self.state.lock().unwrap() = state.clone();
            if *self.corrupt_after_write.lock().unwrap() {
                *self.corrupt_next_read.lock().unwrap() = true;
            }
            Ok(())
        }
    }

    fn service(value: &str) -> (TempDir, RepairService<FakeHost>) {
        let dir = TempDir::new().unwrap();
        let service =
            RepairService::with_host(dir.path().to_path_buf(), FakeHost::new(value)).unwrap();
        (dir, service)
    }

    #[test]
    fn proposal_requires_explicit_apply_and_normalizes_only_clear_faults() {
        let (_dir, service) =
            service(r"C:\Windows;;C:\Windows;%USERPROFILE%\bin;C:\definitely-missing-ucode-dir");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec!["evidence-1".into()])
            .unwrap()
            .unwrap();
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;;C:\Windows;%USERPROFILE%\bin;C:\definitely-missing-ucode-dir"
        );
        let record = service.apply(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::Verified);
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;%USERPROFILE%\bin"
        );
    }

    #[test]
    fn replay_and_wrong_ticket_are_rejected() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        assert!(service.apply(&proposal.id, "ticket-2").is_err());
        service.apply(&proposal.id, "ticket-1").unwrap();
        assert!(service.apply(&proposal.id, "ticket-1").is_err());
    }

    #[test]
    fn expired_proposal_is_rejected_without_mutation() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        let mut record = service.load_record(&proposal.id).unwrap();
        record.proposal.expires_at = Utc::now() - Duration::seconds(1);
        record.proposal.proposal_digest = proposal_digest(&record.proposal);
        service.save_record(&record).unwrap();
        assert!(service.apply(&proposal.id, "ticket-1").is_err());
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;C:\Windows"
        );
    }

    #[test]
    fn stale_before_state_safe_stops_without_overwriting() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        *service.host.state.lock().unwrap() = UserPathState {
            present: true,
            value: r"C:\Windows;D:\new".into(),
            expandable: true,
        };
        let record = service.apply(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::SafeStop);
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;D:\new"
        );
    }

    #[test]
    fn apply_failure_restores_the_exact_snapshot() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        *service.host.fail_writes_remaining.lock().unwrap() = 1;
        let record = service.apply(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::SafeStop);
        assert!(record.detail.contains("快照恢复成功"));
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;C:\Windows"
        );
    }

    #[test]
    fn verification_failure_never_reports_completion() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        *service.host.corrupt_after_write.lock().unwrap() = true;
        let record = service.apply(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::RepairedUnverified);
        assert!(record.detail.contains("不能声明完成"));
    }

    #[test]
    fn rollback_restores_exact_before_state_and_refuses_later_changes() {
        let (_dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        service.apply(&proposal.id, "ticket-1").unwrap();
        let record = service.rollback(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::RolledBack);
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;C:\Windows"
        );

        let proposal = service
            .propose_user_path_normalization("ticket-2", vec![])
            .unwrap()
            .unwrap();
        service.apply(&proposal.id, "ticket-2").unwrap();
        *service.host.state.lock().unwrap() = UserPathState {
            present: true,
            value: r"C:\Windows;D:\later".into(),
            expandable: true,
        };
        let record = service.rollback(&proposal.id, "ticket-2").unwrap();
        assert_eq!(record.proposal.status, RepairStatus::SafeStop);
        assert_eq!(
            service.host.read_user_path().unwrap().value,
            r"C:\Windows;D:\later"
        );
    }

    #[test]
    fn unknown_operation_and_tampered_record_are_rejected() {
        let (dir, service) = service(r"C:\Windows;C:\Windows");
        let proposal = service
            .propose_user_path_normalization("ticket-1", vec![])
            .unwrap()
            .unwrap();
        let path = dir.path().join(format!("{}.json", proposal.id));
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["proposal"]["operationId"] = Value::String("network.winhttp.reset".into());
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(service.apply(&proposal.id, "ticket-1").is_err());
    }

    #[test]
    fn r3_and_disabled_operations_never_enter_the_r2_executor() {
        let enabled = operation_registry()
            .iter()
            .filter(|operation| operation.enabled)
            .collect::<Vec<_>>();
        assert_eq!(enabled.len(), 1);
        assert!(enabled.iter().all(|operation| operation.risk == "r2"));
        assert!(operation_registry()
            .iter()
            .any(|operation| operation.id == "network.winhttp.reset"
                && operation.risk == "r3"
                && !operation.enabled));
        assert!(operation_registry()
            .iter()
            .any(|operation| operation.id == "file.utf8.restore"
                && operation.risk == "r2"
                && !operation.enabled));
    }
}
