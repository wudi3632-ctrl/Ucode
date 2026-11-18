use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::Value;

const TOOL_ID: &str = "system.memory.inspect";
const MAX_PROCESSES: usize = 10;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

pub fn is_memory_request(request: &str) -> bool {
    let value = request.to_lowercase();
    let phrase_match = [
        "内存不足",
        "内存不够",
        "内存占用",
        "内存爆满",
        "内存泄漏",
        "内存压力",
        "out of memory",
        "low memory",
        "memory pressure",
        "high memory usage",
        "ram usage",
        "ram is full",
    ]
    .iter()
    .any(|marker| value.contains(marker));
    let oom_token = value
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|token| token == "oom");
    phrase_match || oom_token
}

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    is_memory_request(request).then(|| (TOOL_ID.into(), Value::Null))
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == TOOL_ID).then(|| run_memory(arguments))
}

fn run_memory(arguments: &Value) -> Result<ProbeOutput, String> {
    if !(arguments.is_null() || arguments.as_object().is_some_and(|value| value.is_empty())) {
        return Err("内存探针不接受模型参数".into());
    }
    let structured = memory_native::query()?;
    let stdout =
        serde_json::to_vec(&structured).map_err(|error| format!("无法序列化内存证据：{error}"))?;
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err("内存证据超出有界输出限制".into());
    }
    Ok(ProbeOutput {
        exit_code: 0,
        stdout: String::from_utf8(stdout)
            .map_err(|error| format!("内存证据不是有效 UTF-8：{error}"))?,
        stderr: String::new(),
    })
}

#[cfg(target_os = "windows")]
mod memory_native {
    use super::MAX_PROCESSES;
    use serde::Serialize;
    use serde_json::{json, Value};
    use std::cmp::Reverse;

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const INVALID_HANDLE_VALUE: isize = -1;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        thread_count: u32,
        parent_process_id: u32,
        priority_class_base: i32,
        flags: u32,
        executable: [u16; 260],
    }

    #[repr(C)]
    struct ProcessMemoryCounters {
        size: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ProcessMemory {
        pid: u32,
        process_name: String,
        working_set_bytes: u64,
        owner_kind: &'static str,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
    }

    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    struct Handle(isize);
    impl Drop for Handle {
        fn drop(&mut self) {
            if self.0 != 0 && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    pub fn query() -> Result<Value, String> {
        let mut status: MemoryStatusEx = unsafe { std::mem::zeroed() };
        status.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return Err(format!("Windows 无法读取内存状态：错误 {}", unsafe {
                GetLastError()
            }));
        }
        if status.total_physical == 0 || status.available_physical > status.total_physical {
            return Err("Windows 返回了无效的物理内存状态".into());
        }

        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!("Windows 无法枚举进程：错误 {}", unsafe {
                GetLastError()
            }));
        }
        let snapshot = Handle(snapshot);
        let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut processes = Vec::new();
        let mut inaccessible = 0u32;
        let mut has_entry = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while has_entry {
            let pid = entry.process_id;
            let name = executable_name(&entry.executable);
            if pid != 0 && !name.is_empty() {
                if let Some(working_set_bytes) = working_set(pid) {
                    let owner_kind = if pid == 4 || name.eq_ignore_ascii_case("system") {
                        "system"
                    } else if name.eq_ignore_ascii_case("svchost") {
                        "service_host"
                    } else {
                        "unknown"
                    };
                    processes.push(ProcessMemory {
                        pid,
                        process_name: name,
                        working_set_bytes,
                        owner_kind,
                    });
                } else {
                    inaccessible = inaccessible.saturating_add(1);
                }
            }
            has_entry = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        processes.sort_by_key(|item| (Reverse(item.working_set_bytes), item.pid));
        let truncated = processes.len() > MAX_PROCESSES;
        processes.truncate(MAX_PROCESSES);

        Ok(json!({
            "memoryLoadPercent": status.memory_load,
            "totalPhysicalBytes": status.total_physical,
            "availablePhysicalBytes": status.available_physical,
            "totalPageFileBytes": status.total_page_file,
            "availablePageFileBytes": status.available_page_file,
            "topProcesses": processes,
            "inaccessibleProcessCount": inaccessible,
            "truncated": truncated,
        }))
    }

    fn working_set(pid: u32) -> Option<u64> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle == 0 {
            return None;
        }
        let handle = Handle(handle);
        let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        counters.size = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        (unsafe {
            GetProcessMemoryInfo(
                handle.0,
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        } != 0)
            .then_some(counters.working_set_size as u64)
    }

    fn executable_name(buffer: &[u16; 260]) -> String {
        let length = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        let mut name: String = String::from_utf16_lossy(&buffer[..length])
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        if name.to_ascii_lowercase().ends_with(".exe") {
            name.truncate(name.len().saturating_sub(4));
        }
        name.chars().take(128).collect()
    }
}

#[cfg(not(target_os = "windows"))]
mod memory_native {
    use serde_json::Value;

    pub fn query() -> Result<Value, String> {
        Err("内存探针只支持 Windows".into())
    }
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence.iter().find(|item| item.tool_id == TOOL_ID)?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "system.memory_pressure_unknown".into(),
                title: "无法确认当前内存压力".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("内存探针不可用；未把失败误报为内存正常，也没有结束任何进程".into()),
        ));
    }

    let load = item
        .structured
        .get("memoryLoadPercent")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let available = item
        .structured
        .get("availablePhysicalBytes")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total = item
        .structured
        .get("totalPhysicalBytes")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if total == 0 || available > total {
        return Some((
            vec![Hypothesis {
                id: "system.memory_pressure_unknown".into(),
                title: "内存证据不完整".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("没有足够的内存证据；未结束任何进程".into()),
        ));
    }
    let severity = if load >= 90 || available < 512 * MIB {
        "critical"
    } else if load >= 80 || available < GIB {
        "constrained"
    } else {
        "healthy"
    };
    let available_gib = available as f64 / GIB as f64;
    let top = item
        .structured
        .get("topProcesses")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|process| process.get("processName"))
        .and_then(Value::as_str);
    let title = match severity {
        "critical" => format!(
            "当前内存压力严重：已用 {load}%，可用 {available_gib:.1} GiB{}",
            top.map(|name| format!("，最大工作集进程为 {name}"))
                .unwrap_or_default()
        ),
        "constrained" => format!(
            "当前内存压力偏高：已用 {load}%，可用 {available_gib:.1} GiB{}",
            top.map(|name| format!("，最大工作集进程为 {name}"))
                .unwrap_or_default()
        ),
        _ => format!("当前内存未达到压力阈值：已用 {load}%，可用 {available_gib:.1} GiB"),
    };
    Some((
        vec![Hypothesis {
            id: "system.memory_pressure".into(),
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
            "当前快照没有显示全局内存压力；如问题间歇出现，需要在复现时重新采样".into()
        } else {
            "请先确认高工作集进程是否为预期任务；UCode 本轮没有结束、挂起或重启任何进程".into()
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rescue::EvidenceState;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn evidence(structured: Value, state: EvidenceState) -> EvidenceNode {
        let bytes = serde_json::to_vec(&structured).unwrap();
        EvidenceNode {
            id: "evidence-memory".into(),
            tool_id: TOOL_ID.into(),
            tool_version: 1,
            state,
            summary: "memory".into(),
            structured,
            exit_code: Some(0),
            content_digest: format!("{:x}", Sha256::digest(bytes)),
            elapsed_ms: 1,
        }
    }

    #[test]
    fn parser_accepts_explicit_memory_pressure_and_rejects_generic_slowness() {
        for (request, expected) in [
            ("内存不足，帮我检查", true),
            ("内存占用 95%，谁占得最多", true),
            ("应用提示 out of memory", true),
            ("high memory usage after startup", true),
            ("OOM 错误", true),
            ("how much room is available", false),
            ("电脑很慢，请检查", false),
            ("进程异常", false),
        ] {
            assert_eq!(is_memory_request(request), expected, "{request}");
        }
    }

    #[test]
    fn analysis_distinguishes_healthy_constrained_critical_and_unavailable() {
        for (load, available, expected) in [
            (50, 8 * GIB, HypothesisState::Contradicted),
            (82, 3 * GIB, HypothesisState::Supported),
            (92, 256 * MIB, HypothesisState::Supported),
        ] {
            let item = evidence(
                json!({
                    "memoryLoadPercent":load,
                    "totalPhysicalBytes":16 * GIB,
                    "availablePhysicalBytes":available,
                    "topProcesses":[{"pid":42,"processName":"node","workingSetBytes":GIB,"ownerKind":"user_process"}]
                }),
                EvidenceState::Observed,
            );
            let (hypotheses, _) = analyze(&[item]).unwrap();
            assert_eq!(
                std::mem::discriminant(&hypotheses[0].state),
                std::mem::discriminant(&expected)
            );
        }
        let unavailable = evidence(json!({"error":"denied"}), EvidenceState::Unavailable);
        let (hypotheses, next) = analyze(&[unavailable]).unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        assert!(next.unwrap().contains("未把失败误报"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_probe_returns_bounded_memory_and_process_metadata() {
        let output = run_memory(&Value::Null).unwrap();
        assert_eq!(output.exit_code, 0);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        let load = value["memoryLoadPercent"].as_u64().unwrap();
        let total = value["totalPhysicalBytes"].as_u64().unwrap();
        let available = value["availablePhysicalBytes"].as_u64().unwrap();
        assert!(load <= 100 && total > 0 && available <= total);
        let processes = value["topProcesses"].as_array().unwrap();
        assert!(processes.len() <= MAX_PROCESSES);
        for process in processes {
            let name = process["processName"].as_str().unwrap();
            assert!(!name.contains(['\\', '/']));
            assert!(process["pid"].as_u64().unwrap() > 0);
            assert!(process["workingSetBytes"].as_u64().is_some());
        }
        let raw = value.to_string();
        for private in ["commandLine", "executablePath", "username", "windowTitle"] {
            assert!(!raw.contains(private), "private field leaked: {private}");
        }
    }
}
