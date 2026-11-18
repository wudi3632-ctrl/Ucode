use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::{json, Value};

const TOOL_ID: &str = "network.local_port.inspect";
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_LISTENERS: usize = 16;

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    explicit_local_port(request)
        .map(|port| (TOOL_ID.into(), json!({ "port": port, "protocol": "tcp" })))
}

pub fn explicit_local_port(request: &str) -> Option<u16> {
    let lower = request.to_lowercase();
    let local_intent = [
        "占用",
        "监听",
        "冲突",
        "被谁",
        "释放",
        "本机",
        "localhost",
        "127.0.0.1",
        "::1",
        "address already in use",
        "in use",
        "listening",
        "bind failed",
        "port conflict",
        "conflict",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !local_intent {
        return None;
    }

    for marker in ["localhost:", "127.0.0.1:", "[::1]:"] {
        if let Some(port) = digits_after_marker(&lower, marker) {
            return Some(port);
        }
    }
    for marker in ["端口", "port"] {
        if let Some(port) = digits_after_marker(&lower, marker) {
            return Some(port);
        }
    }
    None
}

fn digits_after_marker(value: &str, marker: &str) -> Option<u16> {
    let mut offset = 0;
    while let Some(relative) = value[offset..].find(marker) {
        let start = offset + relative + marker.len();
        let tail = value.get(start..)?.trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, ':' | '：' | '=' | '`')
        });
        let digits = tail
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .take(6)
            .collect::<String>();
        let delimiter = tail
            .get(digits.len()..)
            .and_then(|rest| rest.chars().next());
        let safe_boundary = delimiter.is_none_or(|character| {
            character.is_whitespace()
                || matches!(
                    character,
                    ',' | '，' | '.' | '。' | ':' | '：' | '!' | '！' | '?' | '？' | ')' | '）'
                )
        });
        if !digits.is_empty() && safe_boundary {
            if let Ok(port) = digits.parse::<u16>() {
                if port > 0 {
                    return Some(port);
                }
            }
        }
        offset = start;
        if offset >= value.len() {
            break;
        }
    }
    None
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == TOOL_ID).then(|| run_local_port(arguments))
}

fn run_local_port(arguments: &Value) -> Result<ProbeOutput, String> {
    let port = arguments
        .get("port")
        .and_then(Value::as_u64)
        .filter(|port| (1..=u16::MAX as u64).contains(port))
        .ok_or("本机端口探针需要 1-65535 的 port")? as u16;
    let protocol = arguments
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    if protocol != "tcp" {
        return Err("本机端口探针当前只支持 tcp".into());
    }
    let (listeners, truncated) = local_port_native::query(port)?;
    let stdout = serde_json::to_vec(&json!({
        "port": port,
        "protocol": "tcp",
        "occupied": !listeners.is_empty(),
        "listeners": listeners,
        "truncated": truncated,
    }))
    .map_err(|error| format!("无法序列化本机端口证据：{error}"))?;
    Ok(ProbeOutput {
        exit_code: 0,
        stdout: bounded_utf8(&stdout),
        stderr: String::new(),
    })
}

fn bounded_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT_BYTES)]).to_string()
}

#[cfg(target_os = "windows")]
mod local_port_native {
    use super::MAX_LISTENERS;
    use serde::Serialize;
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    const TCP_TABLE_OWNER_PID_LISTENER: u32 = 3;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const NO_ERROR: u32 = 0;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Listener {
        local_address: String,
        pid: u32,
        process_name: Option<String>,
        owner_kind: &'static str,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TcpRowOwnerPid {
        state: u32,
        local_addr: u32,
        local_port: u32,
        remote_addr: u32,
        remote_port: u32,
        owning_pid: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Tcp6RowOwnerPid {
        local_addr: [u8; 16],
        local_scope_id: u32,
        local_port: u32,
        remote_addr: [u8; 16],
        remote_scope_id: u32,
        remote_port: u32,
        state: u32,
        owning_pid: u32,
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            order: i32,
            family: u32,
            table_class: u32,
            reserved: u32,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn QueryFullProcessImageNameW(
            process: isize,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    struct Handle(isize);
    impl Drop for Handle {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    pub fn query(port: u16) -> Result<(Vec<Listener>, bool), String> {
        let mut rows = BTreeSet::new();
        query_family::<TcpRowOwnerPid>(AF_INET, |row| {
            if decode_port(row.local_port) == port {
                rows.insert(listener(
                    Ipv4Addr::from(row.local_addr.to_ne_bytes()).to_string(),
                    row.owning_pid,
                ));
            }
        })?;
        query_family::<Tcp6RowOwnerPid>(AF_INET6, |row| {
            if decode_port(row.local_port) == port {
                rows.insert(listener(
                    Ipv6Addr::from(row.local_addr).to_string(),
                    row.owning_pid,
                ));
            }
        })?;
        let truncated = rows.len() > MAX_LISTENERS;
        Ok((rows.into_iter().take(MAX_LISTENERS).collect(), truncated))
    }

    fn query_family<T: Copy>(family: u32, mut visit: impl FnMut(T)) -> Result<(), String> {
        let mut size = 0u32;
        let first = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                1,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if first != ERROR_INSUFFICIENT_BUFFER && first != NO_ERROR {
            return Err(format!(
                "Windows IP Helper 无法确定 TCP 表大小：错误 {first}"
            ));
        }
        if size < 4 {
            return Ok(());
        }
        let mut buffer = vec![0u8; size as usize];
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast::<c_void>(),
                &mut size,
                1,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if result != NO_ERROR {
            return Err(format!(
                "Windows IP Helper 无法读取 TCP listener 表：错误 {result}"
            ));
        }
        let count = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) } as usize;
        let row_size = std::mem::size_of::<T>();
        let available = buffer.len().saturating_sub(4) / row_size;
        if count > available {
            return Err("Windows IP Helper 返回了越界 TCP 表".into());
        }
        for index in 0..count {
            let pointer = unsafe { buffer.as_ptr().add(4 + index * row_size).cast::<T>() };
            visit(unsafe { std::ptr::read_unaligned(pointer) });
        }
        Ok(())
    }

    fn decode_port(value: u32) -> u16 {
        u16::from_be(value as u16)
    }

    fn listener(local_address: String, pid: u32) -> Listener {
        let process_name = process_name(pid);
        let owner_kind = if pid == 0 || pid == 4 {
            "system"
        } else if process_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("svchost"))
        {
            "service_host"
        } else if process_name.is_some() {
            "user_process"
        } else {
            "unknown"
        };
        Listener {
            local_address,
            pid,
            process_name,
            owner_kind,
        }
    }

    fn process_name(pid: u32) -> Option<String> {
        if pid == 0 || pid == 4 {
            return Some("System".into());
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle == 0 {
            return None;
        }
        let handle = Handle(handle);
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut length) } == 0
        {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit(['\\', '/'])
            .next()
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches(".exe").chars().take(128).collect())
    }
}

#[cfg(not(target_os = "windows"))]
mod local_port_native {
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    pub struct Listener;

    pub fn query(_port: u16) -> Result<(Vec<Listener>, bool), String> {
        Err("本机 TCP listener 探针只支持 Windows".into())
    }
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence.iter().find(|item| item.tool_id == TOOL_ID)?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "network.local_port_unknown".into(),
                title: "无法确认本机端口监听状态".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("本机端口探针不可用；未把失败误报为空闲，也没有结束任何进程".into()),
        ));
    }
    let port = item
        .structured
        .get("port")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let occupied = item
        .structured
        .get("occupied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let listeners = item
        .structured
        .get("listeners")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let owner = listeners.first().map(|listener| {
        let pid = listener
            .get("pid")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let process = listener
            .get("processName")
            .and_then(Value::as_str)
            .unwrap_or("未知进程");
        format!("{process}（PID {pid}）")
    });
    Some((
        vec![Hypothesis {
            id: "network.local_port_occupied".into(),
            title: if occupied {
                format!(
                    "本机 TCP 端口 {port} 正在被{}监听",
                    owner.as_deref().unwrap_or("进程")
                )
            } else {
                format!("本机 TCP 端口 {port} 未发现监听者")
            },
            state: if occupied {
                HypothesisState::Supported
            } else {
                HypothesisState::Contradicted
            },
            confidence: if occupied { 0.99 } else { 0.96 },
            evidence_ids: vec![item.id.clone()],
        }],
        Some(if occupied {
            "请确认该监听进程是否为预期实例；UCode 本轮不会自动结束进程或停止服务".into()
        } else {
            "端口当前没有 TCP listener；应用仍可能因绑定地址、权限或自身配置失败".into()
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rescue::EvidenceState;
    use sha2::{Digest, Sha256};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn evidence(structured: Value, state: EvidenceState) -> EvidenceNode {
        let bytes = serde_json::to_vec(&structured).unwrap();
        EvidenceNode {
            id: "evidence-port".into(),
            tool_id: TOOL_ID.into(),
            tool_version: 1,
            state,
            summary: "port".into(),
            structured,
            exit_code: Some(0),
            content_digest: format!("{:x}", Sha256::digest(bytes)),
            elapsed_ms: 1,
        }
    }

    #[test]
    fn parser_accepts_explicit_local_ports_and_rejects_remote_or_unsafe_values() {
        for (request, expected) in [
            ("本机端口 3082 被谁占用", Some(3082)),
            ("localhost:3000 address already in use", Some(3000)),
            ("127.0.0.1:8080 是否监听", Some(8080)),
            ("port 1420 conflict", Some(1420)),
            (
                "再次检查本机端口 54321：请根据当前真实证据说明是否仍被占用",
                Some(54321),
            ),
            ("example.com:443 连接失败", None),
            ("错误码 3082", None),
            ("端口 0 被占用", None),
            ("端口 65536 被占用", None),
            ("端口 80-90 被占用", None),
            ("端口 80; Stop-Process 1 被占用", None),
        ] {
            assert_eq!(explicit_local_port(request), expected, "{request}");
        }
    }

    #[test]
    fn analysis_distinguishes_occupied_available_and_unavailable() {
        let occupied = evidence(
            json!({"port":3082,"occupied":true,"listeners":[{"pid":42,"processName":"node"}]}),
            EvidenceState::Observed,
        );
        let (hypotheses, next) = analyze(&[occupied]).unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Supported));
        assert!(hypotheses[0].title.contains("node（PID 42）"));
        assert!(next.unwrap().contains("不会自动结束"));

        let available = evidence(
            json!({"port":3082,"occupied":false,"listeners":[]}),
            EvidenceState::Observed,
        );
        let (hypotheses, _) = analyze(&[available]).unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Contradicted));

        let unavailable = evidence(json!({"error":"failed"}), EvidenceState::Unavailable);
        let (hypotheses, next) = analyze(&[unavailable]).unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        assert!(next.unwrap().contains("未把失败误报为空闲"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_probe_observes_a_temporary_listener_then_observes_available() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let output = run_local_port(&json!({"port":port,"protocol":"tcp"})).unwrap();
        assert_eq!(output.exit_code, 0);
        let occupied: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(occupied["occupied"], true);
        let listeners = occupied["listeners"].as_array().unwrap();
        assert!(!listeners.is_empty() && listeners.len() <= MAX_LISTENERS);
        assert!(listeners
            .iter()
            .any(|item| item["localAddress"] == "127.0.0.1"));
        for item in listeners {
            if let Some(name) = item["processName"].as_str() {
                assert!(!name.contains(['\\', '/']));
            }
            assert!(matches!(
                item["ownerKind"].as_str(),
                Some("system" | "service_host" | "user_process" | "unknown")
            ));
        }
        assert!(occupied.to_string().find("CommandLine").is_none());
        drop(listener);
        thread::sleep(Duration::from_millis(150));
        let output = run_local_port(&json!({"port":port,"protocol":"tcp"})).unwrap();
        let available: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(available["occupied"], false);
    }
}
