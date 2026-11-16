use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_MODEL_PROFILE: &str = "qwen2-5-coder-1-5b-q4-k-m";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelPackManifest {
    format: u32,
    id: String,
    name: String,
    runtime: String,
    gguf_relative_path: String,
    bytes: u64,
    sha256: String,
    parameter_class: String,
    context_size: u32,
    minimum_ram_bytes: u64,
    recommended_ram_bytes: u64,
    task_suitability: Vec<String>,
    release_eligible: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPackInfo {
    pub id: String,
    pub name: String,
    pub parameter_class: String,
    pub context_size: u32,
    pub bytes: u64,
    pub minimum_ram_bytes: u64,
    pub recommended_ram_bytes: u64,
    pub task_suitability: Vec<String>,
    pub release_eligible: bool,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedModelPack {
    pub id: String,
    pub gguf_path: PathBuf,
    pub context_size: u32,
}

pub fn inventory(portable_root: &Path) -> Vec<ModelPackInfo> {
    let packs_root = portable_root.join("model-packs");
    let Ok(entries) = fs::read_dir(&packs_root) else {
        return Vec::new();
    };
    let mut packs = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| inspect_manifest(portable_root, &entry.path()))
        .collect::<Vec<_>>();
    packs.sort_by(|left, right| left.id.cmp(&right.id));
    packs
}

pub fn verify_selected(portable_root: &Path, profile_id: &str) -> Result<VerifiedModelPack, String> {
    validate_profile_id(profile_id)?;
    let profile_dir = portable_root.join("model-packs").join(profile_id);
    let manifest = read_manifest(&profile_dir)?;
    verify_manifest(portable_root, &profile_dir, manifest)
}

fn inspect_manifest(portable_root: &Path, profile_dir: &Path) -> ModelPackInfo {
    let fallback_id = profile_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("invalid-profile")
        .to_string();
    match read_manifest(profile_dir) {
        Ok(manifest) => {
            let base = ModelPackInfo {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                parameter_class: manifest.parameter_class.clone(),
                context_size: manifest.context_size,
                bytes: manifest.bytes,
                minimum_ram_bytes: manifest.minimum_ram_bytes,
                recommended_ram_bytes: manifest.recommended_ram_bytes,
                task_suitability: manifest.task_suitability.clone(),
                release_eligible: manifest.release_eligible,
                state: "verified".into(),
                error: None,
            };
            match verify_manifest(portable_root, profile_dir, manifest) {
                Ok(_) => base,
                Err(error) => ModelPackInfo {
                    state: "invalid".into(),
                    error: Some(error),
                    ..base
                },
            }
        }
        Err(error) => ModelPackInfo {
            id: fallback_id.clone(),
            name: fallback_id,
            parameter_class: "unknown".into(),
            context_size: 0,
            bytes: 0,
            minimum_ram_bytes: 0,
            recommended_ram_bytes: 0,
            task_suitability: Vec::new(),
            release_eligible: false,
            state: "invalid".into(),
            error: Some(error),
        },
    }
}

fn read_manifest(profile_dir: &Path) -> Result<ModelPackManifest, String> {
    let manifest_path = profile_dir.join("manifest.json");
    let bytes = fs::read(&manifest_path)
        .map_err(|_| format!("模型包清单缺失：{}", manifest_path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("模型包清单无效：{error}"))
}

fn verify_manifest(
    portable_root: &Path,
    profile_dir: &Path,
    manifest: ModelPackManifest,
) -> Result<VerifiedModelPack, String> {
    if manifest.format != 1 {
        return Err("不支持的模型包清单格式".into());
    }
    validate_profile_id(&manifest.id)?;
    let directory_id = profile_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "模型包目录名称无效".to_string())?;
    if directory_id != manifest.id {
        return Err("模型包 ID 与目录名称不一致".into());
    }
    if manifest.name.trim().is_empty() || manifest.name.chars().count() > 80 {
        return Err("模型包名称无效".into());
    }
    if manifest.runtime != "llama.cpp" {
        return Err("当前版本只支持 llama.cpp GGUF 模型包".into());
    }
    if !(1_024..=32_768).contains(&manifest.context_size) {
        return Err("模型上下文长度超出支持范围".into());
    }
    if manifest.bytes == 0
        || manifest.minimum_ram_bytes == 0
        || manifest.recommended_ram_bytes < manifest.minimum_ram_bytes
        || manifest.task_suitability.is_empty()
        || manifest.task_suitability.len() > 8
    {
        return Err("模型包资源说明不完整".into());
    }
    if manifest.sha256.len() != 64
        || !manifest.sha256.chars().all(|value| value.is_ascii_hexdigit())
    {
        return Err("模型包 SHA-256 无效".into());
    }
    let relative = Path::new(&manifest.gguf_relative_path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("模型路径必须是胶囊内的普通相对路径".into());
    }
    let canonical_root = fs::canonicalize(portable_root)
        .map_err(|error| format!("无法检查便携根目录：{error}"))?;
    let candidate = portable_root.join(relative);
    let canonical_model = fs::canonicalize(&candidate)
        .map_err(|_| format!("模型文件缺失：{}", manifest.gguf_relative_path))?;
    if !canonical_model.starts_with(&canonical_root) {
        return Err("模型路径越过便携胶囊边界".into());
    }
    let metadata = fs::metadata(&canonical_model)
        .map_err(|error| format!("无法读取模型文件：{error}"))?;
    if !metadata.is_file() || metadata.len() != manifest.bytes {
        return Err("模型文件大小与清单不一致".into());
    }
    let actual = digest_file(&canonical_model)?;
    if actual != manifest.sha256.to_ascii_lowercase() {
        return Err("模型文件 SHA-256 与清单不一致".into());
    }
    Ok(VerifiedModelPack {
        id: manifest.id,
        gguf_path: canonical_model,
        context_size: manifest.context_size,
    })
}

fn validate_profile_id(value: &str) -> Result<(), String> {
    let valid = (3..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-');
    valid
        .then_some(())
        .ok_or_else(|| "模型档位 ID 无效".to_string())
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取模型文件：{error}"))?;
    let mut digest = Sha256::new();
    // Keep the chunk on the heap. The packaged Windows GUI main thread has a
    // smaller stack than Rust's test workers; a 1 MiB stack array caused a
    // release-only 0xc00000fd crash before the first window became usable.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("模型校验失败：{error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn fixture(relative_path: &str, bytes: &[u8], declared_hash: Option<&str>) -> TempDir {
        let root = TempDir::new().unwrap();
        let model = root.path().join("model-packs/profile/model.gguf");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        File::create(&model).unwrap().write_all(bytes).unwrap();
        let hash = declared_hash
            .map(str::to_string)
            .unwrap_or_else(|| digest_file(&model).unwrap());
        let manifest = serde_json::json!({
            "format": 1,
            "id": "profile",
            "name": "Fixture",
            "runtime": "llama.cpp",
            "ggufRelativePath": relative_path,
            "bytes": bytes.len(),
            "sha256": hash,
            "parameterClass": "1.5B",
            "contextSize": 4096,
            "minimumRamBytes": 2147483648_u64,
            "recommendedRamBytes": 4294967296_u64,
            "taskSuitability": ["chat", "single-file-edit"],
            "releaseEligible": false
        });
        fs::write(
            root.path().join("model-packs/profile/manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        root
    }

    #[test]
    fn verifies_a_contained_model_pack() {
        let root = fixture("model-packs/profile/model.gguf", b"gguf", None);
        let verified = verify_selected(root.path(), "profile").unwrap();
        assert_eq!(verified.id, "profile");
        assert_eq!(verified.context_size, 4096);
        assert_eq!(inventory(root.path())[0].state, "verified");
    }

    #[test]
    fn default_profile_id_obeys_the_same_stable_id_contract_as_installed_packs() {
        assert!(validate_profile_id(DEFAULT_MODEL_PROFILE).is_ok());
    }

    #[test]
    fn rejects_hash_mismatch() {
        let root = fixture(
            "model-packs/profile/model.gguf",
            b"gguf",
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
        );
        let error = verify_selected(root.path(), "profile").unwrap_err();
        assert!(error.contains("SHA-256"));
        assert_eq!(inventory(root.path())[0].state, "invalid");
    }

    #[test]
    fn rejects_traversal_before_reading_a_file() {
        let root = fixture("../outside.gguf", b"gguf", None);
        let error = verify_selected(root.path(), "profile").unwrap_err();
        assert!(error.contains("普通相对路径"));
    }

    #[test]
    fn rejects_directory_id_mismatch() {
        let root = fixture("model-packs/profile/model.gguf", b"gguf", None);
        let path = root.path().join("model-packs/profile/manifest.json");
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["id"] = serde_json::json!("another-profile");
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(verify_selected(root.path(), "profile")
            .unwrap_err()
            .contains("目录名称"));
    }
}
