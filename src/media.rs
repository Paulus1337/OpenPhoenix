use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::providers::key_ring;
use crate::security::redact;

pub const MAX_MEDIA: usize = 25 * 1024 * 1024;

pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    None
}

pub fn verified_mime(declared: &str, bytes: &[u8]) -> Result<String, String> {
    match sniff_mime(bytes) {
        Some(actual) => Ok(actual.to_string()),
        None if declared == "application/pdf" || declared.starts_with("image/") => Err(format!(
            "[attachment refused: it claims to be {declared} but its contents are not a \
recognised image or PDF]"
        )),
        None => Err(format!("[attachment refused: unsupported type {declared}]")),
    }
}

pub fn image_too_large(mime: &str, bytes: usize) -> Option<String> {
    if !mime.starts_with("image/") || bytes <= MAX_IMAGE_BYTES {
        return None;
    }
    Some(format!(
        "[image dropped: {} KB exceeds the {} KB model limit; ask for a smaller version]",
        bytes / 1024,
        MAX_IMAGE_BYTES / 1024
    ))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u8;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return Err(format!("bad base64 byte 0x{c:02x}")),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

pub fn base_url(cfg: &Config) -> String {
    let base = if !cfg.media_base_url.is_empty() {
        cfg.media_base_url.clone()
    } else if !cfg.base_url.is_empty() {
        cfg.base_url.clone()
    } else {
        "https://api.openai.com/v1".to_string()
    };
    base.trim_end_matches('/').to_string()
}

fn post_json(cfg: &Config, url: &str, body: &Value) -> Result<ureq::Response, String> {
    let key = key_ring(cfg).into_iter().next().unwrap_or_default();
    let mut req = ureq::post(url)
        .timeout(Duration::from_secs(300))
        .set("Content-Type", "application/json");
    if !key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    req.send_string(&body.to_string())
        .map_err(|e| redact(&e.to_string()))
}

fn read_capped(resp: ureq::Response) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_MEDIA as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| redact(&e.to_string()))?;
    if buf.len() > MAX_MEDIA {
        return Err("media larger than 25 MB cap".into());
    }
    Ok(buf)
}

pub fn generate_image(cfg: &Config, prompt: &str) -> Result<Vec<u8>, String> {
    if prompt.trim().is_empty() {
        return Err("empty prompt".into());
    }
    let mut body = serde_json::json!({
        "model": cfg.media_image_model,
        "prompt": prompt,
        "n": 1,
    });

    if cfg.media_image_model.starts_with("dall-e") {
        body["response_format"] = Value::from("b64_json");
    }
    let url = format!("{}/images/generations", base_url(cfg));
    let resp = post_json(cfg, &url, &body)?;
    let text = resp.into_string().map_err(|e| redact(&e.to_string()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
    if let Some(b64) = v["data"][0]["b64_json"].as_str() {
        return b64_decode(b64);
    }
    if let Some(img_url) = v["data"][0]["url"].as_str() {
        crate::ssrf::check_url(img_url)?;
        let resp = ureq::get(img_url)
            .timeout(Duration::from_secs(120))
            .call()
            .map_err(|e| redact(&e.to_string()))?;
        return read_capped(resp);
    }
    Err(format!(
        "no image in response: {}",
        redact(&text.chars().take(300).collect::<String>())
    ))
}

fn generate_clip(
    cfg: &Config,
    endpoint: &str,
    model: &str,
    prompt: &str,
) -> Result<Vec<u8>, String> {
    if prompt.trim().is_empty() {
        return Err("empty prompt".into());
    }
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
    });
    let url = format!("{}/{endpoint}", base_url(cfg));
    let resp = post_json(cfg, &url, &body)?;
    let text = resp.into_string().map_err(|e| redact(&e.to_string()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
    if let Some(b64) = v["data"][0]["b64_json"].as_str() {
        return b64_decode(b64);
    }
    if let Some(media_url) = v["data"][0]["url"].as_str() {
        crate::ssrf::check_url(media_url)?;
        let resp = ureq::get(media_url)
            .timeout(Duration::from_secs(300))
            .call()
            .map_err(|e| redact(&e.to_string()))?;
        return read_capped(resp);
    }
    Err(format!(
        "no media in response: {}",
        redact(&text.chars().take(300).collect::<String>())
    ))
}

pub fn generate_video(cfg: &Config, prompt: &str) -> Result<Vec<u8>, String> {
    generate_clip(cfg, "videos/generations", &cfg.media_video_model, prompt)
}

pub fn generate_music(cfg: &Config, prompt: &str) -> Result<Vec<u8>, String> {
    generate_clip(cfg, "music/generations", &cfg.media_music_model, prompt)
}

pub fn speak(cfg: &Config, text: &str) -> Result<Vec<u8>, String> {
    if text.trim().is_empty() {
        return Err("empty text".into());
    }
    let body = serde_json::json!({
        "model": cfg.media_tts_model,
        "voice": cfg.media_tts_voice,
        "input": text,
    });
    let url = format!("{}/audio/speech", base_url(cfg));
    let resp = post_json(cfg, &url, &body)?;
    let bytes = read_capped(resp)?;
    if bytes.is_empty() {
        return Err("speech endpoint returned no audio".into());
    }
    Ok(bytes)
}

pub fn multipart_fields(
    boundary: &str,
    fields: &[(&str, &str)],
    file_field: &str,
    filename: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 512);
    let push = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
    for (name, value) in fields {
        push(&mut out, &format!("--{boundary}\r\n"));
        push(
            &mut out,
            &format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"),
        );
    }
    push(&mut out, &format!("--{boundary}\r\n"));
    push(
        &mut out,
        &format!(
            "Content-Disposition: form-data; name=\"{file_field}\"; filename=\"{filename}\"\r\n"
        ),
    );
    push(&mut out, "Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(bytes);
    push(&mut out, &format!("\r\n--{boundary}--\r\n"));
    out
}

pub fn multipart_multi(
    boundary: &str,
    fields: &[(&str, &str)],
    files: &[(String, String, Vec<u8>)],
) -> Vec<u8> {
    let total: usize = files.iter().map(|(_, _, b)| b.len()).sum();
    let mut out = Vec::with_capacity(total + 1024);
    let push = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
    for (name, value) in fields {
        push(&mut out, &format!("--{boundary}\r\n"));
        push(
            &mut out,
            &format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"),
        );
    }
    for (part, filename, bytes) in files {
        push(&mut out, &format!("--{boundary}\r\n"));
        push(
            &mut out,
            &format!(
                "Content-Disposition: form-data; name=\"{part}\"; filename=\"{filename}\"\r\n"
            ),
        );
        push(&mut out, "Content-Type: application/octet-stream\r\n\r\n");
        out.extend_from_slice(bytes);
        push(&mut out, "\r\n");
    }
    push(&mut out, &format!("--{boundary}--\r\n"));
    out
}

#[cfg(test)]
mod image_limit_tests {
    use super::*;

    #[test]
    fn magic_numbers_identify_supported_containers() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(
            sniff_mime(&[0xff, 0xd8, 0xff, 0xe0, 0x00]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff_mime(b"RIFF____WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_mime(b"%PDF-1.7\n"), Some("application/pdf"));
        assert_eq!(sniff_mime(b"MZ\x90\x00"), None);
        assert_eq!(sniff_mime(b""), None);
        assert_eq!(sniff_mime(b"RIFF"), None);
    }

    #[test]
    fn a_lying_declared_mime_is_corrected_to_the_real_one() {
        let png = b"\x89PNG\r\n\x1a\nbody";
        assert_eq!(verified_mime("image/jpeg", png).unwrap(), "image/png");
        assert_eq!(verified_mime("application/pdf", png).unwrap(), "image/png");
    }

    #[test]
    fn a_non_image_masquerading_as_an_image_is_refused() {
        let elf = b"\x7fELF\x02\x01\x01\x00 not an image at all";
        let note = verified_mime("image/png", elf).expect_err("must refuse");
        assert!(note.contains("refused"), "{note}");
        assert!(note.contains("image/png"), "{note}");

        let note = verified_mime("application/zip", elf).expect_err("must refuse");
        assert!(note.contains("unsupported type"), "{note}");
    }

    #[test]
    fn oversized_images_are_dropped_with_a_readable_note() {
        assert!(image_too_large("image/png", 1024).is_none());
        assert!(image_too_large("image/png", MAX_IMAGE_BYTES).is_none());
        let note = image_too_large("image/jpeg", MAX_IMAGE_BYTES + 1).expect("note");
        assert!(note.contains("image dropped"), "{note}");
        assert!(note.contains("5120 KB"), "{note}");
    }

    #[test]
    fn non_images_are_never_dropped_by_the_image_limit() {
        assert!(image_too_large("application/pdf", MAX_IMAGE_BYTES * 2).is_none());
        assert!(image_too_large("audio/ogg", MAX_IMAGE_BYTES * 2).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn b64_roundtrip() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(b64_decode(&b64_encode(input)).unwrap(), input);
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_decode("Zm9v\nYmFy").unwrap(), b"foobar");
        assert!(b64_decode("a!b").is_err());
    }

    #[test]
    fn base_url_resolution_order() {
        let mut cfg = Config::default();
        assert_eq!(base_url(&cfg), "https://api.openai.com/v1");
        cfg.base_url = "http://prov/v1/".into();
        assert_eq!(base_url(&cfg), "http://prov/v1");
        cfg.media_base_url = "http://media/v1".into();
        assert_eq!(base_url(&cfg), "http://media/v1");
    }

    #[test]
    fn multipart_fields_layout() {
        let body = multipart_fields("BND", &[("chat_id", "42")], "photo", "img.png", b"PNG");
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("name=\"chat_id\"\r\n\r\n42"));
        assert!(s.contains("name=\"photo\"; filename=\"img.png\""));
        assert!(s.contains("PNG"));
        assert!(s.ends_with("--BND--\r\n"));
    }

    fn mock_server(response_body: &'static str, content_type: &'static str) -> String {
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
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    content_type,
                    response_body.len(),
                    response_body
                );
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn generate_image_b64_shape() {
        let cfg = Config {
            media_base_url: mock_server(r#"{"data":[{"b64_json":"UE5H"}]}"#, "application/json"),
            ..Config::default()
        };
        assert_eq!(generate_image(&cfg, "a bird").unwrap(), b"PNG");
    }

    #[test]
    fn generate_image_rejects_empty_prompt() {
        assert!(generate_image(&Config::default(), "  ").is_err());
    }

    #[test]
    fn speak_returns_raw_bytes() {
        let cfg = Config {
            media_base_url: mock_server("MP3BYTES", "audio/mpeg"),
            ..Config::default()
        };
        assert_eq!(speak(&cfg, "hello").unwrap(), b"MP3BYTES");
    }

    #[test]
    fn speak_rejects_empty_text() {
        assert!(speak(&Config::default(), "").is_err());
    }

    #[test]
    fn generate_video_b64_shape() {
        let cfg = Config {
            media_base_url: mock_server(r#"{"data":[{"b64_json":"TVA0"}]}"#, "application/json"),
            ..Config::default()
        };
        assert_eq!(generate_video(&cfg, "a sunrise").unwrap(), b"MP4");
    }

    #[test]
    fn generate_music_b64_shape() {
        let cfg = Config {
            media_base_url: mock_server(r#"{"data":[{"b64_json":"TVAz"}]}"#, "application/json"),
            ..Config::default()
        };
        assert_eq!(generate_music(&cfg, "calm piano").unwrap(), b"MP3");
    }

    #[test]
    fn generate_clip_rejects_empty_prompt_and_bad_response() {
        assert!(generate_video(&Config::default(), " ").is_err());
        assert!(generate_music(&Config::default(), "").is_err());
        let cfg = Config {
            media_base_url: mock_server(r#"{"data":[]}"#, "application/json"),
            ..Config::default()
        };
        let err = generate_video(&cfg, "x").unwrap_err();
        assert!(err.starts_with("no media in response"), "got: {err}");
    }
}
