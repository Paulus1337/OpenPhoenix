use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use serde_json::{json, Value};

pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

pub fn endpoint(cfg: &crate::config::Config) -> Result<(String, u16), String> {
    if !cfg.http_enabled {
        return Err(
            "the http gateway is off; enable [http] in config.toml and run `phoenix serve`".into(),
        );
    }
    let host = if cfg.http_bind.is_empty() {
        "127.0.0.1".to_string()
    } else {
        cfg.http_bind.clone()
    };
    Ok((host, cfg.http_port))
}

pub fn build_request(host: &str, port: u16, token: &str, prompt: &str) -> String {
    let body = json!({"prompt": prompt}).to_string();
    let mut head =
        format!("POST /run HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n");
    if !token.is_empty() {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    head
}

pub fn parse_response(raw: &str) -> Result<(u16, String), String> {
    let mut parts = raw.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("");
    let status_line = head.lines().next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("unreadable status line: {status_line}"))?;
    Ok((code, body.to_string()))
}

pub fn reply_of(status: u16, body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|_| format!("server sent {status} with a non-JSON body: {}", clip(body)))?;
    if status != 200 {
        let msg = v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(match status {
            401 | 403 => format!("{msg} (check [http] token in config.toml)"),
            _ => format!("server returned {status}: {msg}"),
        });
    }
    let reply = v
        .get("reply")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let media: Vec<String> = v
        .get("media")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if media.is_empty() {
        Ok(reply)
    } else {
        Ok(format!("{reply}\n[media: {}]", media.join(", ")))
    }
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(200).collect();
    t
}

pub fn send(
    host: &str,
    port: u16,
    token: &str,
    prompt: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| {
        format!("cannot reach a phoenix gateway at {addr}: {e}\n  start one with `phoenix serve`")
    })?;
    let timeout = std::time::Duration::from_secs(timeout_secs.max(1));
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    stream
        .write_all(build_request(host, port, token, prompt).as_bytes())
        .map_err(|e| format!("send failed: {e}"))?;
    let mut raw = String::new();
    let mut reader = BufReader::new(stream).take(MAX_REPLY_BYTES as u64);
    reader
        .read_to_string(&mut raw)
        .map_err(|e| format!("read failed: {e}"))?;
    let (status, body) = parse_response(&raw)?;
    reply_of(status, &body)
}

pub fn run(cfg: &crate::config::Config, prompt: Option<&str>, timeout_secs: u64) -> u8 {
    let (host, port) = match endpoint(cfg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let token = cfg.http_token.clone();
    if let Some(p) = prompt {
        return match send(&host, port, &token, p, timeout_secs) {
            Ok(reply) => {
                println!("{reply}");
                0
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        };
    }
    println!("attached to phoenix at {host}:{port}; blank line or ctrl-d to leave");
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("> ");
        let _ = std::io::stdout().flush();
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("read failed: {e}");
                return 2;
            }
        }
        let text = line.trim();
        if text.is_empty() {
            break;
        }
        match send(&host, port, &token, text, timeout_secs) {
            Ok(reply) => println!("{reply}"),
            Err(e) => eprintln!("{e}"),
        }
    }
    println!("detached");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn a_disabled_gateway_says_how_to_turn_it_on() {
        let cfg = Config::default();
        let e = endpoint(&cfg).expect_err("must refuse");
        assert!(e.contains("phoenix serve"), "{e}");
    }

    #[test]
    fn the_endpoint_comes_from_the_same_config_the_server_uses() {
        let cfg = Config {
            http_enabled: true,
            http_bind: "127.0.0.1".into(),
            http_port: 9191,
            ..Config::default()
        };
        assert_eq!(endpoint(&cfg).expect("ok"), ("127.0.0.1".to_string(), 9191));
    }

    #[test]
    fn an_empty_bind_defaults_to_loopback_rather_than_every_interface() {
        let cfg = Config {
            http_enabled: true,
            http_bind: String::new(),
            http_port: 8787,
            ..Config::default()
        };
        assert_eq!(endpoint(&cfg).expect("ok").0, "127.0.0.1");
    }

    #[test]
    fn a_request_carries_the_prompt_and_a_bearer_token() {
        let raw = build_request("127.0.0.1", 8787, "sekrit", "hello there");
        assert!(raw.starts_with("POST /run HTTP/1.1\r\n"));
        assert!(raw.contains("Authorization: Bearer sekrit"));
        assert!(raw.contains("Host: 127.0.0.1:8787"));
        let body = raw.split_once("\r\n\r\n").map(|x| x.1).expect("body");
        let v: Value = serde_json::from_str(body).expect("json body");
        assert_eq!(v["prompt"], "hello there");
        assert!(
            raw.contains(&format!("Content-Length: {}", body.len())),
            "a wrong length makes the server hang waiting for bytes"
        );
    }

    #[test]
    fn no_token_means_no_authorization_header_rather_than_an_empty_one() {
        let raw = build_request("127.0.0.1", 8787, "", "hi");
        assert!(!raw.contains("Authorization"), "{raw}");
    }

    #[test]
    fn a_quote_in_the_prompt_cannot_break_the_json_body() {
        let raw = build_request("h", 1, "", "he said \"hi\" and \\ left");
        let body = raw.split_once("\r\n\r\n").map(|x| x.1).expect("body");
        let v: Value = serde_json::from_str(body).expect("still valid json");
        assert_eq!(v["prompt"], "he said \"hi\" and \\ left");
    }

    #[test]
    fn a_response_splits_into_status_and_body() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"reply\":\"hi\"}";
        let (code, body) = parse_response(raw).expect("parse");
        assert_eq!(code, 200);
        assert_eq!(body, "{\"reply\":\"hi\"}");
    }

    #[test]
    fn a_garbage_response_is_an_error_not_a_zero_status() {
        assert!(parse_response("not http at all").is_err());
    }

    #[test]
    fn a_successful_reply_is_returned_verbatim() {
        let out = reply_of(200, r#"{"reply":"the answer","media":[]}"#).expect("ok");
        assert_eq!(out, "the answer");
    }

    #[test]
    fn media_names_are_appended_so_the_caller_knows_files_were_made() {
        let out = reply_of(200, r#"{"reply":"made it","media":["a.png","b.wav"]}"#).expect("ok");
        assert!(out.contains("made it"));
        assert!(out.contains("a.png"), "{out}");
        assert!(out.contains("b.wav"), "{out}");
    }

    #[test]
    fn an_auth_failure_points_at_the_token_instead_of_just_saying_401() {
        let e = reply_of(401, r#"{"error":"unauthorized"}"#).expect_err("must fail");
        assert!(e.contains("token"), "{e}");
    }

    #[test]
    fn a_server_error_surfaces_its_message() {
        let e = reply_of(500, r#"{"error":"model exploded"}"#).expect_err("must fail");
        assert!(e.contains("model exploded"), "{e}");
    }

    #[test]
    fn a_non_json_body_is_reported_with_a_clipped_excerpt() {
        let e = reply_of(502, "<html>gateway down</html>").expect_err("must fail");
        assert!(e.contains("non-JSON"), "{e}");
        assert!(e.contains("gateway down"), "{e}");
    }

    #[test]
    fn an_unreachable_gateway_suggests_starting_one() {
        let e = send("127.0.0.1", 1, "", "hi", 1).expect_err("must fail");
        assert!(e.contains("phoenix serve"), "{e}");
    }
}
