use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;

pub const CHUNK: usize = 4096;
const GRAPH: &str = "https://graph.facebook.com/v21.0";
const MAX_BODY: usize = 262_144;

#[derive(Clone)]
pub struct WhatsApp {
    token: String,
    phone_id: String,
    pub verify_token: String,
    pub allowed: crate::allowlist::Allowlist,
}

impl std::fmt::Debug for WhatsApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhatsApp")
            .field("token", &"[redacted]")
            .field("phone_id", &self.phone_id)
            .field("allowed", &self.allowed)
            .finish()
    }
}

impl WhatsApp {
    pub fn new(cfg: &Config) -> Result<Self, String> {
        if cfg.wa_token.is_empty() {
            return Err("whatsapp token not set (PHOENIX_WHATSAPP_TOKEN)".into());
        }
        if cfg.wa_phone_id.is_empty() {
            return Err("whatsapp.phone_id not set".into());
        }
        if cfg.wa_verify_token.is_empty() {
            return Err("whatsapp.verify_token not set".into());
        }
        if cfg.wa_allowed.is_empty() {
            return Err("whatsapp.allowed_numbers is empty, refusing to serve everyone".into());
        }
        Ok(WhatsApp {
            token: cfg.wa_token.clone(),
            phone_id: cfg.wa_phone_id.clone(),
            verify_token: cfg.wa_verify_token.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.wa_allowed),
        })
    }

    pub fn wanted(cfg: &Config) -> bool {
        !cfg.wa_token.is_empty()
            || !cfg.wa_phone_id.is_empty()
            || !cfg.wa_verify_token.is_empty()
            || !cfg.wa_allowed.is_empty()
    }

    pub fn send(&self, to: &str, text: &str) -> Result<(), String> {
        for body in chunks(text) {
            let payload = json!({
                "messaging_product": "whatsapp",
                "to": to,
                "type": "text",
                "text": {"body": body}
            });
            let url = format!("{GRAPH}/{}/messages", self.phone_id);
            ureq::post(&url)
                .timeout(Duration::from_secs(30))
                .set("Authorization", &format!("Bearer {}", self.token))
                .set("Content-Type", "application/json")
                .send_string(&payload.to_string())
                .map_err(|e| redact(&e.to_string()))?;
        }
        Ok(())
    }

    pub fn serve(&self, listener: TcpListener, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut last_seen: HashMap<String, u64> = HashMap::new();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if let Err(e) = self.handle(stream, handler, &mut last_seen) {
                let msg = crate::security::one_line(&redact(&e.to_string()), 200);
                eprintln!("whatsapp webhook error: {msg}");
            }
        }
    }

    fn handle(
        &self,
        mut stream: TcpStream,
        handler: &mut dyn FnMut(&str, &str) -> String,
        last_seen: &mut HashMap<String, u64>,
    ) -> std::io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut content_len = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let line_t = line.trim_end();
            if line_t.is_empty() {
                break;
            }
            if let Some((k, v)) = line_t.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let query = target.split_once('?').map(|x| x.1).unwrap_or("");
        match method.as_str() {
            "GET" => match verify_response(query, &self.verify_token) {
                Some(challenge) => plain(&mut stream, 200, "OK", &challenge),
                None => plain(&mut stream, 403, "Forbidden", "verification failed"),
            },
            "POST" => {
                if content_len == 0 || content_len > MAX_BODY {
                    return plain(&mut stream, 400, "Bad Request", "bad body");
                }
                let mut buf = vec![0u8; content_len];
                reader.read_exact(&mut buf)?;

                plain(&mut stream, 200, "OK", "ok")?;
                let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
                for (from, label, text) in parse_events(&v) {
                    if !self.allowed.allows(&from) {
                        continue;
                    }
                    let now = crate::scheduler::now_epoch();
                    let elapsed = last_seen
                        .insert(from.clone(), now)
                        .map(|prev| now.saturating_sub(prev));
                    let text = inbound_text(&label, &text, elapsed);
                    let reply = handler(&from, &text);
                    if let Err(e) = self.send(&from, &reply) {
                        eprintln!(
                            "whatsapp send error: {}",
                            redact(&e).chars().take(200).collect::<String>()
                        );
                    }
                }
                Ok(())
            }
            _ => plain(&mut stream, 405, "Method Not Allowed", "no"),
        }
    }
}

fn plain(stream: &mut TcpStream, code: u16, reason: &str, body: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn query_get<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

pub fn verify_response(query: &str, verify_token: &str) -> Option<String> {
    if verify_token.is_empty() {
        return None;
    }
    if query_get(query, "hub.mode") != Some("subscribe") {
        return None;
    }
    if query_get(query, "hub.verify_token") != Some(verify_token) {
        return None;
    }
    query_get(query, "hub.challenge").map(str::to_string)
}

pub fn inbound_text(label: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "WhatsApp",
        label,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

pub fn parse_events(v: &Value) -> Vec<(String, String, String)> {
    let mut stamped: Vec<(u64, String, String, String)> = Vec::new();
    let empty = Vec::new();
    for entry in v["entry"].as_array().unwrap_or(&empty) {
        for change in entry["changes"].as_array().unwrap_or(&empty) {
            let contacts = change["value"]["contacts"].as_array().unwrap_or(&empty);
            for msg in change["value"]["messages"].as_array().unwrap_or(&empty) {
                if msg["type"].as_str() != Some("text") {
                    continue;
                }
                let from = msg["from"].as_str().unwrap_or("").to_string();
                let body = msg["text"]["body"].as_str().unwrap_or("").to_string();
                if from.is_empty() || body.is_empty() {
                    continue;
                }
                let ts = msg["timestamp"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| msg["timestamp"].as_u64())
                    .unwrap_or(u64::MAX);
                let label = contacts
                    .iter()
                    .find(|c| c["wa_id"].as_str() == Some(from.as_str()))
                    .or_else(|| contacts.first())
                    .and_then(|c| c["profile"]["name"].as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(from.as_str())
                    .to_string();
                stamped.push((ts, from, label, body));
            }
        }
    }
    stamped.sort_by_key(|(ts, ..)| *ts);
    stamped
        .into_iter()
        .map(|(_, from, label, body)| (from, label, body))
        .collect()
}

pub fn chunks(text: &str) -> Vec<String> {
    let text = if text.is_empty() { "(empty)" } else { text };
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(CHUNK)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_full() -> Config {
        Config {
            wa_token: "t".into(),
            wa_phone_id: "123".into(),
            wa_verify_token: "v".into(),
            wa_allowed: vec!["4915551234567".into()],
            ..Config::default()
        }
    }

    #[test]
    fn fail_closed_construction() {
        assert!(WhatsApp::new(&Config::default()).is_err());
        let mut c = cfg_full();
        c.wa_allowed.clear();
        let err = WhatsApp::new(&c).unwrap_err();
        assert!(err.contains("refusing to serve everyone"));
        assert!(WhatsApp::new(&cfg_full()).is_ok());
    }

    #[test]
    fn wanted_detects_partial_config() {
        assert!(!WhatsApp::wanted(&Config::default()));
        let c = Config {
            wa_phone_id: "1".into(),
            ..Config::default()
        };
        assert!(WhatsApp::wanted(&c));
    }

    #[test]
    fn verification_challenge() {
        let q = "hub.mode=subscribe&hub.challenge=42&hub.verify_token=v";
        assert_eq!(verify_response(q, "v").as_deref(), Some("42"));
        assert!(verify_response(q, "wrong").is_none());
        assert!(verify_response("hub.mode=unsubscribe&hub.verify_token=v", "v").is_none());
        assert!(verify_response(q, "").is_none());
    }

    #[test]
    fn event_parsing() {
        let payload = serde_json::json!({
            "entry": [{"changes": [{"value": {"messages": [
                {"type": "text", "from": "4915551234567",
                 "text": {"body": "hello phoenix"}},
                {"type": "image", "from": "4915551234567"},
                {"type": "text", "from": "", "text": {"body": "x"}}
            ]}}]}]
        });
        let events = parse_events(&payload);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            (
                "4915551234567".into(),
                "4915551234567".into(),
                "hello phoenix".into()
            )
        );
        assert!(parse_events(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn events_come_back_in_message_timestamp_order() {
        let payload = serde_json::json!({
            "entry": [{"changes": [{"value": {"messages": [
                {"type": "text", "from": "4915551234567",
                 "timestamp": "1722500002", "text": {"body": "second"}},
                {"type": "text", "from": "4915551234567",
                 "timestamp": "1722500001", "text": {"body": "first"}}
            ]}}]}]
        });
        let events = parse_events(&payload);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].2, "first",
            "delivery order must not beat send order"
        );
        assert_eq!(events[1].2, "second");

        let unstamped = serde_json::json!({
            "entry": [{"changes": [{"value": {"messages": [
                {"type": "text", "from": "1", "text": {"body": "a"}},
                {"type": "text", "from": "1", "text": {"body": "b"}}
            ]}}]}]
        });
        let events = parse_events(&unstamped);
        assert_eq!(
            (events[0].2.as_str(), events[1].2.as_str()),
            ("a", "b"),
            "missing stamps keep arrival order"
        );
    }

    #[test]
    fn event_parsing_carries_the_contact_name() {
        let payload = serde_json::json!({
            "entry": [{"changes": [{"value": {
                "contacts": [{"wa_id": "4915551234567", "profile": {"name": "Paulus"}}],
                "messages": [
                    {"type": "text", "from": "4915551234567",
                     "text": {"body": "hi"}}
            ]}}]}]
        });
        let events = parse_events(&payload);
        assert_eq!(
            events[0],
            ("4915551234567".into(), "Paulus".into(), "hi".into())
        );
    }

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("Paulus", "hello", Some(30));
        assert!(out.starts_with("[WhatsApp Paulus +30s "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("Paulus", "/reset", None), "/reset");
    }

    #[test]
    fn chunking() {
        assert_eq!(chunks(""), vec!["(empty)".to_string()]);
        let big = "x".repeat(CHUNK + 5);
        let c = chunks(&big);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].chars().count(), CHUNK);
        assert_eq!(c[1], "xxxxx");
    }

    #[test]
    fn debug_never_leaks_token() {
        let wa = WhatsApp::new(&cfg_full()).unwrap();
        let dbg = format!("{wa:?}");
        assert!(!dbg.contains("\"t\""));
        assert!(dbg.contains("[redacted]"));
    }
}
