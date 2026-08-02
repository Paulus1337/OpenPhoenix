use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const MAX_PENDING: usize = 64;
pub const CODE_LEN: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u64,
    pub channel: String,
    pub sender: String,
    pub display: String,
    pub code: String,
    pub created_ms: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("pairing.json")
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn code_from_bytes(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut out = String::with_capacity(CODE_LEN);
    for i in 0..CODE_LEN {
        let b = bytes.get(i).copied().unwrap_or(0) as usize;
        let c = ALPHABET.get(b % ALPHABET.len()).copied().unwrap_or(b'2');
        out.push(c as char);
    }
    out
}

pub fn new_code() -> String {
    code_from_bytes(&crate::ws::urandom(CODE_LEN))
}

pub fn load(path: &Path) -> Vec<Request> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    v.get("pending")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let channel = r.get("channel").and_then(Value::as_str)?;
                    let sender = r.get("sender").and_then(Value::as_str)?;
                    if channel.trim().is_empty() || sender.trim().is_empty() {
                        return None;
                    }
                    Some(Request {
                        id: r.get("id").and_then(Value::as_u64).unwrap_or(0),
                        channel: channel.to_string(),
                        sender: sender.to_string(),
                        display: r
                            .get("display")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        code: r
                            .get("code")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        created_ms: r.get("created_ms").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(path: &Path, pending: &[Request]) -> Result<(), String> {
    let arr: Vec<Value> = pending
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "channel": r.channel,
                "sender": r.sender,
                "display": r.display,
                "code": r.code,
                "created_ms": r.created_ms,
            })
        })
        .collect();
    let body = serde_json::to_string_pretty(&json!({"v": 1, "pending": arr}))
        .map_err(|e| e.to_string())?;
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn offer(channel: &str, sender: &str, display: &str) -> Option<String> {
    let (req, fresh) = request_at(&store_path(), channel, sender, display).ok()?;
    if !fresh {
        return None;
    }
    Some(format!(
        "You are not on my allowlist, so I cannot act on that. Pairing code {} is waiting for the operator to approve with `phoenix pairing approve {}`.",
        req.code, req.code
    ))
}

pub fn request_at(
    path: &Path,
    channel: &str,
    sender: &str,
    display: &str,
) -> Result<(Request, bool), String> {
    let channel = channel.trim();
    let sender = sender.trim();
    if channel.is_empty() || sender.is_empty() {
        return Err("pairing needs a channel and a sender".into());
    }
    let _g = guard();
    let mut pending = load(path);
    if let Some(existing) = pending
        .iter()
        .find(|r| r.channel == channel && r.sender.eq_ignore_ascii_case(sender))
    {
        return Ok((existing.clone(), false));
    }
    if pending.len() >= MAX_PENDING {
        return Err(format!(
            "pairing queue is full ({MAX_PENDING} waiting); approve or deny some first"
        ));
    }
    let id = pending.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    let req = Request {
        id,
        channel: channel.to_string(),
        sender: sender.to_string(),
        display: display.trim().to_string(),
        code: new_code(),
        created_ms: now_ms(),
    };
    pending.push(req.clone());
    save(path, &pending)?;
    Ok((req, true))
}

pub fn find<'a>(pending: &'a [Request], key: &str) -> Option<&'a Request> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let by_id = key
        .strip_prefix('#')
        .unwrap_or(key)
        .parse::<u64>()
        .ok()
        .and_then(|id| pending.iter().find(|r| r.id == id));
    if by_id.is_some() {
        return by_id;
    }
    pending
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(key) || r.sender.eq_ignore_ascii_case(key))
}

pub fn take(path: &Path, key: &str) -> Result<Request, String> {
    let _g = guard();
    let mut pending = load(path);
    let Some(hit) = find(&pending, key).cloned() else {
        return Err(format!("no pairing request matches '{key}'"));
    };
    pending.retain(|r| r.id != hit.id);
    save(path, &pending)?;
    Ok(hit)
}

pub fn config_key(channel: &str) -> Option<(&'static str, &'static str)> {
    match channel {
        "telegram" => Some(("telegram", "allowed_chat_ids")),
        "whatsapp" => Some(("whatsapp", "allowed_numbers")),
        "discord" => Some(("discord", "allowed_channel_ids")),
        "slack" => Some(("slack", "allowed_users")),
        "signal" => Some(("signal", "allowed_numbers")),
        "imessage" => Some(("imessage", "allowed_senders")),
        "irc" => Some(("irc", "allowed_nicks")),
        "matrix" => Some(("matrix", "allowed_users")),
        "mattermost" => Some(("mattermost", "allowed_users")),
        _ => None,
    }
}

pub fn approve_hint(req: &Request) -> String {
    match config_key(&req.channel) {
        Some((table, key)) => format!(
            "add \"{}\" to [{}] {} in {} and restart serve",
            req.sender,
            table,
            key,
            crate::config::config_path().display()
        ),
        None => format!(
            "channel '{}' has no allowlist key; add the sender by hand",
            req.channel
        ),
    }
}

pub fn list_text(pending: &[Request], now: u64) -> String {
    if pending.is_empty() {
        return "no pairing requests waiting\n".to_string();
    }
    let mut out = format!("{} pairing request(s) waiting\n", pending.len());
    for r in pending {
        let age = now.saturating_sub(r.created_ms) / 60_000;
        let who = if r.display.is_empty() {
            r.sender.clone()
        } else {
            format!("{} ({})", r.display, r.sender)
        };
        out.push_str(&format!(
            "  #{:<4}{:<12}{:<34}code {}  {age}m ago\n",
            r.id,
            r.channel,
            crate::security::one_line(&who, 32),
            r.code
        ));
    }
    out.push_str("\napprove with: phoenix pairing approve ID|CODE\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_ok(path: &Path, channel: &str, sender: &str, display: &str) -> Request {
        request_at(path, channel, sender, display)
            .map(|(r, _)| r)
            .unwrap()
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-pairing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("pairing.json")
    }

    #[test]
    fn a_request_is_recorded_once_per_sender() {
        let p = tmp();
        let a = request_ok(&p, "telegram", "12345", "Paulus");
        let b = request_ok(&p, "telegram", "12345", "Paulus");
        assert_eq!(a.id, b.id);
        assert_eq!(a.code, b.code);
        assert_eq!(load(&p).len(), 1);
    }

    #[test]
    fn the_same_sender_on_another_channel_is_a_separate_request() {
        let p = tmp();
        request_ok(&p, "telegram", "12345", "");
        request_ok(&p, "signal", "12345", "");
        assert_eq!(load(&p).len(), 2);
    }

    #[test]
    fn requests_are_found_by_id_code_or_sender_and_taken_once() {
        let p = tmp();
        let r = request_ok(&p, "discord", "999", "someone");
        let pending = load(&p);
        assert_eq!(find(&pending, &r.id.to_string()).map(|x| x.id), Some(r.id));
        assert_eq!(find(&pending, &r.code).map(|x| x.id), Some(r.id));
        assert_eq!(
            find(&pending, &r.code.to_lowercase()).map(|x| x.id),
            Some(r.id)
        );
        assert_eq!(find(&pending, "999").map(|x| x.id), Some(r.id));
        assert!(find(&pending, "nope").is_none());
        take(&p, &r.code).unwrap();
        assert!(load(&p).is_empty());
        assert!(take(&p, &r.code).is_err());
    }

    #[test]
    fn the_queue_is_bounded() {
        let p = tmp();
        for i in 0..MAX_PENDING {
            request_ok(&p, "irc", &format!("nick{i}"), "");
        }
        let err = request_at(&p, "irc", "one-too-many", "").unwrap_err();
        assert!(err.contains("full"), "{err}");
        assert_eq!(load(&p).len(), MAX_PENDING);
    }

    #[test]
    fn empty_channel_or_sender_is_refused() {
        let p = tmp();
        assert!(request_at(&p, "", "x", "").is_err());
        assert!(request_at(&p, "telegram", "   ", "").is_err());
    }

    #[test]
    fn codes_use_an_unambiguous_alphabet() {
        let code = code_from_bytes(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(code.len(), CODE_LEN);
        for c in new_code().chars() {
            assert!(
                c.is_ascii_uppercase() || c.is_ascii_digit(),
                "unexpected char {c}"
            );
            assert!(!"01IO".contains(c), "ambiguous char {c}");
        }
    }

    #[test]
    fn every_channel_maps_to_its_allowlist_key() {
        for ch in [
            "telegram",
            "whatsapp",
            "discord",
            "slack",
            "signal",
            "imessage",
            "irc",
            "matrix",
            "mattermost",
        ] {
            assert!(config_key(ch).is_some(), "{ch} has no allowlist key");
        }
        assert!(config_key("carrier-pigeon").is_none());
    }

    #[test]
    fn the_store_is_written_0600_and_survives_a_reload() {
        let p = tmp();
        let r = request_ok(&p, "matrix", "@a:b.c", "A");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let back = load(&p);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], r);
    }

    #[test]
    fn a_damaged_store_reads_as_empty_instead_of_failing() {
        let p = tmp();
        std::fs::write(&p, "{not json").unwrap();
        assert!(load(&p).is_empty());
    }

    #[test]
    fn list_text_names_the_code_and_the_approve_command() {
        let p = tmp();
        let r = request_ok(&p, "telegram", "42", "Paulus");
        let text = list_text(&load(&p), now_ms());
        assert!(text.contains(&r.code), "{text}");
        assert!(text.contains("pairing approve"), "{text}");
        assert!(list_text(&[], 0).contains("no pairing requests"));
    }

    #[test]
    fn the_approve_hint_names_the_config_key() {
        let p = tmp();
        let r = request_ok(&p, "telegram", "42", "");
        let hint = approve_hint(&r);
        assert!(hint.contains("allowed_chat_ids"), "{hint}");
        let odd = Request {
            channel: "carrier-pigeon".into(),
            ..r
        };
        assert!(approve_hint(&odd).contains("no allowlist key"));
    }
}
