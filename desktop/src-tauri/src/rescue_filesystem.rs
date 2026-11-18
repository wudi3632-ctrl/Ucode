use crate::rescue::{Hypothesis, HypothesisState, ProbeOutput};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_HASH_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ACL_ENTRIES: usize = 32;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

pub fn is_filesystem_request(request: &str) -> bool {
    let value = request.to_lowercase();
    let file_scope = [
        "文件",
        "目录",
        "路径",
        "权限",
        "哈希",
        "摘要",
        "sha256",
        "acl",
        "permission",
        "checksum",
        "hash",
        "file",
        "folder",
    ];
    let inspection = [
        "检查", "诊断", "排查", "验证", "确认", "读取", "inspect", "check", "verify", "diagnose",
    ];
    file_scope.iter().any(|item| value.contains(item))
        && inspection.iter().any(|item| value.contains(item))
}

pub fn requested_probes(request: &str) -> Vec<(String, Value)> {
    let Some(path) = extract_explicit_local_path(request) else {
        return Vec::new();
    };
    let value = request.to_lowercase();
    let wants_hash = ["哈希", "摘要", "sha256", "checksum", "hash"]
        .iter()
        .any(|item| value.contains(item));
    let wants_permissions = ["权限", "acl", "permission"]
        .iter()
        .any(|item| value.contains(item));
    let mut probes = Vec::new();
    if wants_hash {
        probes.push((
            "filesystem.hash".into(),
            json!({ "path": path, "expectedSha256": extract_expected_sha256(request) }),
        ));
    }
    if wants_permissions {
        probes.push((
            "filesystem.permissions.inspect".into(),
            json!({ "path": path }),
        ));
    }
    probes
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    match tool_id {
        "filesystem.hash" => Some(run_hash(arguments)),
        "filesystem.permissions.inspect" => Some(run_permissions(arguments)),
        _ => None,
    }
}

pub fn analyze(
    request: &str,
    evidence: &[crate::rescue::EvidenceNode],
) -> (Vec<Hypothesis>, Option<String>) {
    let expected = extract_expected_sha256(request);
    let hash = evidence
        .iter()
        .find(|item| item.tool_id == "filesystem.hash");
    let permissions = evidence
        .iter()
        .find(|item| item.tool_id == "filesystem.permissions.inspect");
    let mut hypotheses = Vec::new();

    if let Some(item) = hash {
        let actual = item
            .structured
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match expected.as_deref() {
            Some(value) if value.eq_ignore_ascii_case(actual) => hypotheses.push(Hypothesis {
                id: "filesystem.hash_match".into(),
                title: "文件 SHA-256 与用户提供的期望值一致".into(),
                state: HypothesisState::Supported,
                confidence: 0.99,
                evidence_ids: vec![item.id.clone()],
            }),
            Some(_) if !actual.is_empty() => hypotheses.push(Hypothesis {
                id: "filesystem.hash_mismatch".into(),
                title: "文件 SHA-256 与用户提供的期望值不一致".into(),
                state: HypothesisState::Supported,
                confidence: 0.99,
                evidence_ids: vec![item.id.clone()],
            }),
            _ => hypotheses.push(Hypothesis {
                id: "filesystem.hash_observed".into(),
                title: "已计算文件摘要，但没有期望值可用于判断完整性".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.45,
                evidence_ids: vec![item.id.clone()],
            }),
        }
    }

    if let Some(item) = permissions {
        let denied = item
            .structured
            .get("entries")
            .map(array_items)
            .unwrap_or_default()
            .iter()
            .any(|entry| {
                entry
                    .get("accessType")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("deny"))
            });
        hypotheses.push(Hypothesis {
            id: "filesystem.explicit_deny".into(),
            title: if denied {
                "ACL 中存在显式拒绝规则，需要结合当前用户身份核对".into()
            } else {
                "ACL 中未发现显式拒绝规则".into()
            },
            state: if denied {
                HypothesisState::Supported
            } else {
                HypothesisState::Contradicted
            },
            confidence: if denied { 0.78 } else { 0.3 },
            evidence_ids: vec![item.id.clone()],
        });
    }

    if hypotheses.is_empty() {
        hypotheses.push(Hypothesis {
            id: "filesystem.target_required".into(),
            title: "需要用户明确提供一个本地绝对文件或目录路径".into(),
            state: HypothesisState::Unresolved,
            confidence: 0.2,
            evidence_ids: Vec::new(),
        });
    }
    let next = if requested_probes(request).is_empty() {
        Some("请明确一个本地绝对路径，并说明要检查 SHA-256 还是 ACL 权限".into())
    } else if hash.is_some() && expected.is_none() {
        Some("如需判断完整性，请提供可信来源的期望 SHA-256；UCode 不会自行猜测".into())
    } else {
        Some("本轮仅采集摘要或 ACL；如仍有故障，请提供具体访问错误与执行身份".into())
    };
    (hypotheses, next)
}

fn run_hash(arguments: &Value) -> Result<ProbeOutput, String> {
    let path = validated_target(arguments, true)?;
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取文件元数据：{error}"))?;
    if !metadata.is_file() {
        return Err("filesystem.hash 只接受单个文件".into());
    }
    if metadata.len() > MAX_HASH_BYTES {
        return Err(format!(
            "文件超过摘要上限（{} MiB），已安全停止",
            MAX_HASH_BYTES / 1024 / 1024
        ));
    }
    let mut file = File::open(&path).map_err(|error| format!("无法打开文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取文件失败：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let structured = json!({
        "path": display_path(&path),
        "sizeBytes": metadata.len(),
        "sha256": format!("{:x}", hasher.finalize()),
    });
    Ok(json_output(structured))
}

fn run_permissions(arguments: &Value) -> Result<ProbeOutput, String> {
    let path = validated_target(arguments, false)?;
    let script = format!(
        "$acl=Get-Acl -LiteralPath $env:UCODE_RESCUE_TARGET -ErrorAction Stop; \
         $all=@($acl.Access); $entries=@($all|Select-Object -First {MAX_ACL_ENTRIES}|ForEach-Object{{[pscustomobject]@{{identity=$_.IdentityReference.Value;accessType=$_.AccessControlType.ToString();rights=$_.FileSystemRights.ToString();inherited=[bool]$_.IsInherited}}}}); \
         [pscustomobject]@{{path=$env:UCODE_RESCUE_TARGET;owner=$acl.Owner;protected=[bool]$acl.AreAccessRulesProtected;entries=$entries;truncated=[bool]($all.Count -gt {MAX_ACL_ENTRIES})}}|ConvertTo-Json -Compress -Depth 5"
    );
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); {script}"),
        ])
        .env("UCODE_RESCUE_TARGET", display_path(&path))
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
        .map_err(|error| format!("无法启动 ACL 只读探针：{error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(_) => break,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("ACL 只读探针在 10 秒后超时".into());
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法收集 ACL 证据：{error}"))?;
    Ok(ProbeOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: bounded_utf8(&output.stdout),
        stderr: bounded_utf8(&output.stderr),
    })
}

fn validated_target(arguments: &Value, reject_directory: bool) -> Result<PathBuf, String> {
    let raw = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or("文件探针缺少 path 参数")?;
    if raw.starts_with("\\\\")
        || raw.contains('*')
        || raw.contains('?')
        || raw.contains('%')
        || raw.contains('$')
    {
        return Err("只允许明确的本地绝对路径；UNC、通配符和变量路径已拒绝".into());
    }
    let candidate = Path::new(raw);
    if !candidate.is_absolute() || !has_drive_prefix(raw) {
        return Err("文件探针需要本地盘符开头的绝对路径".into());
    }
    ensure_local_drive(raw)?;
    let metadata = fs::symlink_metadata(candidate)
        .map_err(|error| format!("目标不存在或无法读取：{error}"))?;
    if is_reparse_point(&metadata) {
        return Err("符号链接或重解析点目标已拒绝；请提供实际本地路径".into());
    }
    if reject_directory && metadata.is_dir() {
        return Err("文件摘要不接受目录".into());
    }
    let canonical =
        fs::canonicalize(candidate).map_err(|error| format!("无法规范化本地路径：{error}"))?;
    let text = canonical.display().to_string();
    if text.starts_with("\\\\") && !text.starts_with("\\\\?\\") {
        return Err("网络路径已拒绝；离线 R0 文件探针只读取本地磁盘".into());
    }
    Ok(canonical)
}

fn extract_explicit_local_path(request: &str) -> Option<String> {
    for quote in ['`', '"', '\''] {
        let mut parts = request.split(quote);
        while let (Some(_), Some(value)) = (parts.next(), parts.next()) {
            let candidate = clean_candidate(value);
            if has_drive_prefix(&candidate) {
                return Some(candidate);
            }
        }
    }
    request
        .split_whitespace()
        .map(clean_candidate)
        .find(|candidate| has_drive_prefix(candidate))
}

fn clean_candidate(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character: char| {
            matches!(
                character,
                '`' | '"' | '\'' | ',' | ';' | '，' | '。' | '；' | '(' | ')' | '（' | '）'
            )
        })
        .to_string()
}

fn has_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn display_path(path: &Path) -> String {
    let value = path.display().to_string();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        value
    }
}

#[cfg(target_os = "windows")]
fn ensure_local_drive(value: &str) -> Result<(), String> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    }

    let root = format!("{}:\\", &value[..1]);
    let wide = std::ffi::OsStr::new(&root)
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    // DRIVE_REMOVABLE, DRIVE_FIXED, DRIVE_CDROM and DRIVE_RAMDISK are local.
    // DRIVE_REMOTE would silently turn an R0 file probe into network access.
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    if matches!(drive_type, 2 | 3 | 5 | 6) {
        Ok(())
    } else if drive_type == 4 {
        Err("映射网络驱动器已拒绝；离线 R0 文件探针只读取本地磁盘".into())
    } else {
        Err("无法确认目标盘符是可用的本地磁盘，已安全停止".into())
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_local_drive(_value: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn extract_expected_sha256(request: &str) -> Option<String> {
    request
        .split(|character: char| !character.is_ascii_hexdigit())
        .find(|token| token.len() == 64)
        .map(|token| token.to_ascii_lowercase())
}

fn json_output(value: Value) -> ProbeOutput {
    ProbeOutput {
        exit_code: 0,
        stdout: serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()),
        stderr: String::new(),
    }
}

fn bounded_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT_BYTES)]).to_string()
}

fn array_items(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn request_parser_requires_an_explicit_local_path_and_bounded_operation() {
        assert!(requested_probes("检查文件 hash").is_empty());
        assert!(requested_probes("检查 \\\\server\\share\\a.txt 的 hash").is_empty());
        let probes = requested_probes("检查 `C:\\Program Files\\demo.txt` 的 SHA256 和 ACL 权限");
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].0, "filesystem.hash");
        assert_eq!(probes[1].0, "filesystem.permissions.inspect");
        assert_eq!(probes[0].1["path"], "C:\\Program Files\\demo.txt");
    }

    #[test]
    fn hash_probe_reads_one_real_file_and_compares_expected_digest() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sample.txt");
        fs::write(&path, b"ucode rescue").unwrap();
        let output = run_hash(&json!({ "path": path.display().to_string() })).unwrap();
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["sizeBytes"], 12);
        assert_eq!(
            value["sha256"],
            "eb81f51ffb72c958b94546995facdb7d8ec82fe2c991a67763e91b7debe24ad1"
        );
    }

    #[test]
    fn filesystem_analysis_never_invents_integrity_without_an_expected_hash() {
        let evidence = crate::rescue::EvidenceNode {
            id: "evidence-hash".into(),
            tool_id: "filesystem.hash".into(),
            tool_version: 1,
            state: crate::rescue::EvidenceState::Observed,
            summary: "摘要已计算".into(),
            structured: json!({ "sha256": "a".repeat(64) }),
            exit_code: Some(0),
            content_digest: "b".repeat(64),
            elapsed_ms: 1,
        };
        let (hypotheses, next) = analyze("检查 C:\\demo.txt 的 hash", &[evidence]);
        assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        assert!(next.unwrap().contains("期望 SHA-256"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn permissions_probe_reads_a_real_bounded_acl_without_file_content() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("acl-sample.txt");
        fs::write(&path, b"private fixture content").unwrap();
        let output = run_permissions(&json!({ "path": path.display().to_string() })).unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        assert_eq!(
            value["path"],
            display_path(&fs::canonicalize(&path).unwrap())
        );
        assert!(array_items(&value["entries"]).len() <= MAX_ACL_ENTRIES);
        assert!(value.get("content").is_none());
    }
}
