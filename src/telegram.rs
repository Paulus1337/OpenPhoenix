use std::io::Read;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::security::redact;

pub const CHUNK: usize = 4000;

pub type Attachments = Vec<(String, String)>;

pub type Transcriber<'a> = &'a dyn Fn(&[u8]) -> Result<String, String>;

#[derive(Clone)]
pub struct Telegram {
    token: String,
    pub allowed: Vec<String>,
    group_mention_only: bool,
}

pub fn gate_group(
    text: &str,
    chat_type: &str,
    username: &str,
    mention_only: bool,
) -> Option<String> {
    let is_group = matches!(chat_type, "group" | "supergroup");
    if !is_group || !mention_only {
        return Some(text.to_string());
    }
    if username.is_empty() {
        return None;
    }
    let tag = format!("@{}", username.to_lowercase());
    let lower = text.to_lowercase();
    let mut out = String::new();
    let mut cursor = 0usize;
    let mut matched = false;
    while let Some(rel) = lower[cursor..].find(&tag) {
        let start = cursor + rel;
        let end = start + tag.len();
        let boundary = lower[end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        if boundary {
            matched = true;
            out.push_str(&text[cursor..start]);
            out.push(' ');
        } else {
            out.push_str(&text[cursor..end]);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    if !matched {
        return None;
    }
    let stripped = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if stripped.is_empty() {
        return None;
    }
    Some(stripped)
}

pub fn valid_callback(data: &str) -> bool {
    for prefix in ["/approve ", "/deny "] {
        if let Some(rest) = data.strip_prefix(prefix) {
            return !rest.is_empty()
                && rest.len() <= 20
                && rest.bytes().all(|b| b.is_ascii_digit());
        }
    }
    false
}

pub fn pick_photo(msg: &serde_json::Value) -> Option<String> {
    Some(
        msg["photo"].as_array()?.last()?["file_id"]
            .as_str()?
            .to_string(),
    )
}

pub fn pick_document(msg: &serde_json::Value) -> Option<(String, String)> {
    let mime = msg["document"]["mime_type"].as_str().unwrap_or("");
    if mime == "application/pdf" || mime.starts_with("image/") {
        Some((
            msg["document"]["file_id"].as_str()?.to_string(),
            mime.to_string(),
        ))
    } else {
        None
    }
}

pub fn split_media(text: &str) -> (String, Vec<String>) {
    let mut media = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        match line.trim().strip_prefix("MEDIA:") {
            Some(p) if !p.trim().is_empty() => media.push(p.trim().to_string()),
            _ => kept.push(line),
        }
    }
    (kept.join("\n").trim().to_string(), media)
}

pub fn chunks(text: &str) -> Vec<String> {
    let text = if text.is_empty() { "(empty)" } else { text };
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(CHUNK)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

impl std::fmt::Debug for Telegram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Telegram")
            .field("token", &"[redacted]")
            .field("allowed", &self.allowed)
            .finish()
    }
}

impl Telegram {
    pub fn new(cfg: &Config) -> Result<Self, String> {
        if cfg.telegram_token.is_empty() {
            return Err("telegram token not set (PHOENIX_TELEGRAM_TOKEN)".into());
        }
        if cfg.telegram_allowed.is_empty() {
            return Err("telegram.allowed_chat_ids is empty, refusing to serve everyone".into());
        }
        Ok(Telegram {
            token: cfg.telegram_token.clone(),
            allowed: cfg.telegram_allowed.clone(),
            group_mention_only: cfg.tg_group_mention_only,
        })
    }

    fn scrub(&self, s: &str) -> String {
        let cleaned = if self.token.is_empty() {
            s.to_string()
        } else {
            s.replace(&self.token, "[redacted]")
        };
        redact(&cleaned)
    }

    fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        let resp = ureq::post(&url)
            .timeout(Duration::from_secs(90))
            .send_form(params)
            .map_err(|e| self.scrub(&e.to_string()))?;
        let text = resp.into_string().map_err(|e| self.scrub(&e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| self.scrub(&e.to_string()))
    }

    pub fn send(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let (body, media) = split_media(text);
        if !body.is_empty() {
            for chunk in chunks(&body) {
                self.call("sendMessage", &[("chat_id", chat_id), ("text", &chunk)])?;
            }
        }
        for path in media {
            if let Err(e) = self.send_media(chat_id, &path) {
                self.call(
                    "sendMessage",
                    &[
                        ("chat_id", chat_id),
                        ("text", &format!("(media delivery failed: {e})")),
                    ],
                )?;
            }
        }
        Ok(())
    }

    fn send_media(&self, chat_id: &str, path: &str) -> Result<(), String> {
        const MAX: usize = 25 * 1024 * 1024;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if bytes.len() > MAX {
            return Err("file over 25 MB".into());
        }
        let p = std::path::Path::new(path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (method, field) = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" => ("sendPhoto", "photo"),
            "ogg" | "oga" | "opus" => ("sendVoice", "voice"),
            "mp3" | "m4a" | "wav" | "flac" => ("sendAudio", "audio"),
            _ => ("sendDocument", "document"),
        };
        let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("file.bin");
        let boundary = format!(
            "phoenix{:x}",
            std::process::id() as u64 ^ bytes.len() as u64
        );
        let body = crate::media::multipart_fields(
            &boundary,
            &[("chat_id", chat_id)],
            field,
            filename,
            &bytes,
        );
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        ureq::post(&url)
            .timeout(Duration::from_secs(120))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
            .map_err(|e| self.scrub(&e.to_string()))?;
        Ok(())
    }

    pub fn send_with_buttons(
        &self,
        chat_id: &str,
        text: &str,
        buttons: &[Vec<(String, String)>],
    ) -> Result<(), String> {
        let keyboard: Vec<Vec<Value>> = buttons
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(label, data)| serde_json::json!({"text": label, "callback_data": data}))
                    .collect()
            })
            .collect();
        let markup = serde_json::json!({ "inline_keyboard": keyboard }).to_string();
        let clipped: String = text.chars().take(CHUNK).collect();
        self.call(
            "sendMessage",
            &[
                ("chat_id", chat_id),
                ("text", &clipped),
                ("reply_markup", &markup),
            ],
        )?;
        Ok(())
    }

    fn download(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let info = self.call("getFile", &[("file_id", file_id)])?;
        let path = info["result"]["file_path"]
            .as_str()
            .ok_or("getFile: no file_path")?;
        let url = format!("https://api.telegram.org/file/bot{}/{}", self.token, path);
        const MAX: u64 = 25 * 1024 * 1024;
        let resp = ureq::get(&url)
            .timeout(Duration::from_secs(120))
            .call()
            .map_err(|e| self.scrub(&e.to_string()))?;
        let mut buf = Vec::new();
        resp.into_reader()
            .take(MAX + 1)
            .read_to_end(&mut buf)
            .map_err(|e| self.scrub(&e.to_string()))?;
        if buf.len() as u64 > MAX {
            return Err("file too large (over 25 MB)".into());
        }
        Ok(buf)
    }

    pub fn serve(
        &self,
        handler: &mut dyn FnMut(&str, &str, Attachments) -> String,
        transcribe: Option<Transcriber>,
    ) -> Result<(), String> {
        let me = self.call("getMe", &[])?;
        let username = me["result"]["username"].as_str().unwrap_or("?").to_string();
        println!(
            "phoenix: serving telegram as @{username} ({} allowed chat(s))",
            self.allowed.len()
        );
        let mut offset: i64 = 0;
        loop {
            let offset_s = offset.to_string();
            let updates = match self.call(
                "getUpdates",
                &[("offset", offset_s.as_str()), ("timeout", "50")],
            ) {
                Ok(u) => u,
                Err(e) => {
                    let msg: String = redact(&e).chars().take(200).collect();
                    println!("telegram poll error: {msg}, retrying");
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let empty = Vec::new();
            for upd in updates["result"].as_array().unwrap_or(&empty) {
                if let Some(id) = upd["update_id"].as_i64() {
                    offset = id + 1;
                }

                let cq = &upd["callback_query"];
                if cq.is_object() {
                    let chat_id = match cq["message"]["chat"]["id"].as_i64() {
                        Some(id) => id.to_string(),
                        None => continue,
                    };
                    let data = cq["data"].as_str().unwrap_or("");
                    if let Some(cq_id) = cq["id"].as_str() {
                        let _ = self.call("answerCallbackQuery", &[("callback_query_id", cq_id)]);
                    }
                    if !valid_callback(data) || !self.allowed.contains(&chat_id) {
                        continue;
                    }
                    let reply = handler(&chat_id, data, Vec::new());
                    if !reply.is_empty() {
                        let _ = self.send(&chat_id, &reply);
                    }
                    continue;
                }

                let msg = &upd["message"];
                let chat_id = match msg["chat"]["id"].as_i64() {
                    Some(id) => id.to_string(),
                    None => continue,
                };
                if !self.allowed.contains(&chat_id) {
                    continue;
                }
                let chat_type = msg["chat"]["type"].as_str().unwrap_or("private");

                let is_group = matches!(chat_type, "group" | "supergroup");
                let mut voice_text = String::new();
                if (msg["voice"].is_object() || msg["audio"].is_object())
                    && !(is_group && self.group_mention_only)
                {
                    let file_id = msg["voice"]["file_id"]
                        .as_str()
                        .or_else(|| msg["audio"]["file_id"].as_str())
                        .unwrap_or("");
                    if let (Some(tr), false) = (transcribe, file_id.is_empty()) {
                        let _ = self.call(
                            "sendChatAction",
                            &[("chat_id", chat_id.as_str()), ("action", "typing")],
                        );
                        match self.download(file_id).and_then(|bytes| tr(&bytes)) {
                            Ok(t) => voice_text = t,
                            Err(e) => {
                                let err: String = redact(&e).chars().take(200).collect();
                                let _ = self
                                    .send(&chat_id, &format!("voice transcription failed: {err}"));
                                continue;
                            }
                        }
                    }
                }

                let attachment = pick_photo(msg)
                    .map(|id| (id, "image/jpeg".to_string()))
                    .or_else(|| pick_document(msg));
                let mut raw = if voice_text.is_empty() {
                    msg["text"]
                        .as_str()
                        .or_else(|| msg["caption"].as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    voice_text
                };
                if raw.is_empty() {
                    if attachment.is_some() {
                        raw = "[attachment]".to_string();
                    } else {
                        continue;
                    }
                }
                let text = match gate_group(&raw, chat_type, &username, self.group_mention_only) {
                    Some(t) => t,
                    None => continue,
                };
                let _ = self.call(
                    "sendChatAction",
                    &[("chat_id", chat_id.as_str()), ("action", "typing")],
                );

                let mut media: Vec<(String, String)> = Vec::new();
                if let Some((file_id, mime)) = attachment {
                    match self.download(&file_id) {
                        Ok(bytes) => media.push((mime, crate::media::b64_encode(&bytes))),
                        Err(e) => {
                            let err: String = redact(&e).chars().take(200).collect();
                            let _ =
                                self.send(&chat_id, &format!("attachment download failed: {err}"));
                            continue;
                        }
                    }
                }
                let reply = handler(&chat_id, &text, media);
                if reply.is_empty() {
                    continue;
                }
                if let Err(e) = self.send(&chat_id, &reply) {
                    let err: String = redact(&e).chars().take(300).collect();
                    let _ = self.send(&chat_id, &format!("error: {err}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(token: &str, allowed: &[&str]) -> Config {
        Config {
            telegram_token: token.into(),
            telegram_allowed: allowed.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn refuses_without_token() {
        assert!(Telegram::new(&cfg("", &["1"])).is_err());
    }

    #[test]
    fn refuses_empty_allowlist_fail_closed() {
        let err = Telegram::new(&cfg("123:abc", &[])).unwrap_err();
        assert!(err.contains("refusing"), "got: {err}");
    }

    #[test]
    fn accepts_valid_config() {
        let tg = Telegram::new(&cfg("123:abc", &["42"])).unwrap();
        assert_eq!(tg.allowed, vec!["42".to_string()]);
    }

    #[test]
    fn group_gating() {
        assert_eq!(
            gate_group("hi", "private", "bot", true).as_deref(),
            Some("hi")
        );

        assert!(gate_group("hi", "group", "bot", true).is_none());
        assert!(gate_group("hi", "supergroup", "bot", true).is_none());

        assert_eq!(
            gate_group("@bot do the thing", "group", "bot", true).as_deref(),
            Some("do the thing")
        );
        assert_eq!(
            gate_group("do @bot the thing", "supergroup", "bot", true).as_deref(),
            Some("do the thing")
        );

        assert_eq!(
            gate_group("hi", "group", "bot", false).as_deref(),
            Some("hi")
        );

        assert!(gate_group("@bot", "group", "bot", true).is_none());

        assert!(gate_group("@bot hi", "group", "", true).is_none());
    }

    #[test]
    fn chunking_at_4000_chars() {
        let text = "x".repeat(CHUNK * 2 + 5);
        let parts = chunks(&text);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].chars().count(), CHUNK);
        assert_eq!(parts[2].chars().count(), 5);
        assert_eq!(chunks(""), vec!["(empty)".to_string()]);

        let uni = "ü".repeat(CHUNK + 1);
        let parts = chunks(&uni);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], "ü");
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_photo_takes_largest_size() {
        let msg = json!({"photo": [
            {"file_id": "small", "width": 90},
            {"file_id": "big", "width": 1280}
        ]});
        assert_eq!(pick_photo(&msg).unwrap(), "big");
        assert!(pick_photo(&json!({"text": "no photo"})).is_none());
    }

    #[test]
    fn pick_document_filters_by_mime() {
        let pdf = json!({"document": {"file_id": "d1", "mime_type": "application/pdf"}});
        assert_eq!(
            pick_document(&pdf).unwrap(),
            ("d1".into(), "application/pdf".into())
        );
        let img = json!({"document": {"file_id": "d2", "mime_type": "image/png"}});
        assert!(pick_document(&img).is_some());
        let zip = json!({"document": {"file_id": "d3", "mime_type": "application/zip"}});
        assert!(pick_document(&zip).is_none());
    }

    #[test]
    fn split_media_separates_paths_from_text() {
        let (body, media) = split_media("look:\nMEDIA:/tmp/a.png\ndone\nMEDIA: /tmp/b.mp3");
        assert_eq!(body, "look:\ndone");
        assert_eq!(
            media,
            vec!["/tmp/a.png".to_string(), "/tmp/b.mp3".to_string()]
        );
        let (body, media) = split_media("plain text");
        assert_eq!(body, "plain text");
        assert!(media.is_empty());
    }
}
