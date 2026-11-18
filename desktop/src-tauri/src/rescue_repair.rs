use crate::rescue_elevated::{
    DisabledElevatedRepairExecutor, ElevatedAction, ElevatedRepairExecutor,
    WindowsElevatedRepairExecutor, MACHINE_PATH_OPERATION, MACHINE_PATH_OPERATION_VERSION,
    MACHINE_PATH_TARGET,
};
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
            id: MACHINE_PATH_OPERATION,
            version: MACHINE_PATH_OPERATION_VERSION,
            risk: "r3",
            authority: "elevated_helper",
            enabled: true,
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

pub struct RepairService<
    H: RepairHost = WindowsRepairHost,
    E: ElevatedRepairExecutor = WindowsElevatedRepairExecutor,
> {
    root: PathBuf,
    host: H,
    elevated: E,
}

impl RepairService<WindowsRepairHost, WindowsElevatedRepairExecutor> {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        Self::with_hosts(root, WindowsRepairHost, WindowsElevatedRepairExecutor)
    }
}

impl<H: RepairHost> RepairService<H, DisabledElevatedRepairExecutor> {
    pub fn with_host(root: PathBuf, host: H) -> Result<Self, String> {
        Self::with_hosts(root, host, DisabledElevatedRepairExecutor)
    }
}

impl<H: RepairHost, E: ElevatedRepairExecutor> RepairService<H, E> {
    pub fn with_hosts(root: PathBuf, host: H, elevated: E) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| format!("无法创建修复证据目录：{error}"))?;
        Ok(Self {
            root,
            host,
            elevated,
        })
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

    pub fn propose_machine_path_normalization(
        &self,
        ticket_id: &str,
        evidence_ids: Vec<String>,
    ) -> Result<Option<RepairProposal>, String> {
        let before = self.elevated.read_machine_path()?;
        let (after, removed) = normalize_user_path(&before);
        if before == after {
            return Ok(None);
        }
        let now = Utc::now();
        let mut proposal = RepairProposal {
            schema_version: REPAIR_SCHEMA_VERSION,
            id: format!("repair-{}", Uuid::new_v4()),
            ticket_id: ticket_id.into(),
            operation_id: MACHINE_PATH_OPERATION.into(),
            operation_version: MACHINE_PATH_OPERATION_VERSION,
            risk: "r3".into(),
            target: MACHINE_PATH_TARGET.into(),
            normalized_arguments: json!({ "after": after, "removedEntries": removed }),
            evidence_ids,
            prerequisites: vec![
                "机器 PATH 与诊断及审批时摘要一致".into(),
                "用户在 Windows 安全桌面明确批准本次管理员操作".into(),
            ],
            snapshot_plan: "在便携证据目录保存原值、注册表值类型和 SHA-256；helper 写入前再次核对"
                .into(),
            expected_postcondition: "机器 PATH 不含空项、重复项和明确不存在的绝对目录".into(),
            verification_probe:
                "管理员 helper 写入后重读 HKLM PATH，未提权宿主再独立重读并核对摘要".into(),
            rollback_contract: "回滚需要再次确认 UAC；仅当当前值仍等于本次应用后摘要时恢复原值"
                .into(),
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
            detail: "等待用户审批；应用时 Windows 将显示 UAC 管理员确认".into(),
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
        if record.proposal.operation_id == MACHINE_PATH_OPERATION {
            return self.apply_machine_path(record);
        }
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

    fn apply_machine_path(&self, mut record: RepairRecord) -> Result<RepairRecord, String> {
        let before = self.elevated.read_machine_path()?;
        if digest_json(&before) != record.proposal.before_digest {
            return self.safe_stop(record, "审批前机器 PATH 已经变化，已拒绝提权写入");
        }
        let after: UserPathState = serde_json::from_value(
            record
                .proposal
                .normalized_arguments
                .get("after")
                .cloned()
                .ok_or("R3 修复参数缺少 after")?,
        )
        .map_err(|_| "R3 修复参数格式无效".to_string())?;
        if digest_json(&after) != record.proposal.expected_after_digest {
            return self.safe_stop(record, "R3 修复参数摘要不匹配，已拒绝提权");
        }

        record.proposal.status = RepairStatus::Applying;
        record.detail = "等待 Windows UAC；批准后短生命周期 helper 将应用机器 PATH 修复".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
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
            applied_at: Utc::now(),
            verified_at: None,
        };
        snapshot.snapshot_digest = snapshot_digest(&snapshot);
        record.snapshot = Some(snapshot.clone());
        self.save_record(&record)?;

        let observed = match self.elevated.mutate_machine_path(
            &self.root,
            &record.proposal,
            &before,
            &after,
            ElevatedAction::Apply,
        ) {
            Ok(value) => value,
            Err(error) => {
                return self.reconcile_machine_path_failure(record, &before, &after, &error)
            }
        };
        record.proposal.status = RepairStatus::RepairedUnverified;
        record.detail = "管理员 helper 已返回，未提权宿主正在独立重读验证".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        let independently_observed = match self.elevated.read_machine_path() {
            Ok(value) => value,
            Err(error) => {
                record.detail = format!(
                    "管理员 helper 已返回目标状态，但未提权宿主无法完成独立重读：{error}；修改保持未验证，可在当前摘要仍匹配时回滚"
                );
                record.updated_at = Utc::now();
                self.save_record(&record)?;
                return Ok(record);
            }
        };
        if digest_json(&observed) != snapshot.after_digest
            || digest_json(&independently_observed) != snapshot.after_digest
        {
            record.detail = "R3 修复返回后独立验证失败；不能声明完成".into();
            record.updated_at = Utc::now();
            self.save_record(&record)?;
            return Ok(record);
        }
        if let Some(value) = record.snapshot.as_mut() {
            value.verified_at = Some(Utc::now());
            value.snapshot_digest = snapshot_digest(value);
        }
        record.proposal.status = RepairStatus::Verified;
        record.detail =
            "机器 PATH 已由管理员 helper 写入，并通过 helper 与未提权宿主双重重读验证".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    fn reconcile_machine_path_failure(
        &self,
        mut record: RepairRecord,
        before: &UserPathState,
        after: &UserPathState,
        helper_error: &str,
    ) -> Result<RepairRecord, String> {
        match self.elevated.read_machine_path() {
            Ok(current) if digest_json(&current) == digest_json(before) => self.safe_stop(
                record,
                &format!("R3 helper 未完成修复，机器 PATH 仍为原摘要：{helper_error}"),
            ),
            Ok(current) if digest_json(&current) == digest_json(after) => {
                record.proposal.status = RepairStatus::RepairedUnverified;
                record.detail = format!(
                    "R3 helper 响应未完成，但未提权复核发现机器 PATH 已等于目标摘要：{helper_error}；未声明双重验证完成，可安全回滚"
                );
                record.updated_at = Utc::now();
                self.save_record(&record)?;
                Ok(record)
            }
            Ok(_) => {
                record.proposal.status = RepairStatus::RepairedUnverified;
                record.detail = format!(
                    "R3 helper 响应未完成，且机器 PATH 与审批前后摘要都不一致：{helper_error}；已停止自动操作，请人工检查"
                );
                record.updated_at = Utc::now();
                self.save_record(&record)?;
                Ok(record)
            }
            Err(read_error) => {
                record.proposal.status = RepairStatus::RepairedUnverified;
                record.detail = format!(
                    "R3 helper 响应未完成，且无法复核机器 PATH：{helper_error}；复核错误：{read_error}；未声明系统未修改"
                );
                record.updated_at = Utc::now();
                self.save_record(&record)?;
                Ok(record)
            }
        }
    }

    pub fn rollback(&self, proposal_id: &str, ticket_id: &str) -> Result<RepairRecord, String> {
        let mut record = self.load_record(proposal_id)?;
        self.validate_identity(&record.proposal, ticket_id)?;
        if !matches!(
            record.proposal.status,
            RepairStatus::Verified | RepairStatus::RepairedUnverified
        ) {
            return Err("只有已写入且保留完整快照的修复可以回滚".into());
        }
        if record.proposal.operation_id == MACHINE_PATH_OPERATION {
            return self.rollback_machine_path(record);
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

    fn rollback_machine_path(&self, mut record: RepairRecord) -> Result<RepairRecord, String> {
        let snapshot = record.snapshot.clone().ok_or("R3 修复快照不存在")?;
        if snapshot.snapshot_digest != snapshot_digest(&snapshot) {
            return Err("R3 修复快照摘要无效，已拒绝回滚".into());
        }
        let current = self.elevated.read_machine_path()?;
        if digest_json(&current) != snapshot.after_digest {
            return self.safe_stop(record, "机器 PATH 在修复后又被修改，已拒绝覆盖后续更改");
        }
        let before: UserPathState = serde_json::from_value(snapshot.before_state.clone())
            .map_err(|_| "R3 快照 before state 无效".to_string())?;
        let observed = match self.elevated.mutate_machine_path(
            &self.root,
            &record.proposal,
            &current,
            &before,
            ElevatedAction::Rollback,
        ) {
            Ok(value) => value,
            Err(error) => {
                record.proposal.status = RepairStatus::RollbackFailed;
                record.detail = format!("R3 helper 回滚失败：{error}");
                record.updated_at = Utc::now();
                self.save_record(&record)?;
                return Ok(record);
            }
        };
        let independently_observed = self.elevated.read_machine_path()?;
        if digest_json(&observed) != snapshot.before_digest
            || digest_json(&independently_observed) != snapshot.before_digest
        {
            record.proposal.status = RepairStatus::RollbackFailed;
            record.detail = "R3 回滚后独立重读摘要与原始快照不一致".into();
        } else {
            record.proposal.status = RepairStatus::RolledBack;
            record.detail = "机器 PATH 已通过第二次 UAC 确认恢复到审批前精确状态".into();
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
                && operation.risk == proposal.risk
        });
        let known_target = match proposal.operation_id.as_str() {
            USER_PATH_OPERATION => {
                proposal.risk == "r2" && proposal.target == USER_ENVIRONMENT_TARGET
            }
            MACHINE_PATH_OPERATION => {
                proposal.risk == "r3" && proposal.target == MACHINE_PATH_TARGET
            }
            _ => false,
        };
        if proposal.schema_version != REPAIR_SCHEMA_VERSION || registered.is_none() || !known_target
        {
            return Err("未知、过期或未启用的 Rescue 修复操作，已拒绝执行".into());
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

    struct FakeElevatedExecutor {
        state: Mutex<UserPathState>,
        calls: Mutex<Vec<ElevatedAction>>,
        fail_after_write: Mutex<bool>,
    }

    impl FakeElevatedExecutor {
        fn new(value: &str) -> Self {
            Self {
                state: Mutex::new(UserPathState {
                    present: true,
                    value: value.into(),
                    expandable: true,
                }),
                calls: Mutex::new(Vec::new()),
                fail_after_write: Mutex::new(false),
            }
        }
    }

    impl ElevatedRepairExecutor for FakeElevatedExecutor {
        fn read_machine_path(&self) -> Result<UserPathState, String> {
            Ok(self.state.lock().unwrap().clone())
        }

        fn mutate_machine_path(
            &self,
            _root: &Path,
            proposal: &RepairProposal,
            expected_current: &UserPathState,
            desired: &UserPathState,
            action: ElevatedAction,
        ) -> Result<UserPathState, String> {
            assert_eq!(proposal.operation_id, MACHINE_PATH_OPERATION);
            let mut state = self.state.lock().unwrap();
            if *state != *expected_current {
                return Err("stale".into());
            }
            *state = desired.clone();
            self.calls.lock().unwrap().push(action);
            if *self.fail_after_write.lock().unwrap() {
                *self.fail_after_write.lock().unwrap() = false;
                return Err("injected response loss".into());
            }
            Ok(state.clone())
        }
    }

    fn service(
        value: &str,
    ) -> (
        TempDir,
        RepairService<FakeHost, DisabledElevatedRepairExecutor>,
    ) {
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
    fn machine_path_uses_r3_executor_and_requires_a_second_elevated_rollback() {
        let dir = TempDir::new().unwrap();
        let service = RepairService::with_hosts(
            dir.path().to_path_buf(),
            FakeHost::new(r"C:\Windows"),
            FakeElevatedExecutor::new(r"C:\Windows;;C:\Windows"),
        )
        .unwrap();
        let proposal = service
            .propose_machine_path_normalization("ticket-r3", vec!["evidence-r3".into()])
            .unwrap()
            .unwrap();
        assert_eq!(proposal.risk, "r3");
        assert_eq!(proposal.operation_id, MACHINE_PATH_OPERATION);
        assert_eq!(service.elevated.calls.lock().unwrap().len(), 0);

        let applied = service.apply(&proposal.id, "ticket-r3").unwrap();
        assert_eq!(applied.proposal.status, RepairStatus::Verified);
        assert_eq!(
            service.elevated.read_machine_path().unwrap().value,
            r"C:\Windows"
        );
        assert_eq!(
            service.elevated.calls.lock().unwrap().as_slice(),
            &[ElevatedAction::Apply]
        );

        let rolled_back = service.rollback(&proposal.id, "ticket-r3").unwrap();
        assert_eq!(rolled_back.proposal.status, RepairStatus::RolledBack);
        assert_eq!(
            service.elevated.read_machine_path().unwrap().value,
            r"C:\Windows;;C:\Windows"
        );
        assert_eq!(
            service.elevated.calls.lock().unwrap().as_slice(),
            &[ElevatedAction::Apply, ElevatedAction::Rollback]
        );
    }

    #[test]
    fn lost_helper_response_preserves_an_unverified_but_rollbackable_r3_state() {
        let dir = TempDir::new().unwrap();
        let service = RepairService::with_hosts(
            dir.path().to_path_buf(),
            FakeHost::new(r"C:\user"),
            FakeElevatedExecutor::new(r"C:\Windows;C:\Windows"),
        )
        .unwrap();
        let proposal = service
            .propose_machine_path_normalization("ticket-r3-loss", vec!["evidence-r3".into()])
            .unwrap()
            .unwrap();
        *service.elevated.fail_after_write.lock().unwrap() = true;

        let applied = service.apply(&proposal.id, "ticket-r3-loss").unwrap();
        assert_eq!(applied.proposal.status, RepairStatus::RepairedUnverified);
        assert!(applied.detail.contains("已等于目标摘要"));

        let rolled_back = service.rollback(&proposal.id, "ticket-r3-loss").unwrap();
        assert_eq!(rolled_back.proposal.status, RepairStatus::RolledBack);
        assert_eq!(
            service.elevated.read_machine_path().unwrap().value,
            r"C:\Windows;C:\Windows"
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
    fn operation_registry_separates_r2_r3_and_disabled_operations() {
        let enabled = operation_registry()
            .iter()
            .filter(|operation| operation.enabled)
            .collect::<Vec<_>>();
        assert_eq!(enabled.len(), 2);
        assert_eq!(
            enabled
                .iter()
                .filter(|operation| operation.risk == "r2")
                .count(),
            1
        );
        assert!(operation_registry()
            .iter()
            .any(|operation| operation.id == "network.winhttp.reset"
                && operation.risk == "r3"
                && !operation.enabled));
        assert!(operation_registry()
            .iter()
            .any(|operation| operation.id == MACHINE_PATH_OPERATION
                && operation.risk == "r3"
                && operation.enabled));
        assert!(operation_registry()
            .iter()
            .any(|operation| operation.id == "file.utf8.restore"
                && operation.risk == "r2"
                && !operation.enabled));
    }
}
