use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;

pub fn inbound_text(sender: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "Matrix",
        sender,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

#[derive(Clone)]
pub struct Matrix {
    homeserver: String,
    token: String,
    user_id: String,
    allowed: crate::allowlist::Allowlist,
}

pub fn classify(sync: &Value, me: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let Some(rooms) = sync["rooms"]["join"].as_object() else {
        return out;
    };
    for (room_id, room) in rooms {
        let Some(events) = room["timeline"]["events"].as_array() else {
            continue;
        };
        for ev in events {
            if ev["type"].as_str() != Some("m.room.message") {
                continue;
            }
            let sender = ev["sender"].as_str().unwrap_or("");
            if sender.is_empty() || sender == me {
                continue;
            }
            let content = &ev["content"];
            if content["msgtype"].as_str() != Some("m.text") {
                continue;
            }
            let Some(body) = content["body"].as_str() else {
                continue;
            };
            out.push((room_id.clone(), sender.to_string(), body.to_string()));
        }
    }
    out
}

impl Matrix {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.matrix_homeserver.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Matrix, String> {
        if cfg.matrix_homeserver.is_empty() {
            return Err("matrix: homeserver missing".into());
        }
        if cfg.matrix_token.is_empty() {
            return Err("matrix: access token missing".into());
        }
        if cfg.matrix_user_id.is_empty() {
            return Err("matrix: user_id missing".into());
        }
        if cfg.matrix_allowed.is_empty() {
            return Err("matrix: allowed_users is empty; refusing to serve everyone".into());
        }
        Ok(Matrix {
            homeserver: cfg.matrix_homeserver.trim_end_matches('/').to_string(),
            token: cfg.matrix_token.clone(),
            user_id: cfg.matrix_user_id.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.matrix_allowed),
        })
    }

    fn get(&self, path_q: &str, timeout: Duration) -> Result<Value, String> {
        let resp = ureq::get(&format!("{}{path_q}", self.homeserver))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(timeout)
            .call()
            .map_err(|e| redact(&e.to_string()))?;
        crate::net::read_json(resp, 8 << 20)
    }

    pub fn typing_path(&self, room: &str) -> String {
        format!(
            "/_matrix/client/v3/rooms/{}/typing/{}",
            urlencode(room),
            urlencode(&self.user_id)
        )
    }

    pub fn typing_payload(typing: bool) -> Value {
        if typing {
            json!({"typing": true, "timeout": 30000})
        } else {
            json!({"typing": false})
        }
    }

    fn typing(&self, room: &str, typing: bool) -> Result<(), String> {
        let url = format!("{}{}", self.homeserver, self.typing_path(room));
        ureq::request("PUT", &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(15))
            .send_string(&Self::typing_payload(typing).to_string())
            .map_err(|e| redact(&e.to_string()))?;
        Ok(())
    }

    pub fn working(&self, room: &str) -> crate::working::Working {
        let start_room = room.to_string();
        let stop_room = start_room.clone();
        let start_matrix = self.clone();
        let stop_matrix = self.clone();
        crate::working::Working::native(
            crate::working::MATRIX_INTERVAL,
            move || {
                let _ = start_matrix.typing(&start_room, true);
            },
            move || {
                let _ = stop_matrix.typing(&stop_room, false);
            },
        )
    }

    fn send(&self, room: &str, txn: u64, text: &str) -> Result<(), String> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/phx{txn}",
            self.homeserver,
            urlencode(room)
        );
        let body = json!({"msgtype": "m.text", "body": text}).to_string();
        ureq::request("PUT", &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(30))
            .send_string(&body)
            .map_err(|e| redact(&e.to_string()))?;
        Ok(())
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut since = String::new();
        let mut txn: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut backoff = 1u64;
        let mut warm = false;
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        loop {
            let path = if since.is_empty() {
                "/_matrix/client/v3/sync?timeout=0".to_string()
            } else {
                format!(
                    "/_matrix/client/v3/sync?timeout=30000&since={}",
                    urlencode(&since)
                )
            };
            match self.get(&path, Duration::from_secs(45)) {
                Ok(sync) => {
                    backoff = 1;
                    let next = sync["next_batch"].as_str().unwrap_or("").to_string();
                    if warm {
                        for (room, sender, body) in classify(&sync, &self.user_id) {
                            if !self.allowed.allows(&sender) {
                                continue;
                            }
                            let now = crate::scheduler::now_epoch();
                            let elapsed = last_seen
                                .insert(sender.clone(), now)
                                .map(|prev| now.saturating_sub(prev));
                            let body = inbound_text(&sender, &body, elapsed);
                            let mut working = self.working(&room);
                            let reply = handler(&sender, &body);
                            working.finish();
                            if !reply.is_empty() {
                                txn += 1;
                                if let Err(e) = self.send(&room, txn, &reply) {
                                    crate::log::error_with(
                                        "matrix",
                                        format!("send failed: {e}"),
                                        &crate::log::Fields::default().channel("matrix"),
                                    );
                                }
                            }
                        }
                    }
                    if !next.is_empty() {
                        since = next;
                        warm = true;
                    }
                }
                Err(e) => {
                    crate::log::warn_with(
                        "matrix",
                        format!("sync failed: {e}; retrying in {backoff}s"),
                        &crate::log::Fields::default().channel("matrix"),
                    );
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("@paulus:server", "hello", Some(60));
        assert!(out.starts_with("[Matrix @paulus:server +1m "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("@p:s", "/reset", None), "/reset");
    }

    fn sync_fixture() -> Value {
        json!({
            "next_batch": "s72595_4483_1934",
            "rooms": {"join": {"!room:example.org": {"timeline": {"events": [
                {"type": "m.room.message", "sender": "@paulus:example.org",
                 "content": {"msgtype": "m.text", "body": "status?"}},
                {"type": "m.room.message", "sender": "@phoenix:example.org",
                 "content": {"msgtype": "m.text", "body": "own echo"}},
                {"type": "m.room.member", "sender": "@paulus:example.org",
                 "content": {}},
                {"type": "m.room.message", "sender": "@paulus:example.org",
                 "content": {"msgtype": "m.image", "body": "pic"}}
            ]}}}}
        })
    }

    #[test]
    fn classify_extracts_text_from_others_only() {
        let msgs = classify(&sync_fixture(), "@phoenix:example.org");
        assert_eq!(
            msgs,
            vec![(
                "!room:example.org".to_string(),
                "@paulus:example.org".to_string(),
                "status?".to_string()
            )]
        );
    }

    #[test]
    fn classify_handles_empty_sync() {
        assert!(classify(&json!({}), "@x:y").is_empty());
    }

    #[test]
    fn typing_wire_shape_uses_room_and_user_ids() {
        let cfg = Config {
            matrix_homeserver: "https://m.example.org".into(),
            matrix_token: "tok".into(),
            matrix_user_id: "@phoenix:example.org".into(),
            matrix_allowed: vec!["@paulus:example.org".into()],
            ..Config::default()
        };
        let matrix = Matrix::new(&cfg).unwrap();
        assert_eq!(
            matrix.typing_path("!r:example.org"),
            "/_matrix/client/v3/rooms/%21r%3Aexample.org/typing/%40phoenix%3Aexample.org"
        );
        assert_eq!(
            Matrix::typing_payload(true),
            json!({"typing": true, "timeout": 30000})
        );
        assert_eq!(Matrix::typing_payload(false), json!({"typing": false}));
    }

    #[test]
    fn urlencode_room_ids() {
        assert_eq!(urlencode("!r:ex.org"), "%21r%3Aex.org");
    }

    #[test]
    fn new_fails_closed() {
        let mut cfg = Config {
            matrix_homeserver: "https://m.example.org".into(),
            matrix_token: "tok".into(),
            matrix_user_id: "@phoenix:example.org".into(),
            ..Config::default()
        };
        assert!(Matrix::new(&cfg).err().unwrap().contains("allowed_users"));
        cfg.matrix_allowed = vec!["@paulus:example.org".into()];
        assert!(Matrix::new(&cfg).is_ok());
    }
}
