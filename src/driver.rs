//! grok ACP driver: xAI extension methods + spawn quirks (t3code parity).

use acp_bridge::acp::{AcpClient, SpawnSpec};
use acp_bridge::session::{Driver, PermissionDecision};
use serde_json::{json, Value};

pub struct GrokDriver;

const PROMPT_COMPLETE: &str = "_x.ai/session/prompt_complete";

impl Driver for GrokDriver {
    fn spawn_spec(&self, stage_dir: &std::path::Path) -> SpawnSpec {
        SpawnSpec {
            command: crate::setup::resolve_command().unwrap_or_else(|| "grok".into()),
            args: crate::setup::acp_argv(),
            env: crate::setup::acp_env(),
            cwd: stage_dir.to_path_buf(),
        }
    }

    fn client_info(&self) -> (&'static str, &'static str) {
        ("gray-grok-sub", env!("CARGO_PKG_VERSION"))
    }

    fn client_capabilities(&self) -> Value {
        json!({
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        })
    }

    /// xai.api_key when a key is configured; else the CLI's cached login.
    fn auth_method_id(&self) -> Option<&'static str> {
        Some(crate::setup::acp_auth_method())
    }

    /// `_meta` on every session/prompt so the prompt_complete notification
    /// can be correlated back to this request (t3code injects promptId +
    /// requestId for exactly this race).
    fn prompt_meta(&self, seq: u64) -> Value {
        let id = format!("gray-xai-prompt-{seq}");
        json!({"promptId": id, "requestId": id})
    }

    /// Grok's real session/prompt response can arrive missing or without a
    /// stopReason — the private notification wins the race.
    fn prompt_complete_methods(&self) -> &'static [&'static str] {
        &[PROMPT_COMPLETE]
    }

    fn ext_request(&self, method: &str, params: &Value) -> Option<Value> {
        match method {
            // No host question surface for a model provider: auto-answer
            // with the first option per question (keyed by id ?? text).
            "x.ai/ask_user_question" | "_x.ai/ask_user_question" => {
                let mut answers = serde_json::Map::new();
                if let Some(questions) = params.get("questions").and_then(Value::as_array) {
                    for q in questions {
                        let key = q
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| q.get("question").and_then(Value::as_str))
                            .unwrap_or_default()
                            .to_string();
                        let label = q
                            .get("options")
                            .and_then(Value::as_array)
                            .and_then(|o| o.first())
                            .and_then(|o| o.get("label").and_then(Value::as_str))
                            .unwrap_or("OK")
                            .to_string();
                        answers.insert(key, json!([label]));
                    }
                }
                Some(json!({"outcome": "accepted", "answers": answers}))
            }
            // t3code always abandons Grok's native plan gate: the plan was
            // captured client-side; the agent should stop and wait.
            "x.ai/exit_plan_mode" | "_x.ai/exit_plan_mode" => Some(json!({
                "outcome": "abandoned",
                "feedback": "The client captured your proposed plan. Stop here and wait for the user's feedback or implementation request in a later turn.",
            })),
            _ => None,
        }
    }

    /// Only the host's own tools (mcp__gray__*) may run; grok's native
    /// tools are denied — model role, not agent runtime.
    fn permission_option(&self, params: &Value) -> PermissionDecision {
        let title = params
            .get("toolCall")
            .and_then(|t| t.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let allow = title.contains("mcp__gray__")
            || title.contains("mcp_gray__")
            || title.contains("gray/")
            || title.starts_with("gray ");
        if !allow {
            return PermissionDecision::Deny;
        }
        let id = params
            .get("options")
            .and_then(Value::as_array)
            .and_then(|o| {
                o.iter()
                    .find(|opt| opt.get("kind").and_then(Value::as_str) == Some("allow_once"))
                    .or_else(|| {
                        o.iter().find(|opt| {
                            opt.get("kind").and_then(Value::as_str) == Some("allow_always")
                        })
                    })
            })
            .and_then(|opt| opt.get("optionId").and_then(Value::as_str))
            .unwrap_or("allow_once")
            .to_string();
        PermissionDecision::Allow(id)
    }

    fn apply_model(
        &self,
        client: &mut AcpClient,
        session_id: &str,
        _session_new_result: &Value,
        model: &str,
        effort: Option<&str>,
        overrides: &[(String, String)],
    ) -> Result<(), String> {
        // "grok-build" is a product placeholder meaning "keep current" —
        // never sent on the wire.
        let effort = effort
            .or_else(|| overrides.iter().find(|(k, _)| k == "effort").map(|(_, v)| v.as_str()));
        let meta = match effort {
            Some(e) if valid_effort_token(e) => json!({"reasoningEffort": e}),
            _ => Value::Null,
        };
        if model == "grok-build" || model.is_empty() {
            if meta.is_null() {
                return Ok(());
            }
            // Effort change alone still sends set_model on the current model.
            return client.set_model(session_id, "grok-build", meta).map_err(|e| {
                if e.contains("-32601") || e.contains("not found") {
                    format!("unknown grok model `grok-build`")
                } else {
                    format!("grok set_model: {e}")
                }
            });
        }
        client.set_model(session_id, model, meta).map_err(|e| {
            if e.contains("-32601") || e.contains("not found") || e.contains("Invalid") {
                format!("unknown grok model `{model}`")
            } else {
                format!("grok set_model: {e}")
            }
        })
    }
}

/// /^[a-z0-9][a-z0-9._-]{0,31}$/i — t3code drops invalid tokens silently.
fn valid_effort_token(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'_' || *c == b'-')
}
