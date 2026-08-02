use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use crate::web::{APP_JS, INDEX_HTML, LOCKED_HTML, STYLE_CSS};

const MAX_BODY: usize = 65_536;
const MAX_HEADERS: usize = 100;
const MAX_HEADER_LINE: u64 = 8_192;
const MAX_HEADER_BYTES: usize = 32_768;
const MAX_LIVE_CONNS: usize = 64;

const LOGO_SVG: &str = include_str!("../assets/phoenix.svg");
const EGG_SVG: &str = include_str!("../assets/phoenix-egg.svg");

#[derive(Debug, Clone)]
pub struct WebOpts {
    pub web: bool,

    pub audit: crate::audit::Audit,

    pub strong_headers: bool,

    pub user: String,

    pub pass: String,

    pub crawlers: Vec<String>,

    pub canvas: bool,

    pub canvas_file: std::path::PathBuf,

    pub model: String,

    pub sessions_dir: std::path::PathBuf,
}

impl Default for WebOpts {
    fn default() -> Self {
        WebOpts {
            web: false,
            audit: crate::audit::Audit::disabled(),
            strong_headers: true,
            user: String::new(),
            pass: String::new(),
            crawlers: Vec::new(),
            canvas: false,
            canvas_file: std::path::PathBuf::new(),
            model: String::new(),
            sessions_dir: std::path::PathBuf::new(),
        }
    }
}

pub const RL_MAX_ATTEMPTS: usize = 10;
pub const RL_WINDOW_SECS: u64 = 60;
pub const RL_LOCKOUT_SECS: u64 = 300;
const RL_MAX_ENTRIES: usize = 10_000;

#[derive(Default)]
pub struct RateLimiter {
    entries: std::sync::Mutex<std::collections::HashMap<String, (Vec<u64>, u64)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    fn loopback(ip: &str) -> bool {
        ip == "127.0.0.1" || ip == "::1" || ip.starts_with("127.")
    }

    pub fn check_at(&self, ip: &str, now: u64) -> bool {
        if Self::loopback(ip) {
            return true;
        }
        let Ok(map) = self.entries.lock() else {
            return true;
        };
        !matches!(map.get(ip), Some((_, locked_until)) if *locked_until > now)
    }

    pub fn record_failure_at(&self, ip: &str, now: u64) {
        if Self::loopback(ip) {
            return;
        }
        let Ok(mut map) = self.entries.lock() else {
            return;
        };
        if map.len() > RL_MAX_ENTRIES {
            map.retain(|_, (_, locked)| *locked > now);
        }
        let entry = map.entry(ip.to_string()).or_insert((Vec::new(), 0));
        entry.0.retain(|t| now.saturating_sub(*t) < RL_WINDOW_SECS);
        entry.0.push(now);
        if entry.0.len() >= RL_MAX_ATTEMPTS {
            entry.1 = now + RL_LOCKOUT_SECS;
            entry.0.clear();
        }
    }

    pub fn reset(&self, ip: &str) {
        if let Ok(mut map) = self.entries.lock() {
            map.remove(ip);
        }
    }
}

pub fn is_loopback_ip(ip: &str) -> bool {
    match ip.trim().parse::<std::net::IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

fn basic_ok(header_val: &str, opts: &WebOpts) -> bool {
    if opts.user.is_empty() || opts.pass.is_empty() {
        return false;
    }
    let Some(b64) = header_val.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(bytes) = crate::media::b64_decode(b64.trim()) else {
        return false;
    };
    let Ok(s) = String::from_utf8(bytes) else {
        return false;
    };
    let Some((u, p)) = s.split_once(':') else {
        return false;
    };
    let want = match opts.pass.strip_prefix("sha256:") {
        Some(hex) => hex.to_ascii_lowercase(),
        None => crate::security::sha256_hex(opts.pass.as_bytes()),
    };
    crate::security::ct_eq(u, &opts.user)
        && crate::security::ct_eq(&crate::security::sha256_hex(p.as_bytes()), &want)
}

fn robots_txt(crawlers: &[String]) -> String {
    let mut out = String::new();
    for ua in crawlers {
        out.push_str(&format!("User-agent: {ua}\nDisallow:\n\n"));
    }
    out.push_str("User-agent: *\nDisallow: /\n");
    out
}

fn sec_headers(strong: bool) -> &'static str {
    if strong {
        concat!(
            "Content-Security-Policy: default-src 'none'; script-src 'self'; ",
            "style-src 'self'; connect-src 'self'; img-src 'self' data:; ",
            "base-uri 'none'; form-action 'self'; frame-ancestors 'none'\r\n",
            "X-Content-Type-Options: nosniff\r\n",
            "X-Frame-Options: DENY\r\n",
            "Referrer-Policy: no-referrer\r\n",
            "Cross-Origin-Opener-Policy: same-origin\r\n",
            "Cross-Origin-Resource-Policy: same-origin\r\n",
            "Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=()\r\n",
            "Cache-Control: no-store\r\n",
        )
    } else {
        "X-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\n"
    }
}

pub fn serve<H>(listener: TcpListener, token: &str, handler: H, opts: &WebOpts)
where
    H: Fn(&str) -> String + Send + Sync + 'static,
{
    let limiter = std::sync::Arc::new(RateLimiter::new());
    let handler = std::sync::Arc::new(handler);
    let token = token.to_string();
    let opts = std::sync::Arc::new(opts.clone());
    let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if live.load(std::sync::atomic::Ordering::SeqCst) >= MAX_LIVE_CONNS {
            let mut s = stream;
            let _ = respond(
                &mut s,
                503,
                "Service Unavailable",
                &json!({"error": "server busy; retry shortly"}),
                &opts,
            );
            continue;
        }
        live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (c_limiter, c_handler, c_token, c_opts, c_live) = (
            limiter.clone(),
            handler.clone(),
            token.clone(),
            opts.clone(),
            live.clone(),
        );
        let spawned = std::thread::Builder::new().spawn(move || {
            let mut call = |p: &str| c_handler(p);
            let _ = handle_limited(stream, &c_token, &mut call, &c_opts, Some(&c_limiter));
            c_live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        });
        if spawned.is_err() {
            live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn respond_raw(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    ctype: &str,
    extra_headers: &str,
    body: &str,
    opts: &WebOpts,
) -> std::io::Result<()> {
    let robots = if opts.crawlers.is_empty() {
        "X-Robots-Tag: noindex, nofollow, noarchive\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\n{}{robots}{extra_headers}\
Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        sec_headers(opts.strong_headers),
        body.len()
    )
}

fn respond_canvas(stream: &mut TcpStream, body: &str, opts: &WebOpts) -> std::io::Result<()> {
    let robots = if opts.crawlers.is_empty() {
        "X-Robots-Tag: noindex, nofollow, noarchive\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'\r\n\
X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\
Referrer-Policy: no-referrer\r\nCache-Control: no-store\r\n{robots}\
Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn respond(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    body: &Value,
    opts: &WebOpts,
) -> std::io::Result<()> {
    respond_raw(
        stream,
        code,
        reason,
        "application/json",
        "",
        &body.to_string(),
        opts,
    )
}

pub fn handle_limited(
    mut stream: TcpStream,
    token: &str,
    handler: &mut dyn FnMut(&str) -> String,
    opts: &WebOpts,
    limiter: Option<&RateLimiter>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let path = match target.split_once('?') {
        Some((p, _query)) => p.to_string(),
        None => target,
    };
    let mut auth_val = String::new();
    let mut content_len = 0usize;
    let mut ws_key = String::new();
    let mut upgrade_ws = false;
    let mut header_count = 0usize;
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.by_ref().take(MAX_HEADER_LINE).read_line(&mut line)?;
        if n == 0 {
            break;
        }
        header_count += 1;
        header_bytes += n;
        if header_count > MAX_HEADERS || header_bytes > MAX_HEADER_BYTES {
            return respond(
                &mut stream,
                431,
                "Request Header Fields Too Large",
                &json!({"error": "too many header bytes"}),
                opts,
            );
        }
        let line_t = line.trim_end();
        if line_t.is_empty() {
            break;
        }
        if let Some((k, v)) = line_t.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let v = v.trim();
            if key == "authorization" {
                auth_val = v.to_string();
            } else if key == "content-length" {
                content_len = v.parse().unwrap_or(0);
            } else if key == "sec-websocket-key" {
                ws_key = v.to_string();
            } else if key == "upgrade" && v.eq_ignore_ascii_case("websocket") {
                upgrade_ws = true;
            }
        }
    }

    let peer_ip = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let now = crate::scheduler::now_epoch();
    if let Some(rl) = limiter {
        if !rl.check_at(&peer_ip, now) {
            return respond(
                &mut stream,
                429,
                "Too Many Requests",
                &json!({"error": "too many failed attempts; try again later"}),
                opts,
            );
        }
    }
    let bearer = !token.is_empty()
        && auth_val.starts_with("Bearer ")
        && crate::security::ct_eq(&auth_val["Bearer ".len()..], token);
    let basic = basic_ok(&auth_val, opts);
    let authed = bearer || basic;
    let credential_offered = !auth_val.is_empty();
    if !authed && credential_offered {
        opts.audit.auth(
            "http",
            &peer_ip,
            crate::audit::Outcome::Blocked,
            &format!("{method} {path}"),
        );
    }
    if let Some(rl) = limiter {
        if authed {
            rl.reset(&peer_ip);
        } else if credential_offered {
            rl.record_failure_at(&peer_ip, now);
        }
    }

    if method == "GET" && path == "/robots.txt" {
        return respond_raw(
            &mut stream,
            200,
            "OK",
            "text/plain; charset=utf-8",
            "",
            &robots_txt(&opts.crawlers),
            opts,
        );
    }
    if opts.web && method == "GET" {
        if let Some((ctype, body)) = match path.as_str() {
            "/" | "/index.html" => Some(("text/html; charset=utf-8", INDEX_HTML)),
            "/app.js" => Some(("application/javascript; charset=utf-8", APP_JS)),
            "/style.css" => Some(("text/css; charset=utf-8", STYLE_CSS)),
            "/logo.svg" | "/favicon.svg" => Some(("image/svg+xml", LOGO_SVG)),
            "/egg.svg" => Some(("image/svg+xml", EGG_SVG)),
            _ => None,
        } {
            if opts.user.is_empty() || opts.pass.is_empty() {
                return respond(
                    &mut stream,
                    403,
                    "Forbidden",
                    &json!({"error": "web UI disabled: set http.username and http.password"}),
                    opts,
                );
            }
            if !basic {
                if path == "/" || path == "/index.html" {
                    return respond_raw(
                        &mut stream,
                        401,
                        "Unauthorized",
                        "text/html; charset=utf-8",
                        "WWW-Authenticate: Basic realm=\"OpenPhoenix\"\r\n",
                        LOCKED_HTML,
                        opts,
                    );
                }
                return respond_raw(
                    &mut stream,
                    401,
                    "Unauthorized",
                    "application/json",
                    "WWW-Authenticate: Basic realm=\"OpenPhoenix\"\r\n",
                    "{\"error\":\"unauthorized\"}",
                    opts,
                );
            }
            return respond_raw(&mut stream, 200, "OK", ctype, "", body, opts);
        }
    }
    if opts.canvas && method == "GET" && (path == "/canvas" || path == "/canvas/version") {
        if opts.user.is_empty() || opts.pass.is_empty() {
            return respond(
                &mut stream,
                403,
                "Forbidden",
                &json!({"error": "canvas disabled: set http.username and http.password"}),
                opts,
            );
        }
        if !authed {
            return respond_raw(
                &mut stream,
                401,
                "Unauthorized",
                "application/json",
                "WWW-Authenticate: Basic realm=\"OpenPhoenix\"\r\n",
                "{\"error\":\"unauthorized\"}",
                opts,
            );
        }
        if path == "/canvas/version" {
            return respond(
                &mut stream,
                200,
                "OK",
                &json!({"v": crate::canvas::version(&opts.canvas_file)}),
                opts,
            );
        }
        return respond_canvas(&mut stream, &crate::canvas::render(&opts.canvas_file), opts);
    }
    if method == "GET" && path == "/health" {
        return respond(&mut stream, 200, "OK", &json!({"ok": true}), opts);
    }
    if !authed {
        return respond(
            &mut stream,
            401,
            "Unauthorized",
            &json!({"error": "unauthorized"}),
            opts,
        );
    }
    if method == "GET" && path == "/ws" && upgrade_ws {
        if ws_key.is_empty() {
            return respond(
                &mut stream,
                400,
                "Bad Request",
                &json!({"error": "missing Sec-WebSocket-Key"}),
                opts,
            );
        }
        let resp = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            crate::ws::server_accept(&ws_key)
        );
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
        stream.set_read_timeout(Some(Duration::from_secs(600)))?;
        while let Ok((_fin, op, payload)) = crate::ws::read_raw_frame(&mut reader) {
            match op {
                crate::ws::OP_TEXT => {
                    let text = String::from_utf8_lossy(&payload);
                    let prompt = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| text.into_owned());
                    let reply = handler(&prompt);
                    let out = json!({"reply": reply}).to_string();
                    if crate::ws::write_frame_server(
                        &mut stream,
                        crate::ws::OP_TEXT,
                        out.as_bytes(),
                    )
                    .is_err()
                    {
                        break;
                    }
                }
                crate::ws::OP_PING => {
                    if crate::ws::write_frame_server(&mut stream, crate::ws::OP_PONG, &payload)
                        .is_err()
                    {
                        break;
                    }
                }
                crate::ws::OP_CLOSE => {
                    let _ =
                        crate::ws::write_frame_server(&mut stream, crate::ws::OP_CLOSE, &payload);
                    break;
                }
                _ => {}
            }
        }
        return Ok(());
    }

    match (method.as_str(), path.as_str()) {
        ("POST", "/run") => {
            if content_len == 0 || content_len > MAX_BODY {
                return respond(
                    &mut stream,
                    400,
                    "Bad Request",
                    &json!({"error": "JSON body required, max 64 KB"}),
                    opts,
                );
            }
            let mut buf = vec![0u8; content_len];
            reader.read_exact(&mut buf)?;
            let body: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
            let Some(prompt) = body.get("prompt").and_then(Value::as_str) else {
                return respond(
                    &mut stream,
                    400,
                    "Bad Request",
                    &json!({"error": "missing prompt"}),
                    opts,
                );
            };
            let reply = handler(prompt);
            let (body, media) = crate::text::split_media(&reply);
            let names: Vec<Value> = media
                .iter()
                .map(|p| {
                    Value::String(
                        std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| p.clone()),
                    )
                })
                .collect();
            let extra = if opts.model.is_empty() {
                String::new()
            } else {
                format!("X-Actual-Model: {}\r\n", opts.model)
            };
            respond_raw(
                &mut stream,
                200,
                "OK",
                "application/json",
                &extra,
                &json!({"reply": body, "media": names}).to_string(),
                opts,
            )
        }
        ("GET", "/sessions") => {
            if opts.sessions_dir.as_os_str().is_empty() {
                return respond(
                    &mut stream,
                    404,
                    "Not Found",
                    &json!({"error": "sessions are not enabled"}),
                    opts,
                );
            }
            let items: Vec<Value> = crate::sessions::list(&opts.sessions_dir)
                .into_iter()
                .map(|(id, n)| json!({"id": id, "messages": n}))
                .collect();
            respond(&mut stream, 200, "OK", &json!({"sessions": items}), opts)
        }
        ("DELETE", p) if p.starts_with("/sessions/") => {
            if opts.sessions_dir.as_os_str().is_empty() {
                return respond(
                    &mut stream,
                    404,
                    "Not Found",
                    &json!({"error": "sessions are not enabled"}),
                    opts,
                );
            }
            let id = p.trim_start_matches("/sessions/");
            let known = crate::sessions::list(&opts.sessions_dir)
                .into_iter()
                .any(|(sid, _)| sid == id);
            if !known {
                return respond(
                    &mut stream,
                    404,
                    "Not Found",
                    &json!({"error": format!("no session named {id}")}),
                    opts,
                );
            }
            crate::sessions::reset(&opts.sessions_dir, id);
            respond(
                &mut stream,
                200,
                "OK",
                &json!({"ok": true, "reset": id}),
                opts,
            )
        }
        ("POST", p) if p.starts_with("/hook/") => {
            let name: String = p
                .trim_start_matches("/hook/")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();
            if name.is_empty() {
                return respond(
                    &mut stream,
                    400,
                    "Bad Request",
                    &json!({"error": "hook name required: /hook/NAME"}),
                    opts,
                );
            }
            if content_len > MAX_BODY {
                return respond(
                    &mut stream,
                    400,
                    "Bad Request",
                    &json!({"error": "body too large, max 64 KB"}),
                    opts,
                );
            }
            let mut buf = vec![0u8; content_len];
            reader.read_exact(&mut buf)?;
            let body = String::from_utf8_lossy(&buf);
            let event = format!("[webhook {name}] {}", body.trim());
            let reply = handler(&event);
            respond(&mut stream, 200, "OK", &json!({"reply": reply}), opts)
        }
        _ => respond(
            &mut stream,
            404,
            "Not Found",
            &json!({"error": "not found"}),
            opts,
        ),
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn a_slow_client_does_not_block_other_clients() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            serve(
                listener,
                "sekrit",
                |p: &str| format!("echo:{p}"),
                &WebOpts::default(),
            );
        });

        let mut slow = TcpStream::connect(addr).unwrap();
        slow.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
            .unwrap();
        slow.flush().unwrap();

        let start = std::time::Instant::now();
        let mut fast = TcpStream::connect(addr).unwrap();
        fast.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n")
            .unwrap();
        fast.flush().unwrap();
        let mut out = String::new();
        let _ = fast.read_to_string(&mut out);
        let elapsed = start.elapsed();

        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "second client waited {elapsed:?} behind a slow one"
        );
    }
}

#[cfg(test)]
mod header_limit_tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn opts() -> WebOpts {
        WebOpts::default()
    }

    fn round_trip(request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let req = request.to_string();
        let client = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            s.flush().unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        });
        let (stream, _) = listener.accept().unwrap();
        let mut handler = |_: &str| "ok".to_string();
        let _ = handle_limited(stream, "sekrit", &mut handler, &opts(), None);
        client.join().unwrap()
    }

    #[test]
    fn a_header_flood_is_rejected_not_buffered_forever() {
        let mut req = String::from("GET /health HTTP/1.1\r\nHost: x\r\n");
        for i in 0..500 {
            req.push_str(&format!("X-Pad-{i}: filler\r\n"));
        }
        req.push_str("\r\n");
        let resp = round_trip(&req);
        assert!(resp.starts_with("HTTP/1.1 431"), "{resp}");
    }

    #[test]
    fn one_absurdly_long_header_line_is_bounded() {
        let req = format!(
            "GET /health HTTP/1.1\r\nHost: x\r\nX-Big: {}\r\n\r\n",
            "a".repeat(40_000)
        );
        let resp = round_trip(&req);
        assert!(
            resp.starts_with("HTTP/1.1 431"),
            "{}",
            &resp[..40.min(resp.len())]
        );
    }

    #[test]
    fn a_normal_request_still_succeeds() {
        let resp =
            round_trip("GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn spin(n: usize) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut h = |p: &str| format!("echo:{p}");
            for _ in 0..n {
                if let Ok((s, _)) = listener.accept() {
                    let _ = handle_limited(s, "sekrit", &mut h, &WebOpts::default(), None);
                }
            }
        });
        addr
    }

    fn talk(addr: std::net::SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn run_roundtrip_with_auth() {
        let addr = spin(1);
        let body = r#"{"prompt":"hi there"}"#;
        let req = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains(r#""reply":"echo:hi there""#));
    }

    #[test]
    fn media_directives_never_reach_the_web_ui_as_raw_paths() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                let mut h = |_p: &str| {
                    "here is the chart\nMEDIA:/root/secret/place/chart-99.png\nenjoy".to_string()
                };
                let _ = handle_limited(s, "sekrit", &mut h, &WebOpts::default(), None);
            }
        });
        let body = r#"{"prompt":"chart"}"#;
        let req = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(
            !resp.contains("MEDIA:"),
            "the directive must be stripped: {resp}"
        );
        assert!(
            !resp.contains("/root/secret/place"),
            "a server filesystem path must not be exposed to the browser: {resp}"
        );
        assert!(resp.contains("chart-99.png"), "got: {resp}");
        assert!(resp.contains("here is the chart"), "got: {resp}");
        assert!(resp.contains("enjoy"), "got: {resp}");
    }

    #[test]
    fn ws_upgrade_and_roundtrip() {
        let addr = spin(1);
        let mut s = TcpStream::connect(addr).unwrap();
        let req = "GET /ws HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 13\r\n\r\n";
        s.write_all(req.as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut one = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut one).unwrap();
            head.push(one[0]);
        }
        let head = String::from_utf8_lossy(&head);
        assert!(head.starts_with("HTTP/1.1 101"), "got: {head}");
        assert!(
            head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "RFC 6455 sample accept mismatch: {head}"
        );
        crate::ws::write_frame(&mut s, crate::ws::OP_TEXT, br#"{"prompt":"ping"}"#).unwrap();
        let (_fin, op, payload) = crate::ws::read_raw_frame(&mut s).unwrap();
        assert_eq!(op, crate::ws::OP_TEXT);
        let text = String::from_utf8(payload).unwrap();
        assert!(text.contains(r#""reply":"echo:ping""#), "got: {text}");
        crate::ws::write_frame(&mut s, crate::ws::OP_CLOSE, &[]).unwrap();
    }

    #[test]
    fn ws_upgrade_needs_auth() {
        let addr = spin(1);
        let resp = talk(
            addr,
            "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
    }

    #[test]
    fn hook_roundtrip_with_auth() {
        let addr = spin(2);
        let body = r#"{"alert":"disk full"}"#;
        let req = format!(
            "POST /hook/nagios HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("echo:[webhook nagios]"), "got: {resp}");
        let resp = talk(
            addr,
            "POST /hook/ HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: 0\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
    }

    #[test]
    fn rate_limiter_locks_out_after_failures() {
        let rl = RateLimiter::new();
        let ip = "203.0.113.7";
        for _ in 0..(RL_MAX_ATTEMPTS - 1) {
            rl.record_failure_at(ip, 1000);
        }
        assert!(rl.check_at(ip, 1000), "should still allow before limit");
        rl.record_failure_at(ip, 1000);
        assert!(!rl.check_at(ip, 1000), "should lock out at limit");
        assert!(
            rl.check_at(ip, 1000 + RL_LOCKOUT_SECS + 1),
            "lockout should expire"
        );
    }

    #[test]
    fn requests_without_credentials_never_consume_the_lockout_budget() {
        let rl = RateLimiter::new();
        let ip = "203.0.113.9";
        for _ in 0..RL_MAX_ATTEMPTS * 3 {
            rl.record_failure_at(ip, 1000);
        }
        assert!(!rl.check_at(ip, 1000), "real bad credentials must lock out");

        let fresh = RateLimiter::new();
        let browser = "203.0.113.10";
        assert!(
            fresh.check_at(browser, 1000),
            "a client that offered no credential must not be penalized"
        );
    }

    #[test]
    fn rate_limiter_window_and_reset_and_loopback() {
        let rl = RateLimiter::new();
        let ip = "198.51.100.9";
        for i in 0..(RL_MAX_ATTEMPTS - 1) {
            rl.record_failure_at(ip, 1000 + i as u64);
        }
        rl.record_failure_at(ip, 1000 + RL_WINDOW_SECS + 100);
        assert!(
            rl.check_at(ip, 1000 + RL_WINDOW_SECS + 100),
            "old attempts expire"
        );

        for _ in 0..RL_MAX_ATTEMPTS {
            rl.record_failure_at(ip, 5000);
        }
        assert!(!rl.check_at(ip, 5000));
        rl.reset(ip);
        assert!(rl.check_at(ip, 5000), "reset clears lockout");

        for _ in 0..(RL_MAX_ATTEMPTS * 2) {
            rl.record_failure_at("127.0.0.1", 9000);
        }
        assert!(rl.check_at("127.0.0.1", 9000), "loopback is exempt");
    }

    #[test]
    fn missing_or_wrong_token_is_401() {
        let addr = spin(2);
        let resp = talk(addr, "POST /run HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
        let resp = talk(
            addr,
            "GET /nope HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrong\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 401"));
    }

    #[test]
    fn health_answers_without_any_token() {
        let addr = spin(2);
        let resp = talk(addr, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains(r#""ok":true"#), "got: {resp}");
        let resp = talk(
            addr,
            "GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrong\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    }

    #[test]
    fn health_and_unknown_paths() {
        let addr = spin(2);
        let resp = talk(
            addr,
            "GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.contains(r#""ok":true"#));
        let resp = talk(
            addr,
            "GET /nope HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn query_strings_do_not_break_route_matching() {
        let addr = spin(2);
        let resp = talk(
            addr,
            "GET /health?cachebust=1 HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.contains(r#""ok":true"#), "got: {resp}");
        let resp = talk(
            addr,
            "GET /nope?x=1 HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }

    #[test]
    fn bad_body_is_400() {
        let addr = spin(1);
        let req = "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: 2\r\n\r\n{}";
        let resp = talk(addr, req);
        assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
    }
}

#[cfg(test)]
mod web_tests {
    use super::*;
    use std::net::TcpStream;

    fn spin(n: usize, opts: WebOpts) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut h = |p: &str| format!("echo:{p}");
            for _ in 0..n {
                if let Ok((s, _)) = listener.accept() {
                    let _ = handle_limited(s, "sekrit", &mut h, &opts, None);
                }
            }
        });
        addr
    }

    fn creds() -> WebOpts {
        WebOpts {
            web: true,
            user: "bob".into(),
            pass: format!("sha256:{}", crate::security::sha256_hex(b"hunter2")),
            ..WebOpts::default()
        }
    }

    fn basic_header(user: &str, pass: &str) -> String {
        format!(
            "Authorization: Basic {}\r\n",
            crate::media::b64_encode(format!("{user}:{pass}").as_bytes())
        )
    }

    fn talk(addr: std::net::SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn strong_headers_on_every_response() {
        let addr = spin(2, WebOpts::default());

        let resp = talk(addr, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "health is open by design: {resp}"
        );
        for h in [
            "Content-Security-Policy: default-src 'none'",
            "X-Content-Type-Options: nosniff",
            "X-Frame-Options: DENY",
            "Referrer-Policy: no-referrer",
            "Cross-Origin-Opener-Policy: same-origin",
            "Cross-Origin-Resource-Policy: same-origin",
            "Permissions-Policy: camera=()",
            "Cache-Control: no-store",
        ] {
            assert!(resp.contains(h), "missing {h} in: {resp}");
        }

        let resp = talk(addr, "GET /robots.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.contains("Content-Security-Policy"));
        assert!(resp.contains("X-Robots-Tag: noindex"));
    }

    #[test]
    fn minimal_headers_when_reduced() {
        let mut opts = creds();
        opts.strong_headers = false;
        let addr = spin(1, opts);
        let req = format!(
            "GET / HTTP/1.1\r\nHost: x\r\n{}\r\n",
            basic_header("bob", "hunter2")
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(!resp.contains("Content-Security-Policy"));
        assert!(resp.contains("X-Content-Type-Options: nosniff"));
        assert!(resp.contains("Cache-Control: no-store"));
    }

    #[test]
    fn ui_requires_credentials() {
        let addr = spin(4, creds());

        let resp = talk(addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
        assert!(resp.contains("WWW-Authenticate: Basic"));

        let req = format!(
            "GET / HTTP/1.1\r\nHost: x\r\n{}\r\n",
            basic_header("bob", "wrong")
        );
        assert!(talk(addr, &req).starts_with("HTTP/1.1 401"));

        let req = format!(
            "GET / HTTP/1.1\r\nHost: x\r\n{}\r\n",
            basic_header("bob", "hunter2")
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("text/html"));

        let body = r#"{"prompt":"hi"}"#;
        let req = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\n{}Content-Length: {}\r\n\r\n{body}",
            basic_header("bob", "hunter2"),
            body.len()
        );
        let resp = talk(addr, &req);
        assert!(resp.contains("echo:hi"), "got: {resp}");
    }

    #[test]
    fn the_run_reply_names_the_model_and_sessions_have_rest_endpoints() {
        let d = std::env::temp_dir().join(format!(
            "px-http-sess-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::sessions::save(
            &d,
            "alpha",
            &[crate::providers::Msg::User {
                content: "hi".into(),
                images: Vec::new(),
            }],
        )
        .unwrap();
        let opts = WebOpts {
            model: "openai/gpt-test".into(),
            sessions_dir: d.clone(),
            ..WebOpts::default()
        };
        let addr = spin(5, opts);

        let body = r#"{"prompt":"ping"}"#;
        let req = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\
Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let resp = talk(addr, &req);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(
            resp.contains("X-Actual-Model: openai/gpt-test"),
            "the reply must name the model that served it: {resp}"
        );

        let resp = talk(
            addr,
            "GET /sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.contains("\"alpha\""), "{resp}");
        assert!(
            resp.contains("\"messages\":1") || resp.contains("\"messages\": 1"),
            "{resp}"
        );

        let resp = talk(
            addr,
            "DELETE /sessions/alpha HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(
            resp.contains("\"reset\":\"alpha\"") || resp.contains("\"reset\": \"alpha\""),
            "{resp}"
        );
        assert!(crate::sessions::load(&d, "alpha").is_empty());

        let resp = talk(
            addr,
            "DELETE /sessions/ghost HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 404"), "{resp}");

        let resp = talk(addr, "GET /sessions HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            resp.starts_with("HTTP/1.1 401"),
            "session endpoints must sit behind auth: {resp}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sessions_endpoints_vanish_when_sessions_are_off() {
        let addr = spin(1, WebOpts::default());
        let resp = talk(
            addr,
            "GET /sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer sekrit\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 404"), "{resp}");
    }

    #[test]
    fn no_credentials_configured_means_no_ui() {
        let opts = WebOpts {
            web: true,
            ..WebOpts::default()
        };
        let addr = spin(1, opts);
        let resp = talk(addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    }

    #[test]
    fn web_disabled_hides_ui() {
        let addr = spin(1, WebOpts::default());
        let resp = talk(addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
    }

    fn canvas_tmpfile() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-http-canvas-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("canvas.html")
    }

    fn canvas_opts(file: std::path::PathBuf) -> WebOpts {
        WebOpts {
            canvas: true,
            canvas_file: file,
            user: "bob".into(),
            pass: format!("sha256:{}", crate::security::sha256_hex(b"hunter2")),
            ..WebOpts::default()
        }
    }

    #[test]
    fn canvas_serves_document_and_version_with_creds() {
        let file = canvas_tmpfile();
        crate::canvas::present(&file, "<h1>board</h1>").unwrap();
        let addr = spin(2, canvas_opts(file));
        let auth = basic_header("bob", "hunter2");
        let resp = talk(
            addr,
            &format!("GET /canvas HTTP/1.1\r\nHost: x\r\n{auth}\r\n"),
        );
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("<h1>board</h1>"), "got: {resp}");
        assert!(resp.contains("'unsafe-inline'"), "canvas CSP missing");
        let resp = talk(
            addr,
            &format!("GET /canvas/version HTTP/1.1\r\nHost: x\r\n{auth}\r\n"),
        );
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("\"v\":"), "got: {resp}");
    }

    #[test]
    fn canvas_fails_closed_without_creds_or_auth() {
        let file = canvas_tmpfile();
        let opts = WebOpts {
            canvas: true,
            canvas_file: file.clone(),
            ..WebOpts::default()
        };
        let addr = spin(1, opts);
        let resp = talk(addr, "GET /canvas HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");

        let addr = spin(1, canvas_opts(file.clone()));
        let bad = basic_header("bob", "wrong");
        let resp = talk(
            addr,
            &format!("GET /canvas HTTP/1.1\r\nHost: x\r\n{bad}\r\n"),
        );
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");

        let addr = spin(1, WebOpts::default());
        let resp = talk(addr, "GET /canvas HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
    }

    #[test]
    fn robots_deny_all_by_default_allowlist_optional() {
        let addr = spin(1, WebOpts::default());
        let resp = talk(addr, "GET /robots.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.contains("User-agent: *\nDisallow: /"));
        assert!(resp.contains("X-Robots-Tag: noindex"));
        let opts = WebOpts {
            crawlers: vec!["Googlebot".into()],
            ..WebOpts::default()
        };
        let addr = spin(1, opts);
        let resp = talk(addr, "GET /robots.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.contains("User-agent: Googlebot\nDisallow:\n"));
        assert!(!resp.contains("X-Robots-Tag"));
    }
}
