//! grok-sub — Gray sidecar that relays chat turns to the Grok CLI's
//! `grok agent stdio` ACP subprocess (t3code GrokDriver parity, model role).

use acp_bridge::relay::{self, Intents, TurnFn};
use acp_bridge::session::{self, Pool};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use grok_sub::{driver::GrokDriver, models, usage, setup};
use grok_sub::setup::AuthState;

const PROVIDER_ID: &str = "grok";
const AUTH_CLI: &str = "grok-login";
const AUTH_KEY: &str = "xai-api-key";
const TOKEN_PREFIX: &str = "grok-sub-";

fn manifest() -> Value {
    json!({
        "name": "grok-sub",
        "display_name": "Grok",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.2",
        "capabilities": ["provider.credentials", "host.say"],
        "providers": [{
            "id": PROVIDER_ID,
            "name": "Grok CLI (xAI)",
            "auth_methods": [
                {"id": AUTH_CLI, "kind": "oauth",
                 "name": "Grok CLI login",
                 "operations": ["login", "refresh", "revoke", "models", "chat"]},
                {"id": AUTH_KEY, "kind": "api_key",
                 "name": "xAI API key (XAI_API_KEY)",
                 "operations": ["models", "chat", "refresh"]},
            ],
            "transport": {
                "kind": "openai-responses",
                "base_url": "https://127.0.0.1:1/",
                "authorization": {"kind": "bearer", "secret_name": "relay_token"},
            },
        }],
        "commands": ["/grok"],
    })
}

#[derive(Debug)]
enum RpcError {
    Protocol(String),
    Unavailable(String),
    Rpc { code: &'static str, message: String },
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::Unavailable(msg.into())
}

fn error_value(e: &RpcError) -> Value {
    match e {
        RpcError::Rpc { code, message } => json!({
            "code": code, "message": message, "retryable": false, "terminal": true,
        }),
        RpcError::Unavailable(message) => json!({
            "code": "unavailable", "message": message, "retryable": true, "terminal": false,
        }),
        RpcError::Protocol(message) => json!({
            "code": "protocol", "message": message, "retryable": false, "terminal": true,
        }),
    }
}

struct LoginOp {
    child: Option<Child>,
}
type Ops = Arc<Mutex<HashMap<String, LoginOp>>>;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn authed() -> bool {
    matches!(setup::auth_state(), AuthState::ApiKey | AuthState::LoggedIn)
}

fn auth_start(ops: &Ops, params: &Value) -> Result<Value, RpcError> {
    let method = params.get("auth_method").and_then(Value::as_str).unwrap_or_default();
    if method == AUTH_KEY {
        if setup::api_key().is_some() {
            return Ok(json!({
                "operation_id": "", "status": "completed",
                "verification_uri": "", "expires_at": now_secs() + 600,
                "retry_after_ms": 0,
            }));
        }
        return Err(unavailable("set XAI_API_KEY first"));
    }
    let bin = setup::resolve_command().ok_or_else(|| unavailable(setup::INSTALL_HINT))?;
    if authed() {
        return Ok(json!({
            "operation_id": "", "status": "completed",
            "verification_uri": "", "expires_at": now_secs() + 600,
            "retry_after_ms": 0,
        }));
    }
    let child = Command::new(&bin)
        .arg("login")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| unavailable(format!("grok login: {e}")))?;
    let op_id = uuid::Uuid::new_v4().to_string();
    ops.lock()
        .unwrap()
        .insert(op_id.clone(), LoginOp { child: Some(child) });
    Ok(json!({
        "operation_id": op_id,
        "status": "pending",
        "verification_uri": "",
        "expires_at": now_secs() + 900,
        "retry_after_ms": 2000,
    }))
}

fn auth_poll(ops: &Ops, params: &Value) -> Result<Value, RpcError> {
    let op_id = params.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    if op_id.is_empty() {
        return Ok(if authed() {
            json!({"state": "completed", "credential": login_material()})
        } else {
            json!({"state": "failed", "error": {
                "code": "unavailable", "message": setup::LOGIN_HINT,
                "retryable": false, "terminal": true,
            }})
        });
    }
    let child_done = {
        let mut g = ops.lock().unwrap();
        let Some(op) = g.get_mut(op_id) else {
            return Ok(json!({"state": "operation_lost"}));
        };
        match op.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(Some(status))) => Some(status.success()),
            Some(Err(_)) => Some(false),
            _ => None,
        }
    };
    if authed() {
        ops.lock().unwrap().remove(op_id);
        return Ok(json!({"state": "completed", "credential": login_material()}));
    }
    match child_done {
        Some(false) => {
            ops.lock().unwrap().remove(op_id);
            Ok(json!({"state": "failed", "error": {
                "code": "login_failed", "message": "grok login exited without a session",
                "retryable": false, "terminal": true,
            }}))
        }
        _ => Ok(json!({"state": "pending", "retry_after_ms": 3000})),
    }
}

fn auth_cancel(ops: &Ops, params: &Value) -> Result<Value, RpcError> {
    let op_id = params.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    if let Some(mut op) = ops.lock().unwrap().remove(op_id) {
        if let Some(mut c) = op.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    Ok(json!({}))
}

fn auth_refresh(params: &Value) -> Result<Value, RpcError> {
    let method = params.get("auth_method").and_then(Value::as_str).unwrap_or_default();
    if method == AUTH_KEY {
        let key = setup::api_key().ok_or_else(|| unavailable("XAI_API_KEY not set"))?;
        let _ = setup::write_api_key_cache(&key);
        return Ok(json!({"credential": {
            "secrets": {"api_key": key}, "metadata": {}, "expires_at": null,
        }}));
    }
    if authed() {
        return Ok(json!({"credential": login_material()}));
    }
    Err(RpcError::Rpc {
        code: "invalid_grant",
        message: setup::LOGIN_HINT.into(),
    })
}

fn auth_revoke(params: &Value) -> Result<Value, RpcError> {
    let method = params.get("auth_method").and_then(Value::as_str).unwrap_or_default();
    if method == AUTH_CLI {
        if let Some(bin) = setup::resolve_command() {
            let _ = Command::new(bin)
                .arg("logout")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    } else if let Some(cache) = setup::cache_path() {
        let _ = std::fs::remove_file(cache);
    }
    Ok(json!({}))
}

fn login_material() -> Value {
    json!({
        "secrets": {"grok_login": "1"},
        "metadata": {},
        "expires_at": null,
    })
}

fn gate_auth(method: &str) -> Result<(), RpcError> {
    if setup::resolve_command().is_none() {
        return Err(unavailable(setup::INSTALL_HINT));
    }
    match method {
        AUTH_KEY => {
            if setup::api_key().is_none() {
                return Err(unavailable("set XAI_API_KEY for this auth method"));
            }
        }
        _ => {
            if !authed() {
                return Err(unavailable(setup::LOGIN_HINT));
            }
        }
    }
    Ok(())
}

fn provider_models(params: &Value) -> Result<Value, RpcError> {
    let method = params.get("auth_method").and_then(Value::as_str).unwrap_or_default();
    gate_auth(method)?;
    if let Some(key) = params
        .get("credential")
        .and_then(|c| c.get("material"))
        .and_then(|m| m.get("secrets"))
        .and_then(|s| s.get("api_key"))
        .and_then(Value::as_str)
    {
        let _ = setup::write_api_key_cache(key);
    }
    Ok(json!({
        "provider": PROVIDER_ID,
        "models": models::catalog(),
        "default_model": "grok-build",
    }))
}

fn provider_chat(
    intents: &Intents,
    pool: &Arc<Pool>,
    params: &Value,
) -> Result<Value, RpcError> {
    let provider = params.get("provider").and_then(Value::as_str).unwrap_or_default();
    if provider != PROVIDER_ID {
        return Err(RpcError::Protocol("unknown provider".into()));
    }
    let method = params.get("auth_method").and_then(Value::as_str).unwrap_or_default();
    gate_auth(method)?;
    let model = params.get("model").and_then(Value::as_str).unwrap_or_default();
    let pool2 = pool.clone();
    let turn: TurnFn = Arc::new(move |raw: &[u8], cancel: &AtomicBool| {
        session::run_turn(&pool2, raw, cancel)
    });
    let (relay_url, relay_token) =
        relay::mint_turn(intents, model, turn, TOKEN_PREFIX).map_err(unavailable)?;
    Ok(json!({"relay_url": relay_url, "relay_token": relay_token}))
}

fn command_run(params: &Value) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    if name != "/grok" {
        return Err(RpcError::Protocol(format!("unknown command {name}")));
    }
    let argv: Vec<String> = params
        .get("argv")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    match argv.first().map(String::as_str).unwrap_or("") {
        "" => Ok(json!({})),
        "status" => {
            let text = match setup::auth_state() {
                AuthState::ApiKey => "xAI API key configured (XAI_API_KEY)".to_string(),
                AuthState::LoggedIn => "Grok CLI: logged in".to_string(),
                AuthState::LoggedOut => setup::LOGIN_HINT.to_string(),
                AuthState::Unknown => {
                    if setup::resolve_command().is_none() {
                        setup::INSTALL_HINT.to_string()
                    } else {
                        "Grok CLI present; login state unknown — run `grok models`".into()
                    }
                }
            };
            Ok(json!({"text": text}))
        }
        _ => Err(RpcError::Protocol("usage: /grok [status]".into())),
    }
}

fn dispatch(
    method: &str,
    params: &Value,
    ops: &Ops,
    intents: &Intents,
    pool: &Arc<Pool>,
) -> Result<Value, RpcError> {
    match method {
        "plugin/manifest" => Ok(manifest()),
        "provider/auth/start" => auth_start(ops, params),
        "provider/auth/poll" => auth_poll(ops, params),
        "provider/auth/cancel" => auth_cancel(ops, params),
        "provider/auth/refresh" => auth_refresh(params),
        "provider/auth/revoke" => auth_revoke(params),
        "provider/usage" => usage::handle(&params).map_err(unavailable),
        "provider/models" => provider_models(params),
        "provider/chat" => provider_chat(intents, pool, params),
        "command/run" => command_run(params),
        "plugin/tools" => Ok(json!({"tools": []})),
        "event/notify" | "plugin/shutdown" => Ok(json!({})),
        _ => Err(RpcError::Protocol(format!("unknown method {method}"))),
    }
}

fn write_response(out: &mut impl Write, id: Value, outcome: Result<Value, RpcError>) -> bool {
    let response = match outcome {
        Ok(result) => json!({"id": id, "result": result}),
        Err(e) => json!({"id": id, "error": error_value(&e)}),
    };
    writeln!(out, "{response}")
        .and_then(|()| out.flush())
        .is_ok()
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return;
    }
    let ops: Ops = Arc::new(Mutex::new(HashMap::new()));
    let intents: Intents = Default::default();
    let pool: Arc<Pool> = Pool::new(Arc::new(GrokDriver));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = dispatch(&method, &params, &ops, &intents, &pool);
        if !id.is_null() && !write_response(&mut out, id, result) {
            return;
        }
        if method == "plugin/shutdown" {
            break;
        }
    }
}
