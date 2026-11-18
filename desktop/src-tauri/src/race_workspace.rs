use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use thiserror::Error;

const DEFAULT_MAX_FILES: usize = 2_000;
const DEFAULT_MAX_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".idea",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    ".vscode",
    "__pycache__",
    "dist",
    "node_modules",
    "target",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateWorkspaceKind {
    GitWorktree,
    BoundedCopy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateWorkspace {
    pub race_id: String,
    pub candidate_id: String,
    pub kind: CandidateWorkspaceKind,
    pub project_root: String,
    pub path: String,
    pub managed_root: String,
    pub base_revision: String,
    pub base_digest: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct CopyLimits {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for CopyLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_bytes: DEFAULT_MAX_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("candidate workspace identifier is invalid: {0}")]
    InvalidIdentifier(String),
    #[error("candidate workspace target already exists: {0}")]
    TargetExists(String),
    #[error("candidate workspace escaped its managed root")]
    ManagedRootEscape,
    #[error("Git command failed: {0}")]
    Git(String),
    #[error("Git candidate worktrees require a clean source checkout")]
    DirtyGitSource,
    #[error("candidate copy contains too many files ({found} > {limit})")]
    TooManyFiles { found: usize, limit: usize },
    #[error("candidate copy exceeds its total size limit ({found} > {limit} bytes)")]
    TooLarge { found: u64, limit: u64 },
    #[error("candidate copy contains an oversized file: {path} ({size} > {limit} bytes)")]
    OversizedFile { path: String, size: u64, limit: u64 },
    #[error("candidate workspace ownership marker is missing or invalid")]
    OwnershipMismatch,
}

pub fn create_candidate_workspace(
    project_root: &Path,
    managed_root: &Path,
    race_id: &str,
    candidate_id: &str,
) -> Result<CandidateWorkspace, WorkspaceError> {
    create_candidate_workspace_with_limits(
        project_root,
        managed_root,
        race_id,
        candidate_id,
        CopyLimits::default(),
    )
}

fn create_candidate_workspace_with_limits(
    project_root: &Path,
    managed_root: &Path,
    race_id: &str,
    candidate_id: &str,
    limits: CopyLimits,
) -> Result<CandidateWorkspace, WorkspaceError> {
    validate_identifier(race_id)?;
    validate_identifier(candidate_id)?;
    let project_root = fs::canonicalize(project_root)?;
    fs::create_dir_all(managed_root)?;
    let managed_root = fs::canonicalize(managed_root)?;
    if managed_root.starts_with(&project_root) || project_root.starts_with(&managed_root) {
        return Err(WorkspaceError::ManagedRootEscape);
    }
    let race_root = managed_root.join(race_id);
    fs::create_dir_all(&race_root)?;
    let race_root = fs::canonicalize(&race_root)?;
    if !race_root.starts_with(&managed_root) {
        return Err(WorkspaceError::ManagedRootEscape);
    }
    let target = race_root.join(candidate_id);
    if target.exists() {
        return Err(WorkspaceError::TargetExists(target.display().to_string()));
    }

    let (kind, base_revision, stats) = if git_repository(&project_root)? {
        if git_dirty(&project_root)? {
            return Err(WorkspaceError::DirtyGitSource);
        }
        let revision = git_output(&project_root, &["rev-parse", "HEAD"])?;
        let target_for_git = process_path(&target);
        let project_for_git = process_path(&project_root);
        let output = Command::new("git")
            .arg("-C")
            .arg(project_for_git)
            .args(["worktree", "add", "--detach"])
            .arg(target_for_git)
            .arg(&revision)
            .output()?;
        if !output.status.success() {
            return Err(WorkspaceError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        let stats = match snapshot_stats(&target, CopyLimits::default()) {
            Ok(stats) => stats,
            Err(error) => {
                let _ = Command::new("git")
                    .arg("-C")
                    .arg(process_path(&project_root))
                    .args(["worktree", "remove", "--force"])
                    .arg(process_path(&target))
                    .output();
                return Err(error);
            }
        };
        (CandidateWorkspaceKind::GitWorktree, revision, stats)
    } else {
        fs::create_dir(&target)?;
        let stats = match copy_bounded(&project_root, &target, limits) {
            Ok(stats) => stats,
            Err(error) => {
                let _ = fs::remove_dir_all(&target);
                return Err(error);
            }
        };
        (CandidateWorkspaceKind::BoundedCopy, "non_git".into(), stats)
    };
    let target = fs::canonicalize(&target)?;
    if !target.starts_with(&race_root) {
        return Err(WorkspaceError::ManagedRootEscape);
    }
    let base_digest = digest_tree(&target)?;
    let workspace = CandidateWorkspace {
        race_id: race_id.into(),
        candidate_id: candidate_id.into(),
        kind,
        project_root: project_root.display().to_string(),
        path: target.display().to_string(),
        managed_root: managed_root.display().to_string(),
        base_revision,
        base_digest,
        file_count: stats.0,
        total_bytes: stats.1,
    };
    write_owner_marker(&race_root, &workspace)?;
    Ok(workspace)
}

pub fn remove_candidate_workspace(workspace: &CandidateWorkspace) -> Result<(), WorkspaceError> {
    let managed_root = fs::canonicalize(&workspace.managed_root)?;
    let race_root = fs::canonicalize(managed_root.join(&workspace.race_id))?;
    let target = fs::canonicalize(&workspace.path)?;
    if !race_root.starts_with(&managed_root) || !target.starts_with(&race_root) {
        return Err(WorkspaceError::ManagedRootEscape);
    }
    verify_owner_marker(&race_root, workspace)?;
    match workspace.kind {
        CandidateWorkspaceKind::GitWorktree => {
            let project = process_path(Path::new(&workspace.project_root));
            let target_for_git = process_path(&target);
            let output = Command::new("git")
                .arg("-C")
                .arg(project)
                .args(["worktree", "remove", "--force"])
                .arg(target_for_git)
                .output()?;
            if !output.status.success() {
                return Err(WorkspaceError::Git(
                    String::from_utf8_lossy(&output.stderr).trim().to_string(),
                ));
            }
        }
        CandidateWorkspaceKind::BoundedCopy => fs::remove_dir_all(&target)?,
    }
    let marker = owner_marker(&race_root, &workspace.candidate_id);
    if marker.is_file() {
        fs::remove_file(marker)?;
    }
    if fs::read_dir(&race_root)?.next().is_none() {
        fs::remove_dir(&race_root)?;
    }
    Ok(())
}

pub fn digest_tree(root: &Path) -> Result<String, WorkspaceError> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0xff]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn copy_bounded(
    source: &Path,
    target: &Path,
    limits: CopyLimits,
) -> Result<(usize, u64), WorkspaceError> {
    let mut stats = (0_usize, 0_u64);
    copy_directory(source, source, target, limits, &mut stats)?;
    Ok(stats)
}

fn copy_directory(
    root: &Path,
    source: &Path,
    target: &Path,
    limits: CopyLimits,
    stats: &mut (usize, u64),
) -> Result<(), WorkspaceError> {
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if IGNORED_DIRECTORIES.contains(&name_text.as_ref()) {
                continue;
            }
            let next = target.join(&name);
            fs::create_dir(&next)?;
            copy_directory(root, &entry.path(), &next, limits, stats)?;
            if fs::read_dir(&next)?.next().is_none() {
                fs::remove_dir(next)?;
            }
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        stats.0 += 1;
        if stats.0 > limits.max_files {
            return Err(WorkspaceError::TooManyFiles {
                found: stats.0,
                limit: limits.max_files,
            });
        }
        if metadata.len() > limits.max_file_bytes {
            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(&entry.path())
                .display()
                .to_string();
            return Err(WorkspaceError::OversizedFile {
                path: relative,
                size: metadata.len(),
                limit: limits.max_file_bytes,
            });
        }
        stats.1 = stats.1.saturating_add(metadata.len());
        if stats.1 > limits.max_bytes {
            return Err(WorkspaceError::TooLarge {
                found: stats.1,
                limit: limits.max_bytes,
            });
        }
        fs::copy(entry.path(), target.join(name))?;
    }
    Ok(())
}

fn snapshot_stats(root: &Path, limits: CopyLimits) -> Result<(usize, u64), WorkspaceError> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    let mut bytes = 0_u64;
    for (_, path) in &files {
        let size = fs::metadata(path)?.len();
        if size > limits.max_file_bytes {
            return Err(WorkspaceError::OversizedFile {
                path: path.display().to_string(),
                size,
                limit: limits.max_file_bytes,
            });
        }
        bytes = bytes.saturating_add(size);
    }
    if files.len() > limits.max_files {
        return Err(WorkspaceError::TooManyFiles {
            found: files.len(),
            limit: limits.max_files,
        });
    }
    if bytes > limits.max_bytes {
        return Err(WorkspaceError::TooLarge {
            found: bytes,
            limit: limits.max_bytes,
        });
    }
    Ok((files.len(), bytes))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), WorkspaceError> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        if entry.file_name().to_string_lossy() == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if IGNORED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            collect_files(root, &entry.path(), output)?;
        } else if metadata.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| WorkspaceError::ManagedRootEscape)?
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, entry.path()));
        }
    }
    Ok(())
}

fn git_repository(root: &Path) -> Result<bool, WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(process_path(root))
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let top = String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace('\\', "/");
    Ok(top.eq_ignore_ascii_case(&process_path(root).to_string_lossy().replace('\\', "/")))
}

fn git_dirty(root: &Path) -> Result<bool, WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(process_path(root))
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(!output.stdout.is_empty())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String, WorkspaceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(process_path(root))
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_identifier(value: &str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkspaceError::InvalidIdentifier(value.into()));
    }
    Ok(())
}

fn write_owner_marker(
    race_root: &Path,
    workspace: &CandidateWorkspace,
) -> Result<(), WorkspaceError> {
    fs::write(
        owner_marker(race_root, &workspace.candidate_id),
        serde_json::to_vec(workspace)?,
    )?;
    Ok(())
}

fn verify_owner_marker(
    race_root: &Path,
    workspace: &CandidateWorkspace,
) -> Result<(), WorkspaceError> {
    let bytes = fs::read(owner_marker(race_root, &workspace.candidate_id))
        .map_err(|_| WorkspaceError::OwnershipMismatch)?;
    let owner: CandidateWorkspace =
        serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::OwnershipMismatch)?;
    if owner.race_id != workspace.race_id
        || owner.candidate_id != workspace.candidate_id
        || owner.path != workspace.path
    {
        return Err(WorkspaceError::OwnershipMismatch);
    }
    Ok(())
}

fn owner_marker(race_root: &Path, candidate_id: &str) -> PathBuf {
    race_root.join(format!(".{candidate_id}.owner.json"))
}

fn process_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn bounded_copies_are_isolated_ignore_generated_directories_and_clean_safely() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let managed = directory.path().join("managed");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
        fs::write(project.join("src/app.py"), "print('original')\n").unwrap();
        fs::write(project.join("node_modules/pkg/index.js"), "generated").unwrap();

        let first =
            create_candidate_workspace(&project, &managed, "race-1", "candidate-a").unwrap();
        let second =
            create_candidate_workspace(&project, &managed, "race-1", "candidate-b").unwrap();
        assert_eq!(first.kind, CandidateWorkspaceKind::BoundedCopy);
        assert!(!Path::new(&first.path).join("node_modules").exists());
        fs::write(
            Path::new(&first.path).join("src/app.py"),
            "print('candidate')\n",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(project.join("src/app.py")).unwrap(),
            "print('original')\n"
        );
        assert_eq!(
            fs::read_to_string(Path::new(&second.path).join("src/app.py")).unwrap(),
            "print('original')\n"
        );

        remove_candidate_workspace(&first).unwrap();
        assert!(!Path::new(&first.path).exists());
        assert!(Path::new(&second.path).exists());
        remove_candidate_workspace(&second).unwrap();
    }

    #[test]
    fn bounded_copy_rejects_size_and_file_count_overflow_without_leaving_a_target() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let managed = directory.path().join("managed");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("a.txt"), "a").unwrap();
        fs::write(project.join("b.txt"), "b").unwrap();
        let result = create_candidate_workspace_with_limits(
            &project,
            &managed,
            "race-limit",
            "candidate-a",
            CopyLimits {
                max_files: 1,
                max_bytes: 10,
                max_file_bytes: 10,
            },
        );
        assert!(matches!(result, Err(WorkspaceError::TooManyFiles { .. })));
        assert!(!managed.join("race-limit/candidate-a").exists());
        assert!(matches!(
            create_candidate_workspace(&project, &managed, "../escape", "candidate-a"),
            Err(WorkspaceError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn cleanup_requires_matching_workspace_ownership() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let managed = directory.path().join("managed");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("a.txt"), "a").unwrap();
        let workspace =
            create_candidate_workspace(&project, &managed, "race-owner", "candidate-a").unwrap();
        let race_root = managed.join("race-owner");
        let marker = owner_marker(&race_root, "candidate-a");
        fs::write(&marker, "{}").unwrap();
        assert!(matches!(
            remove_candidate_workspace(&workspace),
            Err(WorkspaceError::OwnershipMismatch)
        ));
        assert!(Path::new(&workspace.path).exists());
        fs::write(&marker, serde_json::to_vec(&workspace).unwrap()).unwrap();
        remove_candidate_workspace(&workspace).unwrap();
    }

    #[test]
    fn git_candidates_are_detached_isolated_and_removable_even_when_dirty() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let managed = directory.path().join("managed");
        fs::create_dir_all(&project).unwrap();
        git(&project, &["init"]);
        git(&project, &["config", "user.name", "UCode Test"]);
        git(&project, &["config", "user.email", "ucode@example.invalid"]);
        fs::write(project.join("app.txt"), "original\n").unwrap();
        git(&project, &["add", "app.txt"]);
        git(&project, &["commit", "-m", "fixture"]);

        let first =
            create_candidate_workspace(&project, &managed, "race-git", "candidate-a").unwrap();
        let second =
            create_candidate_workspace(&project, &managed, "race-git", "candidate-b").unwrap();
        assert_eq!(first.kind, CandidateWorkspaceKind::GitWorktree);
        assert_eq!(
            git_output(Path::new(&first.path), &["symbolic-ref", "-q", "HEAD"]).unwrap_or_default(),
            ""
        );
        fs::write(Path::new(&first.path).join("app.txt"), "candidate\n").unwrap();
        assert_eq!(
            fs::read_to_string(project.join("app.txt")).unwrap().trim(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(Path::new(&second.path).join("app.txt"))
                .unwrap()
                .trim(),
            "original"
        );

        remove_candidate_workspace(&first).unwrap();
        remove_candidate_workspace(&second).unwrap();
        assert_eq!(
            git_output(&project, &["worktree", "list", "--porcelain"])
                .unwrap()
                .matches("worktree ")
                .count(),
            1
        );
    }

    #[test]
    fn git_candidates_reject_a_dirty_source_instead_of_silently_losing_user_changes() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        git(&project, &["init"]);
        fs::write(project.join("untracked.txt"), "user data").unwrap();
        let result = create_candidate_workspace(
            &project,
            &directory.path().join("managed"),
            "race-dirty",
            "candidate-a",
        );
        assert!(matches!(result, Err(WorkspaceError::DirtyGitSource)));
    }
}
