use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use uuid::Uuid;

const MAX_ATTACHMENTS: usize = 8;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_CONTEXT_BYTES: usize = 120_000;
const SELECTION_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextAttachmentSummary {
    pub id: String,
    pub display_name: String,
    pub source: String,
    pub relative_path: Option<String>,
    pub size_bytes: u64,
    pub included_bytes: usize,
    pub truncated: bool,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextAttachmentError {
    pub display_name: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextAttachmentSelection {
    pub selection_id: String,
    pub attachments: Vec<ContextAttachmentSummary>,
    pub errors: Vec<ContextAttachmentError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPayload {
    pub id: String,
    pub display_name: String,
    pub source: String,
    pub relative_path: Option<String>,
    pub size_bytes: u64,
    pub included_bytes: usize,
    pub truncated: bool,
    pub sha256: String,
    pub content_sha256: String,
    pub content: String,
}

#[derive(Debug, Clone)]
struct PreparedAttachment {
    summary: ContextAttachmentSummary,
    canonical_path: PathBuf,
    content: String,
    content_sha256: String,
}

#[derive(Debug, Clone)]
struct PendingSelection {
    project_id: String,
    created_at: SystemTime,
    attachments: Vec<PreparedAttachment>,
}

#[derive(Default)]
pub struct AttachmentStore {
    selections: HashMap<String, PendingSelection>,
}

impl AttachmentStore {
    pub fn prepare(
        &mut self,
        project_id: &str,
        project_root: &Path,
        paths: Vec<PathBuf>,
    ) -> ContextAttachmentSelection {
        self.remove_expired(SystemTime::now());
        let project_root =
            fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        let mut attachments = Vec::new();
        let mut errors = Vec::new();
        let mut remaining = MAX_CONTEXT_BYTES;
        for path in paths.into_iter().take(MAX_ATTACHMENTS) {
            match prepare_file(&project_root, &path, remaining) {
                Ok(prepared) => {
                    remaining = remaining.saturating_sub(prepared.summary.included_bytes);
                    attachments.push(prepared);
                }
                Err(error) => errors.push(error),
            }
        }
        let selection_id = format!("selection-{}", Uuid::new_v4());
        let summaries = attachments
            .iter()
            .map(|item| item.summary.clone())
            .collect();
        if !attachments.is_empty() {
            self.selections.insert(
                selection_id.clone(),
                PendingSelection {
                    project_id: project_id.to_string(),
                    created_at: SystemTime::now(),
                    attachments,
                },
            );
        }
        ContextAttachmentSelection {
            selection_id,
            attachments: summaries,
            errors,
        }
    }

    pub fn append(
        &mut self,
        selection_id: &str,
        project_id: &str,
        project_root: &Path,
        paths: Vec<PathBuf>,
    ) -> Result<ContextAttachmentSelection, String> {
        let now = SystemTime::now();
        self.remove_expired(now);
        let selection = self
            .selections
            .get(selection_id)
            .ok_or_else(|| "附件选择已失效，请重新添加文件".to_string())?;
        if selection.project_id != project_id {
            return Err("附件选择不属于当前项目".into());
        }

        let project_root =
            fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        let existing_paths = selection
            .attachments
            .iter()
            .map(|item| item.canonical_path.clone())
            .collect::<Vec<_>>();
        let mut known_paths = existing_paths;
        let mut remaining = MAX_CONTEXT_BYTES.saturating_sub(
            selection
                .attachments
                .iter()
                .map(|item| item.summary.included_bytes)
                .sum::<usize>(),
        );
        let mut available_slots = MAX_ATTACHMENTS.saturating_sub(selection.attachments.len());
        let mut prepared = Vec::new();
        let mut errors = Vec::new();

        for path in paths {
            let canonical_path = fs::canonicalize(&path).ok();
            if canonical_path
                .as_ref()
                .is_some_and(|candidate| known_paths.iter().any(|known| known == candidate))
            {
                continue;
            }
            if available_slots == 0 {
                errors.push(ContextAttachmentError {
                    display_name: path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("未命名文件")
                        .to_string(),
                    code: "attachment_limit".into(),
                    message: "本轮最多添加 8 个附件".into(),
                });
                continue;
            }
            match prepare_file(&project_root, &path, remaining) {
                Ok(item) => {
                    remaining = remaining.saturating_sub(item.summary.included_bytes);
                    available_slots -= 1;
                    known_paths.push(item.canonical_path.clone());
                    prepared.push(item);
                }
                Err(error) => errors.push(error),
            }
        }

        let selection = self
            .selections
            .get_mut(selection_id)
            .ok_or_else(|| "附件选择已失效，请重新添加文件".to_string())?;
        selection.created_at = now;
        selection.attachments.extend(prepared);
        Ok(ContextAttachmentSelection {
            selection_id: selection_id.to_string(),
            attachments: selection
                .attachments
                .iter()
                .map(|item| item.summary.clone())
                .collect(),
            errors,
        })
    }

    pub fn resolve(
        &mut self,
        selection_id: Option<&str>,
        project_id: &str,
    ) -> Result<Vec<AttachmentPayload>, String> {
        self.remove_expired(SystemTime::now());
        let Some(selection_id) = selection_id.filter(|value| !value.trim().is_empty()) else {
            return Ok(Vec::new());
        };
        let selection = self
            .selections
            .get(selection_id)
            .ok_or_else(|| "附件选择已失效，请重新添加文件".to_string())?;
        if selection.project_id != project_id {
            return Err("附件选择不属于当前项目".into());
        }
        selection
            .attachments
            .iter()
            .map(|item| {
                let bytes = fs::read(&item.canonical_path).map_err(|_| {
                    format!("附件 {} 已无法读取，请重新添加", item.summary.display_name)
                })?;
                if digest(&bytes) != item.summary.sha256 {
                    return Err(format!(
                        "附件 {} 在选择后发生了变化，请重新添加",
                        item.summary.display_name
                    ));
                }
                Ok(AttachmentPayload {
                    id: item.summary.id.clone(),
                    display_name: item.summary.display_name.clone(),
                    source: item.summary.source.clone(),
                    relative_path: item.summary.relative_path.clone(),
                    size_bytes: item.summary.size_bytes,
                    included_bytes: item.summary.included_bytes,
                    truncated: item.summary.truncated,
                    sha256: item.summary.sha256.clone(),
                    content_sha256: item.content_sha256.clone(),
                    content: item.content.clone(),
                })
            })
            .collect()
    }

    pub fn complete(&mut self, selection_id: Option<&str>) {
        if let Some(selection_id) = selection_id {
            self.selections.remove(selection_id);
        }
    }

    pub fn remove_attachment(
        &mut self,
        selection_id: &str,
        attachment_id: &str,
    ) -> Result<(), String> {
        self.remove_expired(SystemTime::now());
        let empty = {
            let selection = self
                .selections
                .get_mut(selection_id)
                .ok_or_else(|| "附件选择已失效，请重新添加文件".to_string())?;
            let before = selection.attachments.len();
            selection
                .attachments
                .retain(|item| item.summary.id != attachment_id);
            if selection.attachments.len() == before {
                return Err("没有找到要移除的附件".into());
            }
            selection.attachments.is_empty()
        };
        if empty {
            self.selections.remove(selection_id);
        }
        Ok(())
    }

    fn remove_expired(&mut self, now: SystemTime) {
        self.selections.retain(|_, selection| {
            now.duration_since(selection.created_at)
                .map(|age| age <= SELECTION_TTL)
                .unwrap_or(false)
        });
    }
}

fn prepare_file(
    project_root: &Path,
    selected_path: &Path,
    remaining: usize,
) -> Result<PreparedAttachment, ContextAttachmentError> {
    let display_name = selected_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名文件")
        .to_string();
    let fail = |code: &str, message: String| ContextAttachmentError {
        display_name: display_name.clone(),
        code: code.to_string(),
        message,
    };
    let metadata = fs::symlink_metadata(selected_path)
        .map_err(|_| fail("attachment_unreadable", "文件不存在或不可读取".into()))?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(fail(
            "attachment_symlink",
            "不允许添加符号链接或重解析点".into(),
        ));
    }
    if !metadata.is_file() {
        return Err(fail("attachment_not_file", "只能添加普通文件".into()));
    }
    if metadata.len() == 0 {
        return Err(fail("attachment_empty", "空文件没有可用上下文".into()));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(fail(
            "attachment_too_large",
            "单个附件不能超过 256 KiB".into(),
        ));
    }
    if remaining == 0 {
        return Err(fail(
            "attachment_context_full",
            "本轮附件上下文已达到 120,000 bytes，请移除其他文件".into(),
        ));
    }
    let canonical_path = fs::canonicalize(selected_path)
        .map_err(|_| fail("attachment_unreadable", "文件路径无法解析".into()))?;
    let bytes = fs::read(&canonical_path)
        .map_err(|_| fail("attachment_unreadable", "文件读取失败".into()))?;
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(fail(
            "attachment_binary",
            "检测到二进制内容，仅支持 UTF-8 文本".into(),
        ));
    }
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| fail("attachment_encoding", "文件不是有效的 UTF-8 文本".into()))?;
    let included = truncate_utf8(&text, remaining);
    let included_bytes = included.len();
    if included_bytes == 0 {
        return Err(fail(
            "attachment_context_full",
            "本轮附件上下文已达到 120,000 bytes，请移除其他文件".into(),
        ));
    }
    let truncated = included_bytes < bytes.len();
    let relative_path = canonical_path
        .strip_prefix(project_root)
        .ok()
        .map(|value| value.to_string_lossy().replace('\\', "/"));
    let source = if relative_path.is_some() {
        "project"
    } else {
        "external_readonly"
    };
    let summary = ContextAttachmentSummary {
        id: format!("attachment-{}", Uuid::new_v4()),
        display_name,
        source: source.to_string(),
        relative_path,
        size_bytes: metadata.len(),
        included_bytes,
        truncated,
        sha256: digest(&bytes),
    };
    Ok(PreparedAttachment {
        content_sha256: digest(included.as_bytes()),
        summary,
        canonical_path,
        content: included.to_string(),
    })
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prepares_project_and_external_text_without_exposing_external_path() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let project_file = project.join("inside.txt");
        let external_file = root.path().join("outside.txt");
        fs::write(&project_file, "project marker").unwrap();
        fs::write(&external_file, "external marker").unwrap();
        let mut store = AttachmentStore::default();
        let selection = store.prepare("project-1", &project, vec![project_file, external_file]);
        assert!(selection.errors.is_empty());
        assert_eq!(selection.attachments[0].source, "project");
        assert_eq!(
            selection.attachments[0].relative_path.as_deref(),
            Some("inside.txt")
        );
        assert_eq!(selection.attachments[1].source, "external_readonly");
        assert_eq!(selection.attachments[1].relative_path, None);
        let serialized = serde_json::to_string(&selection).unwrap();
        assert!(!serialized.contains(root.path().to_string_lossy().as_ref()));
        let payload = store
            .resolve(Some(&selection.selection_id), "project-1")
            .unwrap();
        assert_eq!(payload[1].content, "external marker");
    }

    #[test]
    fn appends_to_an_existing_selection_and_ignores_duplicate_paths() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let first = project.join("first.txt");
        let second = root.path().join("second.txt");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let mut store = AttachmentStore::default();
        let initial = store.prepare("project-1", &project, vec![first.clone()]);

        let appended = store
            .append(
                &initial.selection_id,
                "project-1",
                &project,
                vec![first, second],
            )
            .unwrap();

        assert_eq!(appended.selection_id, initial.selection_id);
        assert_eq!(appended.attachments.len(), 2);
        assert_eq!(appended.attachments[0].source, "project");
        assert_eq!(appended.attachments[1].source, "external_readonly");
        assert!(appended.errors.is_empty());
        assert_eq!(
            store
                .resolve(Some(&appended.selection_id), "project-1")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn rejects_binary_oversize_project_mismatch_and_changed_files() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let binary = project.join("binary.bin");
        let large = project.join("large.txt");
        let changing = project.join("changing.txt");
        fs::write(&binary, [1_u8, 0, 2]).unwrap();
        fs::write(&large, vec![b'x'; MAX_FILE_BYTES as usize + 1]).unwrap();
        fs::write(&changing, "before").unwrap();
        let mut store = AttachmentStore::default();
        let failed = store.prepare("project-1", &project, vec![binary, large]);
        assert_eq!(failed.errors.len(), 2);
        assert!(failed.attachments.is_empty());
        let selection = store.prepare("project-1", &project, vec![changing.clone()]);
        assert!(store
            .resolve(Some(&selection.selection_id), "project-2")
            .is_err());
        fs::write(changing, "after").unwrap();
        assert!(store
            .resolve(Some(&selection.selection_id), "project-1")
            .unwrap_err()
            .contains("发生了变化"));
    }

    #[test]
    fn removing_a_chip_removes_the_server_side_attachment() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let first = project.join("first.txt");
        let second = project.join("second.txt");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let mut store = AttachmentStore::default();
        let selection = store.prepare("project-1", &project, vec![first, second]);
        store
            .remove_attachment(&selection.selection_id, &selection.attachments[0].id)
            .unwrap();
        let payload = store
            .resolve(Some(&selection.selection_id), "project-1")
            .unwrap();
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0].display_name, "second.txt");
    }

    #[test]
    fn truncates_on_utf8_boundary_and_completes_selection_once() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let first = project.join("first.txt");
        let second = project.join("second.txt");
        fs::write(&first, "a".repeat(MAX_CONTEXT_BYTES - 1)).unwrap();
        fs::write(&second, "你你").unwrap();
        let mut store = AttachmentStore::default();
        let selection = store.prepare("project-1", &project, vec![first, second]);
        assert_eq!(selection.attachments.len(), 1);
        assert_eq!(selection.errors.len(), 1);
        assert_eq!(selection.errors[0].code, "attachment_context_full");
        let payload = store
            .resolve(Some(&selection.selection_id), "project-1")
            .unwrap();
        assert_eq!(payload.len(), 1);
        store.complete(Some(&selection.selection_id));
        assert!(store
            .resolve(Some(&selection.selection_id), "project-1")
            .is_err());
    }

    #[test]
    fn accepts_text_by_content_caps_eight_files_and_expires_the_selection() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let mut paths = Vec::new();
        for index in 0..10 {
            let file = project.join(format!("context-{index}.ucode"));
            fs::write(&file, format!("plain UTF-8 context {index}")).unwrap();
            paths.push(file);
        }
        let invalid = project.join("invalid.txt");
        fs::write(&invalid, [0xff_u8, 0xfe, 0xfd]).unwrap();
        let mut store = AttachmentStore::default();
        let selection = store.prepare("project-1", &project, paths);
        assert_eq!(selection.attachments.len(), MAX_ATTACHMENTS);
        assert!(selection.errors.is_empty());
        assert!(selection
            .attachments
            .iter()
            .all(|item| item.display_name.ends_with(".ucode")));

        let rejected = store.prepare("project-1", &project, vec![invalid]);
        assert_eq!(rejected.errors[0].code, "attachment_encoding");

        store
            .selections
            .get_mut(&selection.selection_id)
            .unwrap()
            .created_at = SystemTime::UNIX_EPOCH;
        assert!(store
            .resolve(Some(&selection.selection_id), "project-1")
            .is_err());
    }
}
