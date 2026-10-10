//! Model catalog: initialize._meta.modelState (no authenticate — it can
//! open a browser) → `grok models` text scrape → grok-build fallback.

use acp_bridge::acp::{AcpClient, SpawnSpec};
use serde_json::{json, Value};

use crate::driver::GrokDriver;
use acp_bridge::session::Driver;

/// modelState rows from a bare initialize (ModelInfo {modelId, name, _meta}).
fn from_initialize() -> Option<Vec<Value>> {
    let bin = crate::setup::resolve_command()?;
    let spec = SpawnSpec {
        command: bin,
        args: crate::setup::acp_argv(),
        env: crate::setup::acp_env(),
        cwd: std::env::temp_dir(),
    };
    let driver = GrokDriver;
    let mut client = AcpClient::spawn(&spec).ok()?;
    let init = client
        .initialize(driver.client_info(), driver.client_capabilities())
        .ok()?;
    let models = init
        .get("_meta")
        .and_then(|m| m.get("modelState"))
        .and_then(|s| s.get("availableModels"))
        .and_then(Value::as_array)?
        .clone();
    Some(models)
}

/// `grok models` bullet lines: `* slug (default)` / `- slug`.
fn from_cli() -> Vec<Value> {
    let text = crate::setup::models_text().unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let slug = line
            .strip_prefix("* ")
            .or_else(|| line.strip_prefix("- "))
            .unwrap_or("")
            .split(' ')
            .next()
            .unwrap_or("")
            .trim();
        if !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
            out.push(json!({"modelId": slug, "name": slug}));
        }
    }
    out
}

/// catalog rows: `{id, name}` (+ reasoning_efforts when advertised).
pub fn catalog() -> Vec<Value> {
    let raw = from_initialize().unwrap_or_else(from_cli);
    let mut rows: Vec<Value> = raw
        .iter()
        .filter_map(|m| {
            let id = m
                .get("modelId")
                .or_else(|| m.get("value"))
                .and_then(Value::as_str)?;
            let mut row = json!({
                "id": id,
                "name": m.get("name").and_then(Value::as_str).unwrap_or(id),
            });
            if let Some(efforts) = m
                .get("_meta")
                .and_then(|x| x.get("reasoningEfforts"))
                .and_then(Value::as_array)
            {
                let vals: Vec<Value> = efforts
                    .iter()
                    .filter_map(|e| {
                        e.get("value")
                            .or_else(|| e.get("id"))
                            .and_then(Value::as_str)
                    })
                    .map(|v| json!(v))
                    .collect();
                if !vals.is_empty() {
                    row["reasoning_efforts"] = json!(vals);
                }
            }
            Some(row)
        })
        .collect();
    if rows.is_empty() {
        rows.push(json!({"id": "grok-build", "name": "Grok Build"}));
    }
    rows
}
