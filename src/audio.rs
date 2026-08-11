use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::providers::key_ring;
use crate::security::redact;

const MAX_AUDIO: usize = 25 * 1024 * 1024;

pub fn multipart(boundary: &str, model: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 512);
    let push = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
    push(&mut out, &format!("--{boundary}\r\n"));
    push(
        &mut out,
        "Content-Disposition: form-data; name=\"model\"\r\n\r\n",
    );
    push(&mut out, &format!("{model}\r\n"));
    push(&mut out, &format!("--{boundary}\r\n"));
    push(
        &mut out,
        &format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"),
    );
    push(&mut out, "Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(bytes);
    push(&mut out, &format!("\r\n--{boundary}--\r\n"));
    out
}

pub fn base_url(cfg: &Config) -> String {
    let base = if !cfg.audio_base_url.is_empty() {
        cfg.audio_base_url.clone()
    } else if !cfg.base_url.is_empty() {
        cfg.base_url.clone()
    } else {
        "https://api.openai.com/v1".to_string()
    };
    base.trim_end_matches('/').to_string()
}

pub fn transcribe(cfg: &Config, bytes: &[u8], filename: &str) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("empty audio".into());
    }
    if bytes.len() > MAX_AUDIO {
        return Err(format!("audio too large: {} bytes", bytes.len()));
    }
    let boundary = format!(
        "phoenix{:x}",
        std::process::id() as u64 ^ bytes.len() as u64
    );
    let body = multipart(&boundary, &cfg.audio_model, filename, bytes);
    let url = format!("{}/audio/transcriptions", base_url(cfg));
    let key = key_ring(cfg).into_iter().next().unwrap_or_default();
    let mut req = ureq::post(&url).timeout(Duration::from_secs(120)).set(
        "Content-Type",
        &format!("multipart/form-data; boundary={boundary}"),
    );
    if !key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    let resp = req.send_bytes(&body).map_err(|e| redact(&e.to_string()))?;
    let text = resp.into_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
    match v["text"].as_str() {
        Some(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        _ => Err("transcription returned no text".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn multipart_contains_fields_and_bytes() {
        let body = multipart("BND", "whisper-1", "voice.ogg", b"AUDIO");
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("--BND\r\n"));
        assert!(s.contains("name=\"model\"\r\n\r\nwhisper-1"));
        assert!(s.contains("filename=\"voice.ogg\""));
        assert!(s.contains("AUDIO"));
        assert!(s.ends_with("--BND--\r\n"));
    }

    #[test]
    fn base_url_resolution_order() {
        let mut cfg = Config::default();
        assert_eq!(base_url(&cfg), "https://api.openai.com/v1");
        cfg.base_url = "http://prov/v1/".into();
        assert_eq!(base_url(&cfg), "http://prov/v1");
        cfg.audio_base_url = "http://audio/v1".into();
        assert_eq!(base_url(&cfg), "http://audio/v1");
    }

    #[test]
    fn size_guards() {
        let cfg = Config::default();
        assert!(transcribe(&cfg, b"", "a.ogg").is_err());
    }

    fn mock_server(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = match s.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let cl = text
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= pos + 4 + cl {
                            break;
                        }
                    }
                }
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn transcribe_parses_text() {
        let cfg = Config {
            audio_base_url: mock_server(r#"{"text": " hello phoenix "}"#),
            ..Config::default()
        };
        let out = transcribe(&cfg, b"OGGDATA", "v.ogg").unwrap();
        assert_eq!(out, "hello phoenix");
    }

    #[test]
    fn transcribe_error_on_empty_text() {
        let cfg = Config {
            audio_base_url: mock_server(r#"{"text": ""}"#),
            ..Config::default()
        };
        assert!(transcribe(&cfg, b"OGGDATA", "v.ogg").is_err());
    }
}
