use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
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

pub fn load() -> Option<Tokens> {
    parse_store(&fs::read_to_string(store_path()).ok()?)
}

pub fn save(t: &Tokens) -> Result<(), String> {
    let dir = store_path();
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = json!({
        "access_token": t.access,
        "refresh_token": t.refresh,
        "expires_at_ms": t.expires_at_ms,
    })
    .to_string();
    fs::write(&dir, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o600));
    }
    Ok(())
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

pub fn fresh_access() -> Option<String> {
    let t = load()?;
    if t.expires_at_ms > now_ms() + SLACK_MS || t.refresh.is_empty() {
        return (!t.access.is_empty()).then_some(t.access);
    }
    match refresh(&t.refresh) {
        Ok(new) => {
            let _ = save(&new);
            Some(new.access)
        }
        Err(e) => {
            eprintln!("oauth refresh failed: {e}");
            (!t.access.is_empty()).then_some(t.access)
        }
    }
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
}
