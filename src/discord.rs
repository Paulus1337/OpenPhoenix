use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;
use crate::ws::{WsClient, WsMsg};

const API: &str = "https://discord.com/api/v10";
const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

const INTENTS: u64 = (1 << 9) | (1 << 12) | (1 << 15);

const MAX_MSG: usize = 2000;

#[derive(Clone)]
pub struct Discord {
    token: String,
    allowed: crate::allowlist::Allowlist,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    None,

    Hello(u64),

    Beat,

    Ack,

    Resume,

    ReIdentify,

    Message(String, String, String),

    Ready(String, String),
}

pub fn inbound_text(author: &str, content: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(content) {
        return content.to_string();
    }
    crate::text::format_envelope(
        "Discord",
        author,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        content,
    )
}

pub fn classify(ev: &Value, seq: &mut Option<u64>) -> Action {
    if let Some(s) = ev["s"].as_u64() {
        *seq = Some(s);
    }
    match ev["op"].as_u64() {
        Some(10) => Action::Hello(ev["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250)),
        Some(1) => Action::Beat,
        Some(11) => Action::Ack,
        Some(7) => Action::Resume,
        Some(9) => {
            if ev["d"].as_bool().unwrap_or(false) {
                Action::Resume
            } else {
                Action::ReIdentify
            }
        }
        Some(0) => match ev["t"].as_str() {
            Some("READY") => Action::Ready(
                ev["d"]["session_id"].as_str().unwrap_or("").to_string(),
                ev["d"]["resume_gateway_url"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            ),
            Some("MESSAGE_CREATE") | Some("MESSAGE_UPDATE") => {
                let edited = ev["t"].as_str() == Some("MESSAGE_UPDATE");
                let d = &ev["d"];
                if d["author"]["bot"].as_bool().unwrap_or(false) {
                    return Action::None;
                }
                let channel = d["channel_id"].as_str().unwrap_or("").to_string();
                let author = d["author"]["global_name"]
                    .as_str()
                    .or_else(|| d["author"]["username"].as_str())
                    .or_else(|| d["author"]["id"].as_str())
                    .unwrap_or("")
                    .to_string();
                let content = d["content"].as_str().unwrap_or("").to_string();
                if channel.is_empty() || content.is_empty() {
                    return Action::None;
                }
                let content = if edited {
                    format!("(edited) {content}")
                } else {
                    content
                };
                Action::Message(channel, author, content)
            }
            _ => Action::None,
        },
        _ => Action::None,
    }
}

pub fn chunks_of(text: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.split_inclusive('\n') {
        if cur.len() + line.len() > limit {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let mut rest = line;
            while rest.len() > limit {
                let mut cut = limit;
                while !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push(rest[..cut].to_string());
                rest = &rest[cut..];
            }
            cur.push_str(rest);
        } else {
            cur.push_str(line);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

impl Discord {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.discord_token.is_empty() || !cfg.discord_allowed.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Discord, String> {
        if cfg.discord_token.is_empty() {
            return Err(
                "discord: token missing (set discord.token or PHOENIX_DISCORD_TOKEN)".into(),
            );
        }
        if cfg.discord_allowed.is_empty() {
            return Err("discord: allowed_channel_ids is empty; refusing to serve everyone".into());
        }
        Ok(Discord {
            token: cfg.discord_token.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.discord_allowed),
        })
    }

    fn scrub(&self, s: &str) -> String {
        redact(&s.replace(&self.token, "[redacted]"))
    }

    fn api_post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let payload = body.to_string();
        crate::net::call_retrying(
            || {
                ureq::post(&format!("{API}{path}"))
                    .set("Authorization", &format!("Bot {}", self.token))
                    .set("Content-Type", "application/json")
                    .timeout(Duration::from_secs(30))
                    .send_string(&payload)
                    .map_err(Box::new)
            },
            1 << 20,
            crate::net::sleep_secs,
        )
        .map_err(|e| self.scrub(&e))
    }

    pub fn send(&self, channel_id: &str, text: &str) -> Result<(), String> {
        for part in chunks_of(text, MAX_MSG) {
            self.api_post(
                &format!("/channels/{channel_id}/messages"),
                &json!({ "content": part }),
            )?;
        }
        Ok(())
    }

    pub fn typing(&self, channel_id: &str) {
        let _ = self.api_post(&format!("/channels/{channel_id}/typing"), &json!({}));
    }

    pub fn working(&self, channel_id: &str) -> crate::working::Working {
        let channel = channel_id.to_string();
        let discord = self.clone();
        crate::working::Working::native(
            crate::working::NATIVE_INTERVAL,
            move || discord.typing(&channel),
            || {},
        )
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut session: Option<(String, String)> = None;
        let mut seq: Option<u64> = None;
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        let mut backoff = 1u64;
        crate::log::info_with(
            "discord",
            format!("serving with {} allowed channel(s)", self.allowed.len()),
            &crate::log::Fields::default().channel("discord"),
        );
        loop {
            let resuming = session.is_some();
            match self.run_socket(handler, &mut session, &mut seq, &mut last_seen) {
                Ok(keep_session) => {
                    backoff = 1;
                    if !keep_session {
                        session = None;
                        seq = None;
                    }
                }
                Err(e) => {
                    crate::log::warn_with(
                        "discord",
                        format!("gateway failed: {}; reconnecting", self.scrub(&e)),
                        &crate::log::Fields::default().channel("discord"),
                    );
                    if !resuming {
                        session = None;
                        seq = None;
                    }
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    fn run_socket(
        &self,
        handler: &mut dyn FnMut(&str, &str) -> String,
        session: &mut Option<(String, String)>,
        seq: &mut Option<u64>,
        last_seen: &mut HashMap<String, u64>,
    ) -> Result<bool, String> {
        let url = match session {
            Some((_, resume_url)) if !resume_url.is_empty() => {
                format!("{resume_url}/?v=10&encoding=json")
            }
            _ => GATEWAY.to_string(),
        };
        let mut ws = WsClient::connect(&url)?;
        ws.set_read_timeout(Some(Duration::from_secs(1)))?;

        let mut interval = Duration::from_millis(41_250);
        let mut last_beat = Instant::now();
        let mut hello_seen = false;

        loop {
            if hello_seen && last_beat.elapsed() >= interval {
                let payload = json!({ "op": 1, "d": *seq }).to_string();
                ws.send_text(&payload)?;
                last_beat = Instant::now();
            }
            let msg = match ws.next()? {
                None => continue,
                Some(WsMsg::Close(code)) => {
                    if code == 4004 || (4010..=4014).contains(&code) {
                        return Err(format!("gateway closed with fatal code {code}"));
                    }
                    return Ok(true);
                }
                Some(WsMsg::Binary(_)) => continue,
                Some(WsMsg::Text(t)) => t,
            };
            let ev: Value = match serde_json::from_str(&msg) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match classify(&ev, seq) {
                Action::Hello(ms) => {
                    interval = Duration::from_millis(ms);
                    hello_seen = true;
                    last_beat = Instant::now();
                    let payload = match session {
                        Some((sid, _)) => json!({
                            "op": 6,
                            "d": { "token": self.token, "session_id": sid, "seq": *seq }
                        }),
                        None => json!({
                            "op": 2,
                            "d": {
                                "token": self.token,
                                "intents": INTENTS,
                                "properties": {
                                    "os": "linux",
                                    "browser": "openphoenix",
                                    "device": "openphoenix"
                                }
                            }
                        }),
                    };
                    ws.send_text(&payload.to_string())?;
                }
                Action::Beat => {
                    let payload = json!({ "op": 1, "d": *seq }).to_string();
                    ws.send_text(&payload)?;
                    last_beat = Instant::now();
                }
                Action::Ack => {}
                Action::Resume => return Ok(true),
                Action::ReIdentify => {
                    std::thread::sleep(Duration::from_secs(3));
                    return Ok(false);
                }
                Action::Ready(sid, resume_url) => {
                    *session = Some((sid, resume_url));
                    crate::log::info_with(
                        "discord",
                        "gateway ready",
                        &crate::log::Fields::default().channel("discord"),
                    );
                }
                Action::Message(channel, author, content) => {
                    if !self.allowed.allows(&channel) {
                        continue;
                    }
                    let now = crate::scheduler::now_epoch();
                    let elapsed = last_seen
                        .insert(channel.clone(), now)
                        .map(|prev| now.saturating_sub(prev));
                    let text = inbound_text(&author, &content, elapsed);
                    let mut working = self.working(&channel);
                    let reply = handler(&channel, &text);
                    working.finish();
                    if !reply.is_empty() {
                        if let Err(e) = self.send(&channel, &reply) {
                            crate::log::error_with(
                                "discord",
                                format!("send failed: {}", self.scrub(&e)),
                                &crate::log::Fields::default().channel("discord"),
                            );
                        }
                    }
                }
                Action::None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(token: &str, allowed: &[&str]) -> Config {
        Config {
            discord_token: token.to_string(),
            discord_allowed: allowed.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn fail_closed_on_partial_config() {
        assert!(Discord::new(&cfg("", &["1"])).is_err());
        assert!(Discord::new(&cfg("tok", &[])).is_err());
        assert!(Discord::new(&cfg("tok", &["1"])).is_ok());
    }

    #[test]
    fn wanted_when_any_field_set() {
        assert!(!Discord::wanted(&cfg("", &[])));
        assert!(Discord::wanted(&cfg("tok", &[])));
        assert!(Discord::wanted(&cfg("", &["1"])));
    }

    #[test]
    fn classify_hello_and_seq() {
        let mut seq = None;
        let ev: Value =
            serde_json::from_str(r#"{"op":10,"d":{"heartbeat_interval":5000}}"#).unwrap();
        assert_eq!(classify(&ev, &mut seq), Action::Hello(5000));
        assert_eq!(seq, None);
    }

    #[test]
    fn classify_dispatch_message() {
        let mut seq = None;
        let ev: Value = serde_json::from_str(
            r#"{"op":0,"s":42,"t":"MESSAGE_CREATE","d":{
                "channel_id":"c1","content":"hi phoenix",
                "author":{"id":"u1","bot":false}}}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ev, &mut seq),
            Action::Message("c1".into(), "u1".into(), "hi phoenix".into())
        );
        assert_eq!(seq, Some(42));
    }

    #[test]
    fn an_edit_is_reprocessed_with_an_edited_prefix() {
        let ev: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_UPDATE","d":{
                "channel_id":"c1","content":"fixed wording",
                "author":{"username":"pau","bot":false}}}"#,
        )
        .unwrap();
        let mut seq = None;
        match classify(&ev, &mut seq) {
            Action::Message(channel, author, content) => {
                assert_eq!(channel, "c1");
                assert_eq!(author, "pau");
                assert_eq!(content, "(edited) fixed wording");
            }
            other => panic!("expected a message, got {other:?}"),
        }
        let bot_edit: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_UPDATE","d":{
                "channel_id":"c1","content":"x",
                "author":{"username":"b","bot":true}}}"#,
        )
        .unwrap();
        assert!(matches!(classify(&bot_edit, &mut seq), Action::None));
        let embed_only: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_UPDATE","d":{
                "channel_id":"c1","content":"",
                "author":{"username":"pau","bot":false}}}"#,
        )
        .unwrap();
        assert!(
            matches!(classify(&embed_only, &mut seq), Action::None),
            "embed-refresh edits with no text must stay silent"
        );
    }

    #[test]
    fn classify_prefers_display_names_over_ids() {
        let mut seq = None;
        let ev: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_CREATE","d":{
                "channel_id":"c1","content":"hi",
                "author":{"id":"u1","username":"paulus","global_name":"Paulus","bot":false}}}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ev, &mut seq),
            Action::Message("c1".into(), "Paulus".into(), "hi".into())
        );
        let ev: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_CREATE","d":{
                "channel_id":"c1","content":"hi",
                "author":{"id":"u1","username":"paulus","bot":false}}}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ev, &mut seq),
            Action::Message("c1".into(), "paulus".into(), "hi".into())
        );
    }

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("Paulus", "hello there", Some(120));
        assert!(out.starts_with("[Discord Paulus +2m "), "{out}");
        assert!(out.ends_with("] hello there"), "{out}");
        let out = inbound_text("Paulus", "/reset", Some(120));
        assert_eq!(out, "/reset");
        let out = inbound_text("a[b]c", "hi", None);
        assert!(out.starts_with("[Discord a(b)c "), "{out}");
    }

    #[test]
    fn classify_ignores_bots_and_empty() {
        let mut seq = None;
        let bot: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_CREATE","d":{
                "channel_id":"c1","content":"beep",
                "author":{"id":"b1","bot":true}}}"#,
        )
        .unwrap();
        assert_eq!(classify(&bot, &mut seq), Action::None);
        let empty: Value = serde_json::from_str(
            r#"{"op":0,"t":"MESSAGE_CREATE","d":{
                "channel_id":"c1","content":"",
                "author":{"id":"u1","bot":false}}}"#,
        )
        .unwrap();
        assert_eq!(classify(&empty, &mut seq), Action::None);
    }

    #[test]
    fn classify_session_control() {
        let mut seq = None;
        let reconnect: Value = serde_json::from_str(r#"{"op":7,"d":null}"#).unwrap();
        assert_eq!(classify(&reconnect, &mut seq), Action::Resume);
        let dead: Value = serde_json::from_str(r#"{"op":9,"d":false}"#).unwrap();
        assert_eq!(classify(&dead, &mut seq), Action::ReIdentify);
        let resumable: Value = serde_json::from_str(r#"{"op":9,"d":true}"#).unwrap();
        assert_eq!(classify(&resumable, &mut seq), Action::Resume);
        let ready: Value = serde_json::from_str(
            r#"{"op":0,"t":"READY","d":{"session_id":"s9","resume_gateway_url":"wss://r"}}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ready, &mut seq),
            Action::Ready("s9".into(), "wss://r".into())
        );
    }

    #[test]
    fn chunking_respects_limit_and_lines() {
        let short = chunks_of("hello", 2000);
        assert_eq!(short, vec!["hello".to_string()]);
        let text = format!("{}\n{}", "a".repeat(1500), "b".repeat(1500));
        let out = chunks_of(&text, 2000);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.len() <= 2000));
        let monster = "x".repeat(4500);
        let out = chunks_of(&monster, 2000);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|c| c.len() <= 2000));
    }
}
