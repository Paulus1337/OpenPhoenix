use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;
use crate::ws::{WsClient, WsMsg};

pub fn inbound_text(sender: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "Mattermost",
        sender,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

pub struct Mattermost {
    url: String,
    token: String,
    allowed: crate::allowlist::Allowlist,
}

pub fn classify(event: &Value) -> Option<(String, String, String)> {
    if event["event"].as_str() != Some("posted") {
        return None;
    }
    let sender = event["data"]["sender_name"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches('@')
        .to_string();
    let post_raw = event["data"]["post"].as_str()?;
    let post: Value = serde_json::from_str(post_raw).ok()?;
    let channel = post["channel_id"].as_str()?.to_string();
    let message = post["message"].as_str()?.to_string();
    if sender.is_empty() || message.is_empty() {
        return None;
    }
    Some((channel, sender, message))
}

impl Mattermost {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.mattermost_url.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Mattermost, String> {
        if cfg.mattermost_url.is_empty() {
            return Err("mattermost: url missing".into());
        }
        if cfg.mattermost_token.is_empty() {
            return Err("mattermost: token missing".into());
        }
        if cfg.mattermost_allowed.is_empty() {
            return Err("mattermost: allowed_users is empty; refusing to serve everyone".into());
        }
        Ok(Mattermost {
            url: cfg.mattermost_url.trim_end_matches('/').to_string(),
            token: cfg.mattermost_token.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.mattermost_allowed),
        })
    }

    fn me(&self) -> Result<String, String> {
        let resp = ureq::get(&format!("{}/api/v4/users/me", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(15))
            .call()
            .map_err(|e| redact(&e.to_string()))?;
        let v: Value = crate::net::read_json(resp, 1 << 20)?;
        Ok(v["username"].as_str().unwrap_or("").to_string())
    }

    fn post(&self, channel: &str, text: &str) -> Result<(), String> {
        let body = json!({"channel_id": channel, "message": text}).to_string();
        ureq::post(&format!("{}/api/v4/posts", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(30))
            .send_string(&body)
            .map_err(|e| redact(&e.to_string()))?;
        Ok(())
    }

    fn ws_url(&self) -> String {
        let ws = if let Some(rest) = self.url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("wss://{}", self.url)
        };
        format!("{ws}/api/v4/websocket")
    }

    fn session(
        &self,
        me: &str,
        handler: &mut dyn FnMut(&str, &str) -> String,
        last_seen: &mut HashMap<String, u64>,
    ) -> Result<(), String> {
        let mut ws = WsClient::connect(&self.ws_url())?;
        ws.set_read_timeout(Some(Duration::from_secs(90)))?;
        ws.send_text(
            &json!({
                "seq": 1,
                "action": "authentication_challenge",
                "data": {"token": self.token}
            })
            .to_string(),
        )?;
        loop {
            match ws.next() {
                Ok(Some(WsMsg::Text(text))) => {
                    let Ok(ev) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let Some((channel, sender, message)) = classify(&ev) else {
                        continue;
                    };
                    if sender == me || !self.allowed.allows(&sender) {
                        continue;
                    }
                    let now = crate::scheduler::now_epoch();
                    let elapsed = last_seen
                        .insert(channel.clone(), now)
                        .map(|prev| now.saturating_sub(prev));
                    let message = inbound_text(&sender, &message, elapsed);
                    let reply = handler(&sender, &message);
                    if !reply.is_empty() {
                        if let Err(e) = self.post(&channel, &reply) {
                            eprintln!("mattermost post failed: {e}");
                        }
                    }
                }
                Ok(Some(WsMsg::Close(_))) => return Err("mattermost: server closed ws".into()),
                Ok(Some(WsMsg::Binary(_))) | Ok(None) => {}
                Err(e) => {
                    if e.contains("timed out") || e.contains("WouldBlock") {
                        ws.send_ping()?;
                        continue;
                    }
                    return Err(format!("mattermost ws: {e}"));
                }
            }
        }
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut backoff = 1u64;
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        loop {
            let me = match self.me() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("mattermost auth failed: {e}; retrying in {backoff}s");
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                    continue;
                }
            };
            match self.session(&me, handler, &mut last_seen) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    eprintln!("{e}; reconnecting in {backoff}s");
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("paulus", "hello", Some(120));
        assert!(out.starts_with("[Mattermost paulus +2m "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("paulus", "/new", None), "/new");
    }

    #[test]
    fn classify_posted_event() {
        let ev = json!({
            "event": "posted",
            "data": {
                "sender_name": "@paulus",
                "post": "{\"channel_id\":\"c1\",\"message\":\"deploy?\",\"user_id\":\"u1\"}"
            }
        });
        assert_eq!(
            classify(&ev),
            Some((
                "c1".to_string(),
                "paulus".to_string(),
                "deploy?".to_string()
            ))
        );
    }

    #[test]
    fn classify_skips_other_events_and_bad_posts() {
        assert_eq!(classify(&json!({"event": "typing"})), None);
        let bad = json!({"event": "posted", "data": {"sender_name": "@x", "post": "not json"}});
        assert_eq!(classify(&bad), None);
        let empty = json!({"event": "posted", "data": {"sender_name": "",
            "post": "{\"channel_id\":\"c\",\"message\":\"m\"}"}});
        assert_eq!(classify(&empty), None);
    }

    #[test]
    fn ws_url_derivation() {
        let mm = Mattermost {
            url: "https://mm.example.org".into(),
            token: "t".into(),
            allowed: crate::allowlist::Allowlist::default(),
        };
        assert_eq!(mm.ws_url(), "wss://mm.example.org/api/v4/websocket");
    }

    #[test]
    fn new_fails_closed() {
        let mut cfg = Config {
            mattermost_url: "https://mm.example.org".into(),
            mattermost_token: "tok".into(),
            ..Config::default()
        };
        assert!(Mattermost::new(&cfg)
            .err()
            .unwrap()
            .contains("allowed_users"));
        cfg.mattermost_allowed = vec!["paulus".into()];
        assert!(Mattermost::new(&cfg).is_ok());
    }
}
