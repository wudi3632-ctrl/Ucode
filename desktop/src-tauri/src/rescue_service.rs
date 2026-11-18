use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_SERVICE_NAME: usize = 128;

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    extract_explicit_service_name(request)
        .map(|name| ("system.service.query".into(), json!({ "name": name })))
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == "system.service.query").then(|| run_service_query(arguments))
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence
        .iter()
        .find(|item| item.tool_id == "system.service.query")?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "system.requested_service_unavailable".into(),
                title: "无法读取指定 Windows 服务；请核对服务名是否准确".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.25,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("请提供服务属性中的内部服务名，而不是显示名称或可执行文件路径".into()),
        ));
    }

    let state = item
        .structured
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let exit_code = item
        .structured
        .get("exitCode")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let running = state.eq_ignore_ascii_case("running");
    let mut hypotheses = vec![Hypothesis {
        id: "system.requested_service_stopped".into(),
        title: if running {
            "指定 Windows 服务当前正在运行".into()
        } else {
            format!("指定 Windows 服务当前状态为 {state}")
        },
        state: if running {
            HypothesisState::Contradicted
        } else {
            HypothesisState::Supported
        },
        confidence: if running { 0.9 } else { 0.96 },
        evidence_ids: vec![item.id.clone()],
    }];
    if exit_code != 0 {
        hypotheses.push(Hypothesis {
            id: "system.requested_service_exit_code".into(),
            title: format!("指定 Windows 服务报告非零退出码 {exit_code}"),
            state: HypothesisState::Supported,
            confidence: 0.92,
            evidence_ids: vec![item.id.clone()],
        });
    }
    Some((
        hypotheses,
        Some(if running {
            "服务当前正在运行；如仍有故障，请提供发生时间、错误码和受影响功能以关联事件日志".into()
        } else {
            "服务未运行；下一步应结合启动类型、依赖服务和相关事件 ID 判断，不能仅凭状态自动启动"
                .into()
        }),
    ))
}

fn extract_explicit_service_name(request: &str) -> Option<String> {
    for marker in ["服务名", "service name", "服务", "service"] {
        let lower = request.to_lowercase();
        let mut offset = 0;
        while let Some(relative) = lower[offset..].find(marker) {
            let start = offset + relative + marker.len();
            let tail = request.get(start..)?.trim_start_matches(|character: char| {
                character.is_whitespace()
                    || matches!(character, ':' | '：' | '=' | '`' | '\'' | '"')
            });
            let candidate = tail
                .chars()
                .take_while(|character| is_service_name_character(*character))
                .take(MAX_SERVICE_NAME + 1)
                .collect::<String>();
            let boundary_is_safe = tail
                .get(candidate.len()..)
                .and_then(|value| value.chars().next())
                .is_none_or(is_service_name_delimiter);
            if boundary_is_safe && valid_service_name(&candidate) {
                return Some(candidate);
            }
            offset = start;
            if offset >= lower.len() {
                break;
            }
        }
    }
    None
}

fn is_service_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

fn is_service_name_delimiter(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ',' | '，'
                | '.'
                | '。'
                | ';'
                | '；'
                | '!'
                | '！'
                | '?'
                | '？'
                | ')'
                | '）'
                | '`'
                | '\''
                | '"'
        )
}

fn valid_service_name(value: &str) -> bool {
    (2..=MAX_SERVICE_NAME).contains(&value.len())
        && value.chars().all(is_service_name_character)
        && value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && ![
            "name", "service", "windows", "error", "failed", "stop", "stopped", "running",
        ]
        .iter()
        .any(|reserved| value.eq_ignore_ascii_case(reserved))
}

fn run_service_query(arguments: &Value) -> Result<ProbeOutput, String> {
    let name = arguments
        .get("name")
        .and_then(Value::as_str)
        .ok_or("服务探针缺少 name 参数")?;
    if !valid_service_name(name) {
        return Err("服务名无效；仅接受 2-128 个 ASCII 字母、数字、点、短横线或下划线".into());
    }
    let script = r#"
$name=$env:UCODE_RESCUE_SERVICE_NAME
$controller=Get-Service -Name $name -ErrorAction Stop
$registry=Get-ItemProperty -LiteralPath (Join-Path 'HKLM:\SYSTEM\CurrentControlSet\Services' $name) -ErrorAction Stop
$startMode=switch([int]$registry.Start){0{'Boot'}1{'System'}2{'Auto'}3{'Manual'}4{'Disabled'}default{'Unknown'}}
[pscustomobject]@{
  name=$controller.Name
  displayName=$controller.DisplayName
  state=$controller.Status.ToString()
  startMode=$startMode
  serviceType=[int64]$registry.Type
  canStop=[bool]$controller.CanStop
  dependencies=@($controller.ServicesDependedOn|Select-Object -First 16 -ExpandProperty Name)
  dependents=@($controller.DependentServices|Select-Object -First 16 -ExpandProperty Name)
}|ConvertTo-Json -Compress -Depth 4
"#;
    let command_text = format!(
        "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $OutputEncoding=[Console]::OutputEncoding; {script}"
    );
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &command_text,
        ])
        .env("UCODE_RESCUE_SERVICE_NAME", name)
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
        .map_err(|error| format!("无法启动服务只读探针：{error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(_) => break,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("服务只读探针在 10 秒后超时".into());
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法收集服务证据：{error}"))?;
    Ok(ProbeOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: bounded_utf8(&output.stdout),
        stderr: bounded_utf8(&output.stderr),
    })
}

fn bounded_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT_BYTES)]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_explicit_internal_names_and_rejects_unsafe_or_generic_input() {
        assert_eq!(
            requested_probe("服务名 Spooler 无法启动").unwrap().1["name"],
            "Spooler"
        );
        assert_eq!(
            requested_probe("diagnose service W32Time failed")
                .unwrap()
                .1["name"],
            "W32Time"
        );
        assert!(requested_probe("系统服务异常，帮我检查").is_none());
        assert!(requested_probe("服务 Spool* 无法启动").is_none());
        assert!(requested_probe("服务 ../../Spooler 无法启动").is_none());
        assert!(requested_probe("service name 'x'; Remove-Item C:\\*").is_none());
    }

    #[test]
    fn analysis_uses_only_the_requested_service_evidence() {
        let evidence = EvidenceNode {
            id: "evidence-service".into(),
            tool_id: "system.service.query".into(),
            tool_version: 1,
            state: EvidenceState::Observed,
            summary: "指定服务 Spooler 当前状态为 Stopped".into(),
            structured: json!({"name":"Spooler","state":"Stopped","exitCode":1067}),
            exit_code: Some(0),
            content_digest: "digest".into(),
            elapsed_ms: 1,
        };
        let (hypotheses, next) = analyze(&[evidence]).unwrap();
        assert!(hypotheses.iter().any(|item| {
            item.id == "system.requested_service_stopped"
                && matches!(item.state, HypothesisState::Supported)
        }));
        assert!(hypotheses
            .iter()
            .any(|item| item.id == "system.requested_service_exit_code"));
        assert!(next.unwrap().contains("不能仅凭状态自动启动"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_service_query_returns_bounded_metadata_without_executable_paths() {
        let output = run_service_query(&json!({"name":"EventLog"})).unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        for key in ["name", "state", "startMode", "serviceType", "dependencies"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        for forbidden in ["pathName", "commandLine", "account", "password"] {
            assert!(value.get(forbidden).is_none(), "leaked {forbidden}");
        }
    }
}
