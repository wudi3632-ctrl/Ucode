use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LegacyError {
    #[error("legacy agent URL must bind to loopback")]
    NonLoopback,
    #[error("legacy agent request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("legacy agent returned an invalid response: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyEffect {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub before: Option<String>,
    pub content: String,
    #[serde(default = "default_risk")]
    pub risk: String,
    #[serde(default)]
    pub before_digest: Option<String>,
    #[serde(default)]
    pub after_digest: Option<String>,
    #[serde(default)]
    pub changes: Vec<LegacyEffectChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyEffectChange {
    pub path: String,
    pub operation: String,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub before_digest: Option<String>,
    #[serde(default)]
    pub after_digest: Option<String>,
}

fn default_risk() -> String {
    "write".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyPlan {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub effect: Option<LegacyEffect>,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub evidence: Vec<Value>,
    #[serde(default)]
    pub recovery: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyAttachment {
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfiguration {
    pub mode: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub allow_network: bool,
}

#[derive(Debug, Deserialize)]
struct LegacyTicket {
    id: String,
}

#[derive(Clone)]
pub struct LegacyAgentClient {
    base_url: String,
    client: Client,
}

impl LegacyAgentClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, LegacyError> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if !(base_url.starts_with("http://127.0.0.1:") || base_url.starts_with("http://localhost:"))
        {
            return Err(LegacyError::NonLoopback);
        }
        Ok(Self {
            base_url,
            client: Client::builder()
                .timeout(Duration::from_secs(210))
                .build()?,
        })
    }

    pub fn health(&self) -> bool {
        self.client
            .get(format!("{}/ready", self.base_url))
            .timeout(Duration::from_secs(2))
            .send()
            .and_then(|response| response.error_for_status())
            .is_ok()
    }

    pub fn select_project(&self, path: &str) -> Result<(), LegacyError> {
        self.post("/api/projects/select", json!({ "path": path }))?;
        Ok(())
    }

    pub fn configure_provider(
        &self,
        configuration: &ProviderConfiguration,
    ) -> Result<Value, LegacyError> {
        let value = serde_json::to_value(configuration)
            .map_err(|error| LegacyError::InvalidResponse(error.to_string()))?;
        self.post("/api/provider/configure", value)
    }

    pub fn create_ticket(&self, request: &str) -> Result<String, LegacyError> {
        let value = self.post("/api/tickets", json!({ "request": request }))?;
        serde_json::from_value::<LegacyTicket>(value)
            .map(|ticket| ticket.id)
            .map_err(|error| LegacyError::InvalidResponse(error.to_string()))
    }

    pub fn plan(&self, ticket_id: &str) -> Result<LegacyPlan, LegacyError> {
        let value = self.post(&format!("/api/tickets/{ticket_id}/plan"), json!({}))?;
        serde_json::from_value(value)
            .map_err(|error| LegacyError::InvalidResponse(error.to_string()))
    }

    pub fn plan_stream<F>(
        &self,
        ticket_id: &str,
        mut on_event: F,
    ) -> Result<LegacyPlan, LegacyError>
    where
        F: FnMut(&Value),
    {
        let mut request_id: Option<String> = None;
        let mut last_sequence = 0_u64;
        let mut reconnects = 0;
        loop {
            let response = self
                .client
                .post(format!("{}/v2/tickets/{ticket_id}/stream", self.base_url))
                .json(
                    &json!({ "requestId": request_id.as_deref(), "afterSequence": last_sequence }),
                )
                .send()
                .and_then(reqwest::blocking::Response::error_for_status);
            let response = match response {
                Ok(response) => {
                    if reconnects > 0 {
                        let status =
                            json!({ "type": "reconnected", "requestId": request_id.as_deref() });
                        on_event(&status);
                    }
                    response
                }
                Err(_error) if request_id.is_some() && reconnects < 1 => {
                    reconnects += 1;
                    let status =
                        json!({ "type": "reconnecting", "requestId": request_id.as_deref() });
                    on_event(&status);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let reader = BufReader::new(response);
            let mut disconnected = false;
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => {
                        disconnected = true;
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                let event: Value = serde_json::from_str(&line)
                    .map_err(|error| LegacyError::InvalidResponse(error.to_string()))?;
                if request_id.is_none() {
                    request_id = event
                        .get("requestId")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                let sequence = event
                    .get("sequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(last_sequence + 1);
                if sequence <= last_sequence {
                    continue;
                }
                last_sequence = sequence;
                on_event(&event);
                match event.get("type").and_then(Value::as_str) {
                    Some("complete") => {
                        let result = event.get("result").cloned().ok_or_else(|| {
                            LegacyError::InvalidResponse("complete event has no result".into())
                        })?;
                        return serde_json::from_value(result)
                            .map_err(|error| LegacyError::InvalidResponse(error.to_string()));
                    }
                    Some("cancelled") => {
                        return Ok(LegacyPlan {
                            kind: "cancelled".into(),
                            content: String::new(),
                            effect: None,
                            route: None,
                            evidence: Vec::new(),
                            recovery: None,
                        })
                    }
                    Some("error") => {
                        return Err(LegacyError::InvalidResponse(
                            event
                                .get("error")
                                .and_then(|value| value.get("message"))
                                .and_then(Value::as_str)
                                .unwrap_or("model stream failed")
                                .to_string(),
                        ))
                    }
                    _ => {}
                }
            }
            if request_id.is_some() && reconnects < 1 {
                reconnects += 1;
                let status = json!({ "type": "reconnecting", "requestId": request_id.as_deref() });
                on_event(&status);
                continue;
            }
            let detail = if disconnected {
                "stream disconnected after reconnect"
            } else {
                "stream ended without a terminal event"
            };
            return Err(LegacyError::InvalidResponse(detail.into()));
        }
    }

    pub fn continue_ticket(&self, ticket_id: &str, content: &str) -> Result<(), LegacyError> {
        self.post(
            &format!("/api/tickets/{ticket_id}/messages"),
            json!({ "content": content }),
        )?;
        Ok(())
    }

    pub fn hydrate_ticket(
        &self,
        ticket_id: &str,
        messages: &[LegacyMessage],
    ) -> Result<(), LegacyError> {
        if messages.is_empty() {
            return Ok(());
        }
        self.post(
            &format!("/api/tickets/{ticket_id}/context"),
            json!({ "messages": messages }),
        )?;
        Ok(())
    }

    pub fn set_skill_context(&self, ticket_id: &str, context: &str) -> Result<(), LegacyError> {
        self.post(
            &format!("/api/tickets/{ticket_id}/skills"),
            json!({ "context": context }),
        )?;
        Ok(())
    }

    pub fn set_attachment_context(
        &self,
        ticket_id: &str,
        attachments: &[LegacyAttachment],
    ) -> Result<(), LegacyError> {
        self.post(
            &format!("/api/tickets/{ticket_id}/attachments"),
            json!({ "attachments": attachments }),
        )?;
        Ok(())
    }

    pub fn approve(
        &self,
        ticket_id: &str,
        effect_id: &str,
        approved: bool,
        selected_paths: Option<&[String]>,
    ) -> Result<Value, LegacyError> {
        self.post(
            &format!("/api/tickets/{ticket_id}/effects/{effect_id}/approval"),
            json!({ "approved": approved, "selectedPaths": selected_paths }),
        )
    }

    pub fn rollback(&self, ticket_id: &str, effect_id: &str) -> Result<Value, LegacyError> {
        self.post(
            &format!("/api/tickets/{ticket_id}/effects/{effect_id}/rollback"),
            json!({}),
        )
    }

    pub fn cancel(&self, ticket_id: &str) -> Result<(), LegacyError> {
        self.post(&format!("/v2/tickets/{ticket_id}/cancel"), json!({}))?;
        Ok(())
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, LegacyError> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .json(&body)
            .send()?;
        let status = response.status();
        let value: Value = response.json()?;
        if !status.is_success() {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Agent request failed");
            return Err(LegacyError::InvalidResponse(message.to_string()));
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_loopback_agent() {
        assert!(matches!(
            LegacyAgentClient::new("http://example.com:3082"),
            Err(LegacyError::NonLoopback)
        ));
    }

    #[test]
    fn parses_effect_response() {
        let plan: LegacyPlan = serde_json::from_value(json!({
            "type": "message",
            "content": "waiting",
            "effect": {
                "id": "effect-1", "path": "a.py", "before": "x", "content": "y",
                "changes": [
                    { "path": "a.py", "operation": "modify", "before": "x", "content": "y" },
                    { "path": "b.py", "operation": "create", "before": null, "content": "z" }
                ]
            }
        }))
        .unwrap();
        let effect = plan.effect.unwrap();
        assert_eq!(effect.path, "a.py");
        assert_eq!(effect.changes.len(), 2);
        assert_eq!(effect.changes[1].operation, "create");
    }

    #[test]
    fn preserves_structured_task_rescue_metadata() {
        let plan: LegacyPlan = serde_json::from_value(json!({
            "type": "needs_clarification",
            "content": "需要更多上下文",
            "route": "failed_test_repair",
            "recovery": {
                "reason": "candidate_no_change",
                "path": "src/cart.py",
                "attempts": 2,
                "actions": ["attach_context", "configure_model"]
            }
        }))
        .unwrap();
        let recovery = plan.recovery.unwrap();
        assert_eq!(recovery["reason"], "candidate_no_change");
        assert_eq!(recovery["path"], "src/cart.py");
        assert_eq!(recovery["attempts"], 2);
    }
}
