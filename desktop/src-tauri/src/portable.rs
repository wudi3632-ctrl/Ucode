use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PortableError {
    #[error("portable root is unavailable")]
    RootUnavailable,
    #[error("project path is not an existing directory: {0}")]
    InvalidProject(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest error: {0}")]
    Manifest(String),
}

#[derive(Debug, Clone)]
pub struct PortableRoot(PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLocator {
    pub absolute_path: String,
    pub relative_path: Option<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapsuleHealth {
    pub status: String,
    pub integrity: String,
    pub root_available: bool,
    pub data_writable: bool,
    pub free_bytes: Option<u64>,
    pub minimum_free_bytes: u64,
    pub can_start_runtime: bool,
    pub side_effects_allowed: bool,
    pub checked_at: String,
    pub issues: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntegrityEntry {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct IntegritySection {
    algorithm: String,
    files: Vec<IntegrityEntry>,
}

#[derive(Debug, Deserialize)]
struct CapsuleManifest {
    format: u64,
    integrity: IntegritySection,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntegrityCache {
    manifest_sha256: String,
    metadata_sha256: String,
    verified_at: String,
}

const MINIMUM_FREE_BYTES: u64 = 256 * 1024 * 1024;

impl PortableRoot {
    pub fn discover() -> Result<Self, PortableError> {
        if let Some(value) = env::var_os("UCODE_PORTABLE_ROOT") {
            return Self::from_path(value);
        }
        let executable = env::current_exe()?;
        let root = executable.parent().ok_or(PortableError::RootUnavailable)?;
        Self::from_path(root)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, PortableError> {
        fs::create_dir_all(path.as_ref())?;
        Ok(Self(fs::canonicalize(path)?))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn data_dir(&self) -> PathBuf {
        self.0.join("data")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.0.join("worktrees")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.0.join("skills")
    }

    pub fn inspect_capsule(&self) -> CapsuleHealth {
        let checked_at = chrono::Utc::now().to_rfc3339();
        let root_available = self.0.is_dir();
        let mut issues = Vec::new();
        let data_writable = root_available && self.probe_data_write().is_ok();
        if !root_available {
            issues.push("便携根目录不可用；请重新插入 U 盘或恢复原目录。".into());
        } else if !data_writable {
            issues.push("便携数据目录不可写；UCode 已停止新的会话和副作用。".into());
        }
        let free_bytes = root_available
            .then(|| available_space(&self.0).ok())
            .flatten();
        if free_bytes.is_some_and(|bytes| bytes < MINIMUM_FREE_BYTES) {
            issues.push(format!(
                "可用空间不足 256 MiB（当前 {} MiB）。",
                free_bytes.unwrap_or(0) / 1024 / 1024
            ));
        }
        let integrity = if cfg!(debug_assertions) && !self.0.join("ucode-manifest.json").is_file() {
            "development".to_string()
        } else {
            match self.verify_integrity_cached() {
                Ok(value) => value,
                Err(PortableError::Manifest(message)) => {
                    issues.push(format!("完整性校验失败：{message}"));
                    "failed".into()
                }
                Err(error) => {
                    issues.push(format!("完整性校验失败：{error}"));
                    "failed".into()
                }
            }
        };
        let integrity_ok = matches!(integrity.as_str(), "verified" | "cached" | "development");
        let can_start_runtime = root_available && data_writable && integrity_ok;
        let side_effects_allowed =
            can_start_runtime && free_bytes.map_or(true, |value| value >= MINIMUM_FREE_BYTES);
        let status = if !can_start_runtime {
            "blocked"
        } else if !side_effects_allowed {
            "warning"
        } else {
            "ready"
        };
        CapsuleHealth {
            status: status.into(),
            integrity,
            root_available,
            data_writable,
            free_bytes,
            minimum_free_bytes: MINIMUM_FREE_BYTES,
            can_start_runtime,
            side_effects_allowed,
            checked_at,
            issues,
        }
    }

    pub fn ensure_side_effects_allowed(&self) -> Result<(), PortableError> {
        let health = self.inspect_runtime_root();
        if health.side_effects_allowed {
            Ok(())
        } else {
            Err(PortableError::Manifest(health.issues.join(" ")))
        }
    }

    fn inspect_runtime_root(&self) -> CapsuleHealth {
        let root_available = self.0.is_dir();
        let data_writable = root_available && self.probe_data_write().is_ok();
        let free_bytes = root_available
            .then(|| available_space(&self.0).ok())
            .flatten();
        let mut issues = Vec::new();
        if !root_available {
            issues.push("便携根目录已断开，请重新插入 U 盘。".into());
        }
        if root_available && !data_writable {
            issues.push("便携数据目录当前不可写。".into());
        }
        if free_bytes.is_some_and(|value| value < MINIMUM_FREE_BYTES) {
            issues.push("便携盘剩余空间不足 256 MiB。".into());
        }
        let allowed = root_available
            && data_writable
            && free_bytes.map_or(true, |value| value >= MINIMUM_FREE_BYTES);
        CapsuleHealth {
            status: if allowed { "ready" } else { "blocked" }.into(),
            integrity: "runtime-check".into(),
            root_available,
            data_writable,
            free_bytes,
            minimum_free_bytes: MINIMUM_FREE_BYTES,
            can_start_runtime: allowed,
            side_effects_allowed: allowed,
            checked_at: chrono::Utc::now().to_rfc3339(),
            issues,
        }
    }

    fn probe_data_write(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(self.data_dir())?;
        let probe = self
            .data_dir()
            .join(format!(".write-probe-{}", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"ucode")?;
        file.sync_all()?;
        drop(file);
        fs::remove_file(probe)
    }

    fn verify_integrity_cached(&self) -> Result<String, PortableError> {
        let manifest_path = self.0.join("ucode-manifest.json");
        let manifest_bytes = fs::read(&manifest_path)
            .map_err(|_| PortableError::Manifest("ucode-manifest.json 缺失".into()))?;
        let manifest: CapsuleManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| PortableError::Manifest(error.to_string()))?;
        if manifest.format != 2 || manifest.integrity.algorithm != "sha256" {
            return Err(PortableError::Manifest("不支持的清单格式或摘要算法".into()));
        }
        let manifest_sha256 = digest_bytes(&manifest_bytes);
        let metadata_sha256 = metadata_digest(&self.0, &manifest.integrity.files)?;
        let cache_path = self.data_dir().join("integrity-cache.json");
        if let Ok(cache) = fs::read(&cache_path).and_then(|bytes| {
            serde_json::from_slice::<IntegrityCache>(&bytes).map_err(std::io::Error::other)
        }) {
            if cache.manifest_sha256 == manifest_sha256 && cache.metadata_sha256 == metadata_sha256
            {
                return Ok("cached".into());
            }
        }
        for entry in &manifest.integrity.files {
            let path = safe_manifest_path(&self.0, &entry.path)?;
            let metadata = fs::metadata(&path)
                .map_err(|_| PortableError::Manifest(format!("缺少 {}", entry.path)))?;
            if !metadata.is_file() || metadata.len() != entry.size {
                return Err(PortableError::Manifest(format!(
                    "文件大小不匹配：{}",
                    entry.path
                )));
            }
            if digest_file(&path)? != entry.sha256.to_lowercase() {
                return Err(PortableError::Manifest(format!(
                    "SHA-256 不匹配：{}",
                    entry.path
                )));
            }
        }
        let cache = IntegrityCache {
            manifest_sha256,
            metadata_sha256,
            verified_at: chrono::Utc::now().to_rfc3339(),
        };
        fs::write(
            cache_path,
            serde_json::to_vec_pretty(&cache)
                .map_err(|error| PortableError::Manifest(error.to_string()))?,
        )?;
        Ok("verified".into())
    }

    pub fn locate_project(&self, path: impl AsRef<Path>) -> Result<ProjectLocator, PortableError> {
        let project = fs::canonicalize(path.as_ref())?;
        if !project.is_dir() {
            return Err(PortableError::InvalidProject(project.display().to_string()));
        }
        let relative_path = project
            .strip_prefix(&self.0)
            .ok()
            .map(|value| value.to_string_lossy().replace('\\', "/"));
        Ok(ProjectLocator {
            absolute_path: project.display().to_string(),
            relative_path,
            fingerprint: fingerprint(&project)?,
        })
    }

    pub fn resolve(&self, locator: &ProjectLocator) -> Option<PathBuf> {
        if let Some(relative) = &locator.relative_path {
            let candidate = self
                .0
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if candidate.is_dir() {
                return fs::canonicalize(candidate).ok();
            }
        }
        let absolute = PathBuf::from(&locator.absolute_path);
        absolute
            .is_dir()
            .then(|| fs::canonicalize(absolute).ok())
            .flatten()
    }
}

fn safe_manifest_path(root: &Path, relative: &str) -> Result<PathBuf, PortableError> {
    let value = Path::new(relative);
    if relative.is_empty()
        || value.is_absolute()
        || value.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(PortableError::Manifest(format!(
            "不安全的清单路径：{relative}"
        )));
    }
    Ok(root.join(value))
}

fn metadata_digest(root: &Path, entries: &[IntegrityEntry]) -> Result<String, PortableError> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |value| value.get())
        .clamp(2, 8);
    let chunk_size = entries.len().div_ceil(workers).max(1);
    let parts = std::thread::scope(|scope| {
        let handles = entries
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|entry| {
                            let path = safe_manifest_path(root, &entry.path)?;
                            let metadata = fs::metadata(&path).map_err(|_| {
                                PortableError::Manifest(format!("缺少 {}", entry.path))
                            })?;
                            let modified = metadata
                                .modified()?
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos();
                            let mut entry_hasher = Sha256::new();
                            entry_hasher.update(entry.path.as_bytes());
                            entry_hasher.update(metadata.len().to_le_bytes());
                            entry_hasher.update(modified.to_le_bytes());
                            Ok::<[u8; 32], PortableError>(entry_hasher.finalize().into())
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| PortableError::Manifest("元数据校验线程异常退出".into()))?
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let mut hasher = Sha256::new();
    for digest in parts.into_iter().flatten() {
        hasher.update(digest);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn digest_file(path: &Path) -> Result<String, PortableError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn digest_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(windows)]
fn available_space(path: &Path) -> Result<u64, std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            free_available: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut free_available = 0_u64;
    let result = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(free_available)
    }
}

#[cfg(not(windows))]
fn available_space(_path: &Path) -> Result<u64, std::io::Error> {
    Ok(u64::MAX)
}

fn fingerprint(project: &Path) -> Result<String, PortableError> {
    let mut hasher = Sha256::new();
    hasher.update(
        project
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_bytes(),
    );
    for marker in [".git/HEAD", "Cargo.toml", "package.json", "pyproject.toml"] {
        let path = project.join(marker);
        if path.is_file() {
            hasher.update(marker.as_bytes());
            hasher.update(fs::read(path)?);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn relative_locator_survives_new_portable_root() {
        let first = tempdir().unwrap();
        let project = first.path().join("workspaces/calculator");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        let first_root = PortableRoot::from_path(first.path()).unwrap();
        let locator = first_root.locate_project(&project).unwrap();

        let second = tempdir().unwrap();
        let moved = second.path().join("workspaces/calculator");
        fs::create_dir_all(&moved).unwrap();
        fs::write(moved.join("package.json"), "{}").unwrap();
        let second_root = PortableRoot::from_path(second.path()).unwrap();

        assert_eq!(
            second_root.resolve(&locator),
            Some(fs::canonicalize(moved).unwrap())
        );
    }

    #[test]
    fn capsule_integrity_is_verified_cached_and_rejects_changed_content() {
        let directory = tempdir().unwrap();
        let payload = directory.path().join("runtime.bin");
        fs::write(&payload, b"trusted").unwrap();
        let manifest = serde_json::json!({
            "format": 2,
            "integrity": { "algorithm": "sha256", "files": [{
                "path": "runtime.bin", "size": 7, "sha256": digest_bytes(b"trusted")
            }] }
        });
        fs::write(
            directory.path().join("ucode-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let root = PortableRoot::from_path(directory.path()).unwrap();
        assert_eq!(root.inspect_capsule().integrity, "verified");
        assert_eq!(root.inspect_capsule().integrity, "cached");
        fs::write(&payload, b"changed").unwrap();
        let health = root.inspect_capsule();
        assert_eq!(health.status, "blocked");
        assert!(health.issues.iter().any(|issue| issue.contains("SHA-256")));
    }
}
