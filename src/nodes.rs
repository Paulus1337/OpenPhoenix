use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const CAPABILITIES: &[&str] = &["camera", "screen", "canvas", "location", "notify", "shell"];
pub const ADMIN_CAPABILITIES: &[&str] = &["shell"];
pub const MAX_NODES: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u64,
    pub name: String,
    pub address: String,
    pub capabilities: Vec<String>,
    pub approved: bool,
    pub created_ms: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("nodes.json")
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn known_capability(c: &str) -> bool {
    CAPABILITIES.contains(&c)
}

pub fn needs_admin(caps: &[String]) -> bool {
    caps.iter()
        .any(|c| ADMIN_CAPABILITIES.contains(&c.as_str()))
}

pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.len() <= 48
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn parse_capabilities(raw: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let c = part.trim().to_ascii_lowercase();
        if c.is_empty() {
            continue;
        }
        if !known_capability(&c) {
            return Err(format!(
                "unknown capability '{c}': expected any of {CAPABILITIES:?}"
            ));
        }
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Ok(out)
}

pub fn load(path: &Path) -> Vec<Node> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    v.get("nodes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|n| {
                    let name = n.get("name").and_then(Value::as_str)?;
                    if !valid_name(name) {
                        return None;
                    }
                    let capabilities = n
                        .get("capabilities")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .filter(|c| known_capability(c))
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(Node {
                        id: n.get("id").and_then(Value::as_u64).unwrap_or(0),
                        name: name.to_string(),
                        address: n
                            .get("address")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        capabilities,
                        approved: n.get("approved").and_then(Value::as_bool).unwrap_or(false),
                        created_ms: n.get("created_ms").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(path: &Path, nodes: &[Node]) -> Result<(), String> {
    let arr: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "id": n.id,
                "name": n.name,
                "address": n.address,
                "capabilities": n.capabilities,
                "approved": n.approved,
                "created_ms": n.created_ms,
            })
        })
        .collect();
    let body =
        serde_json::to_string_pretty(&json!({"v": 1, "nodes": arr})).map_err(|e| e.to_string())?;
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn enroll(path: &Path, name: &str, address: &str, caps: &[String]) -> Result<Node, String> {
    if !valid_name(name) {
        return Err(
            "node names use letters, digits, dot, dash and underscore, up to 48 chars".into(),
        );
    }
    for c in caps {
        if !known_capability(c) {
            return Err(format!("unknown capability '{c}'"));
        }
    }
    if !address.trim().is_empty() {
        crate::ssrf::check_url(address).map_err(|e| format!("node address refused: {e}"))?;
    }
    let _g = guard();
    let mut nodes = load(path);
    if nodes.iter().any(|n| n.name.eq_ignore_ascii_case(name)) {
        return Err(format!("a node named '{name}' already exists"));
    }
    if nodes.len() >= MAX_NODES {
        return Err(format!("node limit reached ({MAX_NODES})"));
    }
    let node = Node {
        id: nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1,
        name: name.trim().to_string(),
        address: address.trim().to_string(),
        capabilities: caps.to_vec(),
        approved: false,
        created_ms: now_ms(),
    };
    nodes.push(node.clone());
    save(path, &nodes)?;
    Ok(node)
}

fn locate(nodes: &[Node], key: &str) -> Option<usize> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if let Ok(id) = key.strip_prefix('#').unwrap_or(key).parse::<u64>() {
        if let Some(i) = nodes.iter().position(|n| n.id == id) {
            return Some(i);
        }
    }
    nodes.iter().position(|n| n.name.eq_ignore_ascii_case(key))
}

pub fn approve(path: &Path, key: &str, admin: bool) -> Result<Node, String> {
    let _g = guard();
    let mut nodes = load(path);
    let Some(i) = locate(&nodes, key) else {
        return Err(format!("no node matches '{key}'"));
    };
    let Some(node) = nodes.get_mut(i) else {
        return Err("node vanished during approve".into());
    };
    if needs_admin(&node.capabilities) && !admin {
        return Err(format!(
            "node '{}' asks for an admin capability ({}); approve it again with --admin",
            node.name,
            node.capabilities.join(", ")
        ));
    }
    node.approved = true;
    let out = node.clone();
    save(path, &nodes)?;
    Ok(out)
}

pub fn reject(path: &Path, key: &str) -> Result<Node, String> {
    let _g = guard();
    let mut nodes = load(path);
    let Some(i) = locate(&nodes, key) else {
        return Err(format!("no node matches '{key}'"));
    };
    let out = nodes.remove(i);
    save(path, &nodes)?;
    Ok(out)
}

pub fn rename(path: &Path, key: &str, name: &str) -> Result<Node, String> {
    if !valid_name(name) {
        return Err("node names use letters, digits, dot, dash and underscore".into());
    }
    let _g = guard();
    let mut nodes = load(path);
    if nodes.iter().any(|n| n.name.eq_ignore_ascii_case(name)) {
        return Err(format!("a node named '{name}' already exists"));
    }
    let Some(i) = locate(&nodes, key) else {
        return Err(format!("no node matches '{key}'"));
    };
    let Some(node) = nodes.get_mut(i) else {
        return Err("node vanished during rename".into());
    };
    node.name = name.trim().to_string();
    let out = node.clone();
    save(path, &nodes)?;
    Ok(out)
}

pub fn allows(nodes: &[Node], name: &str, capability: &str) -> bool {
    nodes.iter().any(|n| {
        n.approved
            && n.name.eq_ignore_ascii_case(name)
            && n.capabilities.iter().any(|c| c == capability)
    })
}

pub fn describe(nodes: &[Node], key: &str) -> Option<String> {
    let i = locate(nodes, key)?;
    let n = nodes.get(i)?;
    let caps = if n.capabilities.is_empty() {
        "(none)".to_string()
    } else {
        n.capabilities.join(", ")
    };
    Some(format!(
        "#{} {}\n  address      {}\n  capabilities {}\n  state        {}\n",
        n.id,
        n.name,
        if n.address.is_empty() {
            "(none)"
        } else {
            &n.address
        },
        caps,
        if n.approved { "approved" } else { "pending" }
    ))
}

pub fn list_text(nodes: &[Node]) -> String {
    if nodes.is_empty() {
        return "no nodes enrolled\n".to_string();
    }
    let mut out = String::new();
    let pending: Vec<&Node> = nodes.iter().filter(|n| !n.approved).collect();
    let paired: Vec<&Node> = nodes.iter().filter(|n| n.approved).collect();
    out.push_str(&format!("{} paired\n", paired.len()));
    for n in &paired {
        out.push_str(&format!(
            "  #{:<4}{:<20}{}\n",
            n.id,
            n.name,
            n.capabilities.join(",")
        ));
    }
    out.push_str(&format!("{} pending\n", pending.len()));
    for n in &pending {
        out.push_str(&format!(
            "  #{:<4}{:<20}{}\n",
            n.id,
            n.name,
            n.capabilities.join(",")
        ));
    }
    if !pending.is_empty() {
        out.push_str("\napprove with: phoenix nodes approve ID|NAME\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-nodes-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("nodes.json")
    }

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_enrolled_node_starts_pending_and_grants_nothing() {
        let p = tmp();
        let n = enroll(&p, "kitchen", "", &caps(&["camera"])).unwrap();
        assert!(!n.approved);
        assert!(!allows(&load(&p), "kitchen", "camera"));
        approve(&p, "kitchen", false).unwrap();
        assert!(allows(&load(&p), "kitchen", "camera"));
    }

    #[test]
    fn approval_never_grants_a_capability_that_was_not_asked_for() {
        let p = tmp();
        enroll(&p, "hall", "", &caps(&["canvas"])).unwrap();
        approve(&p, "hall", true).unwrap();
        let nodes = load(&p);
        assert!(allows(&nodes, "hall", "canvas"));
        assert!(!allows(&nodes, "hall", "camera"));
        assert!(!allows(&nodes, "hall", "shell"));
    }

    #[test]
    fn admin_capabilities_need_an_explicit_admin_approval() {
        let p = tmp();
        enroll(&p, "box", "", &caps(&["shell"])).unwrap();
        let err = approve(&p, "box", false).unwrap_err();
        assert!(err.contains("--admin"), "{err}");
        assert!(!allows(&load(&p), "box", "shell"));
        approve(&p, "box", true).unwrap();
        assert!(allows(&load(&p), "box", "shell"));
    }

    #[test]
    fn a_node_address_must_pass_the_ssrf_gate() {
        let p = tmp();
        assert!(enroll(&p, "evil", "http://169.254.169.254/", &caps(&[])).is_err());
        assert!(enroll(&p, "evil2", "file:///etc/passwd", &caps(&[])).is_err());
        assert!(load(&p).is_empty());
    }

    #[test]
    fn unknown_capabilities_are_refused_at_parse_and_at_enroll() {
        let p = tmp();
        assert!(parse_capabilities("camera,launch-missiles").is_err());
        assert_eq!(parse_capabilities("camera, screen ").unwrap().len(), 2);
        assert_eq!(parse_capabilities("camera,camera").unwrap().len(), 1);
        assert!(enroll(&p, "n", "", &caps(&["root"])).is_err());
    }

    #[test]
    fn names_are_validated_and_unique_across_enroll_and_rename() {
        let p = tmp();
        assert!(enroll(&p, "bad name", "", &caps(&[])).is_err());
        enroll(&p, "a", "", &caps(&[])).unwrap();
        assert!(enroll(&p, "A", "", &caps(&[])).is_err());
        enroll(&p, "b", "", &caps(&[])).unwrap();
        assert!(rename(&p, "b", "a").is_err());
        assert_eq!(rename(&p, "b", "c").unwrap().name, "c");
    }

    #[test]
    fn rejecting_a_node_removes_it_for_good() {
        let p = tmp();
        enroll(&p, "gone", "", &caps(&["notify"])).unwrap();
        reject(&p, "gone").unwrap();
        assert!(load(&p).is_empty());
        assert!(reject(&p, "gone").is_err());
        assert!(approve(&p, "gone", true).is_err());
    }

    #[test]
    fn nodes_are_found_by_id_or_name() {
        let p = tmp();
        let n = enroll(&p, "byid", "", &caps(&[])).unwrap();
        assert!(approve(&p, &n.id.to_string(), false).is_ok());
        assert!(describe(&load(&p), "byid").is_some());
        assert!(describe(&load(&p), "#1").is_some());
        assert!(describe(&load(&p), "nope").is_none());
    }

    #[test]
    fn the_node_limit_is_enforced() {
        let p = tmp();
        for i in 0..MAX_NODES {
            enroll(&p, &format!("n{i}"), "", &caps(&[])).unwrap();
        }
        assert!(enroll(&p, "over", "", &caps(&[])).is_err());
    }

    #[test]
    fn a_damaged_store_reads_as_empty_and_junk_capabilities_are_dropped() {
        let p = tmp();
        std::fs::write(&p, "nonsense").unwrap();
        assert!(load(&p).is_empty());
        std::fs::write(
            &p,
            r#"{"nodes":[{"id":1,"name":"x","capabilities":["camera","root"],"approved":true}]}"#,
        )
        .unwrap();
        let nodes = load(&p);
        assert_eq!(nodes.first().map(|n| n.capabilities.len()), Some(1));
        assert!(!allows(&nodes, "x", "root"));
    }

    #[test]
    fn list_text_separates_paired_from_pending() {
        let p = tmp();
        enroll(&p, "paired", "", &caps(&["screen"])).unwrap();
        approve(&p, "paired", false).unwrap();
        enroll(&p, "waiting", "", &caps(&["notify"])).unwrap();
        let text = list_text(&load(&p));
        assert!(text.contains("1 paired"), "{text}");
        assert!(text.contains("1 pending"), "{text}");
        assert!(text.contains("nodes approve"), "{text}");
        assert!(list_text(&[]).contains("no nodes"));
    }
}
