use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_PROBE_OUTPUT: usize = 64 * 1024;
const ENVIRONMENT_INSPECT_SCRIPT: &str = r#"
$parts=@($env:Path -split ';'|Where-Object{$_}|Select-Object -First 80)
function Get-UCodePathStats([string]$pathValue) {
  $raw=@($pathValue -split ';')
  $nonEmpty=@($raw|Where-Object{$_.Trim()})
  $keys=@{}
  $duplicates=0
  $missing=0
  foreach($part in $nonEmpty){
    $value=$part.Trim().Trim('"')
    $key=$value.Replace('/','\').TrimEnd('\').ToLowerInvariant()
    if($keys.ContainsKey($key)){$duplicates++}else{$keys[$key]=$true}
    if([IO.Path]::IsPathRooted($value)-and -not $value.Contains('%')-and -not (Test-Path -LiteralPath $value -PathType Container)){$missing++}
  }
  [pscustomobject]@{entryCount=$nonEmpty.Count;emptyEntries=[math]::Max(0,$raw.Count-$nonEmpty.Count);duplicateEntries=$duplicates;missingAbsoluteDirectories=$missing}
}
$userPath=(Get-ItemProperty -LiteralPath 'HKCU:\Environment' -Name Path -ErrorAction SilentlyContinue).Path
$machinePath=(Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -Name Path -ErrorAction SilentlyContinue).Path
$userStats=Get-UCodePathStats $userPath
$machineStats=Get-UCodePathStats $machinePath
[pscustomobject]@{
  pathEntries=$parts
  pathEntryCount=$parts.Count
  userPathEntryCount=$userStats.entryCount
  userPathEmptyEntries=$userStats.emptyEntries
  userPathDuplicateEntries=$userStats.duplicateEntries
  userPathMissingAbsoluteDirectories=$userStats.missingAbsoluteDirectories
  machinePathEntryCount=$machineStats.entryCount
  machinePathEmptyEntries=$machineStats.emptyEntries
  machinePathDuplicateEntries=$machineStats.duplicateEntries
  machinePathMissingAbsoluteDirectories=$machineStats.missingAbsoluteDirectories
  temp=$env:TEMP
  userProfilePresent=[bool]$env:USERPROFILE
  processorArchitecture=$env:PROCESSOR_ARCHITECTURE
}|ConvertTo-Json -Compress -Depth 3
"#;
const NETWORK_PROXY_INSPECT_SCRIPT: &str = r#"
$winHttpText=(netsh winhttp show proxy|Out-String).Trim()
$winHttpConfigured=($LASTEXITCODE -eq 0) -and [bool]$winHttpText -and -not ($winHttpText -match 'Direct access|直接访问')
$internet=Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction SilentlyContinue
[pscustomobject]@{
  winHttpConfigured=$winHttpConfigured
  environmentProxyConfigured=[bool]($env:HTTP_PROXY -or $env:HTTPS_PROXY)
  winInetManualProxyEnabled=([int]$internet.ProxyEnable -eq 1)
  winInetProxyServerPresent=[bool]$internet.ProxyServer
  winInetAutoConfigPresent=[bool]$internet.AutoConfigURL
}|ConvertTo-Json -Compress
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    R0,
    R1,
    R2,
    R3,
    R4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescueToolDefinition {
    pub id: &'static str,
    pub version: u16,
    pub domain: &'static str,
    pub risk: RiskClass,
    pub authority: &'static str,
    pub description: &'static str,
}

pub fn tool_registry() -> &'static [RescueToolDefinition] {
    static TOOLS: [RescueToolDefinition; 16] = [
        RescueToolDefinition {
            id: "system.inventory",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Read bounded operating-system and hardware identity",
        },
        RescueToolDefinition {
            id: "system.environment.inspect",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect PATH and selected process environment metadata",
        },
        RescueToolDefinition {
            id: "system.process.list",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "List a bounded summary of high-resource processes",
        },
        RescueToolDefinition {
            id: "system.service.inspect",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect stopped automatic Windows services",
        },
        RescueToolDefinition {
            id: "system.service.query",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect one explicitly named Windows service and bounded dependencies",
        },
        RescueToolDefinition {
            id: "system.eventlog.query",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Read bounded error metadata from System and Application event logs",
        },
        RescueToolDefinition {
            id: "system.memory.inspect",
            version: 1,
            domain: "system",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect global memory pressure and bounded high-working-set processes",
        },
        RescueToolDefinition {
            id: "network.adapter.inspect",
            version: 1,
            domain: "network",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect active adapter, address, gateway and configured DNS evidence",
        },
        RescueToolDefinition {
            id: "network.route.inspect",
            version: 1,
            domain: "network",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect bounded default route evidence",
        },
        RescueToolDefinition {
            id: "network.proxy.inspect",
            version: 1,
            domain: "network",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect WinHTTP and process proxy configuration",
        },
        RescueToolDefinition {
            id: "network.local_port.inspect",
            version: 1,
            domain: "network",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect listeners for one explicitly named local TCP port",
        },
        RescueToolDefinition {
            id: "network.dns.resolve",
            version: 1,
            domain: "network",
            risk: RiskClass::R1,
            authority: "network_read",
            description: "Resolve an approved hostname; never runs without network consent",
        },
        RescueToolDefinition {
            id: "network.tcp.probe",
            version: 1,
            domain: "network",
            risk: RiskClass::R1,
            authority: "network_read",
            description: "Connect to one approved host and port without sending application data",
        },
        RescueToolDefinition {
            id: "storage.volume.inspect",
            version: 1,
            domain: "storage",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect capacity for one explicitly named local Windows volume",
        },
        RescueToolDefinition {
            id: "filesystem.hash",
            version: 1,
            domain: "filesystem",
            risk: RiskClass::R0,
            authority: "read",
            description: "Hash one explicitly named local file within a strict size bound",
        },
        RescueToolDefinition {
            id: "filesystem.permissions.inspect",
            version: 1,
            domain: "filesystem",
            risk: RiskClass::R0,
            authority: "read",
            description: "Inspect a bounded ACL for one explicitly named local path",
        },
    ];
    &TOOLS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait ProbeRunner {
    fn run(&self, tool_id: &str) -> Result<ProbeOutput, String>;

    fn run_with_arguments(&self, tool_id: &str, arguments: &Value) -> Result<ProbeOutput, String> {
        if arguments.is_null() || arguments.as_object().is_some_and(|value| value.is_empty()) {
            self.run(tool_id)
        } else {
            Err(format!("probe does not accept arguments: {tool_id}"))
        }
    }
}

pub struct WindowsProbeRunner;

impl ProbeRunner for WindowsProbeRunner {
    fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
        let script =
            probe_script(tool_id).ok_or_else(|| format!("unsupported Rescue probe: {tool_id}"))?;
        let script = format!(
            "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $OutputEncoding=[Console]::OutputEncoding; {script}"
        );
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start PowerShell probe: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match child
                .try_wait()
                .map_err(|error| format!("failed to poll PowerShell probe: {error}"))?
            {
                Some(_) => break,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Rescue probe timed out after 15 seconds: {tool_id}"
                    ));
                }
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to collect PowerShell probe output: {error}"))?;
        Ok(ProbeOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: bounded_utf8(&output.stdout),
            stderr: bounded_utf8(&output.stderr),
        })
    }

    fn run_with_arguments(&self, tool_id: &str, arguments: &Value) -> Result<ProbeOutput, String> {
        if let Some(result) = crate::rescue_filesystem::run(tool_id, arguments) {
            result
        } else if let Some(result) = crate::rescue_service::run(tool_id, arguments) {
            result
        } else if let Some(result) = crate::rescue_local_port::run(tool_id, arguments) {
            result
        } else if let Some(result) = crate::rescue_volume::run(tool_id, arguments) {
            result
        } else if let Some(result) = crate::rescue_memory::run(tool_id, arguments) {
            result
        } else {
            self.run(tool_id)
        }
    }
}

fn bounded_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PROBE_OUTPUT)]).to_string()
}

fn probe_script(tool_id: &str) -> Option<&'static str> {
    match tool_id {
        "system.inventory" => Some("$os=Get-CimInstance Win32_OperatingSystem; $cs=Get-CimInstance Win32_ComputerSystem; [pscustomobject]@{caption=$os.Caption;version=$os.Version;build=$os.BuildNumber;architecture=$os.OSArchitecture;computer=$env:COMPUTERNAME;manufacturer=$cs.Manufacturer;model=$cs.Model;memoryBytes=[int64]$cs.TotalPhysicalMemory;uptimeSeconds=[int64]((Get-Date)-$os.LastBootUpTime).TotalSeconds}|ConvertTo-Json -Compress"),
        "system.environment.inspect" => Some(ENVIRONMENT_INSPECT_SCRIPT),
        "system.process.list" => Some("@(Get-Process|Sort-Object CPU -Descending|Select-Object -First 25 Name,Id,CPU,WorkingSet64)|ConvertTo-Json -Compress"),
        "system.service.inspect" => Some("@(Get-CimInstance Win32_Service|Where-Object{$_.StartMode -eq 'Auto' -and $_.State -ne 'Running'}|Select-Object -First 40 Name,DisplayName,State,StartMode,ExitCode)|ConvertTo-Json -Compress"),
        "system.eventlog.query" => Some("$start=(Get-Date).AddHours(-24); $queryErrors=@(); $events=@(Get-WinEvent -FilterHashtable @{LogName=@('System','Application');Level=1,2;StartTime=$start} -MaxEvents 30 -ErrorAction SilentlyContinue -ErrorVariable +queryErrors); $realErrors=@($queryErrors|Where-Object{$_.FullyQualifiedErrorId -notmatch 'NoMatchingEventsFound'}); if($realErrors.Count -gt 0){throw $realErrors[0]}; @($events|ForEach-Object{[pscustomobject]@{timeCreated=$_.TimeCreated.ToUniversalTime().ToString('o');logName=$_.LogName;level=$_.LevelDisplayName;eventId=$_.Id;provider=$_.ProviderName;recordId=$_.RecordId}})|ConvertTo-Json -Compress -Depth 3"),
        "network.adapter.inspect" => Some("@(Get-NetIPConfiguration|Where-Object{$_.NetAdapter.Status -eq 'Up'}|ForEach-Object{[pscustomobject]@{alias=$_.InterfaceAlias;description=$_.InterfaceDescription;ipv4=@($_.IPv4Address|ForEach-Object{$_.IPAddress});ipv6=@($_.IPv6Address|ForEach-Object{$_.IPAddress}|Select-Object -First 4);gateway=@($_.IPv4DefaultGateway|ForEach-Object{$_.NextHop});dnsServers=@($_.DNSServer.ServerAddresses)}})|ConvertTo-Json -Compress -Depth 5"),
        "network.route.inspect" => Some("@(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue|Sort-Object RouteMetric|Select-Object -First 12 InterfaceAlias,NextHop,RouteMetric,State)|ConvertTo-Json -Compress"),
        "network.proxy.inspect" => Some(NETWORK_PROXY_INSPECT_SCRIPT),
        // R1 probes are deliberately absent from the automatic runner. A future
        // consent envelope supplies a validated hostname to a dedicated adapter.
        "network.dns.resolve" | "network.tcp.probe" => None,
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Observed,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceNode {
    pub id: String,
    pub tool_id: String,
    pub tool_version: u16,
    pub state: EvidenceState,
    pub summary: String,
    pub structured: Value,
    pub exit_code: Option<i32>,
    pub content_digest: String,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisState {
    Supported,
    Contradicted,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hypothesis {
    pub id: String,
    pub title: String,
    pub state: HypothesisState,
    pub confidence: f32,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisOutcome {
    Diagnosed,
    SafeStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisReport {
    pub version: &'static str,
    pub symptom: String,
    pub domain: String,
    pub outcome: DiagnosisOutcome,
    pub evidence: Vec<EvidenceNode>,
    pub hypotheses: Vec<Hypothesis>,
    pub edges: Vec<EvidenceEdge>,
    pub next_probe: Option<String>,
    pub mutation_performed: bool,
}

pub fn is_rescue_request(request: &str) -> bool {
    if crate::rescue_memory::is_memory_request(request) {
        return true;
    }
    let value = request.to_lowercase();
    let machine_scope = [
        "电脑",
        "windows",
        "系统",
        "网络",
        "上网",
        "wifi",
        "wi-fi",
        "dns",
        "代理",
        "proxy",
        "服务",
        "进程",
        "端口",
        "网站",
        "域名",
        "path",
        "环境变量",
        "powershell",
        "cmd",
        "http",
        "site",
        "port",
        "文件",
        "目录",
        "权限",
        "哈希",
        "摘要",
        "sha256",
        "acl",
        "permission",
        "checksum",
        "event log",
        "eventlog",
        "事件日志",
        "磁盘",
        "硬盘",
        "盘满",
        "空间不足",
        "disk space",
        "disk full",
        "storage full",
        "内存",
        "memory",
        "ram",
        "oom",
    ];
    let symptom = [
        "不能",
        "无法",
        "失败",
        "异常",
        "报错",
        "坏了",
        "冲突",
        "很慢",
        "断开",
        "诊断",
        "检查",
        "修复",
        "排查",
        "not working",
        "failed",
        "broken",
        "diagnose",
        "troubleshoot",
        "满了",
        "不足",
        "不够",
    ];
    machine_scope.iter().any(|item| value.contains(item))
        && symptom.iter().any(|item| value.contains(item))
}

pub fn diagnose(request: &str) -> DiagnosisReport {
    diagnose_with_runner(request, &WindowsProbeRunner)
}

pub fn diagnose_with_runner(request: &str, runner: &dyn ProbeRunner) -> DiagnosisReport {
    let domain = if crate::rescue_volume::is_volume_request(request) {
        "storage"
    } else if crate::rescue_filesystem::is_filesystem_request(request) {
        "filesystem"
    } else if is_network_request(request) {
        "network"
    } else {
        "system"
    };
    let mut probes = vec![("system.inventory".to_string(), Value::Null)];
    let local_port_probe = (domain == "network")
        .then(|| crate::rescue_local_port::requested_probe(request))
        .flatten();
    let volume_probe = (domain == "storage")
        .then(|| crate::rescue_volume::requested_probe(request))
        .flatten();
    let memory_probe = (domain == "system")
        .then(|| crate::rescue_memory::requested_probe(request))
        .flatten();
    let targeted_service = (domain == "system")
        .then(|| crate::rescue_service::requested_probe(request))
        .flatten();
    match domain {
        "network" => {
            if let Some(probe) = local_port_probe {
                probes.push(probe);
            } else {
                probes.extend(
                    [
                        "system.environment.inspect",
                        "network.adapter.inspect",
                        "network.route.inspect",
                        "network.proxy.inspect",
                    ]
                    .into_iter()
                    .map(|id| (id.to_string(), Value::Null)),
                );
            }
        }
        "filesystem" => probes.extend(crate::rescue_filesystem::requested_probes(request)),
        "storage" => {
            if let Some(probe) = volume_probe {
                probes.push(probe);
            }
        }
        _ => {
            if let Some(probe) = memory_probe {
                probes.push(probe);
            } else if let Some(probe) = targeted_service {
                probes.push(probe);
            } else {
                probes.extend(
                    [
                        "system.environment.inspect",
                        "system.process.list",
                        "system.service.inspect",
                    ]
                    .into_iter()
                    .map(|id| (id.to_string(), Value::Null)),
                );
            }
            if is_eventlog_request(request) {
                probes.push(("system.eventlog.query".into(), Value::Null));
            }
        }
    }
    let evidence = probes
        .into_iter()
        .map(|(id, arguments)| collect_with_arguments(runner, &id, &arguments))
        .collect::<Vec<_>>();
    let (hypotheses, edges, next_probe) = match domain {
        "network" => {
            if let Some((hypotheses, next_probe)) = crate::rescue_local_port::analyze(&evidence) {
                let edges = edges_from_hypotheses(&hypotheses);
                (hypotheses, edges, next_probe)
            } else {
                analyze_network(&evidence)
            }
        }
        "filesystem" => {
            let (hypotheses, next_probe) = crate::rescue_filesystem::analyze(request, &evidence);
            let edges = edges_from_hypotheses(&hypotheses);
            (hypotheses, edges, next_probe)
        }
        "storage" => {
            let (hypotheses, next_probe) = crate::rescue_volume::analyze(&evidence)
                .unwrap_or_else(|| (Vec::new(), Some("卷容量证据不可用；未执行清理".into())));
            let edges = edges_from_hypotheses(&hypotheses);
            (hypotheses, edges, next_probe)
        }
        _ => {
            if let Some((hypotheses, next_probe)) = crate::rescue_memory::analyze(&evidence) {
                let edges = edges_from_hypotheses(&hypotheses);
                (hypotheses, edges, next_probe)
            } else if let Some((hypotheses, next_probe)) = crate::rescue_service::analyze(&evidence)
            {
                let edges = edges_from_hypotheses(&hypotheses);
                (hypotheses, edges, next_probe)
            } else {
                analyze_system(&evidence)
            }
        }
    };
    let outcome = if hypotheses
        .iter()
        .any(|item| matches!(item.state, HypothesisState::Supported) && item.confidence >= 0.75)
    {
        DiagnosisOutcome::Diagnosed
    } else {
        DiagnosisOutcome::SafeStop
    };
    DiagnosisReport {
        version: "ucode.rescue.diagnosis.v1",
        symptom: request.trim().chars().take(2_000).collect(),
        domain: domain.into(),
        outcome,
        evidence,
        hypotheses,
        edges,
        next_probe,
        mutation_performed: false,
    }
}

fn is_eventlog_request(request: &str) -> bool {
    let value = request.to_lowercase();
    [
        "事件日志",
        "系统日志",
        "应用日志",
        "event log",
        "事件 id",
        "事件id",
    ]
    .iter()
    .any(|item| value.contains(item))
}

fn is_network_request(request: &str) -> bool {
    let value = request.to_lowercase();
    [
        "网络", "上网", "wifi", "wi-fi", "dns", "代理", "proxy", "网关", "路由", "网站", "域名",
        "端口", "network", "internet", "http", "site", "port",
    ]
    .iter()
    .any(|item| value.contains(item))
}

fn collect_with_arguments(
    runner: &dyn ProbeRunner,
    tool_id: &str,
    arguments: &Value,
) -> EvidenceNode {
    let started = Instant::now();
    let definition = tool_registry()
        .iter()
        .find(|tool| tool.id == tool_id)
        .expect("registered probe");
    match runner.run_with_arguments(tool_id, arguments) {
        Ok(output) => {
            let parsed = serde_json::from_str::<Value>(output.stdout.trim())
                .unwrap_or_else(|_| json!({ "text": output.stdout.trim() }));
            let state = if output.exit_code == 0 {
                EvidenceState::Observed
            } else {
                EvidenceState::Unavailable
            };
            let summary = summarize(tool_id, &parsed, output.exit_code, &output.stderr);
            evidence_node(
                definition,
                state,
                summary,
                parsed,
                Some(output.exit_code),
                started.elapsed().as_millis(),
            )
        }
        Err(error) => evidence_node(
            definition,
            EvidenceState::Unavailable,
            error.clone(),
            json!({ "error": error }),
            None,
            started.elapsed().as_millis(),
        ),
    }
}

fn evidence_node(
    definition: &RescueToolDefinition,
    state: EvidenceState,
    summary: String,
    structured: Value,
    exit_code: Option<i32>,
    elapsed_ms: u128,
) -> EvidenceNode {
    let canonical = serde_json::to_vec(&structured).unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(&canonical));
    EvidenceNode {
        id: format!("evidence-{}", &digest[..16]),
        tool_id: definition.id.into(),
        tool_version: definition.version,
        state,
        summary,
        structured,
        exit_code,
        content_digest: digest,
        elapsed_ms,
    }
}

fn summarize(tool_id: &str, value: &Value, exit_code: i32, stderr: &str) -> String {
    if exit_code != 0 {
        return format!(
            "探针失败（退出码 {exit_code}）：{}",
            stderr.trim().chars().take(300).collect::<String>()
        );
    }
    match tool_id {
        "network.adapter.inspect" => format!("发现 {} 个启用的 IP 配置", array_items(value).len()),
        "network.route.inspect" => format!("发现 {} 条 IPv4 默认路由", array_items(value).len()),
        "network.proxy.inspect" => {
            if proxy_configured(value) {
                "检测到代理配置".into()
            } else {
                "未检测到显式代理配置".into()
            }
        }
        "network.local_port.inspect" => {
            let port = value.get("port").and_then(Value::as_u64).unwrap_or(0);
            let listeners = value.get("listeners").map(array_items).unwrap_or_default();
            if value
                .get("occupied")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                format!("本机 TCP 端口 {port} 有 {} 个监听者", listeners.len())
            } else {
                format!("本机 TCP 端口 {port} 未发现监听者")
            }
        }
        "storage.volume.inspect" => {
            let drive = value
                .get("drive")
                .and_then(Value::as_str)
                .unwrap_or("指定卷");
            let available = value
                .get("availableBytes")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let used = value
                .get("usedPercent")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            format!(
                "{drive} 可用 {:.1} GiB，已用 {used:.1}%",
                available as f64 / 1_073_741_824.0
            )
        }
        "system.memory.inspect" => {
            let load = value
                .get("memoryLoadPercent")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let available = value
                .get("availablePhysicalBytes")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            format!(
                "内存已用 {load}%，可用 {:.1} GiB",
                available as f64 / 1_073_741_824.0
            )
        }
        "system.service.inspect" => format!("发现 {} 个未运行的自动服务", array_items(value).len()),
        "system.service.query" => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("未知服务");
            let state = value
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            format!("指定服务 {name} 当前状态为 {state}")
        }
        "system.eventlog.query" => format!(
            "发现 {} 条最近 24 小时的严重或错误事件元数据",
            array_items(value).len()
        ),
        "system.process.list" => format!("已采集 {} 个高资源进程摘要", array_items(value).len()),
        "filesystem.hash" => format!(
            "已计算 {}（{} bytes）的 SHA-256：{}",
            value
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("指定文件"),
            value.get("sizeBytes").and_then(Value::as_u64).unwrap_or(0),
            value
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or("未返回摘要")
        ),
        "filesystem.permissions.inspect" => format!(
            "已读取 {} 条 ACL 规则",
            value
                .get("entries")
                .map(array_items)
                .unwrap_or_default()
                .len()
        ),
        "system.environment.inspect" => format!(
            "PATH 包含 {} 个条目",
            value
                .get("pathEntryCount")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        "system.inventory" => format!(
            "{} build {}",
            value
                .get("caption")
                .and_then(Value::as_str)
                .unwrap_or("Windows"),
            value.get("build").and_then(Value::as_str).unwrap_or("未知")
        ),
        _ => "已采集证据".into(),
    }
}

fn array_items(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn find_evidence<'a>(evidence: &'a [EvidenceNode], tool_id: &str) -> Option<&'a EvidenceNode> {
    evidence.iter().find(|item| item.tool_id == tool_id)
}

fn observed(item: Option<&EvidenceNode>) -> bool {
    item.is_some_and(|value| matches!(value.state, EvidenceState::Observed))
}

fn edges_from_hypotheses(hypotheses: &[Hypothesis]) -> Vec<EvidenceEdge> {
    hypotheses
        .iter()
        .flat_map(|hypothesis| {
            hypothesis
                .evidence_ids
                .iter()
                .map(|evidence_id| EvidenceEdge {
                    from: evidence_id.clone(),
                    to: hypothesis.id.clone(),
                    relation: if matches!(hypothesis.state, HypothesisState::Contradicted) {
                        "contradicts".into()
                    } else {
                        "supports".into()
                    },
                })
        })
        .collect()
}

fn proxy_configured(value: &Value) -> bool {
    let bounded_flags = [
        "winHttpConfigured",
        "environmentProxyConfigured",
        "winInetManualProxyEnabled",
        "winInetAutoConfigPresent",
    ]
    .iter()
    .any(|key| value.get(key).and_then(Value::as_bool).unwrap_or(false));
    let env_proxy = ["httpProxy", "httpsProxy"].iter().any(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|item| !item.trim().is_empty())
    });
    let win_http = value
        .get("winHttp")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    bounded_flags
        || env_proxy
        || !(win_http.is_empty()
            || win_http.contains("direct access")
            || win_http.contains("直接访问"))
}

fn analyze_network(
    evidence: &[EvidenceNode],
) -> (Vec<Hypothesis>, Vec<EvidenceEdge>, Option<String>) {
    let adapter = find_evidence(evidence, "network.adapter.inspect");
    let route = find_evidence(evidence, "network.route.inspect");
    let proxy = find_evidence(evidence, "network.proxy.inspect");
    let adapters = adapter
        .map(|item| array_items(&item.structured))
        .unwrap_or_default();
    let routes = route
        .map(|item| array_items(&item.structured))
        .unwrap_or_default();
    let has_ipv4 = adapters.iter().any(|item| {
        item.get("ipv4")
            .is_some_and(|value| !array_items(value).is_empty())
    });
    let has_gateway = adapters.iter().any(|item| {
        item.get("gateway")
            .is_some_and(|value| !array_items(value).is_empty())
    }) || !routes.is_empty();
    let has_dns = adapters.iter().any(|item| {
        item.get("dnsServers")
            .is_some_and(|value| !array_items(value).is_empty())
    });
    let has_proxy =
        proxy.is_some_and(|item| observed(Some(item)) && proxy_configured(&item.structured));
    let mut hypotheses = Vec::new();
    let mut edges = Vec::new();
    let mut add = |id: &str,
                   title: &str,
                   state: HypothesisState,
                   confidence: f32,
                   sources: Vec<&EvidenceNode>| {
        let evidence_ids = sources
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        for source in &sources {
            edges.push(EvidenceEdge {
                from: source.id.clone(),
                to: id.into(),
                relation: if matches!(state, HypothesisState::Contradicted) {
                    "contradicts".into()
                } else {
                    "supports".into()
                },
            });
        }
        hypotheses.push(Hypothesis {
            id: id.into(),
            title: title.into(),
            state,
            confidence,
            evidence_ids,
        });
    };
    if observed(adapter) {
        add(
            "network.no_ipv4",
            "启用的网络适配器没有 IPv4 地址",
            if has_ipv4 {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Supported
            },
            if has_ipv4 { 0.08 } else { 0.95 },
            adapter.into_iter().collect(),
        );
        add(
            "network.no_dns",
            "启用的适配器没有 DNS 服务器配置",
            if has_dns {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Supported
            },
            if has_dns { 0.08 } else { 0.88 },
            adapter.into_iter().collect(),
        );
    } else {
        add(
            "network.adapter_unknown",
            "适配器状态未知",
            HypothesisState::Unresolved,
            0.35,
            adapter.into_iter().collect(),
        );
    }
    if observed(route) && observed(adapter) {
        let sources = [adapter, route].into_iter().flatten().collect();
        add(
            "network.no_gateway",
            "缺少可用的 IPv4 默认网关或默认路由",
            if has_gateway {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Supported
            },
            if has_gateway { 0.1 } else { 0.92 },
            sources,
        );
    }
    if observed(proxy) {
        add(
            "network.proxy_configured",
            "显式代理可能影响连接",
            if has_proxy {
                HypothesisState::Supported
            } else {
                HypothesisState::Contradicted
            },
            if has_proxy { 0.65 } else { 0.08 },
            proxy.into_iter().collect(),
        );
    }
    let local_configuration_looks_complete = has_ipv4 && has_gateway && has_dns && !has_proxy;
    add(
        "network.external_or_application",
        "本机基础配置完整，问题可能位于外部连通性、DNS 解析结果或具体应用",
        if local_configuration_looks_complete {
            HypothesisState::Unresolved
        } else {
            HypothesisState::Contradicted
        },
        if local_configuration_looks_complete {
            0.55
        } else {
            0.15
        },
        Vec::new(),
    );
    let next_probe = local_configuration_looks_complete
        .then(|| "network.dns.resolve（R1，需要用户允许网络探测）".into());
    (hypotheses, edges, next_probe)
}

fn analyze_system(
    evidence: &[EvidenceNode],
) -> (Vec<Hypothesis>, Vec<EvidenceEdge>, Option<String>) {
    let services = find_evidence(evidence, "system.service.inspect");
    let environment = find_evidence(evidence, "system.environment.inspect");
    let eventlog = find_evidence(evidence, "system.eventlog.query");
    let stopped = services
        .map(|item| array_items(&item.structured).len())
        .unwrap_or(0);
    let path_entries = environment
        .and_then(|item| item.structured.get("pathEntryCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let user_path_issues = environment
        .map(|item| {
            [
                "userPathEmptyEntries",
                "userPathDuplicateEntries",
                "userPathMissingAbsoluteDirectories",
            ]
            .iter()
            .map(|key| {
                item.structured
                    .get(key)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            })
            .sum::<u64>()
        })
        .unwrap_or(0);
    let machine_path_issues = environment
        .map(|item| {
            [
                "machinePathEmptyEntries",
                "machinePathDuplicateEntries",
                "machinePathMissingAbsoluteDirectories",
            ]
            .iter()
            .map(|key| {
                item.structured
                    .get(key)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            })
            .sum::<u64>()
        })
        .unwrap_or(0);
    let mut hypotheses = Vec::new();
    let mut edges = Vec::new();
    let mut add = |id: &str,
                   title: &str,
                   state: HypothesisState,
                   confidence: f32,
                   source: Option<&EvidenceNode>| {
        let evidence_ids = source
            .into_iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        if let Some(source) = source {
            edges.push(EvidenceEdge {
                from: source.id.clone(),
                to: id.into(),
                relation: "supports".into(),
            });
        }
        hypotheses.push(Hypothesis {
            id: id.into(),
            title: title.into(),
            state,
            confidence,
            evidence_ids,
        });
    };
    if observed(services) && stopped > 0 {
        add(
            "system.stopped_automatic_service",
            "存在未运行的自动服务，需要结合具体症状核对",
            HypothesisState::Supported,
            0.72,
            services,
        );
    } else {
        add(
            "system.service_failure",
            "自动服务异常",
            if observed(services) {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Unresolved
            },
            0.2,
            services,
        );
    }
    if observed(environment) && machine_path_issues > 0 {
        add(
            "system.machine_path_invalid",
            &format!("机器 PATH 有 {machine_path_issues} 个可确定清理的问题；修复需要 R3/UAC"),
            HypothesisState::Supported,
            0.94,
            environment,
        );
    }
    if observed(environment) && user_path_issues > 0 {
        add(
            "system.user_path_invalid",
            &format!("当前用户 PATH 有 {user_path_issues} 个可确定清理的问题"),
            HypothesisState::Supported,
            0.94,
            environment,
        );
    } else if observed(environment) && path_entries == 0 {
        add(
            "system.empty_path",
            "当前进程 PATH 为空",
            HypothesisState::Supported,
            0.9,
            environment,
        );
    } else {
        add(
            "system.environment_specific",
            "需要针对具体命令或应用继续检查环境",
            HypothesisState::Unresolved,
            0.4,
            environment,
        );
    }
    if let Some(events) = eventlog {
        let count = array_items(&events.structured).len();
        add(
            "system.recent_error_events",
            if count > 0 {
                "最近 24 小时存在严重或错误事件；事件元数据是线索，不等于已定位根因"
            } else {
                "最近 24 小时未发现 System/Application 严重或错误事件"
            },
            if count > 0 {
                HypothesisState::Unresolved
            } else if observed(Some(events)) {
                HypothesisState::Contradicted
            } else {
                HypothesisState::Unresolved
            },
            if count > 0 { 0.55 } else { 0.25 },
            Some(events),
        );
    }
    (
        hypotheses,
        edges,
        Some(if eventlog.is_some() {
            "请提供相关事件 ID、应用/服务名和发生时间，以便把日志线索与原始症状对应".into()
        } else {
            "提供具体错误文本、应用或服务名后执行定向只读探针".into()
        }),
    )
}

pub fn compact_report(report: &DiagnosisReport) -> Value {
    json!({
        "version": report.version,
        "symptom": report.symptom,
        "domain": report.domain,
        "outcome": report.outcome,
        "evidence": report.evidence.iter().map(|item| json!({
            "id": item.id,
            "toolId": item.tool_id,
            "state": item.state,
            "summary": item.summary,
            "structured": item.structured,
            "contentDigest": item.content_digest,
        })).collect::<Vec<_>>(),
        "hypotheses": report.hypotheses,
        "edges": report.edges,
        "nextProbe": report.next_probe,
        "mutationPerformed": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureRunner(std::collections::BTreeMap<&'static str, ProbeOutput>);
    impl ProbeRunner for FixtureRunner {
        fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
            self.0
                .get(tool_id)
                .cloned()
                .ok_or_else(|| "fixture missing".into())
        }
    }

    fn fixture(adapter: Value, route: Value, proxy: Value) -> FixtureRunner {
        let mut values = std::collections::BTreeMap::new();
        for (id, value) in [
            (
                "system.inventory",
                json!({"caption":"Windows 11","build":"26100"}),
            ),
            (
                "system.environment.inspect",
                json!({"pathEntryCount":12,"pathEntries":["C:/Windows"]}),
            ),
            ("network.adapter.inspect", adapter),
            ("network.route.inspect", route),
            ("network.proxy.inspect", proxy),
        ] {
            values.insert(
                id,
                ProbeOutput {
                    exit_code: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                },
            );
        }
        FixtureRunner(values)
    }

    fn hypothesis<'a>(report: &'a DiagnosisReport, id: &str) -> &'a Hypothesis {
        report.hypotheses.iter().find(|item| item.id == id).unwrap()
    }

    #[test]
    fn registry_has_unique_versioned_tools_and_never_marks_dns_as_r0() {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tool_registry() {
            assert!(seen.insert(tool.id));
            assert!(tool.version > 0);
            assert!(!tool.description.is_empty());
        }
        let dns = tool_registry()
            .iter()
            .find(|tool| tool.id == "network.dns.resolve")
            .unwrap();
        assert_eq!(dns.risk, RiskClass::R1);
        assert!(probe_script(dns.id).is_none());
        let tcp = tool_registry()
            .iter()
            .find(|tool| tool.id == "network.tcp.probe")
            .unwrap();
        assert_eq!(tcp.risk, RiskClass::R1);
        assert!(probe_script(tcp.id).is_none());
        for id in [
            "system.eventlog.query",
            "system.service.query",
            "system.memory.inspect",
            "network.local_port.inspect",
            "storage.volume.inspect",
            "filesystem.hash",
            "filesystem.permissions.inspect",
        ] {
            let tool = tool_registry().iter().find(|tool| tool.id == id).unwrap();
            assert_eq!(tool.risk, RiskClass::R0);
        }
        assert!(probe_script("system.eventlog.query").is_some());
        assert!(probe_script("filesystem.hash").is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_environment_probe_returns_bounded_structured_user_path_evidence() {
        let output = WindowsProbeRunner
            .run("system.environment.inspect")
            .unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.len() <= MAX_PROBE_OUTPUT);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        assert!(value
            .get("pathEntryCount")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value
            .get("userPathEntryCount")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value
            .get("userPathDuplicateEntries")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value
            .get("userPathMissingAbsoluteDirectories")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value
            .get("machinePathEntryCount")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value
            .get("machinePathDuplicateEntries")
            .and_then(Value::as_u64)
            .is_some());
        assert!(value.get("userPath").is_none());
        assert!(value.get("machinePath").is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_proxy_probe_returns_only_bounded_presence_flags() {
        let output = WindowsProbeRunner.run("network.proxy.inspect").unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        for key in [
            "winHttpConfigured",
            "environmentProxyConfigured",
            "winInetManualProxyEnabled",
            "winInetProxyServerPresent",
            "winInetAutoConfigPresent",
        ] {
            assert!(value.get(key).and_then(Value::as_bool).is_some(), "{key}");
        }
        for raw in [
            "winHttp",
            "httpProxy",
            "httpsProxy",
            "noProxy",
            "proxyServer",
        ] {
            assert!(value.get(raw).is_none(), "raw proxy field leaked: {raw}");
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_event_log_probe_returns_only_bounded_metadata() {
        let output = WindowsProbeRunner.run("system.eventlog.query").unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.len() <= MAX_PROBE_OUTPUT);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        let items = array_items(&value);
        assert!(items.len() <= 30);
        for item in items {
            assert!(item.get("eventId").is_some());
            assert!(item.get("provider").is_some());
            assert!(item.get("message").is_none());
        }
    }

    #[test]
    fn identical_network_symptom_distinguishes_missing_gateway_from_proxy() {
        let healthy_adapter = json!([{"alias":"Ethernet","ipv4":["192.168.1.20"],"gateway":["192.168.1.1"],"dnsServers":["1.1.1.1"]}]);
        let missing_gateway = diagnose_with_runner(
            "电脑无法上网，请诊断",
            &fixture(
                json!([{"alias":"Ethernet","ipv4":["169.254.2.4"],"gateway":[],"dnsServers":["1.1.1.1"]}]),
                json!([]),
                json!({"winHttp":"Direct access (no proxy server).","httpProxy":null,"httpsProxy":null}),
            ),
        );
        let stale_proxy = diagnose_with_runner(
            "电脑无法上网，请诊断",
            &fixture(
                healthy_adapter,
                json!([{"InterfaceAlias":"Ethernet","NextHop":"192.168.1.1"}]),
                json!({"winHttp":"Proxy Server(s) : 127.0.0.1:7890","httpProxy":"http://127.0.0.1:7890","httpsProxy":null}),
            ),
        );
        assert!(matches!(
            hypothesis(&missing_gateway, "network.no_gateway").state,
            HypothesisState::Supported
        ));
        assert!(matches!(
            hypothesis(&stale_proxy, "network.no_gateway").state,
            HypothesisState::Contradicted
        ));
        assert!(matches!(
            hypothesis(&stale_proxy, "network.proxy_configured").state,
            HypothesisState::Supported
        ));
        assert_ne!(
            serde_json::to_string(&missing_gateway.hypotheses).unwrap(),
            serde_json::to_string(&stale_proxy.hypotheses).unwrap()
        );
        assert!(!missing_gateway.mutation_performed && !stale_proxy.mutation_performed);
    }

    #[test]
    fn explicit_site_and_port_failures_route_to_network_rescue() {
        for request in [
            "https://example.com 无法打开，请诊断",
            "example.com:443 端口连接失败，请排查",
            "这个网站访问异常，请检查网络",
        ] {
            assert!(is_rescue_request(request), "did not route: {request}");
            assert!(is_network_request(request), "wrong domain: {request}");
        }
    }

    #[test]
    fn explicit_local_port_runs_only_inventory_and_listener_probe() {
        struct LocalPortFixture {
            occupied: bool,
        }
        impl ProbeRunner for LocalPortFixture {
            fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
                if tool_id == "system.inventory" {
                    return Ok(ProbeOutput {
                        exit_code: 0,
                        stdout: json!({"caption":"Windows 11","build":"26100"}).to_string(),
                        stderr: String::new(),
                    });
                }
                Err(format!("unexpected probe: {tool_id}"))
            }

            fn run_with_arguments(
                &self,
                tool_id: &str,
                arguments: &Value,
            ) -> Result<ProbeOutput, String> {
                if tool_id != "network.local_port.inspect" {
                    return self.run(tool_id);
                }
                assert_eq!(arguments, &json!({"port":3082,"protocol":"tcp"}));
                Ok(ProbeOutput {
                    exit_code: 0,
                    stdout: if self.occupied {
                        json!({"port":3082,"protocol":"tcp","occupied":true,"listeners":[{"localAddress":"127.0.0.1","pid":1234,"processName":"node","ownerKind":"user_process"}],"truncated":false})
                    } else {
                        json!({"port":3082,"protocol":"tcp","occupied":false,"listeners":[],"truncated":false})
                    }
                    .to_string(),
                    stderr: String::new(),
                })
            }
        }

        let occupied = diagnose_with_runner(
            "本机端口 3082 被占用，帮我诊断并释放",
            &LocalPortFixture { occupied: true },
        );
        let available = diagnose_with_runner(
            "本机端口 3082 被占用，帮我诊断并释放",
            &LocalPortFixture { occupied: false },
        );
        for report in [&occupied, &available] {
            assert_eq!(report.evidence.len(), 2);
            assert!(report.evidence.iter().all(|item| matches!(
                item.tool_id.as_str(),
                "system.inventory" | "network.local_port.inspect"
            )));
            assert!(!report.mutation_performed);
            assert!(report.evidence.iter().all(|item| !matches!(
                item.tool_id.as_str(),
                "network.tcp.probe"
                    | "network.adapter.inspect"
                    | "network.route.inspect"
                    | "network.proxy.inspect"
            )));
        }
        assert!(matches!(
            hypothesis(&occupied, "network.local_port_occupied").state,
            HypothesisState::Supported
        ));
        assert!(matches!(
            hypothesis(&available, "network.local_port_occupied").state,
            HypothesisState::Contradicted
        ));
        assert_ne!(
            serde_json::to_string(&occupied.hypotheses).unwrap(),
            serde_json::to_string(&available.hypotheses).unwrap()
        );
    }

    #[test]
    fn explicit_volume_capacity_runs_only_inventory_and_volume_probe() {
        struct VolumeFixture;
        impl ProbeRunner for VolumeFixture {
            fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
                if tool_id == "system.inventory" {
                    return Ok(ProbeOutput {
                        exit_code: 0,
                        stdout: json!({"caption":"Windows 11","build":"26100"}).to_string(),
                        stderr: String::new(),
                    });
                }
                Err(format!("unexpected probe: {tool_id}"))
            }

            fn run_with_arguments(
                &self,
                tool_id: &str,
                arguments: &Value,
            ) -> Result<ProbeOutput, String> {
                if tool_id != "storage.volume.inspect" {
                    return self.run(tool_id);
                }
                assert_eq!(arguments, &json!({"drive":"C:"}));
                Ok(ProbeOutput {
                    exit_code: 0,
                    stdout: json!({
                        "drive":"C:",
                        "driveType":"fixed",
                        "totalBytes":100_u64 * 1024 * 1024 * 1024,
                        "availableBytes":8_u64 * 1024 * 1024 * 1024,
                        "usedBytes":92_u64 * 1024 * 1024 * 1024,
                        "usedPercent":92.0,
                        "readOnly":false
                    })
                    .to_string(),
                    stderr: String::new(),
                })
            }
        }

        let report = diagnose_with_runner("C盘空间不足，请诊断但不要删除文件", &VolumeFixture);
        assert_eq!(report.domain, "storage");
        assert_eq!(report.evidence.len(), 2);
        assert!(report.evidence.iter().all(|item| matches!(
            item.tool_id.as_str(),
            "system.inventory" | "storage.volume.inspect"
        )));
        assert!(matches!(
            hypothesis(&report, "storage.volume_capacity_pressure").state,
            HypothesisState::Supported
        ));
        assert!(report
            .next_probe
            .as_deref()
            .unwrap()
            .contains("没有枚举或删除"));
        assert!(!report.mutation_performed);
    }

    #[test]
    fn explicit_memory_pressure_runs_only_inventory_and_memory_probe() {
        struct MemoryFixture;
        impl ProbeRunner for MemoryFixture {
            fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
                let value = match tool_id {
                    "system.inventory" => json!({"caption":"Windows 11","build":"26100"}),
                    "system.memory.inspect" => json!({
                        "memoryLoadPercent":91,
                        "totalPhysicalBytes":16_u64 * 1024 * 1024 * 1024,
                        "availablePhysicalBytes":500_u64 * 1024 * 1024,
                        "totalPageFileBytes":24_u64 * 1024 * 1024 * 1024,
                        "availablePageFileBytes":8_u64 * 1024 * 1024 * 1024,
                        "topProcesses":[{"pid":42,"processName":"node","workingSetBytes":2_u64 * 1024 * 1024 * 1024,"ownerKind":"unknown"}],
                        "inaccessibleProcessCount":2,
                        "truncated":false
                    }),
                    _ => return Err(format!("unexpected probe: {tool_id}")),
                };
                Ok(ProbeOutput {
                    exit_code: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                })
            }
        }

        let report = diagnose_with_runner("内存占用 95%，请诊断但不要结束任何进程", &MemoryFixture);
        assert_eq!(report.domain, "system");
        assert_eq!(report.evidence.len(), 2);
        assert!(report.evidence.iter().all(|item| matches!(
            item.tool_id.as_str(),
            "system.inventory" | "system.memory.inspect"
        )));
        assert!(matches!(
            hypothesis(&report, "system.memory_pressure").state,
            HypothesisState::Supported
        ));
        assert!(report.next_probe.as_deref().unwrap().contains("没有结束"));
        assert!(!report.mutation_performed);
    }

    #[test]
    fn complete_local_network_stops_before_r1_network_probe() {
        let report = diagnose_with_runner(
            "Windows 网络无法访问网站",
            &fixture(
                json!([{"alias":"Wi-Fi","ipv4":["10.0.0.20"],"gateway":["10.0.0.1"],"dnsServers":["10.0.0.1"]}]),
                json!([{"InterfaceAlias":"Wi-Fi","NextHop":"10.0.0.1"}]),
                json!({"winHttp":"Direct access (no proxy server).","httpProxy":null,"httpsProxy":null}),
            ),
        );
        assert!(matches!(report.outcome, DiagnosisOutcome::SafeStop));
        assert!(report.next_probe.as_deref().unwrap().contains("R1"));
        assert!(report
            .evidence
            .iter()
            .all(|item| item.tool_id != "network.dns.resolve"));
    }

    #[test]
    fn failed_probe_is_explicitly_unavailable_not_a_fact() {
        struct Failing;
        impl ProbeRunner for Failing {
            fn run(&self, _: &str) -> Result<ProbeOutput, String> {
                Err("access denied".into())
            }
        }
        let report = diagnose_with_runner("系统服务异常，帮我诊断", &Failing);
        assert!(report
            .evidence
            .iter()
            .all(|item| matches!(item.state, EvidenceState::Unavailable)));
        assert!(matches!(report.outcome, DiagnosisOutcome::SafeStop));
        assert!(!report.mutation_performed);
    }

    #[test]
    fn explicit_service_name_runs_one_bounded_query_and_drives_the_diagnosis() {
        struct ServiceFixture;
        impl ProbeRunner for ServiceFixture {
            fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
                let value = match tool_id {
                    "system.inventory" => json!({"caption":"Windows 11","build":"26100"}),
                    "system.environment.inspect" => json!({"pathEntryCount":20}),
                    "system.process.list" | "system.service.inspect" => json!([]),
                    _ => return Err(format!("unexpected probe: {tool_id}")),
                };
                Ok(ProbeOutput {
                    exit_code: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                })
            }

            fn run_with_arguments(
                &self,
                tool_id: &str,
                arguments: &Value,
            ) -> Result<ProbeOutput, String> {
                if tool_id != "system.service.query" {
                    return self.run(tool_id);
                }
                assert_eq!(arguments, &json!({"name":"Spooler"}));
                Ok(ProbeOutput {
                    exit_code: 0,
                    stdout: json!({
                        "name":"Spooler",
                        "displayName":"Print Spooler",
                        "state":"Stopped",
                        "startMode":"Auto",
                        "serviceType":272,
                        "canStop":false,
                        "dependencies":["RPCSS"],
                        "dependents":[]
                    })
                    .to_string(),
                    stderr: String::new(),
                })
            }
        }

        let report = diagnose_with_runner("服务名 Spooler 无法启动，请诊断", &ServiceFixture);
        assert_eq!(report.evidence.len(), 2);
        assert!(report.evidence.iter().all(|item| matches!(
            item.tool_id.as_str(),
            "system.inventory" | "system.service.query"
        )));
        let evidence = find_evidence(&report.evidence, "system.service.query").unwrap();
        assert_eq!(evidence.structured["name"], "Spooler");
        assert!(matches!(
            hypothesis(&report, "system.requested_service_stopped").state,
            HypothesisState::Supported
        ));
        assert!(report
            .next_probe
            .as_deref()
            .unwrap()
            .contains("不能仅凭状态自动启动"));
        assert!(!report.mutation_performed);
    }

    #[test]
    fn eventlog_service_name_does_not_trigger_a_generic_event_log_scan() {
        assert!(!is_eventlog_request(
            "服务名 EventLog 异常，请只诊断这个服务"
        ));
        assert!(is_eventlog_request("检查 Windows Event Log 中的错误事件"));
        assert!(is_eventlog_request("检查 Windows 事件日志中的错误事件"));
    }

    #[test]
    fn system_diagnosis_distinguishes_a_repairable_user_path_fault() {
        let mut values = std::collections::BTreeMap::new();
        for (id, value) in [
            (
                "system.inventory",
                json!({"caption":"Windows 11","build":"26100"}),
            ),
            (
                "system.environment.inspect",
                json!({"pathEntryCount":20,"userPathEmptyEntries":1,"userPathDuplicateEntries":1,"userPathMissingAbsoluteDirectories":0}),
            ),
            ("system.process.list", json!([])),
            ("system.service.inspect", json!([])),
        ] {
            values.insert(
                id,
                ProbeOutput {
                    exit_code: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                },
            );
        }
        let report = diagnose_with_runner("Windows PATH 异常，请检查修复", &FixtureRunner(values));
        let finding = hypothesis(&report, "system.user_path_invalid");
        assert!(matches!(finding.state, HypothesisState::Supported));
        assert!(finding.confidence > 0.9);
        assert!(matches!(report.outcome, DiagnosisOutcome::Diagnosed));
        assert!(!report.mutation_performed);
    }

    #[test]
    fn event_log_metadata_runs_only_for_an_explicit_log_request() {
        let mut values = std::collections::BTreeMap::new();
        for (id, value) in [
            (
                "system.inventory",
                json!({"caption":"Windows 11","build":"26100"}),
            ),
            ("system.environment.inspect", json!({"pathEntryCount":20})),
            ("system.process.list", json!([])),
            ("system.service.inspect", json!([])),
            (
                "system.eventlog.query",
                json!([{"timeCreated":"2026-11-18T00:00:00Z","logName":"System","level":"Error","eventId":7000,"provider":"Service Control Manager","recordId":42}]),
            ),
        ] {
            values.insert(
                id,
                ProbeOutput {
                    exit_code: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                },
            );
        }
        let report =
            diagnose_with_runner("检查 Windows 事件日志中的系统错误", &FixtureRunner(values));
        let event = find_evidence(&report.evidence, "system.eventlog.query").unwrap();
        assert_eq!(array_items(&event.structured).len(), 1);
        assert!(event.structured.to_string().find("message").is_none());
        assert!(matches!(
            hypothesis(&report, "system.recent_error_events").state,
            HypothesisState::Unresolved
        ));
        assert!(matches!(report.outcome, DiagnosisOutcome::SafeStop));
    }

    #[test]
    fn explicit_file_hash_is_compared_without_reading_arbitrary_content() {
        struct FilesystemFixture;
        impl ProbeRunner for FilesystemFixture {
            fn run(&self, tool_id: &str) -> Result<ProbeOutput, String> {
                if tool_id == "system.inventory" {
                    return Ok(ProbeOutput {
                        exit_code: 0,
                        stdout: json!({"caption":"Windows 11","build":"26100"}).to_string(),
                        stderr: String::new(),
                    });
                }
                Err("unexpected probe".into())
            }

            fn run_with_arguments(
                &self,
                tool_id: &str,
                arguments: &Value,
            ) -> Result<ProbeOutput, String> {
                if tool_id == "filesystem.hash" {
                    assert_eq!(arguments["path"], "C:\\demo.txt");
                    return Ok(ProbeOutput {
                        exit_code: 0,
                        stdout:
                            json!({"path":"C:\\demo.txt","sizeBytes":4,"sha256":"a".repeat(64)})
                                .to_string(),
                        stderr: String::new(),
                    });
                }
                self.run(tool_id)
            }
        }
        let report = diagnose_with_runner(
            &format!("检查 `C:\\demo.txt` 的 SHA256，期望值 {}", "a".repeat(64)),
            &FilesystemFixture,
        );
        assert_eq!(report.domain, "filesystem");
        assert!(matches!(
            hypothesis(&report, "filesystem.hash_match").state,
            HypothesisState::Supported
        ));
        assert!(matches!(report.outcome, DiagnosisOutcome::Diagnosed));
        assert!(report
            .evidence
            .iter()
            .all(|item| item.structured.get("content").is_none()));
        assert!(find_evidence(&report.evidence, "filesystem.hash")
            .unwrap()
            .summary
            .contains(&"a".repeat(64)));
        assert!(!report.mutation_performed);
    }
}
