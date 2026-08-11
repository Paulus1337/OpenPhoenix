use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const AUTH_BASE: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CALLBACK_URL: &str = "https://auth.openai.com/deviceauth/callback";
pub const BACKEND_URL: &str = "https://chatgpt.com/backend-api/codex";
const SLACK_MS: u64 = 120_000;
const DEVICE_TIMEOUT_MS: u64 = 15 * 60_000;
const DEVICE_MIN_INTERVAL_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub account_id: String,
    pub expires_at_ms: u64,
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("codex.json")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn headers() -> Vec<(&'static str, String)> {
    vec![
        ("originator", "phoenix".to_string()),
        ("User-Agent", format!("phoenix/{}", crate::VERSION)),
    ]
}

pub fn decode_jwt_account_id(access: &str) -> Option<String> {
    let payload = decode_jwt_payload(access)?;
    payload
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn decode_jwt_payload(jwt: &str) -> Option<Value> {
    let mid = jwt.split('.').nth(1)?;
    let bytes = crate::media::b64_decode(mid).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn jwt_expiry_ms(access: &str) -> Option<u64> {
    let exp = decode_jwt_payload(access)?.get("exp")?.as_u64()?;
    Some(exp.saturating_mul(1000))
}

pub fn parse_store(text: &str) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(Tokens {
        access: v["access_token"].as_str()?.to_string(),
        refresh: v["refresh_token"].as_str().unwrap_or("").to_string(),
        account_id: v["account_id"].as_str().unwrap_or("").to_string(),
        expires_at_ms: v["expires_at_ms"].as_u64().unwrap_or(0),
    })
}

pub const TOKEN_VAR: &str = "PHOENIX_CODEX_TOKENS";

pub fn load() -> Option<Tokens> {
    parse_store(&crate::secrets::sealed_token_get(TOKEN_VAR)?)
}

pub fn save(t: &Tokens) -> Result<(), String> {
    let body = json!({
        "access_token": t.access,
        "refresh_token": t.refresh,
        "account_id": t.account_id,
        "expires_at_ms": t.expires_at_ms,
    })
    .to_string();
    crate::secrets::sealed_token_put(TOKEN_VAR, &body)
}

pub fn tokens_from_exchange(text: &str, now_ms: u64) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    let access = v["access_token"].as_str()?.to_string();
    let refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
    let expires_at_ms = v["expires_in"]
        .as_u64()
        .map(|s| now_ms + s * 1000)
        .or_else(|| jwt_expiry_ms(&access))
        .unwrap_or(now_ms);
    let account_id = decode_jwt_account_id(&access).unwrap_or_default();
    Some(Tokens {
        access,
        refresh,
        account_id,
        expires_at_ms,
    })
}

pub fn parse_refresh_response(text: &str, old: &Tokens, now_ms: u64) -> Option<Tokens> {
    let v: Value = serde_json::from_str(text).ok()?;
    let access = v["access_token"].as_str()?.to_string();
    let refresh = v["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&old.refresh)
        .to_string();
    let expires_at_ms = v["expires_in"]
        .as_u64()
        .map(|s| now_ms + s * 1000)
        .or_else(|| jwt_expiry_ms(&access))
        .unwrap_or(now_ms);
    let account_id = decode_jwt_account_id(&access)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| old.account_id.clone());
    Some(Tokens {
        access,
        refresh,
        account_id,
        expires_at_ms,
    })
}

fn post_form(url: &str, body: &str) -> Result<String, String> {
    let mut req = ureq::post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .timeout(Duration::from_secs(30));
    for (k, v) in headers() {
        req = req.set(k, &v);
    }
    match req.send_string(body) {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let detail: String = r
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            Err(format!("HTTP {code}: {detail}"))
        }
        Err(other) => Err(other.to_string()),
    }
}

fn post_json(url: &str, body: &Value) -> Result<(u16, String), String> {
    let mut req = ureq::post(url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(30));
    for (k, v) in headers() {
        req = req.set(k, &v);
    }
    match req.send_string(&body.to_string()) {
        Ok(r) => Ok((r.status(), r.into_string().map_err(|e| e.to_string())?)),
        Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
        Err(other) => Err(other.to_string()),
    }
}

pub fn refresh(old: &Tokens) -> Result<Tokens, String> {
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={CLIENT_ID}",
        urlencode(&old.refresh)
    );
    let text = post_form(&format!("{AUTH_BASE}/oauth/token"), &body)?;
    parse_refresh_response(&text, old, now_ms())
        .ok_or_else(|| "codex token refresh returned no access_token".to_string())
}

fn refresh_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

pub fn usable(tokens: &Tokens) -> bool {
    !tokens.access.is_empty() && tokens.expires_at_ms > now_ms()
}

pub fn fresh_auth() -> Option<Tokens> {
    let tokens = load()?;
    if tokens.expires_at_ms > now_ms() + SLACK_MS {
        return (!tokens.access.is_empty()).then_some(tokens);
    }
    if tokens.refresh.is_empty() {
        return usable(&tokens).then_some(tokens);
    }
    let _guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
    let current = load()?;
    if current.expires_at_ms > now_ms() + SLACK_MS && !current.access.is_empty() {
        return Some(current);
    }
    match refresh(&current) {
        Ok(new) => {
            let _ = save(&new);
            Some(new)
        }
        Err(e) => {
            crate::log::warn("codex", format!("token refresh failed: {e}"));
            usable(&current).then_some(current)
        }
    }
}

pub fn fresh_access() -> Option<String> {
    fresh_auth().map(|t| t.access).filter(|s| !s.is_empty())
}

pub fn force_refresh() -> Option<Tokens> {
    let _guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
    let t = load()?;
    if t.refresh.is_empty() {
        return None;
    }
    match refresh(&t) {
        Ok(new) => {
            let _ = save(&new);
            Some(new)
        }
        Err(e) => {
            crate::log::warn("codex", format!("forced token refresh failed: {e}"));
            None
        }
    }
}

pub fn account_id() -> Option<String> {
    fresh_auth().map(|t| t.account_id).filter(|s| !s.is_empty())
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

pub struct DeviceCode {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval_ms: u64,
}

pub fn request_device_code() -> Result<DeviceCode, String> {
    let (status, text) = post_json(
        &format!("{AUTH_BASE}/api/accounts/deviceauth/usercode"),
        &json!({ "client_id": CLIENT_ID }),
    )?;
    if status != 200 {
        return Err(format!("device code request failed: HTTP {status}: {text}"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let device_auth_id = v["device_auth_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("device code response missing device_auth_id")?
        .to_string();
    let user_code = v["user_code"]
        .as_str()
        .or_else(|| v["usercode"].as_str())
        .filter(|s| !s.is_empty())
        .ok_or("device code response missing user_code")?
        .to_string();
    let interval_ms = v["interval"]
        .as_u64()
        .map(|s| s * 1000)
        .unwrap_or(5000)
        .max(DEVICE_MIN_INTERVAL_MS);
    Ok(DeviceCode {
        device_auth_id,
        user_code,
        verification_url: format!("{AUTH_BASE}/codex/device"),
        interval_ms,
    })
}

fn poll_once(dc: &DeviceCode) -> Result<Option<(String, String)>, String> {
    let (status, text) = post_json(
        &format!("{AUTH_BASE}/api/accounts/deviceauth/token"),
        &json!({ "device_auth_id": dc.device_auth_id, "user_code": dc.user_code }),
    )?;
    if status == 200 {
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let code = v["authorization_code"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("device authorization response missing authorization_code")?
            .to_string();
        let verifier = v["code_verifier"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("device authorization response missing code_verifier")?
            .to_string();
        return Ok(Some((code, verifier)));
    }
    if status == 403 || status == 404 {
        return Ok(None);
    }
    Err(format!(
        "device authorization failed: HTTP {status}: {text}"
    ))
}

pub fn exchange(code: &str, verifier: &str) -> Result<Tokens, String> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={CLIENT_ID}&code_verifier={}",
        urlencode(code),
        urlencode(CALLBACK_URL),
        urlencode(verifier)
    );
    let text = post_form(&format!("{AUTH_BASE}/oauth/token"), &body)?;
    tokens_from_exchange(&text, now_ms())
        .ok_or_else(|| "codex token exchange returned no access_token".to_string())
}

pub fn login(mut on_code: impl FnMut(&DeviceCode)) -> Result<Tokens, String> {
    let dc = request_device_code()?;
    on_code(&dc);
    let deadline = now_ms() + DEVICE_TIMEOUT_MS;
    loop {
        if now_ms() >= deadline {
            return Err("codex device authorization timed out after 15 minutes".to_string());
        }
        match poll_once(&dc)? {
            Some((code, verifier)) => {
                let tokens = exchange(&code, &verifier)?;
                save(&tokens)?;
                return Ok(tokens);
            }
            None => std::thread::sleep(Duration::from_millis(dc.interval_ms)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt_with(payload: &Value) -> String {
        let head = crate::media::b64_encode(b"{\"alg\":\"none\"}");
        let body = crate::media::b64_encode(payload.to_string().as_bytes());
        format!("{head}.{body}.sig")
    }

    #[test]
    fn account_id_is_read_from_the_codex_jwt_claim() {
        let jwt = jwt_with(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-123" },
            "exp": 4102444800u64
        }));
        assert_eq!(decode_jwt_account_id(&jwt).as_deref(), Some("acct-123"));
        assert_eq!(decode_jwt_account_id("not-a-jwt"), None);
        let no_claim = jwt_with(&json!({ "sub": "x" }));
        assert_eq!(decode_jwt_account_id(&no_claim), None);
    }

    #[test]
    fn exchange_reads_tokens_and_account_and_expiry() {
        let jwt = jwt_with(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-9" }
        }));
        let text = json!({
            "access_token": jwt,
            "refresh_token": "rt-1",
            "expires_in": 3600u64
        })
        .to_string();
        let t = tokens_from_exchange(&text, 1_000).unwrap();
        assert_eq!(t.refresh, "rt-1");
        assert_eq!(t.account_id, "acct-9");
        assert_eq!(t.expires_at_ms, 1_000 + 3_600_000);
    }

    #[test]
    fn exchange_falls_back_to_jwt_exp_when_expires_in_absent() {
        let jwt = jwt_with(&json!({ "exp": 2_000u64 }));
        let text = json!({ "access_token": jwt, "refresh_token": "r" }).to_string();
        let t = tokens_from_exchange(&text, 0).unwrap();
        assert_eq!(t.expires_at_ms, 2_000_000);
    }

    #[test]
    fn refresh_keeps_old_refresh_and_account_when_omitted() {
        let old = Tokens {
            access: "old".into(),
            refresh: "rt-old".into(),
            account_id: "acct-keep".into(),
            expires_at_ms: 0,
        };
        let text = json!({ "access_token": "new", "expires_in": 60u64 }).to_string();
        let t = parse_refresh_response(&text, &old, 1_000).unwrap();
        assert_eq!(t.access, "new");
        assert_eq!(t.refresh, "rt-old");
        assert_eq!(t.account_id, "acct-keep");
        assert_eq!(t.expires_at_ms, 1_000 + 60_000);
    }

    #[test]
    fn store_roundtrips() {
        let t = Tokens {
            access: "a".into(),
            refresh: "r".into(),
            account_id: "acct".into(),
            expires_at_ms: 42,
        };
        let text = json!({
            "access_token": t.access,
            "refresh_token": t.refresh,
            "account_id": t.account_id,
            "expires_at_ms": t.expires_at_ms,
        })
        .to_string();
        assert_eq!(parse_store(&text), Some(t));
        assert_eq!(parse_store("junk"), None);
    }

    #[test]
    fn urlencode_escapes_reserved_characters() {
        assert_eq!(urlencode("a b/c+d"), "a%20b%2Fc%2Bd");
        assert_eq!(urlencode("safe-_.~AZ09"), "safe-_.~AZ09");
    }
}
