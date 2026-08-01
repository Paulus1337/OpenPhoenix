use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;

#[derive(Debug)]
pub struct IMessage {
    cli_path: String,
    db_path: String,
    allowed: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct Inbound {
    pub key: String,
    pub sender: String,
    pub text: String,

    pub target: Value,
}

pub fn inbound_text(sender: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "iMessage",
        sender,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

pub fn classify(v: &Value, allowed: &[String]) -> Option<Inbound> {
    if v["method"].as_str()? != "message" {
        return None;
    }
    let m = &v["params"]["message"];
    if !m.is_object() {
        return None;
    }
    if m["is_from_me"].as_bool().unwrap_or(false)
        || m["is_reaction"].as_bool().unwrap_or(false)
        || m["is_tapback"].as_bool().unwrap_or(false)
    {
        return None;
    }
    let sender = m["sender"].as_str().unwrap_or("").trim().to_string();
    let text = m["text"].as_str().unwrap_or("").trim().to_string();
    if sender.is_empty() || text.is_empty() {
        return None;
    }

    if !allowed.iter().any(|a| a == &sender) {
        return None;
    }
    let target = if let Some(chat_id) = m["chat_id"].as_i64() {
        json!({"chat_id": chat_id})
    } else {
        json!({"to": sender})
    };
    let key = m["chat_guid"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| sender.clone());
    Some(Inbound {
        key,
        sender,
        text,
        target,
    })
}

impl IMessage {
    pub fn wanted(cfg: &Config) -> bool {
        cfg.imessage_enabled
    }

    pub fn new(cfg: &Config) -> Result<IMessage, String> {
        if !cfg.imessage_enabled {
            return Err("imessage: not enabled".into());
        }
        if cfg.imessage_allowed.is_empty() {
            return Err("imessage: allowed_senders is empty; refusing to answer everyone".into());
        }
        Ok(IMessage {
            cli_path: if cfg.imessage_cli_path.is_empty() {
                "imsg".to_string()
            } else {
                cfg.imessage_cli_path.clone()
            },
            db_path: cfg.imessage_db_path.clone(),
            allowed: cfg.imessage_allowed.clone(),
        })
    }

    fn spawn_rpc(&self) -> Result<Child, String> {
        let mut cmd = Command::new(&self.cli_path);
        cmd.arg("rpc");
        if !self.db_path.is_empty() {
            cmd.args(["--db", &self.db_path]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("imessage: cannot spawn {}: {e}", self.cli_path))
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut backoff = 1u64;
        println!(
            "phoenix: serving imessage via {} ({} allowed sender(s))",
            self.cli_path,
            self.allowed.len()
        );
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        loop {
            match self.run_rpc(handler, &mut last_seen) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    println!("imessage error: {}, restarting", redact(&e));
                }
            }
            std::thread::sleep(Duration::from_secs(backoff));
            backoff = (backoff * 2).min(60);
        }
    }

    pub fn run_rpc(
        &self,
        handler: &mut dyn FnMut(&str, &str) -> String,
        last_seen: &mut HashMap<String, u64>,
    ) -> Result<(), String> {
        let mut child = self.spawn_rpc()?;
        let result = self.pump(&mut child, handler, last_seen);
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    fn pump(
        &self,
        child: &mut Child,
        handler: &mut dyn FnMut(&str, &str) -> String,
        last_seen: &mut HashMap<String, u64>,
    ) -> Result<(), String> {
        let mut stdin = child.stdin.take().ok_or("imessage: no child stdin")?;
        let stdout = child.stdout.take().ok_or("imessage: no child stdout")?;
        let subscribe = json!({
            "jsonrpc": "2.0", "id": 1, "method": "watch.subscribe", "params": {}
        });
        writeln!(stdin, "{subscribe}").map_err(|e| format!("imessage: subscribe: {e}"))?;
        let mut next_id: u64 = 2;
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| format!("imessage: read: {e}"))?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(inbound) = classify(&v, &self.allowed) else {
                continue;
            };
            let now = crate::scheduler::now_epoch();
            let elapsed = last_seen
                .insert(inbound.key.clone(), now)
                .map(|prev| now.saturating_sub(prev));
            let text = inbound_text(&inbound.sender, &inbound.text, elapsed);
            let reply = handler(&inbound.key, &text);
            if reply.trim().is_empty() {
                continue;
            }
            let mut params = inbound.target.clone();
            params["text"] = Value::from(reply);
            let req = json!({
                "jsonrpc": "2.0", "id": next_id, "method": "send", "params": params
            });
            next_id += 1;
            writeln!(stdin, "{req}").map_err(|e| format!("imessage: send: {e}"))?;
        }
        Err("imessage: rpc stream ended".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("+15550001", "hello", Some(7200));
        assert!(out.starts_with("[iMessage +15550001 +2h "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("+15550001", "/reset", None), "/reset");
    }

    fn cfg(enabled: bool, allowed: &[&str]) -> Config {
        Config {
            imessage_enabled: enabled,
            imessage_allowed: allowed.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    fn note(msg: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": "message", "params": {"message": msg}})
    }

    #[test]
    fn fail_closed_construction() {
        assert!(IMessage::new(&cfg(false, &["+15550001"])).is_err());
        let err = IMessage::new(&cfg(true, &[])).unwrap_err();
        assert!(err.contains("allowed_senders"), "got: {err}");
        assert!(IMessage::new(&cfg(true, &["+15550001"])).is_ok());
    }

    #[test]
    fn classify_admits_allowed_plain_message() {
        let allowed = vec!["+15550001".to_string()];
        let v = note(json!({
            "sender": "+15550001", "text": "ping", "is_from_me": false,
            "chat_id": 7, "chat_guid": "iMessage;-;+15550001"
        }));
        let inbound = classify(&v, &allowed).unwrap();
        assert_eq!(inbound.key, "iMessage;-;+15550001");
        assert_eq!(inbound.sender, "+15550001");
        assert_eq!(inbound.text, "ping");
        assert_eq!(inbound.target, json!({"chat_id": 7}));

        let v = note(json!({"sender": "+15550001", "text": "hi"}));
        let inbound = classify(&v, &allowed).unwrap();
        assert_eq!(inbound.key, "+15550001");
        assert_eq!(inbound.target, json!({"to": "+15550001"}));
    }

    #[test]
    fn classify_skips_noise_and_strangers() {
        let allowed = vec!["+15550001".to_string()];

        assert!(classify(
            &note(json!({"sender": "+19999999", "text": "hi"})),
            &allowed
        )
        .is_none());

        assert!(classify(
            &note(json!({"sender": "+15550001", "text": "hi", "is_from_me": true})),
            &allowed
        )
        .is_none());

        assert!(classify(
            &note(json!({"sender": "+15550001", "text": "Loved x", "is_tapback": true})),
            &allowed
        )
        .is_none());
        assert!(classify(
            &note(json!({"sender": "+15550001", "text": "x", "is_reaction": true})),
            &allowed
        )
        .is_none());

        assert!(classify(
            &note(json!({"sender": "+15550001", "text": "  "})),
            &allowed
        )
        .is_none());
        assert!(classify(&json!({"method": "error", "params": {}}), &allowed).is_none());
        assert!(classify(&json!({"id": 1, "result": {}}), &allowed).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rpc_roundtrip_with_fake_imsg() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("px-imsg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("sent.json");
        let script = dir.join("imsg");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nread -r subscribe\n\
echo '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"subscription\":1}}}}'\n\
echo '{}'\nread -r sent\nprintf '%s' \"$sent\" > '{}'\n",
                serde_json::to_string(&note(json!({
                    "sender": "+15550001", "text": "ping", "chat_id": 7,
                    "chat_guid": "iMessage;-;+15550001"
                })))
                .unwrap()
                .replace('\'', "'\\''"),
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut c = cfg(true, &["+15550001"]);
        c.imessage_cli_path = script.display().to_string();
        let im = IMessage::new(&c).unwrap();
        let mut seen = Vec::new();
        let mut last_seen = HashMap::new();
        let err = im
            .run_rpc(
                &mut |key, text| {
                    seen.push((key.to_string(), text.to_string()));
                    "pong".to_string()
                },
                &mut last_seen,
            )
            .unwrap_err();
        assert!(err.contains("stream ended"), "got: {err}");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].0, "iMessage;-;+15550001");
        assert!(seen[0].1.starts_with("[iMessage +15550001 "), "{seen:?}");
        assert!(seen[0].1.ends_with("] ping"), "{seen:?}");
        let sent: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(sent["method"], "send");
        assert_eq!(sent["params"]["chat_id"], 7);
        assert_eq!(sent["params"]["text"], "pong");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
