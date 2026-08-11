use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SLACK_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_at_ms: u64,
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("oauth.json")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn parse_store(text: &str) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(Tokens {
        access: v["access_token"].as_str()?.to_string(),
        refresh: v["refresh_token"].as_str().unwrap_or("").to_string(),
        expires_at_ms: v["expires_at_ms"].as_u64().unwrap_or(0),
    })
}

pub const TOKEN_VAR: &str = "PHOENIX_OAUTH_TOKENS";

pub fn load() -> Option<Tokens> {
    parse_store(&crate::secrets::sealed_token_get(TOKEN_VAR)?)
}

pub fn save(t: &Tokens) -> Result<(), String> {
    let body = json!({
        "access_token": t.access,
        "refresh_token": t.refresh,
        "expires_at_ms": t.expires_at_ms,
    })
    .to_string();
    crate::secrets::sealed_token_put(TOKEN_VAR, &body)
}

pub fn parse_refresh_response(text: &str, old_refresh: &str, now_ms: u64) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    let access = v["access_token"].as_str()?.to_string();
    let refresh = v["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(old_refresh)
        .to_string();
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    Some(Tokens {
        access,
        refresh,
        expires_at_ms: now_ms + expires_in * 1000,
    })
}

enum RefreshFail {
    Retry(String),
    Fatal(String),
}

fn classify(e: ureq::Error) -> RefreshFail {
    match e {
        ureq::Error::Status(code, r) => {
            let detail: String = r
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            let msg = format!("HTTP {code}: {detail}");
            if crate::providers::RETRY_CODES.contains(&code) {
                RefreshFail::Retry(msg)
            } else {
                RefreshFail::Fatal(msg)
            }
        }
        other => RefreshFail::Retry(other.to_string()),
    }
}

fn refresh_with<F>(
    post: F,
    refresh_token: &str,
    attempts: u32,
    wait_ms: u64,
) -> Result<Tokens, String>
where
    F: Fn(&str) -> Result<String, RefreshFail>,
{
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    })
    .to_string();
    let mut last = String::new();
    for attempt in 0..attempts {
        match post(&body) {
            Ok(text) => {
                return parse_refresh_response(&text, refresh_token, now_ms())
                    .ok_or_else(|| "token endpoint returned no access_token".to_string())
            }
            Err(RefreshFail::Fatal(e)) => return Err(e),
            Err(RefreshFail::Retry(e)) => {
                last = e;
                if attempt + 1 < attempts {
                    std::thread::sleep(Duration::from_millis(wait_ms << attempt));
                }
            }
        }
    }
    Err(last)
}

pub fn refresh(refresh_token: &str) -> Result<Tokens, String> {
    refresh_with(
        |body| {
            let resp = ureq::post(TOKEN_URL)
                .set("Content-Type", "application/json")
                .timeout(Duration::from_secs(30))
                .send_string(body)
                .map_err(classify)?;
            resp.into_string()
                .map_err(|e| RefreshFail::Retry(e.to_string()))
        },
        refresh_token,
        3,
        2_000,
    )
}

fn refresh_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

pub fn usable(tokens: &Tokens) -> bool {
    !tokens.access.is_empty() && tokens.expires_at_ms > now_ms()
}

pub fn refresh_access() -> Result<Option<String>, String> {
    let _guard = refresh_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let tokens = load().ok_or_else(|| "no stored Anthropic OAuth tokens".to_string())?;
    if tokens.refresh.is_empty() {
        return Err("stored Anthropic OAuth session has no refresh token".to_string());
    }
    let new = refresh(&tokens.refresh)?;
    if new.access.is_empty() {
        return Err("token endpoint returned an empty access token".to_string());
    }
    save(&new)?;
    Ok(Some(new.access))
}

pub fn fresh_access() -> Option<String> {
    let tokens = load()?;
    if tokens.expires_at_ms > now_ms() + SLACK_MS {
        return (!tokens.access.is_empty()).then_some(tokens.access);
    }
    if tokens.refresh.is_empty() {
        return usable(&tokens).then_some(tokens.access);
    }
    let _guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
    let current = load()?;
    if current.expires_at_ms > now_ms() + SLACK_MS && !current.access.is_empty() {
        return Some(current.access);
    }
    match refresh(&current.refresh) {
        Ok(new) => match save(&new) {
            Ok(()) => Some(new.access),
            Err(error) => {
                crate::log::error(
                    "oauth",
                    format!("refreshed token could not be saved: {error}"),
                );
                None
            }
        },
        Err(e) => {
            crate::log::warn("oauth", format!("token refresh failed: {e}"));
            usable(&current).then_some(current.access)
        }
    }
}

pub fn force_refresh() -> Option<String> {
    match refresh_access() {
        Ok(access) => access,
        Err(error) => {
            crate::log::warn("oauth", format!("forced token refresh failed: {error}"));
            None
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn b64url(bytes: &[u8]) -> String {
    crate::media::b64_encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

pub struct Login {
    pub url: String,
    pub verifier: String,
    pub state: String,
}

pub fn begin_login() -> Login {
    let verifier = b64url(&crate::ws::urandom(32));
    let state = b64url(&crate::ws::urandom(24));
    let challenge = b64url(&hex_bytes(&crate::security::sha256_hex(
        verifier.as_bytes(),
    )));
    let url = format!(
        "{AUTHORIZE_URL}?code=true&client_id={CLIENT_ID}&response_type=code\
&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        urlencode(REDIRECT_URI),
        urlencode(SCOPES),
    );
    Login {
        url,
        verifier,
        state,
    }
}

pub fn tokens_from_exchange(text: &str, now_ms: u64) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    let access = v["access_token"].as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    let refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    Some(Tokens {
        access,
        refresh,
        expires_at_ms: now_ms + expires_in * 1000,
    })
}

pub fn exchange(code: &str, state: &str, verifier: &str) -> Result<Tokens, String> {
    let body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    })
    .to_string();
    let text = match ureq::post(TOKEN_URL)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .send_string(&body)
    {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, r)) => {
            let detail: String = r
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            return Err(format!("HTTP {code}: {detail}"));
        }
        Err(other) => return Err(other.to_string()),
    };
    tokens_from_exchange(&text, now_ms())
        .ok_or_else(|| "token endpoint returned no access_token".to_string())
}

pub fn import_from_claude_cli() -> Option<Tokens> {
    let path = crate::config::home_dir()
        .join(".claude")
        .join(".credentials.json");
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let o = v.get("claudeAiOauth").unwrap_or(&v);
            if let Some(access) = o
                .get("accessToken")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                return Some(Tokens {
                    access: access.to_string(),
                    refresh: o
                        .get("refreshToken")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    expires_at_ms: o.get("expiresAt").and_then(Value::as_u64).unwrap_or(0),
                });
            }
        }
    }
    if let Ok(tok) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !tok.is_empty() {
            return Some(Tokens {
                access: tok,
                refresh: String::new(),
                expires_at_ms: 0,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_parsing() {
        let t = Tokens {
            access: "sk-ant-oat-abc".into(),
            refresh: "sk-ant-ort-def".into(),
            expires_at_ms: 1234,
        };
        let text = json!({
            "access_token": t.access,
            "refresh_token": t.refresh,
            "expires_at_ms": t.expires_at_ms,
        })
        .to_string();
        assert_eq!(parse_store(&text), Some(t));
        assert_eq!(parse_store("junk"), None);
    }

    #[test]
    fn an_expired_access_token_without_a_refresh_token_is_not_offered() {
        assert!(!usable(&Tokens {
            access: "sk-ant-oat-old".into(),
            refresh: String::new(),
            expires_at_ms: 1,
        }));
        assert!(usable(&Tokens {
            access: "sk-ant-oat-live".into(),
            refresh: String::new(),
            expires_at_ms: now_ms() + 600_000,
        }));
    }

    #[test]
    fn refresh_response_keeps_old_refresh_when_missing() {
        let text = r#"{"access_token":"sk-ant-oat-new","expires_in":600}"#;
        let t = parse_refresh_response(text, "sk-ant-ort-old", 1000).unwrap();
        assert_eq!(t.access, "sk-ant-oat-new");
        assert_eq!(t.refresh, "sk-ant-ort-old");
        assert_eq!(t.expires_at_ms, 1000 + 600_000);
    }

    #[test]
    fn refresh_response_takes_new_refresh() {
        let text = r#"{"access_token":"a","refresh_token":"sk-ant-ort-new","expires_in":60}"#;
        let t = parse_refresh_response(text, "old", 0).unwrap();
        assert_eq!(t.refresh, "sk-ant-ort-new");
        assert!(parse_refresh_response(r#"{"expires_in":60}"#, "x", 0).is_none());
    }

    #[test]
    fn refresh_retries_transient_failures_then_succeeds() {
        let calls = std::cell::Cell::new(0u32);
        let out = refresh_with(
            |_| {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    Err(RefreshFail::Retry("HTTP 503: busy".into()))
                } else {
                    Ok(r#"{"access_token":"sk-ant-oat-ok","expires_in":60}"#.into())
                }
            },
            "sk-ant-ort-r",
            3,
            0,
        );
        assert_eq!(calls.get(), 3);
        assert_eq!(out.unwrap().access, "sk-ant-oat-ok");
    }

    #[test]
    fn refresh_fails_fast_on_non_retryable_status() {
        let calls = std::cell::Cell::new(0u32);
        let out = refresh_with(
            |_| {
                calls.set(calls.get() + 1);
                Err(RefreshFail::Fatal("HTTP 401: bad grant".into()))
            },
            "r",
            3,
            0,
        );
        assert_eq!(calls.get(), 1, "auth failures must not burn retries");
        assert!(out.unwrap_err().contains("401"));
    }

    #[test]
    fn refresh_exhausts_retries_and_keeps_last_error() {
        let calls = std::cell::Cell::new(0u32);
        let out = refresh_with(
            |_| {
                calls.set(calls.get() + 1);
                Err(RefreshFail::Retry("HTTP 500: flap".into()))
            },
            "r",
            3,
            0,
        );
        assert_eq!(calls.get(), 3);
        assert!(out.unwrap_err().contains("500"));
    }

    #[test]
    fn b64url_is_url_safe_and_unpadded() {
        assert_eq!(b64url(&[0xff, 0xff]), "__8");
        let s = b64url(&[0xfb, 0xff, 0xbf]);
        assert!(!s.contains('+') && !s.contains('/') && !s.contains('='));
    }

    #[test]
    fn hex_bytes_roundtrips_a_known_digest() {
        let raw = hex_bytes(&crate::security::sha256_hex(b""));
        assert_eq!(&raw[..4], &[0xe3, 0xb0, 0xc4, 0x42]);
        assert_eq!(raw.len(), 32);
    }

    #[test]
    fn begin_login_builds_a_pkce_authorize_url() {
        let lg = begin_login();
        assert!(lg.url.starts_with(AUTHORIZE_URL));
        assert!(lg.url.contains("code_challenge_method=S256"));
        assert!(lg.url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(lg.url.contains("code_challenge="));
        assert!(lg.url.contains(&format!("state={}", lg.state)));
        assert!(!lg.verifier.is_empty() && lg.verifier != lg.state);
        assert!(lg
            .url
            .contains("redirect_uri=https%3A%2F%2Fconsole.anthropic.com"));
        assert!(!lg.url.contains("redirect_uri=https://"));
    }

    #[test]
    fn exchange_response_parses_access_and_refresh() {
        let text = r#"{"access_token":"sk-ant-oat01-x","refresh_token":"sk-ant-ort01-y","expires_in":3600}"#;
        let t = tokens_from_exchange(text, 1_000).unwrap();
        assert_eq!(t.access, "sk-ant-oat01-x");
        assert_eq!(t.refresh, "sk-ant-ort01-y");
        assert_eq!(t.expires_at_ms, 1_000 + 3_600_000);
        assert!(tokens_from_exchange(r#"{"refresh_token":"r"}"#, 0).is_none());
    }
}
