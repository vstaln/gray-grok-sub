//! Binary resolution + credential probing for grok (Grok Build CLI).

use serde_json::Value;
use std::process::{Command, Stdio};

pub const INSTALL_HINT: &str =
    "Grok CLI not found — install Grok Build CLI (https://x.ai/cli) and run `grok login`";
pub const LOGIN_HINT: &str = "Grok CLI not logged in — run `grok login`";
pub const AUTH_API_KEY: &str = "xai.api_key";
pub const AUTH_CACHED: &str = "cached_token";

const BINARY_ENV: &str = "GRAY_GROK_SUB_COMMAND";

pub fn resolve_command() -> Option<String> {
    if let Ok(explicit) = std::env::var(BINARY_ENV) {
        let explicit = explicit.trim().to_string();
        if !explicit.is_empty() {
            return Some(explicit);
        }
    }
    which_on_path("grok")
}

/// `grok models` text output (exit code is 0 in both login states).
pub fn models_text() -> Option<String> {
    let bin = resolve_command()?;
    let out = Command::new(&bin)
        .arg("models")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    if let Ok(err) = String::from_utf8(out.stderr) {
        text.push('\n');
        text.push_str(&err);
    }
    Some(text)
}

/// t3code's verdict: XAI_API_KEY → api_key; else `grok models` text probe.
pub fn auth_state() -> AuthState {
    if api_key().is_some() {
        return AuthState::ApiKey;
    }
    let Some(text) = models_text() else {
        return AuthState::Unknown;
    };
    let lower = text.to_lowercase();
    if lower.contains("you are logged in") || lower.contains("logged in") {
        AuthState::LoggedIn
    } else if lower.contains("not authenticated") || lower.contains("not logged in") {
        AuthState::LoggedOut
    } else {
        AuthState::Unknown
    }
}

pub enum AuthState {
    ApiKey,
    LoggedIn,
    LoggedOut,
    Unknown,
}

/// The ACP authenticate methodId this environment resolves to.
pub fn acp_auth_method() -> &'static str {
    if api_key().is_some() {
        AUTH_API_KEY
    } else {
        AUTH_CACHED
    }
}

pub fn api_key() -> Option<String> {
    if let Ok(k) = std::env::var("XAI_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    read_api_key_cache()
}

fn cache_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::Path::new(&h).join(".grok-sub.json"))
}

pub fn write_api_key_cache(key: &str) -> std::io::Result<()> {
    let Some(path) = cache_path() else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(true).write(true).mode(0o600);
        let mut f = opts.open(path)?;
        use std::io::Write;
        f.write_all(format!("{}\n", serde_json::json!({"api_key": key})).as_bytes())
    }
    #[cfg(not(unix))]
    std::fs::write(path, format!("{}\n", serde_json::json!({"api_key": key})))
}

pub fn read_api_key_cache() -> Option<String> {
    let path = cache_path()?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let k = v.get("api_key").and_then(Value::as_str)?.trim().to_string();
    if k.is_empty() {
        None
    } else {
        Some(k)
    }
}

/// Spawn argv: `grok [--permission-mode X] agent stdio` per t3code.
pub fn acp_argv() -> Vec<String> {
    let mode = std::env::var("GRAY_GROK_SUB_PERMISSION").unwrap_or_default();
    let mut args: Vec<String> = Vec::new();
    match mode.trim() {
        "approval-required" => args.extend(["--permission-mode", "default"].map(str::to_string)),
        "auto-accept-edits" => args.extend(["--permission-mode", "acceptEdits"].map(str::to_string)),
        "auto" => args.extend(["--permission-mode", "auto"].map(str::to_string)),
        "full-access" => {
            return ["agent", "--always-approve", "stdio"].map(str::to_string).to_vec()
        }
        _ => {}
    }
    args.extend(["agent", "stdio"].map(str::to_string));
    args
}

/// Env injected into the ACP child.
pub fn acp_env() -> Vec<(String, String)> {
    let mut env = vec![("GROK_OAUTH2_REFERRER".into(), "gray".into())];
    // A cached key (not env-set) must reach the child explicitly.
    if std::env::var("XAI_API_KEY").is_err() {
        if let Some(key) = read_api_key_cache() {
            env.push(("XAI_API_KEY".into(), key));
        }
    }
    env
}

fn which_on_path(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let candidate = format!("{dir}/{name}");
        if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(m) = std::fs::metadata(&candidate) {
                    if m.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            return Some(candidate);
        }
    }
    None
}
