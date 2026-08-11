use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const MAX_DEVICES: usize = 64;
pub const TOKEN_BYTES: usize = 32;
pub const SCOPES: &[&str] = &["read", "act"];

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: u64,
    pub name: String,
    pub scope: String,
    pub hash: String,
    pub created_ms: u64,
    pub last_seen_ms: u64,
    pub revoked: bool,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("devices.json")
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn known_scope(s: &str) -> bool {
    SCOPES.contains(&s)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn token_hash(token: &str) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()).as_ref())
}

pub fn new_token() -> String {
    hex(&crate::ws::urandom(TOKEN_BYTES))
}

pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.len() <= 48
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn load(path: &Path) -> Vec<Device> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    v.get("devices")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    let name = d.get("name").and_then(Value::as_str)?;
                    let hash = d.get("hash").and_then(Value::as_str).unwrap_or("");
                    let revoked = d.get("revoked").and_then(Value::as_bool).unwrap_or(false);
                    if name.trim().is_empty() || (hash.trim().is_empty() && !revoked) {
                        return None;
                    }
                    let scope = d
                        .get("scope")
                        .and_then(Value::as_str)
                        .unwrap_or("read")
                        .to_string();
                    Some(Device {
                        id: d.get("id").and_then(Value::as_u64).unwrap_or(0),
                        name: name.to_string(),
                        scope: if known_scope(&scope) {
                            scope
                        } else {
                            "read".to_string()
                        },
                        hash: hash.to_string(),
                        created_ms: d.get("created_ms").and_then(Value::as_u64).unwrap_or(0),
                        last_seen_ms: d.get("last_seen_ms").and_then(Value::as_u64).unwrap_or(0),
                        revoked,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(path: &Path, devices: &[Device]) -> Result<(), String> {
    let arr: Vec<Value> = devices
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "scope": d.scope,
                "hash": d.hash,
                "created_ms": d.created_ms,
                "last_seen_ms": d.last_seen_ms,
                "revoked": d.revoked,
            })
        })
        .collect();
    let body = serde_json::to_string_pretty(&json!({"v": 1, "devices": arr}))
        .map_err(|e| e.to_string())?;
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn add(path: &Path, name: &str, scope: &str) -> Result<(Device, String), String> {
    if !valid_name(name) {
        return Err(
            "device names use letters, digits, dot, dash and underscore, up to 48 chars".into(),
        );
    }
    if !known_scope(scope) {
        return Err(format!("scope must be one of {SCOPES:?}"));
    }
    let name = name.trim();
    let _g = guard();
    let mut devices = load(path);
    if devices
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case(name) && !d.revoked)
    {
        return Err(format!("a device named '{name}' is already paired"));
    }
    if devices.iter().filter(|d| !d.revoked).count() >= MAX_DEVICES {
        return Err(format!("device limit reached ({MAX_DEVICES})"));
    }
    let token = new_token();
    let dev = Device {
        id: devices.iter().map(|d| d.id).max().unwrap_or(0) + 1,
        name: name.to_string(),
        scope: scope.to_string(),
        hash: token_hash(&token),
        created_ms: now_ms(),
        last_seen_ms: 0,
        revoked: false,
    };
    devices.push(dev.clone());
    save(path, &devices)?;
    Ok((dev, token))
}

fn locate(devices: &[Device], key: &str) -> Option<usize> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if let Ok(id) = key.strip_prefix('#').unwrap_or(key).parse::<u64>() {
        if let Some(i) = devices.iter().position(|d| d.id == id) {
            return Some(i);
        }
    }
    devices
        .iter()
        .position(|d| d.name.eq_ignore_ascii_case(key))
}

pub fn rotate(path: &Path, key: &str) -> Result<(Device, String), String> {
    let _g = guard();
    let mut devices = load(path);
    let Some(i) = locate(&devices, key) else {
        return Err(format!("no device matches '{key}'"));
    };
    let token = new_token();
    let Some(dev) = devices.get_mut(i) else {
        return Err("device vanished during rotation".into());
    };
    if dev.revoked {
        return Err(format!("device '{}' is revoked; add it again", dev.name));
    }
    dev.hash = token_hash(&token);
    dev.created_ms = now_ms();
    dev.last_seen_ms = 0;
    let out = dev.clone();
    save(path, &devices)?;
    Ok((out, token))
}

pub fn revoke(path: &Path, key: &str) -> Result<Device, String> {
    let _g = guard();
    let mut devices = load(path);
    let Some(i) = locate(&devices, key) else {
        return Err(format!("no device matches '{key}'"));
    };
    let Some(dev) = devices.get_mut(i) else {
        return Err("device vanished during revoke".into());
    };
    dev.revoked = true;
    dev.hash = String::new();
    let out = dev.clone();
    save(path, &devices)?;
    Ok(out)
}

pub fn remove(path: &Path, key: &str) -> Result<Device, String> {
    let _g = guard();
    let mut devices = load(path);
    let Some(i) = locate(&devices, key) else {
        return Err(format!("no device matches '{key}'"));
    };
    let out = devices.remove(i);
    save(path, &devices)?;
    Ok(out)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn authenticate(path: &Path, token: &str) -> Option<Device> {
    if token.trim().is_empty() {
        return None;
    }
    let want = token_hash(token);
    let mut devices = load(path);
    let i = devices
        .iter()
        .position(|d| !d.revoked && !d.hash.is_empty() && crate::security::ct_eq(&d.hash, &want))?;
    let dev = devices.get_mut(i)?;
    dev.last_seen_ms = now_ms();
    let out = dev.clone();
    let _ = save(path, &devices);
    Some(out)
}

pub fn list_text(devices: &[Device], now: u64) -> String {
    let live: Vec<&Device> = devices.iter().filter(|d| !d.revoked).collect();
    if live.is_empty() {
        return "no devices paired\n".to_string();
    }
    let mut out = format!("{} device(s)\n", live.len());
    for d in live {
        let seen = if d.last_seen_ms == 0 {
            "never".to_string()
        } else {
            format!("{}m ago", now.saturating_sub(d.last_seen_ms) / 60_000)
        };
        out.push_str(&format!(
            "  #{:<4}{:<24}{:<8}last seen {seen}\n",
            d.id, d.name, d.scope
        ));
    }
    let revoked = devices.iter().filter(|d| d.revoked).count();
    if revoked > 0 {
        out.push_str(&format!("{revoked} revoked\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-devices-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("devices.json")
    }

    #[test]
    fn a_token_is_shown_once_and_only_its_hash_is_stored() {
        let p = tmp();
        let (dev, token) = add(&p, "phone", "read").unwrap();
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains(&token), "the raw token reached disk");
        assert!(raw.contains(&dev.hash));
        assert_eq!(authenticate(&p, &token).map(|d| d.id), Some(dev.id));
    }

    #[test]
    fn authentication_rejects_wrong_revoked_and_empty_tokens() {
        let p = tmp();
        let (_, token) = add(&p, "laptop", "act").unwrap();
        assert!(authenticate(&p, "").is_none());
        assert!(authenticate(&p, &new_token()).is_none());
        revoke(&p, "laptop").unwrap();
        assert!(authenticate(&p, &token).is_none());
    }

    #[test]
    fn rotation_invalidates_the_old_token_immediately() {
        let p = tmp();
        let (_, old) = add(&p, "watch", "read").unwrap();
        let (_, new) = rotate(&p, "watch").unwrap();
        assert_ne!(old, new);
        assert!(authenticate(&p, &old).is_none());
        assert!(authenticate(&p, &new).is_some());
    }

    #[test]
    fn a_revoked_device_cannot_be_rotated_back_to_life() {
        let p = tmp();
        add(&p, "old", "read").unwrap();
        revoke(&p, "old").unwrap();
        assert!(rotate(&p, "old").is_err());
    }

    #[test]
    fn last_seen_is_recorded_on_a_successful_authentication() {
        let p = tmp();
        let (_, token) = add(&p, "tablet", "read").unwrap();
        assert_eq!(load(&p).first().map(|d| d.last_seen_ms), Some(0));
        authenticate(&p, &token).unwrap();
        assert!(load(&p).first().map(|d| d.last_seen_ms).unwrap_or(0) > 0);
    }

    #[test]
    fn names_and_scopes_are_validated_and_duplicates_refused() {
        let p = tmp();
        assert!(add(&p, "", "read").is_err());
        assert!(add(&p, "bad name", "read").is_err());
        assert!(add(&p, &"x".repeat(49), "read").is_err());
        assert!(add(&p, "ok", "root").is_err());
        add(&p, "ok", "act").unwrap();
        assert!(add(&p, "OK", "read").is_err());
    }

    #[test]
    fn a_revoked_name_can_be_paired_again() {
        let p = tmp();
        add(&p, "phone", "read").unwrap();
        revoke(&p, "phone").unwrap();
        let (dev, _) = add(&p, "phone", "act").unwrap();
        assert_eq!(dev.scope, "act");
        assert_eq!(load(&p).len(), 2);
    }

    #[test]
    fn devices_are_found_by_id_or_name_and_removal_is_final() {
        let p = tmp();
        let (dev, _) = add(&p, "kiosk", "read").unwrap();
        assert!(remove(&p, &dev.id.to_string()).is_ok());
        assert!(load(&p).is_empty());
        assert!(remove(&p, "kiosk").is_err());
    }

    #[test]
    fn the_store_is_0600_and_a_damaged_file_reads_as_empty() {
        let p = tmp();
        add(&p, "phone", "read").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::write(&p, "}{").unwrap();
        assert!(load(&p).is_empty());
    }

    #[test]
    fn the_device_limit_is_enforced() {
        let p = tmp();
        for i in 0..MAX_DEVICES {
            add(&p, &format!("d{i}"), "read").unwrap();
        }
        assert!(add(&p, "one-too-many", "read").is_err());
    }

    #[test]
    fn list_text_hides_no_secrets_and_counts_revoked() {
        let p = tmp();
        let (_, token) = add(&p, "phone", "read").unwrap();
        add(&p, "gone", "read").unwrap();
        revoke(&p, "gone").unwrap();
        let text = list_text(&load(&p), now_ms());
        assert!(text.contains("phone"), "{text}");
        assert!(!text.contains(&token), "{text}");
        assert!(text.contains("1 revoked"), "{text}");
        assert!(list_text(&[], 0).contains("no devices"));
    }
}
