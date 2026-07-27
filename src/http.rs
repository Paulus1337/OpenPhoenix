use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

const MAX_BODY: usize = 65_536;

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const STYLE_CSS: &str = include_str!("web/style.css");
const LOGO_SVG: &str = include_str!("../assets/phoenix.svg");
const EGG_SVG: &str = include_str!("../assets/phoenix-egg.svg");

#[derive(Debug, Clone)]
pub struct WebOpts {
    pub web: bool,

    pub strong_headers: bool,

    pub user: String,

    pub pass: String,

    pub crawlers: Vec<String>,

    pub canvas: bool,

    pub canvas_file: std::path::PathBuf,
}

impl Default for WebOpts {
    fn default() -> Self {
        WebOpts {
            web: false,
            strong_headers: true,
            user: String::new(),
            pass: String::new(),
            crawlers: Vec::new(),
            canvas: false,
            canvas_file: std::path::PathBuf::new(),
        }
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
    u == opts.user && crate::security::sha256_hex(p.as_bytes()) == want
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

pub fn serve(
    listener: TcpListener,
    token: &str,
    handler: &mut dyn FnMut(&str) -> String,
    opts: &WebOpts,
) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = handle(stream, token, handler, opts);
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

pub fn handle(
    mut stream: TcpStream,
    token: &str,
    handler: &mut dyn FnMut(&str) -> String,
    opts: &WebOpts,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut auth_val = String::new();
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
            let key = k.trim().to_ascii_lowercase();
            let v = v.trim();
            if key == "authorization" {
                auth_val = v.to_string();
            } else if key == "content-length" {
                content_len = v.parse().unwrap_or(0);
            }
        }
    }

    let bearer = !token.is_empty() && auth_val == format!("Bearer {token}");
    let basic = basic_ok(&auth_val, opts);
    let authed = bearer || basic;

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
    if !authed {
        return respond(
            &mut stream,
            401,
            "Unauthorized",
            &json!({"error": "unauthorized"}),
            opts,
        );
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/health") => respond(&mut stream, 200, "OK", &json!({"ok": true}), opts),
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
                    let _ = handle(s, "sekrit", &mut h, &WebOpts::default());
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
    fn missing_or_wrong_token_is_401() {
        let addr = spin(2);
        let resp = talk(addr, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 401"), "got: {resp}");
        let resp = talk(
            addr,
            "GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrong\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 401"));
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
                    let _ = handle(s, "sekrit", &mut h, &opts);
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
        assert!(resp.starts_with("HTTP/1.1 401"));
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
