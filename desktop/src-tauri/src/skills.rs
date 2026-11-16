use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_FILES: usize = 250;
const MAX_BYTES: u64 = 10 * 1024 * 1024;
const MAX_INSTRUCTIONS: usize = 24 * 1024;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("skill filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid skill: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreview {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub source_path: String,
    pub capabilities: Vec<String>,
    pub compatible: bool,
    pub compatibility_reason: Option<String>,
    pub instructions_digest: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Clone)]
pub struct SkillRuntime {
    root: PathBuf,
}

impl SkillRuntime {
    pub fn new(root: PathBuf) -> Result<Self, SkillError> {
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join(".trash"))?;
        Ok(Self { root })
    }

    pub fn preview(&self, source: &Path) -> Result<SkillPreview, SkillError> {
        let source = source.canonicalize()?;
        if !source.is_dir() {
            return Err(SkillError::Invalid("请选择技能目录".into()));
        }
        let (file_count, total_bytes) = scan_tree(&source)?;
        let instructions_path = source.join("SKILL.md");
        if !instructions_path.is_file() {
            return Err(SkillError::Invalid("技能目录缺少 SKILL.md".into()));
        }
        let instructions = fs::read_to_string(&instructions_path)?;
        if instructions.len() > MAX_INSTRUCTIONS {
            return Err(SkillError::Invalid("SKILL.md 超过 24 KiB 注入上限".into()));
        }
        let json_manifest = source.join("skill.json");
        let manifest: Option<Value> = if json_manifest.is_file() {
            let raw = fs::read_to_string(json_manifest)?;
            Some(
                serde_json::from_str(&raw)
                    .map_err(|error| SkillError::Invalid(format!("skill.json 无效：{error}")))?,
            )
        } else {
            None
        };
        let frontmatter = parse_frontmatter(&instructions);
        let fallback_name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("skill");
        let name = field(&manifest, "name")
            .or_else(|| frontmatter("name"))
            .unwrap_or_else(|| fallback_name.to_string());
        let id = normalize_id(&field(&manifest, "id").unwrap_or_else(|| name.clone()))?;
        let version = field(&manifest, "version")
            .or_else(|| frontmatter("version"))
            .unwrap_or_else(|| "1".into());
        let description = field(&manifest, "description")
            .or_else(|| frontmatter("description"))
            .unwrap_or_default();
        let capabilities = manifest
            .as_ref()
            .and_then(|value| value.get("capabilities"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let requirements = manifest
            .as_ref()
            .and_then(|value| value.get("requires"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let lower = instructions.to_lowercase();
        let implicit_external = requirements.is_empty()
            && lower.contains("mcp")
            && (lower.contains("server") || lower.contains("tools"));
        let compatible = requirements.is_empty() && !implicit_external;
        let compatibility_reason = if !requirements.is_empty() {
            Some(format!("需要未内置运行时：{}", requirements.join(", ")))
        } else if implicit_external {
            Some("该技能依赖外部 MCP/工具运行时；UCode 当前只支持纯说明型技能".into())
        } else {
            None
        };
        Ok(SkillPreview {
            id,
            name,
            version,
            description,
            source_path: source.display().to_string(),
            capabilities,
            compatible,
            compatibility_reason,
            instructions_digest: digest(&instructions),
            file_count,
            total_bytes,
        })
    }

    pub fn install(&self, preview: &SkillPreview) -> Result<PathBuf, SkillError> {
        let source = PathBuf::from(&preview.source_path).canonicalize()?;
        let verified = self.preview(&source)?;
        if verified.instructions_digest != preview.instructions_digest || verified.id != preview.id
        {
            return Err(SkillError::Invalid(
                "技能在审批后发生变化，请重新预览".into(),
            ));
        }
        let destination = self.root.join(&preview.id);
        if destination.exists() {
            return Err(SkillError::Invalid("同名技能已安装，请先卸载旧版本".into()));
        }
        copy_tree(&source, &destination)?;
        Ok(destination)
    }

    pub fn uninstall(&self, relative_path: &str) -> Result<PathBuf, SkillError> {
        let source = self.resolve_installed(relative_path)?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
        let destination = self.root.join(".trash").join(format!(
            "{}-{stamp}",
            source.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::rename(&source, &destination)?;
        Ok(destination)
    }

    pub fn instructions(
        &self,
        relative_path: &str,
        expected_digest: &str,
    ) -> Result<String, SkillError> {
        let content = fs::read_to_string(self.resolve_installed(relative_path)?.join("SKILL.md"))?;
        if content.len() > MAX_INSTRUCTIONS || digest(&content) != expected_digest {
            return Err(SkillError::Invalid(
                "技能完整性校验失败，需要重新安装并审批".into(),
            ));
        }
        Ok(content)
    }

    fn resolve_installed(&self, relative_path: &str) -> Result<PathBuf, SkillError> {
        let relative = Path::new(relative_path);
        if relative.components().count() != 1 {
            return Err(SkillError::Invalid("技能路径无效".into()));
        }
        let canonical = self.root.join(relative).canonicalize()?;
        let root = self.root.canonicalize()?;
        if !canonical.starts_with(&root) || canonical.starts_with(root.join(".trash")) {
            return Err(SkillError::Invalid("技能路径越界".into()));
        }
        Ok(canonical)
    }
}

fn scan_tree(root: &Path) -> Result<(usize, u64), SkillError> {
    let mut stack = vec![root.to_path_buf()];
    let mut count = 0usize;
    let mut bytes = 0u64;
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(SkillError::Invalid("技能不能包含符号链接".into()));
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                count += 1;
                bytes = bytes.saturating_add(metadata.len());
                if count > MAX_FILES {
                    return Err(SkillError::Invalid("技能文件数超过 250".into()));
                }
                if bytes > MAX_BYTES {
                    return Err(SkillError::Invalid("技能总大小超过 10 MiB".into()));
                }
            }
        }
    }
    Ok((count, bytes))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), SkillError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(SkillError::Invalid("技能不能包含符号链接".into()));
        }
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn normalize_id(value: &str) -> Result<String, SkillError> {
    let id = value
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() || id.len() > 80 {
        return Err(SkillError::Invalid("技能 id 无效".into()));
    }
    Ok(id)
}
fn field(manifest: &Option<Value>, key: &str) -> Option<String> {
    manifest.as_ref()?.get(key)?.as_str().map(str::to_string)
}
fn parse_frontmatter(content: &str) -> impl Fn(&str) -> Option<String> + '_ {
    move |key| {
        if !content.starts_with("---") {
            return None;
        }
        content
            .lines()
            .skip(1)
            .take_while(|line| line.trim() != "---")
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
            })
    }
}
fn digest(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn installs_verifies_and_trashes_instruction_skill() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("SKILL.md"), "---\nname: Concise\nversion: 1\ndescription: concise answers\n---\nAlways answer concisely.").unwrap();
        let runtime = SkillRuntime::new(directory.path().join("skills")).unwrap();
        let preview = runtime.preview(&source).unwrap();
        assert!(preview.compatible);
        runtime.install(&preview).unwrap();
        assert!(runtime
            .instructions(&preview.id, &preview.instructions_digest)
            .unwrap()
            .contains("concisely"));
        fs::write(
            directory
                .path()
                .join("skills")
                .join(&preview.id)
                .join("SKILL.md"),
            "tampered",
        )
        .unwrap();
        assert!(runtime
            .instructions(&preview.id, &preview.instructions_digest)
            .is_err());
        assert!(runtime.uninstall(&preview.id).unwrap().exists());
    }
    #[test]
    fn external_mcp_skill_is_honestly_incompatible() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: REA\n---\nUse the MCP server tools.",
        )
        .unwrap();
        let runtime = SkillRuntime::new(directory.path().join("skills")).unwrap();
        assert!(!runtime.preview(&source).unwrap().compatible);
    }
}
