use std::collections::HashSet;
use std::io::Read;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::discord::chunks_of;
use crate::security::redact;
use crate::ws::{WsClient, WsMsg};

const MAX_MSG: usize = 3900;

pub struct Slack {
    app_token: String,
    bot_token: String,
    allowed: HashSet<String>,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    None,

    Disconnect,

    Message(String, String),
}

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
            let text = event["text"].as_str().unwrap_or("").to_string();
            if channel.is_empty() || text.is_empty() {
                return (ack, Action::None);
            }
            (ack, Action::Message(channel, text))
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
            allowed: cfg.slack_allowed.iter().cloned().collect(),
        })
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
        let mut buf = String::new();
        resp.into_reader()
            .take(1 << 20)
            .read_to_string(&mut buf)
            .map_err(|e| self.scrub(&e.to_string()))?;
        let v: Value = serde_json::from_str(&buf).map_err(|e| e.to_string())?;
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

    pub fn send(&self, channel: &str, text: &str) -> Result<(), String> {
        for part in chunks_of(text, MAX_MSG) {
            self.api_post(
                &self.bot_token,
                "chat.postMessage",
                &json!({ "channel": channel, "text": part }),
            )?;
        }
        Ok(())
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut backoff = 1u64;
        println!(
            "phoenix: serving slack ({} allowed channel(s))",
            self.allowed.len()
        );
        loop {
            match self.run_socket(handler) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    println!("slack socket error: {}, reconnecting", self.scrub(&e));
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    fn run_socket(&self, handler: &mut dyn FnMut(&str, &str) -> String) -> Result<(), String> {
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
                Action::Message(channel, text) => {
                    if !self.allowed.contains(&channel) {
                        continue;
                    }
                    let reply = handler(&channel, &text);
                    if !reply.is_empty() {
                        if let Err(e) = self.send(&channel, &reply) {
                            println!("slack send error: {}", self.scrub(&e));
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
        assert_eq!(action, Action::Message("C1".into(), "hi".into()));
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
