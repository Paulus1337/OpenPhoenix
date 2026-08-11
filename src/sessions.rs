use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::providers::{Msg, ToolCall};

pub const SESSION_KEY_MAX: usize = 96;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    pub provider: String,
    pub model: String,
    pub thinking: String,
    pub reasoning_visible: bool,
    pub colab_partner: String,
    pub colab_auto: bool,
    pub colab_rounds: u32,
    pub partner_thinking: String,
}

fn preferences_file(dir: &Path, chat_id: &str) -> PathBuf {
    dir.join("preferences")
        .join(format!("{}.json", sanitize(chat_id)))
}

pub fn preferences_from_agent(agent: &crate::agent::Agent) -> Preferences {
    let colab = agent.colab_preferences();
    Preferences {
        provider: agent.cfg.provider.clone(),
        model: agent.cfg.model.clone(),
        thinking: agent.cfg.thinking.clone(),
        reasoning_visible: agent.cfg.reasoning_visible,
        colab_partner: colab
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_default(),
        colab_auto: colab.as_ref().is_some_and(|value| value.1),
        colab_rounds: colab.as_ref().map(|value| value.2).unwrap_or(0),
        partner_thinking: colab.map(|value| value.3).unwrap_or_default(),
    }
}

pub fn save_preferences(
    dir: &Path,
    chat_id: &str,
    preferences: &Preferences,
) -> Result<(), String> {
    if preferences.provider.len() > 64
        || preferences.model.len() > 512
        || preferences.thinking.len() > 32
        || preferences.colab_partner.len() > 576
        || preferences.partner_thinking.len() > 32
    {
        return Err("session preferences are too long".to_string());
    }
    let path = preferences_file(dir, chat_id);
    let Some(parent) = path.parent() else {
        return Err("session preferences have no parent directory".to_string());
    };
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let body = json!({
        "v": 2,
        "provider": preferences.provider,
        "model": preferences.model,
        "thinking": preferences.thinking,
        "reasoning_visible": preferences.reasoning_visible,
        "colab_partner": preferences.colab_partner,
        "colab_auto": preferences.colab_auto,
        "colab_rounds": preferences.colab_rounds,
        "partner_thinking": preferences.partner_thinking,
    });
    crate::security::write_atomic(&path, body.to_string().as_bytes(), Some(0o600))
        .map_err(|error| error.to_string())
}

pub fn load_preferences(dir: &Path, chat_id: &str) -> Option<Preferences> {
    let path = preferences_file(dir, chat_id);
    let text = fs::read_to_string(&path).ok()?;
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            let stamp = crate::scheduler::now_epoch();
            let quarantine = path.with_extension(format!("corrupt.{stamp}.json"));
            let _ = fs::rename(&path, quarantine);
            crate::log::warn_with(
                "sessions",
                format!("preferences unreadable ({error}); quarantined and defaults restored"),
                &crate::log::Fields::default().session(chat_id),
            );
            return None;
        }
    };
    if !matches!(value["v"].as_u64(), Some(1 | 2)) {
        return None;
    }
    let preferences = Preferences {
        provider: value["provider"].as_str()?.to_string(),
        model: value["model"].as_str()?.to_string(),
        thinking: value["thinking"].as_str()?.to_string(),
        reasoning_visible: value["reasoning_visible"].as_bool().unwrap_or(false),
        colab_partner: value["colab_partner"].as_str().unwrap_or("").to_string(),
        colab_auto: value["colab_auto"].as_bool().unwrap_or(false),
        colab_rounds: value["colab_rounds"].as_u64().unwrap_or(0).min(20) as u32,
        partner_thinking: value["partner_thinking"].as_str().unwrap_or("").to_string(),
    };
    if !crate::config::known_kind(&preferences.provider)
        || preferences.model.is_empty()
        || !crate::config::THINKING_LEVELS.contains(&preferences.thinking.as_str())
    {
        return None;
    }
    Some(preferences)
}

pub fn restore_preferences(
    agent: &mut crate::agent::Agent,
    preferences: &Preferences,
) -> Result<(), String> {
    if !crate::config::known_kind(&preferences.provider)
        || preferences.model.is_empty()
        || !crate::config::THINKING_LEVELS.contains(&preferences.thinking.as_str())
    {
        return Err("saved session preferences are not valid".to_string());
    }
    agent.retarget(&format!("{}/{}", preferences.provider, preferences.model))?;
    agent.cfg.thinking = preferences.thinking.clone();
    agent.set_reasoning_visible(preferences.reasoning_visible);
    if !preferences.colab_partner.is_empty() {
        if let Err(error) = agent.restore_colab(
            &preferences.colab_partner,
            preferences.colab_rounds,
            preferences.colab_auto,
            &preferences.partner_thinking,
        ) {
            agent.preserve_unavailable_colab(
                &preferences.colab_partner,
                preferences.colab_rounds,
                preferences.colab_auto,
                &format!("saved partner is unavailable: {error}"),
            );
        }
    }
    Ok(())
}

pub fn save_agent_state(
    dir: &Path,
    chat_id: &str,
    agent: &crate::agent::Agent,
) -> Result<(), String> {
    save(dir, chat_id, &agent.history)?;
    save_preferences(dir, chat_id, &preferences_from_agent(agent))
}

pub fn restore_agent_state(dir: &Path, chat_id: &str, agent: &mut crate::agent::Agent) {
    agent.history = load(dir, chat_id);
    if let Some(preferences) = load_preferences(dir, chat_id) {
        if let Err(error) = restore_preferences(agent, &preferences) {
            crate::log::warn_with(
                "sessions",
                format!("saved model preferences could not be restored: {error}"),
                &crate::log::Fields::default().session(chat_id),
            );
        }
    }
}

pub const MISSING_TOOL_RESULT: &str =
    "tool result missing: the run ended before this tool reported back";

pub fn sanitize(id: &str) -> String {
    let mapped: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('_');
    if trimmed.is_empty() {
        return "session".to_string();
    }
    if trimmed.chars().count() <= SESSION_KEY_MAX {
        return trimmed.to_string();
    }
    let digest = crate::security::sha256_hex(id.as_bytes());
    let head: String = trimmed.chars().take(SESSION_KEY_MAX - 17).collect();
    format!("{head}-{}", &digest[..16])
}

fn file(dir: &Path, chat_id: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize(chat_id)))
}

fn msg_to_json(m: &Msg) -> Value {
    match m {
        Msg::User { content, images } => {
            if images.is_empty() {
                json!({"role": "user", "content": content})
            } else {
                let imgs: Vec<Value> = images
                    .iter()
                    .map(|(mime, b64)| json!({"mime": mime, "b64": b64}))
                    .collect();
                json!({"role": "user", "content": content, "images": imgs})
            }
        }
        Msg::Assistant {
            content,
            tool_calls,
        } => {
            let tcs: Vec<Value> = tool_calls
                .iter()
                .map(|t| json!({"id": t.id, "name": t.name, "args": t.args}))
                .collect();
            json!({"role": "assistant", "content": content, "tool_calls": tcs})
        }
        Msg::Tool { id, content } => json!({"role": "tool", "id": id, "content": content}),
    }
}

fn msg_from_json(v: &Value) -> Option<Msg> {
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match v.get("role").and_then(Value::as_str)? {
        "user" => {
            let images: Vec<(String, String)> = v
                .get("images")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|i| {
                            Some((
                                i.get("mime")?.as_str()?.to_string(),
                                i.get("b64")?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Msg::User { content, images })
        }
        "assistant" => {
            let tool_calls = v
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| {
                            Some(ToolCall {
                                id: t.get("id")?.as_str()?.trim().to_string(),
                                name: t.get("name")?.as_str()?.to_string(),
                                args: t.get("args").cloned().unwrap_or_else(|| json!({})),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Msg::Assistant {
                content,
                tool_calls,
            })
        }
        "tool" => Some(Msg::Tool {
            id: v.get("id").and_then(Value::as_str)?.trim().to_string(),
            content,
        }),
        _ => None,
    }
}

pub fn save(dir: &Path, chat_id: &str, history: &[Msg]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let (history, _) = repair(history);
    let arr: Vec<Value> = history.iter().map(msg_to_json).collect();
    let p = file(dir, chat_id);
    let body = Value::Array(arr).to_string();
    crate::security::write_atomic(&p, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn repair(history: &[Msg]) -> (Vec<Msg>, usize) {
    let mut out: Vec<Msg> = Vec::with_capacity(history.len());
    let mut fixes = 0usize;
    let mut i = 0usize;
    while i < history.len() {
        let (content, original_calls) = match &history[i] {
            Msg::Tool { .. } => {
                fixes += 1;
                i += 1;
                continue;
            }
            Msg::Assistant {
                content,
                tool_calls,
            } if !tool_calls.is_empty() => (content.clone(), tool_calls.clone()),
            _ => {
                out.push(history[i].clone());
                i += 1;
                continue;
            }
        };
        let mut tool_calls = Vec::new();
        for call in original_calls {
            if call.id.trim().is_empty()
                || tool_calls
                    .iter()
                    .any(|seen: &crate::providers::ToolCall| seen.id == call.id)
            {
                fixes += 1;
            } else {
                tool_calls.push(call);
            }
        }
        i += 1;
        let mut results: Vec<(String, String)> = Vec::new();
        while let Some(Msg::Tool { id, content }) = history.get(i) {
            let wanted = tool_calls.iter().any(|call| call.id == *id);
            let duplicate = results.iter().any(|(seen, _)| seen == id);
            if wanted && !duplicate {
                results.push((id.clone(), content.clone()));
            } else {
                fixes += 1;
            }
            i += 1;
        }
        if tool_calls.is_empty() {
            if !content.trim().is_empty() {
                out.push(Msg::Assistant {
                    content,
                    tool_calls: Vec::new(),
                });
            } else {
                fixes += 1;
            }
            continue;
        }
        out.push(Msg::Assistant {
            content,
            tool_calls: tool_calls.clone(),
        });
        for call in &tool_calls {
            match results.iter().find(|(id, _)| *id == call.id) {
                Some((id, content)) => out.push(Msg::Tool {
                    id: id.clone(),
                    content: content.clone(),
                }),
                None => {
                    fixes += 1;
                    out.push(Msg::Tool {
                        id: call.id.clone(),
                        content: MISSING_TOOL_RESULT.to_string(),
                    });
                }
            }
        }
    }
    (out, fixes)
}

pub fn load(dir: &Path, chat_id: &str) -> Vec<Msg> {
    let p = file(dir, chat_id);
    let Ok(text) = fs::read_to_string(&p) else {
        return Vec::new();
    };
    let v = match serde_json::from_str::<Value>(&text) {
        Ok(v) => v,
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let quarantine = p.with_extension(format!("corrupt.{stamp}.json"));
            match fs::rename(&p, &quarantine) {
                Ok(()) => crate::log::warn_with(
                    "sessions",
                    format!("transcript unreadable ({e}); quarantined and started fresh"),
                    &crate::log::Fields::default().session(chat_id),
                ),
                Err(re) => crate::log::error_with(
                    "sessions",
                    format!(
                        "transcript unreadable ({e}); quarantine failed ({re}); refusing overwrite"
                    ),
                    &crate::log::Fields::default().session(chat_id),
                ),
            }
            return Vec::new();
        }
    };
    let raw: Vec<Msg> = v
        .as_array()
        .map(|a| a.iter().filter_map(msg_from_json).collect())
        .unwrap_or_default();
    let (fixed, fixes) = repair(&raw);
    if fixes > 0 {
        crate::log::warn_with(
            "sessions",
            format!("repaired {fixes} tool-call pairing problem(s) on load"),
            &crate::log::Fields::default().session(chat_id),
        );
    }
    fixed
}

fn snapshot_file(dir: &Path, chat_id: &str, name: &str) -> PathBuf {
    dir.join("snapshots")
        .join(format!("{}--{}.json", sanitize(chat_id), sanitize(name)))
}

pub fn snapshot(dir: &Path, chat_id: &str, name: &str) -> Result<String, String> {
    let history = load(dir, chat_id);
    if history.is_empty() {
        return Err(format!("no stored session for {chat_id}"));
    }
    let dst = snapshot_file(dir, chat_id, name);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let arr: Vec<Value> = history.iter().map(msg_to_json).collect();
    crate::security::write_atomic(&dst, Value::Array(arr).to_string().as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "snapshot {name} saved for {chat_id} ({} messages)",
        history.len()
    ))
}

pub fn restore(dir: &Path, chat_id: &str, name: &str) -> Result<String, String> {
    let src = snapshot_file(dir, chat_id, name);
    if !src.is_file() {
        return Err(format!("no snapshot named {name} for {chat_id}"));
    }
    let raw = fs::read_to_string(&src).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("snapshot unreadable: {e}"))?;
    let history: Vec<Msg> = v
        .as_array()
        .map(|a| a.iter().filter_map(msg_from_json).collect())
        .unwrap_or_default();
    if history.is_empty() {
        return Err(format!("snapshot {name} holds no usable messages"));
    }
    let (repaired, fixes) = repair(&history);
    save(dir, chat_id, &repaired)?;
    let note = if fixes > 0 {
        format!(" ({fixes} repairs applied)")
    } else {
        String::new()
    };
    Ok(format!(
        "session {chat_id} restored from {name}: {} messages{note}",
        repaired.len()
    ))
}

pub fn diff(dir: &Path, chat_id: &str, name: &str) -> Result<String, String> {
    let src = snapshot_file(dir, chat_id, name);
    if !src.is_file() {
        return Err(format!("no snapshot named {name} for {chat_id}"));
    }
    let raw = fs::read_to_string(&src).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("snapshot unreadable: {e}"))?;
    let snap: Vec<Msg> = v
        .as_array()
        .map(|a| a.iter().filter_map(msg_from_json).collect())
        .unwrap_or_default();
    let live = load(dir, chat_id);
    let snap_json: Vec<String> = snap.iter().map(|m| msg_to_json(m).to_string()).collect();
    let live_json: Vec<String> = live.iter().map(|m| msg_to_json(m).to_string()).collect();
    let common = snap_json
        .iter()
        .zip(live_json.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == snap_json.len() && common == live_json.len() {
        return Ok(format!(
            "live session matches snapshot {name}: {} messages, no drift",
            live_json.len()
        ));
    }
    let mut lines = vec![format!(
        "snapshot {name}: {} messages | live: {} | shared prefix: {}",
        snap_json.len(),
        live_json.len(),
        common
    )];
    if common < snap_json.len() {
        lines.push(format!(
            "snapshot has {} message(s) past the shared prefix (restore would rewind these away)",
            snap_json.len() - common
        ));
    }
    if common < live_json.len() {
        lines.push(format!(
            "live session has {} message(s) past the shared prefix (a restore would drop these)",
            live_json.len() - common
        ));
    }
    Ok(lines.join("\n"))
}

pub fn snapshots(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir.join("snapshots")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(stem) = n.strip_suffix(".json") {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    out
}

pub fn reset(dir: &Path, chat_id: &str) {
    let _ = fs::remove_file(file(dir, chat_id));
    let _ = fs::remove_file(preferences_file(dir, chat_id));
}

pub fn list(dir: &Path) -> Vec<(String, usize)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, usize)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                let id = p.file_stem()?.to_string_lossy().to_string();
                let n = fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .and_then(|v| v.as_array().map(Vec::len))
                    .unwrap_or(0);
                Some((id, n))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    fn user(t: &str) -> Msg {
        Msg::User {
            content: t.into(),
            images: Vec::new(),
        }
    }

    fn calls(ids: &[&str]) -> Msg {
        Msg::Assistant {
            content: String::new(),
            tool_calls: ids
                .iter()
                .map(|id| ToolCall {
                    id: (*id).into(),
                    name: "shell".into(),
                    args: json!({}),
                })
                .collect(),
        }
    }

    fn tool(id: &str, body: &str) -> Msg {
        Msg::Tool {
            id: id.into(),
            content: body.into(),
        }
    }

    fn ids(history: &[Msg]) -> Vec<String> {
        history
            .iter()
            .map(|m| match m {
                Msg::User { .. } => "user".to_string(),
                Msg::Assistant { tool_calls, .. } => format!("assistant[{}]", tool_calls.len()),
                Msg::Tool { id, .. } => format!("tool:{id}"),
            })
            .collect()
    }

    #[test]
    fn a_paired_history_is_left_alone() {
        let h = vec![
            user("hi"),
            calls(&["a", "b"]),
            tool("a", "1"),
            tool("b", "2"),
        ];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 0);
        assert_eq!(ids(&out), ids(&h));
    }

    #[test]
    fn a_missing_tool_result_is_synthesized_so_providers_do_not_400() {
        let h = vec![user("hi"), calls(&["a", "b"]), tool("a", "1")];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 1);
        assert_eq!(ids(&out), vec!["user", "assistant[2]", "tool:a", "tool:b"]);
        match out.last() {
            Some(Msg::Tool { id, content }) => {
                assert_eq!(id, "b");
                assert_eq!(content, MISSING_TOOL_RESULT);
            }
            other => panic!("expected a synthetic tool result, got {other:?}"),
        }
    }

    #[test]
    fn results_are_reordered_to_match_the_call_order() {
        let h = vec![calls(&["a", "b"]), tool("b", "2"), tool("a", "1")];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 0);
        assert_eq!(ids(&out), vec!["assistant[2]", "tool:a", "tool:b"]);
    }

    #[test]
    fn an_orphan_tool_result_with_no_call_is_dropped() {
        let h = vec![user("hi"), tool("ghost", "x"), user("again")];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 1);
        assert_eq!(ids(&out), vec!["user", "user"]);
    }

    #[test]
    fn a_result_for_an_unknown_id_is_dropped_and_the_real_one_synthesized() {
        let h = vec![calls(&["a"]), tool("zzz", "stray")];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 2);
        assert_eq!(ids(&out), vec!["assistant[1]", "tool:a"]);
    }

    #[test]
    fn duplicate_results_for_one_call_are_collapsed() {
        let h = vec![calls(&["a"]), tool("a", "first"), tool("a", "second")];
        let (out, fixes) = repair(&h);
        assert_eq!(fixes, 1);
        assert_eq!(ids(&out), vec!["assistant[1]", "tool:a"]);
        match out.last() {
            Some(Msg::Tool { content, .. }) => assert_eq!(content, "first"),
            other => panic!("expected the first result kept, got {other:?}"),
        }
    }

    #[test]
    fn empty_and_duplicate_tool_call_ids_are_removed_deterministically() {
        let h = vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![
                crate::providers::ToolCall {
                    id: String::new(),
                    name: "a".into(),
                    args: json!({}),
                },
                crate::providers::ToolCall {
                    id: "same".into(),
                    name: "b".into(),
                    args: json!({}),
                },
                crate::providers::ToolCall {
                    id: "same".into(),
                    name: "c".into(),
                    args: json!({}),
                },
            ],
        }];
        let (fixed, fixes) = repair(&h);
        assert_eq!(fixes, 3);
        assert_eq!(ids(&fixed), vec!["assistant[1]", "tool:same"]);
        let (again, second_fixes) = repair(&fixed);
        assert_eq!(second_fixes, 0);
        assert_eq!(ids(&again), ids(&fixed));
    }

    #[test]
    fn an_interrupted_turn_round_trips_through_disk_repaired() {
        let d = std::env::temp_dir().join(format!("phx-sess-repair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let broken = vec![user("do it"), calls(&["a", "b"]), tool("a", "done")];
        save(&d, "chat", &broken).unwrap();
        let loaded = load(&d, "chat");
        assert_eq!(
            ids(&loaded),
            vec!["user", "assistant[2]", "tool:a", "tool:b"]
        );
        let (again, fixes) = repair(&loaded);
        assert_eq!(fixes, 0, "repair must be idempotent");
        assert_eq!(ids(&again), ids(&loaded));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_null_content_message_is_kept_not_dropped() {
        let v = json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "a", "name": "shell", "args": {}}]},
            {"role": "tool", "id": "a", "content": "out"}
        ]);
        let raw: Vec<Msg> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(msg_from_json)
            .collect();
        assert_eq!(raw.len(), 3, "null content must not drop the message");
        let (out, fixes) = repair(&raw);
        assert_eq!(fixes, 0);
        assert_eq!(ids(&out), vec!["user", "assistant[1]", "tool:a"]);
    }
}

#[cfg(test)]
mod atomic_save_tests {
    use super::*;

    #[test]
    fn user_images_survive_a_save_and_restore() {
        let d = std::env::temp_dir().join(format!("phx-sess-images-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let history = vec![Msg::User {
            content: "look at this".into(),
            images: vec![("image/png".into(), "aGVsbG8=".into())],
        }];
        save(&d, "chat", &history).unwrap();
        let back = load(&d, "chat");
        let _ = fs::remove_dir_all(&d);
        assert_eq!(back.len(), 1);
        match &back[0] {
            Msg::User { content, images } => {
                assert_eq!(content, "look at this");
                assert_eq!(
                    images.len(),
                    1,
                    "a restored turn must keep its attachment data"
                );
                assert_eq!(images[0].0, "image/png");
                assert_eq!(images[0].1, "aGVsbG8=");
            }
            other => panic!("expected the user turn back, got {other:?}"),
        }
    }

    #[test]
    fn a_save_never_leaves_a_partial_file_or_temp_behind() {
        let d = std::env::temp_dir().join(format!("phx-sess-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let history = vec![Msg::User {
            content: "first".into(),
            images: Vec::new(),
        }];
        save(&d, "chat", &history).unwrap();
        assert_eq!(load(&d, "chat").len(), 1);

        let bigger = vec![
            Msg::User {
                content: "first".into(),
                images: Vec::new(),
            },
            Msg::Assistant {
                content: "second".into(),
                tool_calls: Vec::new(),
            },
        ];
        save(&d, "chat", &bigger).unwrap();
        assert_eq!(load(&d, "chat").len(), 2);

        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
        let _ = fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-sess-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample_history() -> Vec<Msg> {
        vec![
            Msg::User {
                content: "hi".into(),
                images: Vec::new(),
            },
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "t1".into(),
                    name: "shell".into(),
                    args: json!({"command": "ls"}),
                }],
            },
            Msg::Tool {
                id: "t1".into(),
                content: "file.txt".into(),
            },
            Msg::Assistant {
                content: "done".into(),
                tool_calls: vec![],
            },
        ]
    }

    #[test]
    fn snapshot_and_restore_roundtrip() {
        let d = tmpdir();
        let h = sample_history();
        save(&d, "7", &h).unwrap();
        let note = snapshot(&d, "7", "before-refactor").unwrap();
        assert!(note.contains("before-refactor"), "{note}");
        reset(&d, "7");
        assert!(load(&d, "7").is_empty());
        let back = restore(&d, "7", "before-refactor").unwrap();
        assert!(back.contains("restored"), "{back}");
        assert_eq!(load(&d, "7").len(), h.len());
        assert_eq!(snapshots(&d), vec!["7--before-refactor".to_string()]);
    }

    #[test]
    fn snapshot_of_nothing_and_restore_of_missing_fail_cleanly() {
        let d = tmpdir();
        assert!(snapshot(&d, "9", "x").is_err());
        assert!(restore(&d, "9", "x").is_err());
    }

    #[test]
    fn a_diff_names_drift_in_both_directions_deterministically() {
        let d = tmpdir();
        let h = sample_history();
        save(&d, "7", &h).unwrap();
        snapshot(&d, "7", "base").unwrap();

        let same = diff(&d, "7", "base").unwrap();
        assert!(same.contains("no drift"), "{same}");

        let mut grown = h.clone();
        grown.push(Msg::User {
            content: "new turn".into(),
            images: Vec::new(),
        });
        save(&d, "7", &grown).unwrap();
        let ahead = diff(&d, "7", "base").unwrap();
        assert!(
            ahead.contains("live session has 1 message(s) past the shared prefix"),
            "{ahead}"
        );

        save(&d, "7", &h[..2]).unwrap();
        let behind = diff(&d, "7", "base").unwrap();
        assert!(
            behind.contains("snapshot has 2 message(s) past the shared prefix"),
            "{behind}"
        );
        assert_eq!(
            diff(&d, "7", "base").unwrap(),
            behind,
            "the diff is deterministic run to run"
        );
        assert!(diff(&d, "7", "missing").is_err());
    }

    #[test]
    fn roundtrip() {
        let dir = tmpdir();
        save(&dir, "123", &sample_history()).unwrap();
        let back = load(&dir, "123");
        assert_eq!(back.len(), 4);
        match &back[1] {
            Msg::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls[0].name, "shell");
                assert_eq!(tool_calls[0].args["command"], "ls");
            }
            other => panic!("wrong msg: {other:?}"),
        }
    }

    #[test]
    fn model_and_thinking_preferences_survive_a_restart_without_secrets() {
        let dir = tmpdir();
        let preferences = Preferences {
            provider: "anthropic".into(),
            model: "claude-fable-5".into(),
            thinking: "high".into(),
            reasoning_visible: true,
            colab_partner: String::new(),
            colab_auto: false,
            colab_rounds: 0,
            partner_thinking: String::new(),
        };
        save_preferences(&dir, "chat", &preferences).unwrap();
        assert_eq!(load_preferences(&dir, "chat"), Some(preferences));
        let path = preferences_file(&dir, "chat");
        let body = fs::read_to_string(&path).unwrap();
        assert!(!body.contains("api_key"));
        assert!(!body.contains("token"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn legacy_v1_preferences_remain_compatible() {
        let dir = tmpdir();
        let path = preferences_file(&dir, "legacy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            r#"{"v":1,"provider":"openai","model":"gpt-5","thinking":"off"}"#,
        )
        .unwrap();
        let loaded = load_preferences(&dir, "legacy").expect("legacy preferences");
        assert_eq!(loaded.provider, "openai");
        assert!(loaded.colab_partner.is_empty());
    }

    #[test]
    fn colab_preferences_round_trip_in_v2() {
        let dir = tmpdir();
        let preferences = Preferences {
            provider: "openai".into(),
            model: "gpt-5.6-sol".into(),
            thinking: "high".into(),
            reasoning_visible: true,
            colab_partner: "anthropic/claude-opus-5".into(),
            colab_auto: false,
            colab_rounds: 2,
            partner_thinking: "medium".into(),
        };
        save_preferences(&dir, "team", &preferences).unwrap();
        assert_eq!(load_preferences(&dir, "team"), Some(preferences));
        let body = fs::read_to_string(preferences_file(&dir, "team")).unwrap();
        assert!(body.contains(r#""v":2"#), "{body}");
        assert!(!body.contains("api_key"));
    }

    #[test]
    fn reset_removes_transcript_and_preferences() {
        let dir = tmpdir();
        save(&dir, "chat", &sample_history()).unwrap();
        save_preferences(
            &dir,
            "chat",
            &Preferences {
                provider: "openai".into(),
                model: "gpt-5".into(),
                thinking: "off".into(),
                reasoning_visible: false,
                colab_partner: String::new(),
                colab_auto: false,
                colab_rounds: 0,
                partner_thinking: String::new(),
            },
        )
        .unwrap();
        reset(&dir, "chat");
        assert!(load(&dir, "chat").is_empty());
        assert!(load_preferences(&dir, "chat").is_none());
    }

    #[test]
    fn reset_and_list() {
        let dir = tmpdir();
        save(&dir, "a", &sample_history()).unwrap();
        save(&dir, "b", &[]).unwrap();
        assert_eq!(list(&dir), vec![("a".into(), 4), ("b".into(), 0)]);
        reset(&dir, "a");
        assert_eq!(list(&dir).len(), 1);
        assert!(load(&dir, "a").is_empty());
    }

    #[test]
    fn ids_are_sanitized() {
        assert_eq!(sanitize("../../etc"), "etc");
        assert_eq!(sanitize("-100123"), "-100123");
        let dir = tmpdir();
        save(&dir, "../evil", &[]).unwrap();
        assert!(dir.join("evil.json").exists());
    }

    #[test]
    fn long_keys_are_bounded_and_stay_unique() {
        let a = format!("matrix-{}", "x".repeat(400));
        let b = format!("matrix-{}", "x".repeat(401));
        let sa = sanitize(&a);
        let sb = sanitize(&b);
        assert!(sa.chars().count() <= SESSION_KEY_MAX, "{}", sa.len());
        assert!(sb.chars().count() <= SESSION_KEY_MAX);
        assert_ne!(sa, sb, "distinct long keys must not collide");
        assert_eq!(sa, sanitize(&a), "sanitize must be deterministic");
    }

    #[test]
    fn a_corrupt_transcript_is_quarantined_not_destroyed() {
        let dir = tmpdir();
        let p = dir.join("c.json");
        fs::write(&p, "{ this is not valid json").unwrap();
        let loaded = load(&dir, "c");
        assert!(loaded.is_empty(), "a corrupt file must not yield messages");
        assert!(!p.exists(), "the corrupt file must be moved aside");
        let kept: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt."))
            .collect();
        assert_eq!(kept.len(), 1, "exactly one quarantined copy must survive");
        let body = fs::read_to_string(kept[0].path()).unwrap();
        assert_eq!(body, "{ this is not valid json", "bytes must be preserved");

        save(&dir, "c", &sample_history()).unwrap();
        assert!(p.exists());
        assert!(kept[0].path().exists(), "quarantined copy must remain");
    }

    #[test]
    fn empty_or_all_separator_ids_get_a_safe_name() {
        assert_eq!(sanitize(""), "session");
        assert_eq!(sanitize("///"), "session");
        assert_eq!(sanitize(".."), "session");
    }

    #[cfg(unix)]
    #[test]
    fn session_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir();
        save(&dir, "p", &sample_history()).unwrap();
        let mode = fs::metadata(dir.join("p.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
