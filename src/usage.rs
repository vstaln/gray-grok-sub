//! `provider/usage`: Grok/xAI subscription card.
//!
//! xAI exposes rate limits per response (headers), not a fetchable
//! subscription feed, and the `grok` CLI has no usage command. The card
//! reports the connection honestly with no fabricated windows.

use serde_json::{Value, json};

use crate::setup;

/// `provider/usage` entry: presence of a credential is the whole signal.
pub fn handle(_params: &Value) -> Result<Value, String> {
    if setup::api_key().is_none() && setup::resolve_command().is_none() {
        return Err("no Grok credential or CLI".to_string());
    }
    Ok(json!({
        "available": true,
        "title": "Grok",
        "windows": [],
        "note": "xAI publishes rate limits per-response only — no subscription quota feed",
    }))
}
