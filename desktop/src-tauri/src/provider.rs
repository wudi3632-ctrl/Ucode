use crate::legacy::{LegacyAgentClient, ProviderConfiguration};
use crate::storage::AppSettings;
use reqwest::Url;
use serde::Serialize;
use serde_json::Value;

const CREDENTIAL_SERVICE: &str = "ucode-portable";
const CREDENTIAL_USER: &str = "openai-compatible";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretStatus {
    pub saved: bool,
    pub storage: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub ok: bool,
    pub mode: String,
    pub model: String,
    pub base_url: Option<String>,
}

pub fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if !["bundled", "openai-compatible"].contains(&settings.model_provider.as_str()) {
        return Err("unsupported model provider".into());
    }
    if settings.model_provider == "bundled" {
        return Ok(());
    }
    if settings.provider_base_url.len() > 2_048 || settings.provider_model.len() > 200 {
        return Err("model provider configuration is too long".into());
    }
    if settings.provider_model.trim().is_empty() {
        return Err("provider model is required".into());
    }
    validate_endpoint(
        &settings.provider_base_url,
        settings.allow_network_providers,
    )?;
    Ok(())
}

pub fn validate_endpoint(value: &str, allow_network: bool) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|_| "provider URL is invalid".to_string())?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("provider URL must be a plain HTTP(S) endpoint".into());
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if !allow_network && !["127.0.0.1", "localhost", "::1", "[::1]"].contains(&host.as_str()) {
        return Err(
            "network providers are disabled; use localhost or enable network providers".into(),
        );
    }
    Ok(url)
}

pub fn configure(
    settings: &AppSettings,
    legacy: &LegacyAgentClient,
    transient_secret: Option<&str>,
) -> Result<ProviderTestResult, String> {
    validate_settings(settings)?;
    let api_key = if settings.model_provider == "openai-compatible" {
        match transient_secret.filter(|value| !value.is_empty()) {
            Some(value) => value.to_string(),
            None => load_secret()?.unwrap_or_default(),
        }
    } else {
        String::new()
    };
    let response = legacy
        .configure_provider(&ProviderConfiguration {
            mode: settings.model_provider.clone(),
            base_url: settings.provider_base_url.trim().to_string(),
            model: settings.provider_model.trim().to_string(),
            api_key,
            allow_network: settings.allow_network_providers,
        })
        .map_err(|error| error.to_string())?;
    Ok(test_result(&response, settings))
}

fn test_result(response: &Value, settings: &AppSettings) -> ProviderTestResult {
    ProviderTestResult {
        ok: true,
        mode: response
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or(&settings.model_provider)
            .to_string(),
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(if settings.model_provider == "bundled" {
                "bundled-coder"
            } else {
                settings.provider_model.as_str()
            })
            .to_string(),
        base_url: response
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

#[cfg(target_os = "windows")]
fn credential_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(|error| format!("credential store unavailable: {error}"))
}

#[cfg(target_os = "windows")]
pub fn secret_status() -> Result<ProviderSecretStatus, String> {
    let saved = match credential_entry()?.get_password() {
        Ok(_) => true,
        Err(keyring::Error::NoEntry) => false,
        Err(error) => return Err(format!("credential store unavailable: {error}")),
    };
    Ok(ProviderSecretStatus {
        saved,
        storage: "windows-credential-manager",
    })
}

#[cfg(target_os = "windows")]
pub fn save_secret(secret: &str) -> Result<ProviderSecretStatus, String> {
    if secret.is_empty() {
        return Err("API key cannot be empty".into());
    }
    if secret.len() > 8_192 {
        return Err("API key is too long".into());
    }
    credential_entry()?
        .set_password(secret)
        .map_err(|error| format!("could not save credential: {error}"))?;
    secret_status()
}

#[cfg(target_os = "windows")]
pub fn clear_secret() -> Result<ProviderSecretStatus, String> {
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => secret_status(),
        Err(error) => Err(format!("could not clear credential: {error}")),
    }
}

#[cfg(target_os = "windows")]
fn load_secret() -> Result<Option<String>, String> {
    match credential_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("credential store unavailable: {error}")),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn secret_status() -> Result<ProviderSecretStatus, String> {
    Ok(ProviderSecretStatus {
        saved: false,
        storage: "unavailable",
    })
}
#[cfg(not(target_os = "windows"))]
pub fn save_secret(_secret: &str) -> Result<ProviderSecretStatus, String> {
    Err("secure credential storage is unavailable on this platform".into())
}
#[cfg(not(target_os = "windows"))]
pub fn clear_secret() -> Result<ProviderSecretStatus, String> {
    secret_status()
}
#[cfg(not(target_os = "windows"))]
fn load_secret() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_policy_defaults_to_loopback() {
        assert!(validate_endpoint("http://127.0.0.1:1234/v1", false).is_ok());
        assert!(validate_endpoint("http://localhost:11434/v1", false).is_ok());
        assert!(validate_endpoint("http://[::1]:1234/v1", false).is_ok());
        assert!(validate_endpoint("https://example.com/v1", false).is_err());
        assert!(validate_endpoint("https://example.com/v1", true).is_ok());
        assert!(validate_endpoint("http://user:pass@localhost/v1", false).is_err());
        assert!(validate_endpoint("http://localhost/v1?key=x", false).is_err());
    }

    #[test]
    fn provider_settings_never_contain_a_secret_field() {
        let encoded = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(!encoded.to_ascii_lowercase().contains("apikey"));
        assert!(!encoded.to_ascii_lowercase().contains("secret"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_credential_manager_round_trip_uses_a_disposable_entry() {
        let user = format!("provider-test-{}", uuid::Uuid::new_v4());
        let entry = keyring::Entry::new("ucode-portable-test", &user).unwrap();
        entry.set_password("disposable-secret").unwrap();
        assert_eq!(entry.get_password().unwrap(), "disposable-secret");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
