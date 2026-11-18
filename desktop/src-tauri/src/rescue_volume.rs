use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const TOOL_ID: &str = "storage.volume.inspect";
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    explicit_volume_drive(request).map(|drive| (TOOL_ID.into(), json!({ "drive": drive })))
}

pub fn is_volume_request(request: &str) -> bool {
    explicit_volume_drive(request).is_some()
}

pub fn explicit_volume_drive(request: &str) -> Option<String> {
    let lower = request.to_lowercase();
    let capacity_intent = [
        "磁盘空间",
        "硬盘空间",
        "剩余空间",
        "可用空间",
        "空间不足",
        "空间不够",
        "盘满",
        "盘快满",
        "容量不足",
        "磁盘容量",
        "disk full",
        "low disk space",
        "disk space",
        "free space",
        "storage full",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !capacity_intent || contains_unsafe_scope(request) {
        return None;
    }

    let characters = request.chars().collect::<Vec<_>>();
    let mut drives = BTreeSet::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphabetic() {
            continue;
        }
        let drive = character.to_ascii_uppercase();
        if characters.get(index + 1) == Some(&':') {
            if characters
                .get(index + 2)
                .is_none_or(|next| safe_boundary(*next))
            {
                drives.insert(format!("{drive}:"));
            }
            continue;
        }
        let mut next = index + 1;
        while characters
            .get(next)
            .is_some_and(|item| item.is_whitespace())
        {
            next += 1;
        }
        if characters.get(next) == Some(&'盘') {
            drives.insert(format!("{drive}:"));
        }
    }
    if let Some(drive) = drive_after_marker(&lower, "drive") {
        drives.insert(drive);
    }
    (drives.len() == 1).then(|| drives.into_iter().next().unwrap())
}

fn contains_unsafe_scope(request: &str) -> bool {
    request.contains(['\n', '\r', ';', '|', '&', '`'])
        || request.contains("$(")
        || request.contains("\\\\")
        || request.contains("//")
}

fn safe_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ',' | '，' | '.' | '。' | ':' | '：' | '!' | '！' | '?' | '？' | ')' | '）'
        )
}

fn drive_after_marker(value: &str, marker: &str) -> Option<String> {
    let start = value.find(marker)? + marker.len();
    let tail = value.get(start..)?.trim_start();
    let mut characters = tail.chars();
    let drive = characters.next()?.to_ascii_uppercase();
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let remainder = characters.as_str();
    let remainder = remainder.strip_prefix(':').unwrap_or(remainder);
    remainder
        .chars()
        .next()
        .is_none_or(safe_boundary)
        .then(|| format!("{drive}:"))
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == TOOL_ID).then(|| run_volume(arguments))
}

fn run_volume(arguments: &Value) -> Result<ProbeOutput, String> {
    let drive = arguments
        .get("drive")
        .and_then(Value::as_str)
        .filter(|value| is_canonical_drive(value))
        .ok_or("卷容量探针需要单个规范盘符，例如 C:")?;
    let structured = volume_native::query(drive)?;
    let stdout = serde_json::to_vec(&structured)
        .map_err(|error| format!("无法序列化卷容量证据：{error}"))?;
    Ok(ProbeOutput {
        exit_code: 0,
        stdout: String::from_utf8_lossy(&stdout[..stdout.len().min(MAX_OUTPUT_BYTES)]).into(),
        stderr: String::new(),
    })
}

fn is_canonical_drive(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_uppercase() && bytes[1] == b':'
}

#[cfg(target_os = "windows")]
mod volume_native {
    use serde_json::{json, Value};

    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;
    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
        fn GetVolumeInformationW(
            root_path_name: *const u16,
            volume_name_buffer: *mut u16,
            volume_name_size: u32,
            volume_serial_number: *mut u32,
            maximum_component_length: *mut u32,
            file_system_flags: *mut u32,
            file_system_name_buffer: *mut u16,
            file_system_name_size: u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn query(drive: &str) -> Result<Value, String> {
        let root = format!("{drive}\\");
        let wide = root
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
        let drive_type_name = match drive_type {
            DRIVE_REMOVABLE => "removable",
            DRIVE_FIXED => "fixed",
            DRIVE_CDROM => "optical",
            DRIVE_RAMDISK => "ramdisk",
            DRIVE_REMOTE => return Err("卷容量探针拒绝网络盘".into()),
            DRIVE_NO_ROOT_DIR => return Err("指定盘符不存在".into()),
            DRIVE_UNKNOWN => return Err("Windows 无法识别指定卷类型".into()),
            _ => "unknown",
        };

        let mut available = 0u64;
        let mut total = 0u64;
        let mut total_free = 0u64;
        if unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut total_free)
        } == 0
        {
            return Err(format!("Windows 无法读取指定卷容量：错误 {}", unsafe {
                GetLastError()
            }));
        }
        if total == 0 {
            return Err("Windows 返回了零容量卷".into());
        }

        let mut flags = 0u32;
        if unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut flags,
                std::ptr::null_mut(),
                0,
            )
        } == 0
        {
            return Err(format!("Windows 无法读取指定卷属性：错误 {}", unsafe {
                GetLastError()
            }));
        }

        let used = total.saturating_sub(total_free);
        let used_percent = ((used as f64 / total as f64) * 1_000.0).round() / 10.0;
        Ok(json!({
            "drive": drive,
            "driveType": drive_type_name,
            "totalBytes": total,
            "availableBytes": available,
            "usedBytes": used,
            "usedPercent": used_percent,
            "readOnly": flags & FILE_READ_ONLY_VOLUME != 0,
        }))
    }
}

#[cfg(not(target_os = "windows"))]
mod volume_native {
    use serde_json::Value;

    pub fn query(_drive: &str) -> Result<Value, String> {
        Err("卷容量探针只支持 Windows".into())
    }
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence.iter().find(|item| item.tool_id == TOOL_ID)?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "storage.volume_capacity_unknown".into(),
                title: "无法确认指定本地卷的容量状态".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("卷容量探针不可用；未把失败误报为空间充足，也没有删除任何文件".into()),
        ));
    }

    let drive = item
        .structured
        .get("drive")
        .and_then(Value::as_str)
        .unwrap_or("指定卷");
    let available = item
        .structured
        .get("availableBytes")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total = item
        .structured
        .get("totalBytes")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if total == 0 {
        return Some((
            vec![Hypothesis {
                id: "storage.volume_capacity_unknown".into(),
                title: "卷容量证据不完整".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("没有足够的容量证据；未执行清理".into()),
        ));
    }
    let available_percent = available as f64 / total as f64 * 100.0;
    let severity = if available < 2 * GIB || available_percent < 2.0 {
        "critical"
    } else if available < 10 * GIB || available_percent < 10.0 {
        "constrained"
    } else {
        "healthy"
    };
    let available_gib = available as f64 / GIB as f64;
    let title = match severity {
        "critical" => format!("{drive} 可用空间严重不足，仅剩 {available_gib:.1} GiB"),
        "constrained" => format!("{drive} 可用空间偏低，剩余 {available_gib:.1} GiB"),
        _ => format!("{drive} 当前可用空间充足，剩余 {available_gib:.1} GiB"),
    };
    Some((
        vec![Hypothesis {
            id: "storage.volume_capacity_pressure".into(),
            title,
            state: if severity == "healthy" {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Supported
            },
            confidence: 0.99,
            evidence_ids: vec![item.id.clone()],
        }],
        Some(if severity == "healthy" {
            "容量不足不是当前已观测到的原因；如仍有故障，请提供具体错误和发生时间".into()
        } else {
            "请先审查可安全清理的具体目标；UCode 本轮没有枚举或删除任何文件".into()
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rescue::EvidenceState;
    use sha2::{Digest, Sha256};

    fn evidence(structured: Value, state: EvidenceState) -> EvidenceNode {
        let bytes = serde_json::to_vec(&structured).unwrap();
        EvidenceNode {
            id: "evidence-volume".into(),
            tool_id: TOOL_ID.into(),
            tool_version: 1,
            state,
            summary: "volume".into(),
            structured,
            exit_code: Some(0),
            content_digest: format!("{:x}", Sha256::digest(bytes)),
            elapsed_ms: 1,
        }
    }

    #[test]
    fn parser_accepts_one_explicit_drive_and_rejects_unsafe_scope() {
        for (request, expected) in [
            ("C盘满了，帮我检查剩余空间", Some("C:")),
            ("D: 空间不足，请诊断", Some("D:")),
            ("drive E is low disk space", Some("E:")),
            ("C:\\Windows 很大，帮我读取", None),
            ("C盘和D盘空间不足", None),
            ("\\\\server\\share 空间不足", None),
            ("C:; Remove-Item -Recurse 空间不足", None),
            ("磁盘坏了", None),
            ("C盘很慢", None),
        ] {
            assert_eq!(
                explicit_volume_drive(request).as_deref(),
                expected,
                "{request}"
            );
        }
    }

    #[test]
    fn analysis_distinguishes_healthy_constrained_critical_and_unavailable() {
        for (available, expected) in [
            (40 * GIB, HypothesisState::Contradicted),
            (8 * GIB, HypothesisState::Supported),
            (GIB, HypothesisState::Supported),
        ] {
            let item = evidence(
                json!({"drive":"C:","totalBytes":100 * GIB,"availableBytes":available}),
                EvidenceState::Observed,
            );
            let (hypotheses, _) = analyze(&[item]).unwrap();
            assert!(
                std::mem::discriminant(&hypotheses[0].state) == std::mem::discriminant(&expected)
            );
        }
        let unavailable = evidence(json!({"error":"denied"}), EvidenceState::Unavailable);
        let (hypotheses, next) = analyze(&[unavailable]).unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        assert!(next.unwrap().contains("未把失败误报"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_probe_reads_one_local_volume_without_names_or_files() {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let output = run_volume(&json!({"drive":drive})).unwrap();
        assert_eq!(output.exit_code, 0);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        assert!(value["totalBytes"].as_u64().unwrap() > 0);
        assert!(value["availableBytes"].as_u64().unwrap() <= value["totalBytes"].as_u64().unwrap());
        assert!(value["usedBytes"].as_u64().unwrap() <= value["totalBytes"].as_u64().unwrap());
        assert!((0.0..=100.0).contains(&value["usedPercent"].as_f64().unwrap()));
        for private in [
            "volumeLabel",
            "serialNumber",
            "fileSystem",
            "files",
            "directories",
        ] {
            assert!(
                value.get(private).is_none(),
                "private field leaked: {private}"
            );
        }
    }
}
