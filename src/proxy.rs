use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_LINES: usize = 100;
const MAX_LINE_BYTES: usize = 16 * 1024;
const CAPTURE_BODY_CHARS: usize = 4000;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn capture_path() -> PathBuf {
    crate::config::home().join("proxy.jsonl")
}

pub fn is_secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization"
        || n == "proxy-authorization"
        || n == "cookie"
        || n == "set-cookie"
        || n.starts_with("x-api-key")
        || n.starts_with("api-key")
        || n.contains("-token")
        || n.contains("secret")
}

pub fn redact_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            if is_secret_header(k) {
                (k.clone(), "[redacted]".to_string())
            } else {
                (k.clone(), crate::security::redact(v))
            }
        })
        .collect()
}

pub fn join_upstream(base: &str, path: &str) -> Result<String, String> {
    if path.contains("..") {
        return Err(format!("refusing path with '..': {path}"));
    }
    if !path.starts_with('/') {
        return Err(format!("path must start with '/': {path}"));
    }
    Ok(format!("{}{}", base.trim_end_matches('/'), path))
}

pub fn parse_request(reader: &mut impl BufRead) -> Result<Request, String> {
    let mut line = String::new();
    let n = read_line_capped(reader, &mut line)?;
    if n == 0 {
        return Err("client closed before sending a request".into());
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Err(format!("malformed request line: {}", line.trim_end()));
    }
    let mut headers = Vec::new();
    let mut content_len = 0usize;
    for _ in 0..MAX_HEADER_LINES {
        let mut h = String::new();
        if read_line_capped(reader, &mut h)? == 0 {
            break;
        }
        let t = h.trim_end();
        if t.is_empty() {
            break;
        }
        let Some((k, v)) = t.split_once(':') else {
            continue;
        };
        let k = k.trim().to_string();
        let v = v.trim().to_string();
        if k.eq_ignore_ascii_case("content-length") {
            content_len = v.parse().unwrap_or(0);
        }
        headers.push((k, v));
    }
    if content_len > MAX_BODY_BYTES {
        return Err(format!(
            "request body of {content_len} bytes exceeds the {MAX_BODY_BYTES} byte cap"
        ));
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("short body: {e}"))?;
    }
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn read_line_capped(reader: &mut impl BufRead, out: &mut String) -> Result<usize, String> {
    let mut taken = reader.take(MAX_LINE_BYTES as u64);
    let n = taken
        .read_line(out)
        .map_err(|e| format!("read failed: {e}"))?;
    if n >= MAX_LINE_BYTES {
        return Err("header line exceeded its cap".into());
    }
    Ok(n)
}

fn clip(text: &str) -> String {
    let n = text.chars().count();
    if n <= CAPTURE_BODY_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(CAPTURE_BODY_CHARS).collect();
    format!("{head}…[{} more chars]", n - CAPTURE_BODY_CHARS)
}

pub fn capture_entry(
    req: &Request,
    status: u16,
    resp_headers: &[(String, String)],
    resp_body: &str,
    elapsed_ms: u64,
) -> Value {
    let req_body = String::from_utf8_lossy(&req.body).to_string();
    json!({
        "v": 1,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "method": req.method,
        "path": req.path,
        "status": status,
        "elapsed_ms": elapsed_ms,
        "request": {
            "headers": redact_headers(&req.headers)
                .into_iter()
                .map(|(k, v)| json!([k, v]))
                .collect::<Vec<_>>(),
            "body": clip(&crate::security::redact(&req_body)),
        },
        "response": {
            "headers": redact_headers(resp_headers)
                .into_iter()
                .map(|(k, v)| json!([k, v]))
                .collect::<Vec<_>>(),
            "body": clip(&crate::security::redact(resp_body)),
        },
    })
}

pub fn append_capture(path: &Path, entry: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let line = format!("{entry}\n");
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| e.to_string())?;
    f.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} OK\r\n");
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if lk == "transfer-encoding" || lk == "content-length" || lk == "connection" {
            continue;
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

fn error_response(stream: &mut TcpStream, status: u16, msg: &str) {
    let body = json!({"error": msg}).to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
}

type Forwarded = (u16, Vec<(String, String)>, String);

fn forward(upstream: &str, req: &Request) -> Result<Forwarded, String> {
    let url = join_upstream(upstream, &req.path)?;
    crate::ssrf::check_url(&url)?;
    let mut r = match req.method.as_str() {
        "GET" => ureq::get(&url),
        "POST" => ureq::post(&url),
        "PUT" => ureq::put(&url),
        "DELETE" => ureq::delete(&url),
        other => return Err(format!("unsupported method {other}")),
    }
    .timeout(std::time::Duration::from_secs(120));
    for (k, v) in &req.headers {
        let lk = k.to_ascii_lowercase();
        if lk == "host" || lk == "content-length" || lk == "connection" {
            continue;
        }
        r = r.set(k, v);
    }
    let resp = if req.body.is_empty() {
        r.call()
    } else {
        r.send_bytes(&req.body)
    };
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let headers = collect_headers(&r);
            let body = r.into_string().unwrap_or_default();
            return Ok((code, headers, body));
        }
        Err(e) => return Err(format!("upstream request failed: {e}")),
    };
    let status = resp.status();
    let headers = collect_headers(&resp);
    let body = resp.into_string().map_err(|e| e.to_string())?;
    Ok((status, headers, body))
}

fn collect_headers(resp: &ureq::Response) -> Vec<(String, String)> {
    resp.headers_names()
        .into_iter()
        .filter_map(|n| resp.header(&n).map(|v| (n.clone(), v.to_string())))
        .collect()
}

pub fn serve(port: u16, upstream: &str, capture: &Path) -> Result<(), String> {
    if upstream.trim().is_empty() {
        return Err("no upstream url; set provider.base_url or pass one".into());
    }
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot bind 127.0.0.1:{port}: {e}"))?;
    let actual = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    println!("recording proxy on http://127.0.0.1:{actual}");
    println!("  upstream  {upstream}");
    println!("  capture   {}", capture.display());
    println!("  point provider.base_url at http://127.0.0.1:{actual} then talk to phoenix");
    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(s) => s,
            Err(_) => continue,
        };
        let started = std::time::Instant::now();
        let req = {
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => continue,
            });
            parse_request(&mut reader)
        };
        let req = match req {
            Ok(r) => r,
            Err(e) => {
                error_response(&mut stream, 400, &e);
                continue;
            }
        };
        match forward(upstream, &req) {
            Ok((status, headers, body)) => {
                let entry = capture_entry(
                    &req,
                    status,
                    &headers,
                    &body,
                    started.elapsed().as_millis() as u64,
                );
                if let Err(e) = append_capture(capture, &entry) {
                    eprintln!("capture failed: {e}");
                }
                let _ = write_response(&mut stream, status, &headers, body.as_bytes());
                println!("  {} {} -> {}", req.method, req.path, status);
            }
            Err(e) => {
                let entry = capture_entry(&req, 502, &[], &e, started.elapsed().as_millis() as u64);
                let _ = append_capture(capture, &entry);
                error_response(&mut stream, 502, &e);
                eprintln!("  {} {} -> {e}", req.method, req.path);
            }
        }
    }
    Ok(())
}

pub fn log_text(capture: &Path, limit: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(capture) else {
        return format!(
            "no captures yet at {}; run `phoenix proxy run` first\n",
            capture.display()
        );
    };
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "no captures yet\n".to_string();
    }
    let start = lines.len().saturating_sub(limit);
    let mut out = format!(
        "{} captures, showing {}\n",
        lines.len(),
        lines.len() - start
    );
    for l in &lines[start..] {
        let Ok(v) = serde_json::from_str::<Value>(l) else {
            continue;
        };
        out.push_str(&format!(
            "  {:<6}{:<44}{:<5}{}ms\n",
            v["method"].as_str().unwrap_or("?"),
            v["path"].as_str().unwrap_or("?"),
            v["status"].as_u64().unwrap_or(0),
            v["elapsed_ms"].as_u64().unwrap_or(0),
        ));
    }
    out
}

pub fn show_text(capture: &Path, index: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(capture) else {
        return "no captures yet\n".to_string();
    };
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if index == 0 || index > lines.len() {
        return format!("no capture #{index}; there are {}\n", lines.len());
    }
    match serde_json::from_str::<Value>(lines[index - 1]) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default() + "\n",
        Err(e) => format!("capture #{index} is not readable: {e}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_of(raw: &str) -> Result<Request, String> {
        let mut r = BufReader::new(raw.as_bytes());
        parse_request(&mut r)
    }

    #[test]
    fn a_request_line_and_headers_parse() {
        let r = req_of("POST /v1/chat HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello")
            .expect("parse");
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/chat");
        assert_eq!(r.body, b"hello");
        assert!(r.headers.iter().any(|(k, v)| k == "Host" && v == "x"));
    }

    #[test]
    fn a_body_shorter_than_content_length_is_an_error_not_a_silent_truncation() {
        assert!(req_of("POST /x HTTP/1.1\r\nContent-Length: 50\r\n\r\nshort").is_err());
    }

    #[test]
    fn a_malformed_request_line_is_refused() {
        assert!(req_of("garbage\r\n\r\n").is_err());
        assert!(req_of("").is_err());
    }

    #[test]
    fn an_oversized_body_is_refused_before_allocating_it() {
        let raw = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let e = req_of(&raw).expect_err("must refuse");
        assert!(e.contains("cap"), "{e}");
    }

    #[test]
    fn credential_headers_never_reach_the_capture() {
        let headers = vec![
            ("Authorization".into(), "Bearer sk-ant-secret".into()),
            ("X-Api-Key".into(), "nvapi-secret".into()),
            ("Cookie".into(), "session=abc".into()),
            ("Content-Type".into(), "application/json".into()),
        ];
        let out = redact_headers(&headers);
        let find = |name: &str| {
            out.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(find("Authorization"), "[redacted]");
        assert_eq!(find("X-Api-Key"), "[redacted]");
        assert_eq!(find("Cookie"), "[redacted]");
        assert_eq!(
            find("Content-Type"),
            "application/json",
            "ordinary headers must survive or the capture is useless"
        );
    }

    #[test]
    fn secret_header_names_are_matched_by_shape() {
        assert!(is_secret_header("authorization"));
        assert!(is_secret_header("AUTHORIZATION"));
        assert!(is_secret_header("x-api-key"));
        assert!(is_secret_header("x-some-token"));
        assert!(is_secret_header("x-client-secret"));
        assert!(!is_secret_header("content-type"));
        assert!(!is_secret_header("user-agent"));
    }

    #[test]
    fn a_key_inside_a_body_is_redacted_too() {
        let req = Request {
            method: "POST".into(),
            path: "/v1/chat".into(),
            headers: Vec::new(),
            body: b"{\"key\":\"sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\"}".to_vec(),
        };
        let entry = capture_entry(&req, 200, &[], "ok", 5);
        let body = entry["request"]["body"].as_str().unwrap_or("");
        assert!(
            !body.contains("abcdefghijklmnopqrstuvwxyz0123456789"),
            "a key in the payload must not persist: {body}"
        );
    }

    #[test]
    fn a_long_body_is_clipped_so_a_stream_cannot_fill_the_disk() {
        let big = "x".repeat(CAPTURE_BODY_CHARS * 3);
        let req = Request {
            method: "POST".into(),
            path: "/x".into(),
            headers: Vec::new(),
            body: big.clone().into_bytes(),
        };
        let entry = capture_entry(&req, 200, &[], &big, 1);
        let got = entry["request"]["body"].as_str().unwrap_or("");
        assert!(got.len() < big.len());
        assert!(got.contains("more chars"), "{got}");
    }

    #[test]
    fn upstream_paths_join_without_doubling_the_slash() {
        assert_eq!(
            join_upstream("https://api.example/v1/", "/chat/completions").expect("join"),
            "https://api.example/v1/chat/completions"
        );
        assert_eq!(
            join_upstream("https://api.example/v1", "/models").expect("join"),
            "https://api.example/v1/models"
        );
    }

    #[test]
    fn a_traversing_path_is_refused() {
        assert!(join_upstream("https://api.example/v1", "/../../admin").is_err());
        assert!(join_upstream("https://api.example/v1", "no-leading-slash").is_err());
    }

    #[test]
    fn an_entry_records_status_timing_and_both_sides() {
        let req = Request {
            method: "POST".into(),
            path: "/v1/chat".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        };
        let entry = capture_entry(&req, 429, &[("retry-after".into(), "2".into())], "slow", 91);
        assert_eq!(entry["status"], 429);
        assert_eq!(entry["elapsed_ms"], 91);
        assert_eq!(entry["method"], "POST");
        assert_eq!(entry["response"]["body"], "slow");
        assert!(entry["ts"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn captures_append_and_read_back_newest_last() {
        let p = std::env::temp_dir().join(format!("phx-proxy-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        for i in 1..=3u16 {
            let req = Request {
                method: "GET".into(),
                path: format!("/{i}"),
                headers: Vec::new(),
                body: Vec::new(),
            };
            append_capture(&p, &capture_entry(&req, 200 + i, &[], "", 1)).expect("append");
        }
        let text = log_text(&p, 2);
        assert!(text.starts_with("3 captures, showing 2"), "{text}");
        assert!(text.contains("/3"), "{text}");
        assert!(!text.contains("/1"), "only the last two: {text}");
        let one = show_text(&p, 1);
        assert!(one.contains("\"path\": \"/1\""), "{one}");
        assert!(show_text(&p, 99).contains("no capture #99"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_empty_capture_file_says_how_to_make_one() {
        let p = std::env::temp_dir().join("phx-proxy-missing.jsonl");
        let _ = std::fs::remove_file(&p);
        assert!(log_text(&p, 10).contains("proxy run"));
    }

    #[test]
    fn serving_without_an_upstream_is_refused() {
        assert!(serve(0, "  ", Path::new("/tmp/x.jsonl")).is_err());
    }
}
