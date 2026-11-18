use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::Value;

const TOOL_ID: &str = "system.cpu.inspect";
const MAX_PROCESSES: usize = 10;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

pub fn is_cpu_request(request: &str) -> bool {
    let value = request.to_lowercase();
    let explanatory = [
        "什么是",
        "是什么意思",
        "怎么理解",
        "what is",
        "explain",
        "介绍",
    ]
    .iter()
    .any(|marker| value.contains(marker));
    if explanatory {
        return false;
    }
    [
        "cpu占用",
        "cpu 占用",
        "cpu使用率",
        "cpu 使用率",
        "cpu爆满",
        "cpu 爆满",
        "cpu跑满",
        "cpu 跑满",
        "cpu过高",
        "cpu 过高",
        "cpu 100",
        "cpu100",
        "cpu at 100",
        "处理器占用",
        "处理器使用率",
        "处理器负载",
        "处理器 100",
        "处理器100",
        "high cpu usage",
        "cpu usage high",
        "cpu load high",
        "processor usage",
        "processor load",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    is_cpu_request(request).then(|| (TOOL_ID.into(), Value::Null))
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == TOOL_ID).then(|| run_cpu(arguments))
}

fn run_cpu(arguments: &Value) -> Result<ProbeOutput, String> {
    if !(arguments.is_null() || arguments.as_object().is_some_and(|value| value.is_empty())) {
        return Err("CPU 探针不接受模型参数".into());
    }
    let structured = cpu_native::query()?;
    let stdout =
        serde_json::to_vec(&structured).map_err(|error| format!("无法序列化 CPU 证据：{error}"))?;
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err("CPU 证据超出有界输出限制".into());
    }
    Ok(ProbeOutput {
        exit_code: 0,
        stdout: String::from_utf8(stdout)
            .map_err(|error| format!("CPU 证据不是有效 UTF-8：{error}"))?,
        stderr: String::new(),
    })
}

#[cfg(target_os = "windows")]
mod cpu_native {
    use super::MAX_PROCESSES;
    use serde::Serialize;
    use serde_json::{json, Value};
    use std::cmp::Ordering;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const INVALID_HANDLE_VALUE: isize = -1;
    const SAMPLE_DURATION: Duration = Duration::from_millis(500);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
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

    #[derive(Debug, Clone)]
    struct SystemTimes {
        idle: u64,
        kernel: u64,
        user: u64,
    }

    #[derive(Debug, Clone)]
    struct ProcessTimes {
        name: String,
        creation: u64,
        cpu: u64,
    }

    struct ProcessSnapshot {
        items: HashMap<u32, ProcessTimes>,
        inaccessible: u32,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ProcessCpu {
        pid: u32,
        process_name: String,
        cpu_percent: f64,
        owner_kind: &'static str,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
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
        let started = Instant::now();
        let system_before = system_times()?;
        let processes_before = process_times()?;
        std::thread::sleep(SAMPLE_DURATION);
        let processes_after = process_times()?;
        let system_after = system_times()?;
        let sample_window_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let (total_delta, cpu_load) = calculate_system_load(&system_before, &system_after)
            .ok_or_else(|| "Windows 返回了无效的 CPU 采样时间".to_string())?;

        let mut disappeared = 0u32;
        let mut processes = Vec::new();
        for (pid, before) in &processes_before.items {
            let Some(after) = processes_after.items.get(pid) else {
                disappeared = disappeared.saturating_add(1);
                continue;
            };
            let Some(process_delta) = matching_process_delta(before, after) else {
                disappeared = disappeared.saturating_add(1);
                continue;
            };
            if process_delta == 0 {
                continue;
            }
            let cpu_percent =
                round_tenth((100.0 * process_delta as f64 / total_delta as f64).clamp(0.0, 100.0));
            let owner_kind = if *pid == 4 || after.name.eq_ignore_ascii_case("system") {
                "system"
            } else if after.name.eq_ignore_ascii_case("svchost") {
                "service_host"
            } else {
                "unknown"
            };
            processes.push(ProcessCpu {
                pid: *pid,
                process_name: after.name.clone(),
                cpu_percent,
                owner_kind,
            });
        }
        processes.sort_by(|left, right| {
            right
                .cpu_percent
                .partial_cmp(&left.cpu_percent)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.pid.cmp(&right.pid))
        });
        let truncated = processes.len() > MAX_PROCESSES;
        processes.truncate(MAX_PROCESSES);

        Ok(json!({
            "cpuLoadPercent": cpu_load,
            "sampleWindowMs": sample_window_ms,
            "topProcesses": processes,
            "inaccessibleProcessCount": processes_after.inaccessible,
            "disappearedProcessCount": disappeared,
            "truncated": truncated,
        }))
    }

    fn system_times() -> Result<SystemTimes, String> {
        let mut idle = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return Err(format!("Windows 无法读取 CPU 状态：错误 {}", unsafe {
                GetLastError()
            }));
        }
        Ok(SystemTimes {
            idle: file_time(idle),
            kernel: file_time(kernel),
            user: file_time(user),
        })
    }

    fn process_times() -> Result<ProcessSnapshot, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!("Windows 无法枚举进程：错误 {}", unsafe {
                GetLastError()
            }));
        }
        let snapshot = Handle(snapshot);
        let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut items = HashMap::new();
        let mut inaccessible = 0u32;
        let mut has_entry = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while has_entry {
            let pid = entry.process_id;
            let name = executable_name(&entry.executable);
            if pid != 0 && !name.is_empty() {
                if let Some((creation, cpu)) = query_process(pid) {
                    items.insert(
                        pid,
                        ProcessTimes {
                            name,
                            creation,
                            cpu,
                        },
                    );
                } else {
                    inaccessible = inaccessible.saturating_add(1);
                }
            }
            has_entry = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        Ok(ProcessSnapshot {
            items,
            inaccessible,
        })
    }

    fn query_process(pid: u32) -> Option<(u64, u64)> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle == 0 {
            return None;
        }
        let handle = Handle(handle);
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        (unsafe { GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user) }
            != 0)
            .then_some((
                file_time(creation),
                file_time(kernel).saturating_add(file_time(user)),
            ))
    }

    fn file_time(value: FileTime) -> u64 {
        ((value.high as u64) << 32) | value.low as u64
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

    fn round_tenth(value: f64) -> f64 {
        (value * 10.0).round() / 10.0
    }

    fn calculate_system_load(before: &SystemTimes, after: &SystemTimes) -> Option<(u64, f64)> {
        let total_before = before.kernel.checked_add(before.user)?;
        let total_after = after.kernel.checked_add(after.user)?;
        let total_delta = total_after.checked_sub(total_before)?;
        let idle_delta = after.idle.checked_sub(before.idle)?;
        if total_delta == 0 || idle_delta > total_delta {
            return None;
        }
        Some((
            total_delta,
            round_tenth(100.0 * (total_delta - idle_delta) as f64 / total_delta as f64),
        ))
    }

    fn matching_process_delta(before: &ProcessTimes, after: &ProcessTimes) -> Option<u64> {
        (before.creation == after.creation)
            .then(|| after.cpu.checked_sub(before.cpu))
            .flatten()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn file_time_and_rounding_are_stable() {
            assert_eq!(file_time(FileTime { low: 5, high: 1 }), (1_u64 << 32) + 5);
            assert_eq!(round_tenth(12.345), 12.3);
            assert_eq!(round_tenth(12.36), 12.4);
        }

        #[test]
        fn system_load_rejects_zero_regression_and_impossible_idle_time() {
            let before = SystemTimes {
                idle: 20,
                kernel: 60,
                user: 40,
            };
            assert!(calculate_system_load(&before, &before).is_none());
            assert!(calculate_system_load(
                &before,
                &SystemTimes {
                    idle: 19,
                    kernel: 70,
                    user: 50,
                }
            )
            .is_none());
            assert!(calculate_system_load(
                &before,
                &SystemTimes {
                    idle: 50,
                    kernel: 70,
                    user: 50,
                }
            )
            .is_none());
            let (_, load) = calculate_system_load(
                &before,
                &SystemTimes {
                    idle: 25,
                    kernel: 70,
                    user: 50,
                },
            )
            .unwrap();
            assert_eq!(load, 75.0);
        }

        #[test]
        fn process_delta_requires_matching_creation_time_and_monotonic_cpu() {
            let before = ProcessTimes {
                name: "node".into(),
                creation: 100,
                cpu: 200,
            };
            assert_eq!(
                matching_process_delta(
                    &before,
                    &ProcessTimes {
                        name: "node".into(),
                        creation: 100,
                        cpu: 260,
                    }
                ),
                Some(60)
            );
            assert!(matching_process_delta(
                &before,
                &ProcessTimes {
                    name: "other".into(),
                    creation: 101,
                    cpu: 260,
                }
            )
            .is_none());
            assert!(matching_process_delta(
                &before,
                &ProcessTimes {
                    name: "node".into(),
                    creation: 100,
                    cpu: 199,
                }
            )
            .is_none());
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod cpu_native {
    use serde_json::Value;

    pub fn query() -> Result<Value, String> {
        Err("CPU 探针只支持 Windows".into())
    }
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence.iter().find(|item| item.tool_id == TOOL_ID)?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "system.cpu_pressure_unknown".into(),
                title: "无法确认当前 CPU 压力".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("CPU 探针不可用；未把失败误报为 CPU 正常，也没有结束任何进程".into()),
        ));
    }

    let load = item
        .structured
        .get("cpuLoadPercent")
        .and_then(Value::as_f64);
    let Some(load) = load.filter(|value| value.is_finite() && (0.0..=100.0).contains(value)) else {
        return Some((
            vec![Hypothesis {
                id: "system.cpu_pressure_unknown".into(),
                title: "CPU 证据不完整".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("没有足够的 CPU 证据；未结束任何进程".into()),
        ));
    };
    let severity = if load >= 90.0 {
        "critical"
    } else if load >= 75.0 {
        "constrained"
    } else {
        "healthy"
    };
    let top = item
        .structured
        .get("topProcesses")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let top_name = top
        .and_then(|process| process.get("processName"))
        .and_then(Value::as_str);
    let top_cpu = top
        .and_then(|process| process.get("cpuPercent"))
        .and_then(Value::as_f64);
    let top_fact = match (top_name, top_cpu) {
        (Some(name), Some(cpu)) => format!("，窗口内最高为 {name} {cpu:.1}%"),
        _ => String::new(),
    };
    let title = match severity {
        "critical" => format!("当前 CPU 压力严重：整机 {load:.1}%{top_fact}"),
        "constrained" => format!("当前 CPU 压力偏高：整机 {load:.1}%{top_fact}"),
        _ => format!("当前 CPU 未达到压力阈值：整机 {load:.1}%{top_fact}"),
    };
    Some((
        vec![Hypothesis {
            id: "system.cpu_pressure".into(),
            title,
            state: if severity == "healthy" {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Supported
            },
            confidence: 0.98,
            evidence_ids: vec![item.id.clone()],
        }],
        Some(if severity == "healthy" {
            "当前采样窗口没有显示全局 CPU 压力；如问题间歇出现，需要在复现时重新采样".into()
        } else {
            "请先确认高 CPU 进程是否为预期任务；UCode 本轮没有结束、挂起或重启任何进程".into()
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
            id: "evidence-cpu".into(),
            tool_id: TOOL_ID.into(),
            tool_version: 1,
            state,
            summary: "cpu".into(),
            structured,
            exit_code: Some(0),
            content_digest: format!("{:x}", Sha256::digest(bytes)),
            elapsed_ms: 1,
        }
    }

    #[test]
    fn parser_accepts_explicit_cpu_pressure_and_rejects_generic_or_explanatory_input() {
        for (request, expected) in [
            ("CPU占用 100%，帮我检查", true),
            ("处理器使用率很高", true),
            ("CPU 跑满了", true),
            ("CPU at 100%", true),
            ("high CPU usage after startup", true),
            ("processor usage is high", true),
            ("电脑很慢，请检查", false),
            ("什么是 CPU 使用率", false),
            ("what is cpu usage", false),
            ("进程异常", false),
        ] {
            assert_eq!(is_cpu_request(request), expected, "{request}");
        }
    }

    #[test]
    fn analysis_distinguishes_healthy_constrained_critical_and_unavailable() {
        for (load, expected) in [
            (20.0, HypothesisState::Contradicted),
            (80.0, HypothesisState::Supported),
            (95.0, HypothesisState::Supported),
        ] {
            let item = evidence(
                json!({
                    "cpuLoadPercent":load,
                    "sampleWindowMs":505,
                    "topProcesses":[{"pid":42,"processName":"node","cpuPercent":30.0,"ownerKind":"unknown"}]
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
    fn real_probe_returns_bounded_cpu_and_process_metadata() {
        let output = run_cpu(&Value::Null).unwrap();
        assert_eq!(output.exit_code, 0);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        let load = value["cpuLoadPercent"].as_f64().unwrap();
        let window = value["sampleWindowMs"].as_u64().unwrap();
        assert!((0.0..=100.0).contains(&load));
        assert!(window >= 450);
        let processes = value["topProcesses"].as_array().unwrap();
        assert!(processes.len() <= MAX_PROCESSES);
        for process in processes {
            let name = process["processName"].as_str().unwrap();
            assert!(!name.contains(['\\', '/']));
            assert!(!name.chars().any(char::is_control));
            assert!(process["pid"].as_u64().unwrap() > 0);
            let cpu = process["cpuPercent"].as_f64().unwrap();
            assert!((0.0..=100.0).contains(&cpu));
        }
        let raw = value.to_string();
        for private in ["commandLine", "executablePath", "username", "windowTitle"] {
            assert!(!raw.contains(private), "private field leaked: {private}");
        }
    }
}
