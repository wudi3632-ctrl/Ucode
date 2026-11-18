use crate::rescue::{EvidenceNode, EvidenceState};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const PROBE_SCHEMA_VERSION: u16 = 1;
pub const DNS_PROBE: &str = "network.dns.resolve";
pub const TCP_PROBE: &str = "network.tcp.probe";
const TOOL_VERSION: u16 = 1;
const TCP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    AwaitingApproval,
    Running,
    Observed,
    Unavailable,
    Rejected,
    SafeStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct R1ProbeProposal {
    pub schema_version: u16,
    pub id: String,
    pub ticket_id: String,
    pub tool_id: String,
    pub tool_version: u16,
    pub risk: String,
    pub target: String,
    pub normalized_arguments: Value,
    pub evidence_ids: Vec<String>,
    pub purpose: String,
    pub data_boundary: String,
    pub timeout_ms: u64,
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: ProbeStatus,
    pub proposal_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct R1ProbeRecord {
    pub proposal: R1ProbeProposal,
    pub evidence: Option<EvidenceNode>,
    pub detail: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkTarget {
    host: String,
    port: Option<u16>,
}

pub fn requested_probe_description(request: &str) -> Option<String> {
    let target = extract_network_target(request)?;
    Some(match target.port {
        Some(port) => format!(
            "network.tcp.probe（R1，等待允许一次性探测 {}:{port}）",
            display_host(&target.host)
        ),
        None => format!(
            "network.dns.resolve（R1，等待允许一次性解析 {}）",
            target.host
        ),
    })
}

pub trait NetworkProbeHost: Send + Sync {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String>;
    fn connect(&self, address: SocketAddr, timeout: Duration) -> Result<(), String>;
}

pub struct SystemNetworkProbeHost;

impl NetworkProbeHost for SystemNetworkProbeHost {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        let mut addresses = (host, 0)
            .to_socket_addrs()
            .map_err(|error| error.to_string())?
            .map(|address| address.ip())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(8)
            .collect::<Vec<_>>();
        addresses.sort();
        Ok(addresses)
    }

    fn connect(&self, address: SocketAddr, timeout: Duration) -> Result<(), String> {
        TcpStream::connect_timeout(&address, timeout)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub struct NetworkProbeService<H: NetworkProbeHost = SystemNetworkProbeHost> {
    root: PathBuf,
    host: H,
    resolution_gate: StdMutex<()>,
}

impl NetworkProbeService<SystemNetworkProbeHost> {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        Self::with_host(root, SystemNetworkProbeHost)
    }
}

impl<H: NetworkProbeHost> NetworkProbeService<H> {
    pub fn with_host(root: PathBuf, host: H) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| format!("无法创建网络探针证据目录：{error}"))?;
        Ok(Self {
            root,
            host,
            resolution_gate: StdMutex::new(()),
        })
    }

    pub fn propose_for_request(
        &self,
        ticket_id: &str,
        request: &str,
        evidence_ids: Vec<String>,
    ) -> Result<Option<R1ProbeProposal>, String> {
        let Some(target) = extract_network_target(request) else {
            return Ok(None);
        };
        let (tool_id, target_text, purpose, data_boundary, timeout_ms) =
            if let Some(port) = target.port {
                (
                    TCP_PROBE,
                    format!("{}:{port}", display_host(&target.host)),
                    "确认目标主机的指定 TCP 端口是否能够建立连接",
                    format!(
                        "仅向 {} 的 {port} 端口发起 TCP 握手；不发送 HTTP 或应用数据",
                        target.host
                    ),
                    12_000,
                )
            } else {
                (
                    DNS_PROBE,
                    target.host.clone(),
                    "确认受影响域名能否通过当前 Windows 解析器得到地址",
                    format!(
                        "仅把域名 {} 交给当前系统 DNS 解析器；不访问解析后的网页",
                        target.host
                    ),
                    8_000,
                )
            };
        let now = Utc::now();
        let mut proposal = R1ProbeProposal {
            schema_version: PROBE_SCHEMA_VERSION,
            id: format!("probe-{}", Uuid::new_v4()),
            ticket_id: ticket_id.into(),
            tool_id: tool_id.into(),
            tool_version: TOOL_VERSION,
            risk: "r1".into(),
            target: target_text,
            normalized_arguments: json!({ "host": target.host, "port": target.port }),
            evidence_ids,
            purpose: purpose.into(),
            data_boundary,
            timeout_ms,
            nonce: Uuid::new_v4().to_string(),
            created_at: now,
            expires_at: now + ChronoDuration::minutes(10),
            status: ProbeStatus::AwaitingApproval,
            proposal_digest: String::new(),
        };
        proposal.proposal_digest = proposal_digest(&proposal);
        self.save_record(&R1ProbeRecord {
            proposal: proposal.clone(),
            evidence: None,
            detail: "等待用户允许本次精确网络探测".into(),
            updated_at: now,
        })?;
        Ok(Some(proposal))
    }

    pub fn reject(&self, proposal_id: &str, ticket_id: &str) -> Result<R1ProbeRecord, String> {
        let _resolution = self
            .resolution_gate
            .lock()
            .map_err(|_| "网络探针决议锁损坏，已安全停止".to_string())?;
        let mut record = self.load_record(proposal_id)?;
        self.validate_identity(&record.proposal, ticket_id)?;
        if record.proposal.status != ProbeStatus::AwaitingApproval {
            return Err("网络探针方案已经处理，不能重复拒绝".into());
        }
        record.proposal.status = ProbeStatus::Rejected;
        record.detail = "用户选择保持离线；没有发起 DNS 或 TCP 网络请求".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    pub fn execute(&self, proposal_id: &str, ticket_id: &str) -> Result<R1ProbeRecord, String> {
        let _resolution = self
            .resolution_gate
            .lock()
            .map_err(|_| "网络探针决议锁损坏，已安全停止".to_string())?;
        let mut record = self.load_record(proposal_id)?;
        self.validate_for_execute(&record.proposal, ticket_id)?;
        record.proposal.status = ProbeStatus::Running;
        record.detail = "正在执行已批准的一次性网络探针".into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;

        let started = Instant::now();
        let host = record
            .proposal
            .normalized_arguments
            .get("host")
            .and_then(Value::as_str)
            .ok_or("网络探针缺少 host 参数")?;
        let (summary, structured) = match record.proposal.tool_id.as_str() {
            DNS_PROBE => self.run_dns(host),
            TCP_PROBE => {
                let port = record
                    .proposal
                    .normalized_arguments
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or("TCP 探针缺少有效端口")?;
                self.run_tcp(host, port)
            }
            _ => return self.safe_stop(record, "未知 R1 网络探针，已拒绝执行"),
        };
        let evidence = evidence_node(
            &record.proposal.tool_id,
            summary,
            structured,
            started.elapsed().as_millis(),
        );
        record.proposal.status = ProbeStatus::Observed;
        record.detail = evidence.summary.clone();
        record.evidence = Some(evidence);
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    fn run_dns(&self, host: &str) -> (String, Value) {
        match self.host.resolve(host) {
            Ok(addresses) if !addresses.is_empty() => (
                format!("DNS 已把 {host} 解析为 {} 个地址", addresses.len()),
                json!({ "host": host, "resolved": true, "addresses": addresses.iter().map(ToString::to_string).collect::<Vec<_>>() }),
            ),
            Ok(_) => (
                format!("DNS 没有为 {host} 返回地址"),
                json!({ "host": host, "resolved": false, "addresses": [] }),
            ),
            Err(error) => (
                format!("DNS 解析 {host} 失败：{}", bounded_error(&error)),
                json!({ "host": host, "resolved": false, "addresses": [], "error": bounded_error(&error) }),
            ),
        }
    }

    fn run_tcp(&self, host: &str, port: u16) -> (String, Value) {
        let addresses = match self.host.resolve(host) {
            Ok(addresses) => addresses.into_iter().take(4).collect::<Vec<_>>(),
            Err(error) => {
                return (
                    format!(
                        "无法解析 {host}，因此没有尝试 TCP 连接：{}",
                        bounded_error(&error)
                    ),
                    json!({ "host": host, "port": port, "connected": false, "addresses": [], "error": bounded_error(&error) }),
                )
            }
        };
        let shown = addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for ip in &addresses {
            let address = SocketAddr::new(*ip, port);
            match self.host.connect(address, TCP_TIMEOUT) {
                Ok(()) => {
                    return (
                        format!("已与 {} 建立 TCP 连接", display_socket(address)),
                        json!({ "host": host, "port": port, "connected": true, "connectedAddress": address.to_string(), "addresses": shown }),
                    )
                }
                Err(error) => errors.push(format!(
                    "{}: {}",
                    display_socket(address),
                    bounded_error(&error)
                )),
            }
        }
        (
            format!(
                "无法连接 {}:{port}；已尝试 {} 个地址",
                display_host(host),
                addresses.len()
            ),
            json!({ "host": host, "port": port, "connected": false, "addresses": shown, "errors": errors }),
        )
    }

    fn validate_identity(&self, proposal: &R1ProbeProposal, ticket_id: &str) -> Result<(), String> {
        if proposal.ticket_id != ticket_id {
            return Err("网络探针方案不属于当前任务".into());
        }
        if proposal.schema_version != PROBE_SCHEMA_VERSION
            || proposal.tool_version != TOOL_VERSION
            || proposal.risk != "r1"
            || !matches!(proposal.tool_id.as_str(), DNS_PROBE | TCP_PROBE)
        {
            return Err("未知或过期的 R1 网络探针，已拒绝执行".into());
        }
        let host = proposal
            .normalized_arguments
            .get("host")
            .and_then(Value::as_str)
            .ok_or("网络探针参数缺少 host")?;
        if normalize_host(host).as_deref() != Some(host) {
            return Err("网络探针 host 参数无效".into());
        }
        match proposal.tool_id.as_str() {
            DNS_PROBE
                if proposal
                    .normalized_arguments
                    .get("port")
                    .is_some_and(|value| !value.is_null()) =>
            {
                return Err("DNS 探针不能携带端口".into())
            }
            TCP_PROBE => {
                let port = proposal
                    .normalized_arguments
                    .get("port")
                    .and_then(Value::as_u64)
                    .filter(|value| (1..=u16::MAX as u64).contains(value));
                if port.is_none() {
                    return Err("TCP 探针端口无效".into());
                }
            }
            _ => {}
        }
        if proposal.proposal_digest != proposal_digest(proposal) {
            return Err("网络探针方案摘要无效，已拒绝执行".into());
        }
        Ok(())
    }

    fn validate_for_execute(
        &self,
        proposal: &R1ProbeProposal,
        ticket_id: &str,
    ) -> Result<(), String> {
        self.validate_identity(proposal, ticket_id)?;
        if proposal.status != ProbeStatus::AwaitingApproval {
            return Err("网络探针已经执行或拒绝，不能重放".into());
        }
        if Utc::now() > proposal.expires_at {
            return Err("网络探针审批已过期，请重新诊断".into());
        }
        Ok(())
    }

    fn safe_stop(&self, mut record: R1ProbeRecord, detail: &str) -> Result<R1ProbeRecord, String> {
        record.proposal.status = ProbeStatus::SafeStop;
        record.detail = detail.into();
        record.updated_at = Utc::now();
        self.save_record(&record)?;
        Ok(record)
    }

    fn record_path(&self, proposal_id: &str) -> Result<PathBuf, String> {
        if proposal_id.is_empty()
            || !proposal_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("网络探针方案 ID 无效".into());
        }
        Ok(self.root.join(format!("{proposal_id}.json")))
    }

    fn load_record(&self, proposal_id: &str) -> Result<R1ProbeRecord, String> {
        let path = self.record_path(proposal_id)?;
        serde_json::from_slice(&fs::read(path).map_err(|_| "网络探针方案不存在".to_string())?)
            .map_err(|_| "网络探针方案记录损坏".to_string())
    }

    fn save_record(&self, record: &R1ProbeRecord) -> Result<(), String> {
        let path = self.record_path(&record.proposal.id)?;
        let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
        fs::write(&temporary, bytes).map_err(|error| format!("无法保存网络探针记录：{error}"))?;
        fs::rename(&temporary, &path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("无法提交网络探针记录：{error}")
        })
    }
}

fn extract_network_target(request: &str) -> Option<NetworkTarget> {
    let explicit_port = extract_keyword_port(request);
    let cleaned = request.replace(
        ['，', '。', '；', '、', '（', '）', '【', '】', '<', '>'],
        " ",
    );
    for token in cleaned.split_whitespace() {
        if let Some(mut target) = parse_target_token(token) {
            if target.port.is_none() {
                target.port = explicit_port;
            }
            if target.port.is_none() && target.host.parse::<IpAddr>().is_ok() {
                continue;
            }
            return Some(target);
        }
    }
    None
}

fn parse_target_token(raw: &str) -> Option<NetworkTarget> {
    let token = raw.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | ',' | ';' | '!' | '?' | '(' | ')'
        )
    });
    let lower = token.to_ascii_lowercase();
    let (authority, default_port) = if let Some(value) = lower.strip_prefix("https://") {
        (value.split(['/', '?', '#']).next()?, Some(443))
    } else if let Some(value) = lower.strip_prefix("http://") {
        (value.split(['/', '?', '#']).next()?, Some(80))
    } else {
        if token.contains("//") || token.contains(['/', '\\', '@']) {
            return None;
        }
        (lower.as_str(), None)
    };
    let (host, port) = parse_authority(authority)?;
    Some(NetworkTarget {
        host: normalize_host(host)?,
        port: port.or(default_port),
    })
}

fn parse_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[1..end];
        let remainder = &authority[end + 1..];
        let port = if remainder.is_empty() {
            None
        } else {
            Some(
                remainder
                    .strip_prefix(':')?
                    .parse::<u16>()
                    .ok()
                    .filter(|value| *value > 0)?,
            )
        };
        return Some((host, port));
    }
    if authority.matches(':').count() == 1 {
        let (host, candidate) = authority.rsplit_once(':')?;
        if let Ok(port) = candidate.parse::<u16>() {
            return (port > 0).then_some((host, Some(port)));
        }
    }
    Some((authority.trim_end_matches('.'), None))
}

fn normalize_host(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty() || value.len() > 253 || !value.is_ascii() {
        return None;
    }
    if value.parse::<IpAddr>().is_ok() || value == "localhost" {
        return Some(value);
    }
    if !value.contains('.') {
        return None;
    }
    let valid = value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            && label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
            && label
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric())
    });
    valid.then_some(value)
}

fn extract_keyword_port(request: &str) -> Option<u16> {
    let normalized = request
        .to_ascii_lowercase()
        .replace(['：', ':', '=', '，', ','], " ");
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for window in tokens.windows(2) {
        if matches!(window[0], "端口" | "port") {
            if let Ok(port) = window[1].parse::<u16>() {
                if port > 0 {
                    return Some(port);
                }
            }
        }
    }
    None
}

fn evidence_node(
    tool_id: &str,
    summary: String,
    structured: Value,
    elapsed_ms: u128,
) -> EvidenceNode {
    let bytes = serde_json::to_vec(&structured).unwrap_or_default();
    let content_digest = format!("{:x}", Sha256::digest(bytes));
    EvidenceNode {
        id: format!("evidence-{}", &content_digest[..16]),
        tool_id: tool_id.into(),
        tool_version: TOOL_VERSION,
        state: EvidenceState::Observed,
        summary,
        structured,
        exit_code: Some(0),
        content_digest,
        elapsed_ms,
    }
}

fn proposal_digest(proposal: &R1ProbeProposal) -> String {
    let mut value = serde_json::to_value(proposal).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("proposalDigest".into(), Value::String(String::new()));
        object.insert("status".into(), Value::String("awaiting_approval".into()));
    }
    digest_json(&value)
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn bounded_error(value: &str) -> String {
    value.replace(['\r', '\n'], " ").chars().take(240).collect()
}

fn display_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.into()
    }
}

fn display_socket(address: SocketAddr) -> String {
    address.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct FakeNetworkHost {
        addresses: Mutex<Result<Vec<IpAddr>, String>>,
        connected: Mutex<bool>,
        resolve_calls: Mutex<usize>,
        connect_calls: Mutex<usize>,
    }

    impl FakeNetworkHost {
        fn healthy() -> Self {
            Self {
                addresses: Mutex::new(Ok(vec!["203.0.113.10".parse().unwrap()])),
                connected: Mutex::new(true),
                resolve_calls: Mutex::new(0),
                connect_calls: Mutex::new(0),
            }
        }
    }

    impl NetworkProbeHost for FakeNetworkHost {
        fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
            *self.resolve_calls.lock().unwrap() += 1;
            self.addresses.lock().unwrap().clone()
        }

        fn connect(&self, _address: SocketAddr, _timeout: Duration) -> Result<(), String> {
            *self.connect_calls.lock().unwrap() += 1;
            if *self.connected.lock().unwrap() {
                Ok(())
            } else {
                Err("connection refused".into())
            }
        }
    }

    fn service() -> (TempDir, NetworkProbeService<FakeNetworkHost>) {
        let dir = TempDir::new().unwrap();
        let service =
            NetworkProbeService::with_host(dir.path().to_path_buf(), FakeNetworkHost::healthy())
                .unwrap();
        (dir, service)
    }

    #[test]
    fn explicit_domain_proposes_dns_without_network_before_approval() {
        let (_dir, service) = service();
        let proposal = service
            .propose_for_request(
                "ticket-1",
                "example.com 无法解析，请诊断",
                vec!["evidence-r0".into()],
            )
            .unwrap()
            .unwrap();
        assert_eq!(proposal.tool_id, DNS_PROBE);
        assert_eq!(proposal.target, "example.com");
        assert_eq!(*service.host.resolve_calls.lock().unwrap(), 0);
        assert_eq!(*service.host.connect_calls.lock().unwrap(), 0);
    }

    #[test]
    fn explicit_https_url_proposes_tcp_443_and_runs_once() {
        let (_dir, service) = service();
        let proposal = service
            .propose_for_request("ticket-1", "https://example.com/login 无法打开", vec![])
            .unwrap()
            .unwrap();
        assert_eq!(proposal.tool_id, TCP_PROBE);
        assert_eq!(proposal.target, "example.com:443");
        let record = service.execute(&proposal.id, "ticket-1").unwrap();
        assert_eq!(record.proposal.status, ProbeStatus::Observed);
        assert_eq!(
            record.evidence.as_ref().unwrap().structured["connected"],
            true
        );
        assert_eq!(*service.host.resolve_calls.lock().unwrap(), 1);
        assert_eq!(*service.host.connect_calls.lock().unwrap(), 1);
        assert!(service.execute(&proposal.id, "ticket-1").is_err());
    }

    #[test]
    fn concurrent_approvals_execute_the_probe_exactly_once() {
        let (_dir, service) = service();
        let proposal = service
            .propose_for_request("ticket-1", "https://example.com 无法打开", vec![])
            .unwrap()
            .unwrap();
        let service = std::sync::Arc::new(service);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let service = std::sync::Arc::clone(&service);
            let barrier = std::sync::Arc::clone(&barrier);
            let proposal_id = proposal.id.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                service.execute(&proposal_id, "ticket-1")
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert_eq!(*service.host.resolve_calls.lock().unwrap(), 1);
        assert_eq!(*service.host.connect_calls.lock().unwrap(), 1);
    }

    #[test]
    fn missing_target_and_bare_ip_do_not_create_a_probe() {
        let (_dir, service) = service();
        assert!(service
            .propose_for_request("ticket-1", "电脑无法上网，请诊断", vec![])
            .unwrap()
            .is_none());
        assert!(service
            .propose_for_request("ticket-1", "检查 192.0.2.1", vec![])
            .unwrap()
            .is_none());
    }

    #[test]
    fn rejection_wrong_ticket_expiry_and_tampering_never_run_network() {
        let (dir, service) = service();
        let proposal = service
            .propose_for_request("ticket-1", "example.com DNS 失败", vec![])
            .unwrap()
            .unwrap();
        assert!(service.execute(&proposal.id, "ticket-2").is_err());
        let rejected = service.reject(&proposal.id, "ticket-1").unwrap();
        assert_eq!(rejected.proposal.status, ProbeStatus::Rejected);
        assert!(service.execute(&proposal.id, "ticket-1").is_err());
        assert_eq!(*service.host.resolve_calls.lock().unwrap(), 0);

        let expired = service
            .propose_for_request("ticket-2", "example.org DNS 失败", vec![])
            .unwrap()
            .unwrap();
        let mut record = service.load_record(&expired.id).unwrap();
        record.proposal.expires_at = Utc::now() - ChronoDuration::seconds(1);
        record.proposal.proposal_digest = proposal_digest(&record.proposal);
        service.save_record(&record).unwrap();
        assert!(service.execute(&expired.id, "ticket-2").is_err());

        let tampered = service
            .propose_for_request("ticket-3", "example.net DNS 失败", vec![])
            .unwrap()
            .unwrap();
        let path = dir.path().join(format!("{}.json", tampered.id));
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["proposal"]["normalizedArguments"]["host"] = Value::String("other.example".into());
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(service.execute(&tampered.id, "ticket-3").is_err());
        assert_eq!(*service.host.resolve_calls.lock().unwrap(), 0);
    }

    #[test]
    fn negative_dns_and_tcp_results_are_preserved_as_observed_evidence() {
        let (_dir, service) = service();
        *service.host.addresses.lock().unwrap() = Err("name not found".into());
        let dns = service
            .propose_for_request("ticket-1", "missing.example DNS 异常", vec![])
            .unwrap()
            .unwrap();
        let dns_record = service.execute(&dns.id, "ticket-1").unwrap();
        assert_eq!(dns_record.proposal.status, ProbeStatus::Observed);
        assert_eq!(
            dns_record.evidence.as_ref().unwrap().structured["resolved"],
            false
        );

        *service.host.addresses.lock().unwrap() = Ok(vec!["203.0.113.11".parse().unwrap()]);
        *service.host.connected.lock().unwrap() = false;
        let tcp = service
            .propose_for_request("ticket-2", "example.com:8443 端口无法连接", vec![])
            .unwrap()
            .unwrap();
        let tcp_record = service.execute(&tcp.id, "ticket-2").unwrap();
        assert_eq!(
            tcp_record.evidence.as_ref().unwrap().structured["connected"],
            false
        );
        assert!(tcp_record.detail.contains("无法连接"));
    }

    #[test]
    fn parser_accepts_explicit_ports_and_rejects_paths_or_wildcards() {
        assert_eq!(
            extract_network_target("请检查 localhost 端口 3000"),
            Some(NetworkTarget {
                host: "localhost".into(),
                port: Some(3000)
            })
        );
        assert_eq!(
            extract_network_target("检查 [2001:db8::1]:443"),
            Some(NetworkTarget {
                host: "2001:db8::1".into(),
                port: Some(443)
            })
        );
        assert!(extract_network_target("检查 *.example.com").is_none());
        assert!(extract_network_target("检查 C:\\Windows\\System32").is_none());
    }

    #[test]
    fn next_probe_description_names_the_exact_approved_boundary() {
        assert_eq!(
            requested_probe_description("https://example.com/login 无法打开").as_deref(),
            Some("network.tcp.probe（R1，等待允许一次性探测 example.com:443）")
        );
        assert_eq!(
            requested_probe_description("example.org DNS 解析失败").as_deref(),
            Some("network.dns.resolve（R1，等待允许一次性解析 example.org）")
        );
        assert!(requested_probe_description("电脑无法上网").is_none());
    }
}
