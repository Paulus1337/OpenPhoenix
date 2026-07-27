use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::providers::{Msg, ToolCall};

pub fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn file(dir: &Path, chat_id: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize(chat_id)))
}

fn msg_to_json(m: &Msg) -> Value {
    match m {
        Msg::User { content, .. } => json!({"role": "user", "content": content}),
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
    let content = v.get("content").and_then(Value::as_str)?.to_string();
    match v.get("role").and_then(Value::as_str)? {
        "user" => Some(Msg::User {
            content,
            images: Vec::new(),
        }),
        "assistant" => {
            let tool_calls = v
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| {
                            Some(ToolCall {
                                id: t.get("id")?.as_str()?.to_string(),
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
            id: v.get("id").and_then(Value::as_str)?.to_string(),
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
    let arr: Vec<Value> = history.iter().map(msg_to_json).collect();
    let p = file(dir, chat_id);
    fs::write(&p, Value::Array(arr).to_string()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn load(dir: &Path, chat_id: &str) -> Vec<Msg> {
    let Ok(text) = fs::read_to_string(file(dir, chat_id)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v.as_array()
        .map(|a| a.iter().filter_map(msg_from_json).collect())
        .unwrap_or_default()
}

pub fn reset(dir: &Path, chat_id: &str) {
    let _ = fs::remove_file(file(dir, chat_id));
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
        assert_eq!(sanitize("../../etc"), "______etc");
        assert_eq!(sanitize("-100123"), "-100123");
        let dir = tmpdir();
        save(&dir, "../evil", &[]).unwrap();
        assert!(dir.join("___evil.json").exists());
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
