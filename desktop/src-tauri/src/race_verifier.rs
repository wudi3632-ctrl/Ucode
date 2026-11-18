use crate::race::{
    ChangeMetrics, HardFailure, RaceTarget, RaceTargetOperation, ValidationFact, ValidationOutcome,
};
use crate::race_workspace::{digest_tree, CandidateWorkspace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_OUTPUT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOperation {
    Create,
    Modify,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedChange {
    pub path: String,
    pub operation: ChangeOperation,
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateProposal {
    pub version: String,
    pub race_id: String,
    pub candidate_id: String,
    pub strategy: String,
    pub generation_attempt: u8,
    pub objective: String,
    pub changes: Vec<ProposedChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub diff: String,
    pub facts: Vec<ValidationFact>,
    pub metrics: ChangeMetrics,
    pub result_digest: String,
}

#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("candidate workspace is unavailable")]
    WorkspaceUnavailable,
    #[error("candidate proposal identity does not match its workspace")]
    IdentityMismatch,
    #[error("candidate proposal contains no changes")]
    EmptyProposal,
}

struct PreparedChange {
    path: String,
    operation: ChangeOperation,
    absolute: PathBuf,
    before: String,
    after: String,
}

pub fn verify_candidate(
    workspace: &CandidateWorkspace,
    proposal: &CandidateProposal,
    allowed_paths: &[String],
    targets: &[RaceTarget],
    acceptance: &[String],
) -> Result<VerificationResult, VerificationError> {
    if proposal.version != "ucode.candidate.v1"
        || proposal.race_id != workspace.race_id
        || proposal.candidate_id != workspace.candidate_id
        || proposal.generation_attempt == 0
        || proposal.generation_attempt > 2
    {
        return Err(VerificationError::IdentityMismatch);
    }
    if proposal.changes.is_empty() {
        return Err(VerificationError::EmptyProposal);
    }
    let root =
        fs::canonicalize(&workspace.path).map_err(|_| VerificationError::WorkspaceUnavailable)?;
    let allowed: HashSet<_> = allowed_paths
        .iter()
        .map(|value| value.replace('\\', "/"))
        .collect();
    let expected_operations = targets
        .iter()
        .map(|target| (target.path.replace('\\', "/"), target.operation))
        .collect::<std::collections::HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut facts = Vec::new();
    let mut prepared = Vec::new();
    for change in &proposal.changes {
        let normalized = change.path.replace('\\', "/");
        let invalid = normalized.is_empty()
            || normalized.len() > 260
            || Path::new(&normalized).is_absolute()
            || normalized
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            || Path::new(&normalized)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)));
        if invalid || !allowed.contains(&normalized) || !seen.insert(normalized.clone()) {
            facts.push(hard_fact(
                "path-boundary",
                format!("candidate path is outside the allowed scope: {normalized}"),
                HardFailure::WorkspaceEscape,
            ));
            return finish_failed(workspace, facts);
        }
        if let Some(expected) = expected_operations.get(&normalized) {
            let matches = matches!(
                (expected, &change.operation),
                (RaceTargetOperation::Create, ChangeOperation::Create)
                    | (RaceTargetOperation::Modify, ChangeOperation::Modify)
                    | (RaceTargetOperation::Delete, ChangeOperation::Delete)
            );
            if !matches {
                facts.push(hard_fact(
                    "mandate-operation",
                    format!(
                        "candidate operation for {normalized} does not match the requested operation"
                    ),
                    HardFailure::OperationMismatch,
                ));
                return finish_failed(workspace, facts);
            }
        }
        let absolute = root.join(&normalized);
        if path_crosses_link(&root, &absolute)? {
            facts.push(hard_fact(
                "path-boundary",
                format!("candidate path crosses a link or reparse point: {normalized}"),
                HardFailure::WorkspaceEscape,
            ));
            return finish_failed(workspace, facts);
        }
        let exists = absolute.is_file();
        let before = if exists {
            fs::read_to_string(&absolute)?
        } else {
            String::new()
        };
        let replayable = match change.operation {
            ChangeOperation::Create => !absolute.exists(),
            ChangeOperation::Modify | ChangeOperation::Delete => exists,
        };
        if !replayable {
            facts.push(hard_fact(
                "candidate-replay",
                format!("candidate operation does not match current file state: {normalized}"),
                HardFailure::NonReplayable,
            ));
            return finish_failed(workspace, facts);
        }
        let after = match change.operation {
            ChangeOperation::Delete => String::new(),
            ChangeOperation::Create | ChangeOperation::Modify => {
                let Some(content) = change
                    .content
                    .as_ref()
                    .filter(|content| !content.trim().is_empty())
                else {
                    facts.push(hard_fact(
                        "candidate-completeness",
                        format!("candidate did not provide complete content for {normalized}"),
                        HardFailure::NonReplayable,
                    ));
                    return finish_failed(workspace, facts);
                };
                content.clone()
            }
        };
        if before == after {
            facts.push(hard_fact(
                "non-empty-change",
                format!("candidate produced no content change for {normalized}"),
                HardFailure::EmptyChange,
            ));
            return finish_failed(workspace, facts);
        }
        if change.operation == ChangeOperation::Delete && looks_like_test(&normalized) {
            facts.push(hard_fact(
                "test-preservation",
                format!("candidate deleted a test file: {normalized}"),
                HardFailure::TestDeleted,
            ));
            return finish_failed(workspace, facts);
        }
        let lost = public_symbols(&normalized, &before)
            .difference(&public_symbols(&normalized, &after))
            .cloned()
            .collect::<Vec<_>>();
        if !lost.is_empty() {
            facts.push(hard_fact(
                "interface-preservation",
                format!(
                    "candidate removed existing interfaces from {normalized}: {}",
                    lost.join(", ")
                ),
                HardFailure::InterfaceLost,
            ));
            return finish_failed(workspace, facts);
        }
        prepared.push(PreparedChange {
            path: normalized,
            operation: change.operation.clone(),
            absolute,
            before,
            after,
        });
    }
    let missing = allowed.difference(&seen).cloned().collect::<BTreeSet<_>>();
    if !missing.is_empty() {
        facts.push(hard_fact(
            "mandate-coverage",
            format!(
                "candidate omitted required target paths: {}",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            ),
            HardFailure::MandateIncomplete,
        ));
        return finish_failed(workspace, facts);
    }
    facts.push(pass_fact(
        "mandate-coverage",
        format!(
            "candidate covered all {} required target paths",
            allowed.len()
        ),
    ));
    if !expected_operations.is_empty() {
        facts.push(pass_fact(
            "mandate-operation",
            "every candidate operation matches the explicit create/modify/delete request".into(),
        ));
    }
    facts.push(pass_fact(
        "path-boundary",
        format!(
            "{} changed paths stay inside the candidate mandate",
            prepared.len()
        ),
    ));
    facts.push(pass_fact(
        "interface-preservation",
        "no existing public interface was removed".into(),
    ));

    for change in &prepared {
        match change.operation {
            ChangeOperation::Delete => fs::remove_file(&change.absolute)?,
            ChangeOperation::Create | ChangeOperation::Modify => {
                if let Some(parent) = change.absolute.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&change.absolute, &change.after)?;
            }
        }
    }
    facts.push(pass_fact(
        "candidate-replay",
        "candidate changes were replayed only inside its isolated workspace".into(),
    ));

    let mut diff = String::new();
    let mut changed_lines = 0_u32;
    for change in &prepared {
        let patch = TextDiff::from_lines(&change.before, &change.after)
            .unified_diff()
            .header(&format!("a/{}", change.path), &format!("b/{}", change.path))
            .to_string();
        changed_lines += patch
            .lines()
            .filter(|line| {
                (line.starts_with('+') && !line.starts_with("+++"))
                    || (line.starts_with('-') && !line.starts_with("---"))
            })
            .count() as u32;
        diff.push_str(&patch);
    }

    for change in &prepared {
        if matches!(change.operation, ChangeOperation::Delete) {
            continue;
        }
        if let Some(fact) = syntax_fact(&root, &change.path)? {
            facts.push(fact);
        }
    }
    for command in acceptance {
        facts.push(run_acceptance(&root, command));
    }

    let result_digest = digest_tree(&root).map_err(|error| {
        VerificationError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        ))
    })?;
    Ok(VerificationResult {
        diff,
        facts,
        metrics: ChangeMetrics {
            modified_files: prepared.len() as u32,
            unrelated_files: 0,
            deleted_files: prepared
                .iter()
                .filter(|change| change.operation == ChangeOperation::Delete)
                .count() as u32,
            changed_lines,
        },
        result_digest,
    })
}

pub fn acceptance_is_supported(value: &str) -> bool {
    parse_acceptance(value).is_some()
}

fn syntax_fact(root: &Path, relative: &str) -> Result<Option<ValidationFact>, VerificationError> {
    let extension = Path::new(relative)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "json" {
        let content = fs::read_to_string(root.join(relative))?;
        return Ok(Some(
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(_) => pass_fact("syntax", format!("JSON syntax passed: {relative}")),
                Err(error) => hard_fact(
                    "syntax",
                    format!("JSON syntax failed in {relative}: {error}"),
                    HardFailure::SyntaxError,
                ),
            },
        ));
    }
    let invocation = match extension.as_str() {
        "py" => Some((
            "python",
            vec!["-m".into(), "py_compile".into(), relative.into()],
        )),
        "js" | "mjs" | "cjs" => Some(("node", vec!["--check".into(), relative.into()])),
        _ => None,
    };
    let Some((program, arguments)) = invocation else {
        return Ok(None);
    };
    Ok(Some(command_fact(
        root,
        "syntax",
        format!("{program} {}", arguments.join(" ")),
        program,
        &arguments,
        false,
    )))
}

fn run_acceptance(root: &Path, value: &str) -> ValidationFact {
    let Some((program, arguments)) = parse_acceptance(value) else {
        return ValidationFact {
            check_id: "acceptance".into(),
            outcome: ValidationOutcome::Failed,
            summary: format!("unsupported mechanical acceptance command: {value}"),
            required: true,
            hard_failure: Some(HardFailure::NonReplayable),
            command: Some(value.into()),
            exit_code: None,
            evidence_digests: Vec::new(),
        };
    };
    command_fact(root, "acceptance", value.into(), program, &arguments, true)
}

fn parse_acceptance(value: &str) -> Option<(&'static str, Vec<String>)> {
    let trimmed = value.trim();
    if trimmed.chars().any(|character| {
        matches!(
            character,
            ';' | '|' | '&' | '>' | '<' | '`' | '$' | '\r' | '\n'
        )
    }) {
        return None;
    }
    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        ["pytest", paths @ ..] if paths.iter().all(|path| safe_argument(path)) => Some((
            "python",
            ["-m", "pytest"]
                .into_iter()
                .map(str::to_string)
                .chain(paths.iter().map(|value| (*value).to_string()))
                .collect(),
        )),
        ["python", "-m", "pytest", paths @ ..] if paths.iter().all(|path| safe_argument(path)) => {
            Some((
                "python",
                parts[1..]
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            ))
        }
        ["cargo", "test"] => Some(("cargo", vec!["test".into()])),
        ["cargo", "test", filter] if safe_argument(filter) => {
            Some(("cargo", vec!["test".into(), (*filter).into()]))
        }
        ["npm", "test"] => Some((
            "npm",
            vec!["test".into(), "--".into(), "--runInBand".into()],
        )),
        ["npm", "run", script]
            if matches!(*script, "test" | "typecheck" | "build" | "check" | "lint") =>
        {
            Some(("npm", vec!["run".into(), (*script).into()]))
        }
        _ => None,
    }
}

fn safe_argument(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 260
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | '/' | '\\' | ':')
        })
        && !value
            .replace('\\', "/")
            .split('/')
            .any(|segment| segment == "..")
}

fn command_fact(
    root: &Path,
    check_id: &str,
    label: String,
    program: &str,
    arguments: &[String],
    required: bool,
) -> ValidationFact {
    match run_bounded(root, program, arguments) {
        Ok(output) if output.exit_code == Some(0) => ValidationFact {
            check_id: check_id.into(),
            outcome: ValidationOutcome::Passed,
            summary: format!("{label} passed{}", preview_suffix(&output.preview)),
            required,
            hard_failure: None,
            command: Some(label),
            exit_code: output.exit_code,
            evidence_digests: vec![output.digest],
        },
        Ok(output) => ValidationFact {
            check_id: check_id.into(),
            outcome: ValidationOutcome::Failed,
            summary: format!("{label} failed{}", preview_suffix(&output.preview)),
            required,
            hard_failure: Some(if check_id == "syntax" {
                HardFailure::SyntaxError
            } else {
                HardFailure::RequiredTestFailed
            }),
            command: Some(label),
            exit_code: output.exit_code,
            evidence_digests: vec![output.digest],
        },
        Err(error) => ValidationFact {
            check_id: check_id.into(),
            outcome: ValidationOutcome::Failed,
            summary: format!("{label} could not run: {error}"),
            required,
            hard_failure: Some(HardFailure::NonReplayable),
            command: Some(label),
            exit_code: None,
            evidence_digests: Vec::new(),
        },
    }
}

struct CommandOutput {
    exit_code: Option<i32>,
    preview: String,
    digest: String,
}

fn run_bounded(root: &Path, program: &str, arguments: &[String]) -> Result<CommandOutput, String> {
    let mut child = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr unavailable".to_string())?;
    let stdout_reader = thread::spawn(move || read_limited(stdout));
    let stderr_reader = thread::spawn(move || read_limited(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("command exceeded the 120 second validation limit".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut bytes = stdout_reader
        .join()
        .map_err(|_| "stdout reader failed".to_string())?;
    bytes.extend(
        stderr_reader
            .join()
            .map_err(|_| "stderr reader failed".to_string())?,
    );
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok(CommandOutput {
        exit_code: status.code(),
        preview: String::from_utf8_lossy(&bytes)
            .trim()
            .chars()
            .take(1_500)
            .collect(),
        digest,
    })
}

fn read_limited(mut reader: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) if output.len() < MAX_COMMAND_OUTPUT => {
                let remaining = MAX_COMMAND_OUTPUT - output.len();
                output.extend_from_slice(&buffer[..length.min(remaining)]);
            }
            Ok(_) => {}
        }
    }
    output
}

fn public_symbols(path: &str, content: &str) -> BTreeSet<String> {
    if content.is_empty() {
        return BTreeSet::new();
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let prefixes: &[&str] = match extension.as_str() {
                "py" => &["def ", "class ", "async def "],
                "js" | "mjs" | "cjs" | "ts" | "tsx" => &[
                    "export function ",
                    "export async function ",
                    "export class ",
                    "export const ",
                    "export let ",
                ],
                _ => &[],
            };
            prefixes.iter().find_map(|prefix| {
                line.strip_prefix(prefix).and_then(|rest| {
                    let name = rest
                        .split(|character: char| {
                            !(character.is_ascii_alphanumeric() || character == '_')
                        })
                        .next()
                        .unwrap_or_default();
                    (!name.is_empty() && !name.starts_with('_')).then(|| format!("{prefix}{name}"))
                })
            })
        })
        .collect()
}

fn path_crosses_link(root: &Path, target: &Path) -> Result<bool, VerificationError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| VerificationError::WorkspaceUnavailable)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if !current.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

fn looks_like_test(path: &str) -> bool {
    let value = path.to_ascii_lowercase();
    let name = Path::new(&value)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    value
        .split('/')
        .any(|part| matches!(part, "test" | "tests" | "__tests__"))
        || name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn finish_failed(
    workspace: &CandidateWorkspace,
    facts: Vec<ValidationFact>,
) -> Result<VerificationResult, VerificationError> {
    Ok(VerificationResult {
        diff: String::new(),
        facts,
        metrics: ChangeMetrics::default(),
        result_digest: workspace.base_digest.clone(),
    })
}

fn hard_fact(check_id: &str, summary: String, hard_failure: HardFailure) -> ValidationFact {
    ValidationFact {
        check_id: check_id.into(),
        outcome: ValidationOutcome::Failed,
        summary,
        required: true,
        hard_failure: Some(hard_failure),
        command: None,
        exit_code: None,
        evidence_digests: Vec::new(),
    }
}

fn pass_fact(check_id: &str, summary: String) -> ValidationFact {
    ValidationFact {
        check_id: check_id.into(),
        outcome: ValidationOutcome::Passed,
        summary,
        required: true,
        hard_failure: None,
        command: None,
        exit_code: None,
        evidence_digests: Vec::new(),
    }
}

fn preview_suffix(preview: &str) -> String {
    if preview.is_empty() {
        String::new()
    } else {
        format!(": {}", preview.replace(['\r', '\n'], " "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race_workspace::{create_candidate_workspace, remove_candidate_workspace};
    use tempfile::tempdir;

    fn proposal(workspace: &CandidateWorkspace, changes: Vec<ProposedChange>) -> CandidateProposal {
        CandidateProposal {
            version: "ucode.candidate.v1".into(),
            race_id: workspace.race_id.clone(),
            candidate_id: workspace.candidate_id.clone(),
            strategy: "minimal_change".into(),
            generation_attempt: 1,
            objective: "fix".into(),
            changes,
        }
    }

    #[test]
    fn proposal_applies_only_inside_the_candidate_and_produces_a_diff() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("app.json"), "{\"value\":1}\n").unwrap();
        let workspace = create_candidate_workspace(
            &project,
            &directory.path().join("managed"),
            "race-verify",
            "candidate-a",
        )
        .unwrap();
        let result = verify_candidate(
            &workspace,
            &proposal(
                &workspace,
                vec![ProposedChange {
                    path: "app.json".into(),
                    operation: ChangeOperation::Modify,
                    content: Some("{\"value\":2}\n".into()),
                }],
            ),
            &["app.json".into()],
            &[],
            &[],
        )
        .unwrap();
        assert!(result.diff.contains("-{\"value\":1}"));
        assert_eq!(result.metrics.modified_files, 1);
        assert_eq!(
            fs::read_to_string(project.join("app.json")).unwrap(),
            "{\"value\":1}\n"
        );
        assert_eq!(
            fs::read_to_string(Path::new(&workspace.path).join("app.json")).unwrap(),
            "{\"value\":2}\n"
        );
        remove_candidate_workspace(&workspace).unwrap();
    }

    #[test]
    fn scope_escape_test_deletion_and_interface_loss_are_hard_failures() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(project.join("tests")).unwrap();
        fs::write(project.join("api.py"), "def public_api():\n    return 1\n").unwrap();
        fs::write(
            project.join("tests/test_api.py"),
            "def test_api():\n    assert True\n",
        )
        .unwrap();
        let managed = directory.path().join("managed");

        let workspace =
            create_candidate_workspace(&project, &managed, "race-hard", "candidate-a").unwrap();
        let escaped = verify_candidate(
            &workspace,
            &proposal(
                &workspace,
                vec![ProposedChange {
                    path: "../outside.py".into(),
                    operation: ChangeOperation::Create,
                    content: Some("x=1".into()),
                }],
            ),
            &["api.py".into()],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            escaped.facts[0].hard_failure,
            Some(HardFailure::WorkspaceEscape)
        );
        remove_candidate_workspace(&workspace).unwrap();

        let workspace =
            create_candidate_workspace(&project, &managed, "race-hard", "candidate-b").unwrap();
        let lost = verify_candidate(
            &workspace,
            &proposal(
                &workspace,
                vec![ProposedChange {
                    path: "api.py".into(),
                    operation: ChangeOperation::Modify,
                    content: Some("def replacement():\n    return 2\n".into()),
                }],
            ),
            &["api.py".into()],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(lost.facts[0].hard_failure, Some(HardFailure::InterfaceLost));
        remove_candidate_workspace(&workspace).unwrap();

        let workspace =
            create_candidate_workspace(&project, &managed, "race-hard", "candidate-c").unwrap();
        let deleted = verify_candidate(
            &workspace,
            &proposal(
                &workspace,
                vec![ProposedChange {
                    path: "tests/test_api.py".into(),
                    operation: ChangeOperation::Delete,
                    content: None,
                }],
            ),
            &["tests/test_api.py".into()],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            deleted.facts[0].hard_failure,
            Some(HardFailure::TestDeleted)
        );
        remove_candidate_workspace(&workspace).unwrap();
    }

    #[test]
    fn mandate_requires_every_target_and_the_requested_operation() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("modify.txt"), "before\n").unwrap();
        fs::write(project.join("delete.txt"), "remove\n").unwrap();
        let managed = directory.path().join("managed");
        let allowed = vec![
            "create.txt".into(),
            "modify.txt".into(),
            "delete.txt".into(),
        ];
        let targets = vec![
            RaceTarget {
                path: "create.txt".into(),
                operation: RaceTargetOperation::Create,
            },
            RaceTarget {
                path: "modify.txt".into(),
                operation: RaceTargetOperation::Modify,
            },
            RaceTarget {
                path: "delete.txt".into(),
                operation: RaceTargetOperation::Delete,
            },
        ];

        let incomplete_workspace =
            create_candidate_workspace(&project, &managed, "race-mandate", "incomplete").unwrap();
        let incomplete = verify_candidate(
            &incomplete_workspace,
            &proposal(
                &incomplete_workspace,
                vec![ProposedChange {
                    path: "create.txt".into(),
                    operation: ChangeOperation::Create,
                    content: Some("created\n".into()),
                }],
            ),
            &allowed,
            &targets,
            &[],
        )
        .unwrap();
        assert!(incomplete
            .facts
            .iter()
            .any(|fact| fact.hard_failure == Some(HardFailure::MandateIncomplete)));
        remove_candidate_workspace(&incomplete_workspace).unwrap();

        let wrong_operation_workspace =
            create_candidate_workspace(&project, &managed, "race-mandate", "wrong-op").unwrap();
        let wrong_operation = verify_candidate(
            &wrong_operation_workspace,
            &proposal(
                &wrong_operation_workspace,
                vec![ProposedChange {
                    path: "modify.txt".into(),
                    operation: ChangeOperation::Delete,
                    content: None,
                }],
            ),
            &["modify.txt".into()],
            &[RaceTarget {
                path: "modify.txt".into(),
                operation: RaceTargetOperation::Modify,
            }],
            &[],
        )
        .unwrap();
        assert!(wrong_operation
            .facts
            .iter()
            .any(|fact| fact.hard_failure == Some(HardFailure::OperationMismatch)));
        remove_candidate_workspace(&wrong_operation_workspace).unwrap();

        let complete_workspace =
            create_candidate_workspace(&project, &managed, "race-mandate", "complete").unwrap();
        let complete = verify_candidate(
            &complete_workspace,
            &proposal(
                &complete_workspace,
                vec![
                    ProposedChange {
                        path: "create.txt".into(),
                        operation: ChangeOperation::Create,
                        content: Some("created\n".into()),
                    },
                    ProposedChange {
                        path: "modify.txt".into(),
                        operation: ChangeOperation::Modify,
                        content: Some("after\n".into()),
                    },
                    ProposedChange {
                        path: "delete.txt".into(),
                        operation: ChangeOperation::Delete,
                        content: None,
                    },
                ],
            ),
            &allowed,
            &targets,
            &[],
        )
        .unwrap();
        assert!(complete
            .facts
            .iter()
            .all(|fact| fact.hard_failure.is_none()));
        assert_eq!(complete.metrics.modified_files, 3);
        remove_candidate_workspace(&complete_workspace).unwrap();
    }

    #[test]
    fn acceptance_parser_allows_known_commands_and_rejects_shell_composition() {
        for value in [
            "pytest",
            "pytest tests/test_app.py",
            "cargo test",
            "npm test",
            "npm run typecheck",
        ] {
            assert!(
                acceptance_is_supported(value),
                "expected supported: {value}"
            );
        }
        for value in [
            "cargo test; rm -rf .",
            "powershell test.ps1",
            "npm run arbitrary",
            "pytest ../outside.py",
        ] {
            assert!(
                !acceptance_is_supported(value),
                "expected rejected: {value}"
            );
        }
    }

    #[test]
    fn invalid_json_is_a_syntax_hard_failure() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("app.json"), "{}\n").unwrap();
        let workspace = create_candidate_workspace(
            &project,
            &directory.path().join("managed"),
            "race-syntax",
            "candidate-a",
        )
        .unwrap();
        let result = verify_candidate(
            &workspace,
            &proposal(
                &workspace,
                vec![ProposedChange {
                    path: "app.json".into(),
                    operation: ChangeOperation::Modify,
                    content: Some("{broken".into()),
                }],
            ),
            &["app.json".into()],
            &[],
            &[],
        )
        .unwrap();
        assert!(result
            .facts
            .iter()
            .any(|fact| fact.hard_failure == Some(HardFailure::SyntaxError)));
        remove_candidate_workspace(&workspace).unwrap();
    }
}
