use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::discord::chunks_of;
use crate::security::redact;
use crate::ws::{WsClient, WsMsg};

const MAX_MSG: usize = 3900;

#[derive(Clone)]
pub struct Slack {
    app_token: String,
    bot_token: String,
    allowed: crate::allowlist::Allowlist,
    names: std::cell::RefCell<HashMap<String, String>>,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    None,

    Disconnect,

    Message(String, String, Option<String>, String),
}

pub fn pick_name(user: &Value, fallback: &str) -> String {
    for v in [
        &user["profile"]["display_name"],
        &user["real_name"],
        &user["profile"]["real_name"],
        &user["name"],
    ] {
        if let Some(s) = v.as_str() {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    fallback.to_string()
}

pub fn inbound_text(sender: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "Slack",
        sender,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

pub type OnSlackMessage<'a> = dyn FnMut(&str, Option<&str>, &str) -> String + 'a;

pub fn classify(ev: &Value) -> (Option<String>, Action) {
    match ev["type"].as_str() {
        Some("disconnect") => (None, Action::Disconnect),
        Some("events_api") => {
            let ack = ev["envelope_id"].as_str().map(str::to_string);
            let event = &ev["payload"]["event"];
            let is_plain_user_msg = event["type"].as_str() == Some("message")
                && event["bot_id"].is_null()
                && event["subtype"].is_null();
            if !is_plain_user_msg {
                return (ack, Action::None);
            }
            let channel = event["channel"].as_str().unwrap_or("").to_string();
            let sender = event["user"].as_str().unwrap_or("").to_string();
            let text = event["text"].as_str().unwrap_or("").to_string();
            let thread = event["thread_ts"].as_str().map(str::to_string);
            if channel.is_empty() || text.is_empty() {
                return (ack, Action::None);
            }
            (ack, Action::Message(channel, sender, thread, text))
        }
        _ => (None, Action::None),
    }
}

impl Slack {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.slack_app_token.is_empty()
            || !cfg.slack_bot_token.is_empty()
            || !cfg.slack_allowed.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Slack, String> {
        if cfg.slack_app_token.is_empty() {
            return Err(
                "slack: app_token missing (set slack.app_token or PHOENIX_SLACK_APP_TOKEN)".into(),
            );
        }
        if cfg.slack_bot_token.is_empty() {
            return Err(
                "slack: bot_token missing (set slack.bot_token or PHOENIX_SLACK_BOT_TOKEN)".into(),
            );
        }
        if cfg.slack_allowed.is_empty() {
            return Err("slack: allowed_channel_ids is empty; refusing to serve everyone".into());
        }
        Ok(Slack {
            app_token: cfg.slack_app_token.clone(),
            bot_token: cfg.slack_bot_token.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.slack_allowed),
            names: std::cell::RefCell::new(HashMap::new()),
        })
    }

    fn display_name(&self, user: &str) -> String {
        if user.is_empty() {
            return String::new();
        }
        if let Some(hit) = self.names.borrow().get(user) {
            return hit.clone();
        }
        let label = match self.api_post(&self.bot_token, "users.info", &json!({ "user": user })) {
            Ok(v) => pick_name(&v["user"], user),
            Err(_) => user.to_string(),
        };
        self.names
            .borrow_mut()
            .insert(user.to_string(), label.clone());
        label
    }

    fn scrub(&self, s: &str) -> String {
        redact(
            &s.replace(&self.app_token, "[redacted]")
                .replace(&self.bot_token, "[redacted]"),
        )
    }

    fn api_post(&self, token: &str, method: &str, body: &Value) -> Result<Value, String> {
        let resp = ureq::post(&format!("https://slack.com/api/{method}"))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json; charset=utf-8")
            .timeout(Duration::from_secs(30))
            .send_string(&body.to_string())
            .map_err(|e| self.scrub(&e.to_string()))?;
        let v: Value = crate::net::read_json(resp, 1 << 20).map_err(|e| self.scrub(&e))?;
        if !v["ok"].as_bool().unwrap_or(false) {
            return Err(format!(
                "slack {method}: {}",
                v["error"].as_str().unwrap_or("unknown error")
            ));
        }
        Ok(v)
    }

    fn socket_url(&self) -> Result<String, String> {
        let v = self.api_post(&self.app_token, "apps.connections.open", &json!({}))?;
        v["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "apps.connections.open: no url".into())
    }

    pub fn send_in(&self, channel: &str, thread: Option<&str>, text: &str) -> Result<(), String> {
        for part in chunks_of(text, MAX_MSG) {
            let mut body = json!({ "channel": channel, "text": part });
            if let Some(ts) = thread {
                body["thread_ts"] = json!(ts);
            }
            self.api_post(&self.bot_token, "chat.postMessage", &body)?;
        }
        Ok(())
    }

    pub fn serve(&self, handler: &mut OnSlackMessage<'_>) {
        let mut backoff = 1u64;
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        crate::log::info_with(
            "slack",
            format!("serving with {} allowed channel(s)", self.allowed.len()),
            &crate::log::Fields::default().channel("slack"),
        );
        loop {
            match self.run_socket(handler, &mut last_seen) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    crate::log::warn_with(
                        "slack",
                        format!("socket failed: {}; reconnecting", self.scrub(&e)),
                        &crate::log::Fields::default().channel("slack"),
                    );
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    fn run_socket(
        &self,
        handler: &mut OnSlackMessage<'_>,
        last_seen: &mut HashMap<String, u64>,
    ) -> Result<(), String> {
        let url = self.socket_url()?;
        let mut ws = WsClient::connect(&url)?;

        ws.set_read_timeout(Some(Duration::from_secs(30)))?;
        loop {
            let msg = match ws.next()? {
                None => {
                    ws.send_ping()?;
                    continue;
                }
                Some(WsMsg::Close(_)) => return Ok(()),
                Some(WsMsg::Binary(_)) => continue,
                Some(WsMsg::Text(t)) => t,
            };
            let ev: Value = match serde_json::from_str(&msg) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (ack, action) = classify(&ev);
            if let Some(id) = ack {
                ws.send_text(&json!({ "envelope_id": id }).to_string())?;
            }
            match action {
                Action::Disconnect => {
                    let _ = ws.send_close(1000);
                    return Ok(());
                }
                Action::Message(channel, sender, thread, text) => {
                    if !self.allowed.allows(&channel) {
                        continue;
                    }
                    let now = crate::scheduler::now_epoch();
                    let elapsed = last_seen
                        .insert(channel.clone(), now)
                        .map(|prev| now.saturating_sub(prev));
                    let text = inbound_text(&self.display_name(&sender), &text, elapsed);
                    let working_slack = self.clone();
                    let working_channel = channel.clone();
                    let working_thread = thread.clone();
                    let mut working = crate::working::Working::delayed(move || {
                        if let Err(e) = working_slack.send_in(
                            &working_channel,
                            working_thread.as_deref(),
                            crate::working::FALLBACK_NOTICE,
                        ) {
                            crate::log::warn_with(
                                "slack",
                                format!("working notice failed: {}", working_slack.scrub(&e)),
                                &crate::log::Fields::default().channel("slack"),
                            );
                        }
                    });
                    let reply = handler(&channel, thread.as_deref(), &text);
                    working.finish();
                    if !reply.is_empty() {
                        if let Err(e) = self.send_in(&channel, thread.as_deref(), &reply) {
                            crate::log::error_with(
                                "slack",
                                format!("send failed: {}", self.scrub(&e)),
                                &crate::log::Fields::default().channel("slack"),
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

    fn cfg(app: &str, bot: &str, allowed: &[&str]) -> Config {
        Config {
            slack_app_token: app.to_string(),
            slack_bot_token: bot.to_string(),
            slack_allowed: allowed.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn fail_closed_on_partial_config() {
        assert!(Slack::new(&cfg("", "b", &["C1"])).is_err());
        assert!(Slack::new(&cfg("a", "", &["C1"])).is_err());
        assert!(Slack::new(&cfg("a", "b", &[])).is_err());
        assert!(Slack::new(&cfg("a", "b", &["C1"])).is_ok());
    }

    #[test]
    fn classify_events_and_acks() {
        let ev: Value = serde_json::from_str(
            r#"{"type":"events_api","envelope_id":"e1","payload":{"event":{
                "type":"message","channel":"C1","user":"U1","text":"hi"}}}"#,
        )
        .unwrap();
        let (ack, action) = classify(&ev);
        assert_eq!(ack.as_deref(), Some("e1"));
        assert_eq!(
            action,
            Action::Message("C1".into(), "U1".into(), None, "hi".into())
        );
    }

    #[test]
    fn pick_name_prefers_profile_then_falls_back() {
        let v: Value =
            serde_json::from_str(r#"{"real_name":"Paul Us","profile":{"display_name":"Paulus"}}"#)
                .unwrap();
        assert_eq!(pick_name(&v, "U1"), "Paulus");
        let v: Value = serde_json::from_str(r#"{"real_name":"Paul Us","profile":{}}"#).unwrap();
        assert_eq!(pick_name(&v, "U1"), "Paul Us");
        let v: Value = serde_json::from_str(r#"{"profile":{"display_name":"  "}}"#).unwrap();
        assert_eq!(pick_name(&v, "U1"), "U1");
    }

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("Paulus", "hello", Some(60));
        assert!(out.starts_with("[Slack Paulus +1m "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("Paulus", "/reset", None), "/reset");
    }

    #[test]
    fn classify_thread_messages_carry_thread_ts() {
        let ev: Value = serde_json::from_str(
            r#"{"type":"events_api","envelope_id":"e9","payload":{"event":{
                "type":"message","channel":"C1","user":"U1","text":"in thread",
                "ts":"1722.002","thread_ts":"1722.001"}}}"#,
        )
        .unwrap();
        let (ack, action) = classify(&ev);
        assert_eq!(ack.as_deref(), Some("e9"));
        assert_eq!(
            action,
            Action::Message(
                "C1".into(),
                "U1".into(),
                Some("1722.001".into()),
                "in thread".into()
            )
        );
    }

    #[test]
    fn classify_acks_ignored_events_too() {
        let ev: Value = serde_json::from_str(
            r#"{"type":"events_api","envelope_id":"e2","payload":{"event":{
                "type":"message","channel":"C1","bot_id":"B9","text":"echo"}}}"#,
        )
        .unwrap();
        let (ack, action) = classify(&ev);
        assert_eq!(ack.as_deref(), Some("e2"));
        assert_eq!(action, Action::None);

        let ev: Value = serde_json::from_str(
            r#"{"type":"events_api","envelope_id":"e3","payload":{"event":{
                "type":"message","subtype":"message_changed","channel":"C1","text":"x"}}}"#,
        )
        .unwrap();
        let (ack, action) = classify(&ev);
        assert_eq!(ack.as_deref(), Some("e3"));
        assert_eq!(action, Action::None);
    }

    #[test]
    fn classify_control_frames() {
        let hello: Value = serde_json::from_str(r#"{"type":"hello"}"#).unwrap();
        assert_eq!(classify(&hello), (None, Action::None));
        let bye: Value =
            serde_json::from_str(r#"{"type":"disconnect","reason":"refresh"}"#).unwrap();
        assert_eq!(classify(&bye), (None, Action::Disconnect));
    }
}
