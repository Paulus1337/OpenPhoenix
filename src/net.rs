use std::io::Read;

use serde_json::Value;

pub const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;

pub fn read_json(resp: ureq::Response, max_bytes: u64) -> Result<Value, String> {
    let cap = max_bytes.clamp(1, MAX_JSON_BYTES);
    let mut buf = Vec::with_capacity(4096);
    resp.into_reader()
        .take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| crate::security::redact(&e.to_string()))?;
    if buf.len() as u64 > cap {
        return Err(format!("response exceeds {cap} bytes"));
    }
    if buf.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&buf).map_err(|e| {
        let head = String::from_utf8_lossy(&buf[..buf.len().min(200)]).to_string();
        format!(
            "bad JSON: {e}: {}",
            crate::security::one_line(&crate::security::redact(&head), 200)
        )
    })
}

#[cfg(test)]
pub fn json(resp: ureq::Response) -> Result<Value, String> {
    read_json(resp, MAX_JSON_BYTES)
}

pub const RETRY_ATTEMPTS: u32 = 3;
pub const RETRY_CAP_SECS: u64 = 60;

pub fn retry_after_secs(resp: &ureq::Response) -> Option<u64> {
    resp.header("retry-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| (v.ceil() as u64).min(RETRY_CAP_SECS))
}

pub fn backoff_secs(attempt: u32, retry_after: Option<u64>) -> u64 {
    match retry_after {
        Some(s) => s.min(RETRY_CAP_SECS),
        None => (1u64 << attempt.min(6)).min(RETRY_CAP_SECS),
    }
}

pub fn should_retry(code: u16) -> bool {
    code == 429 || code == 408 || (500..=599).contains(&code)
}

pub fn sleep_secs(secs: u64) {
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

pub type SendResult = Result<ureq::Response, Box<ureq::Error>>;

pub fn call_retrying(
    mut send: impl FnMut() -> SendResult,
    max_bytes: u64,
    mut sleep: impl FnMut(u64),
) -> Result<Value, String> {
    let mut last = String::new();
    for attempt in 0..RETRY_ATTEMPTS {
        match send() {
            Ok(r) => return read_json(r, max_bytes),
            Err(boxed) => match *boxed {
                ureq::Error::Status(code, r) => {
                    if !should_retry(code) || attempt + 1 == RETRY_ATTEMPTS {
                        let body = read_json(r, 4096)
                            .map(|v| v.to_string())
                            .unwrap_or_else(|e| e);
                        return Err(format!(
                            "HTTP {code}: {}",
                            crate::security::one_line(&crate::security::redact(&body), 200)
                        ));
                    }
                    let wait = backoff_secs(attempt, retry_after_secs(&r));
                    last = format!("HTTP {code}");
                    sleep(wait);
                }
                other => {
                    let msg = crate::security::redact(&other.to_string());
                    if attempt + 1 == RETRY_ATTEMPTS {
                        return Err(crate::security::one_line(&msg, 200));
                    }
                    last = crate::security::one_line(&msg, 120);
                    sleep(backoff_secs(attempt, None));
                }
            },
        }
    }
    Err(format!("gave up after {RETRY_ATTEMPTS} attempts: {last}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn serve(body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let payload = body.to_string();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut r = std::io::BufReader::new(match s.try_clone() {
                    Ok(c) => c,
                    Err(_) => return,
                });
                let mut line = String::new();
                while std::io::BufRead::read_line(&mut r, &mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    line.clear();
                }
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
            }
        });
        format!("http://{addr}/")
    }

    fn get(url: &str) -> ureq::Response {
        ureq::get(url).call().expect("call")
    }

    #[test]
    fn a_json_body_parses() {
        let url = serve(r#"{"ok":true,"n":7}"#);
        let v = json(get(&url)).expect("parses");
        assert_eq!(v["ok"], true);
        assert_eq!(v["n"], 7);
    }

    #[test]
    fn an_empty_body_is_null_not_an_error() {
        let url = serve("");
        assert_eq!(json(get(&url)).expect("empty ok"), Value::Null);
    }

    #[test]
    fn a_body_over_the_cap_is_refused_rather_than_buffered() {
        let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(4096));
        let url = serve(&big);
        let err = read_json(get(&url), 512).expect_err("must refuse");
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn malformed_json_reports_a_clipped_redacted_head() {
        let url = serve(&format!("not json ghp_{}", "a".repeat(36)));
        let err = json(get(&url)).expect_err("must fail");
        assert!(err.starts_with("bad JSON"), "{err}");
        assert!(!err.contains(&"a".repeat(36)), "secret leaked: {err}");
        assert!(err.len() < 400, "error grew to {}", err.len());
    }

    #[test]
    fn retry_after_is_honoured_and_capped() {
        assert_eq!(backoff_secs(0, Some(5)), 5);
        assert_eq!(backoff_secs(0, Some(9_999)), RETRY_CAP_SECS);
        assert_eq!(backoff_secs(0, None), 1);
        assert_eq!(backoff_secs(1, None), 2);
        assert_eq!(backoff_secs(2, None), 4);
        assert!(backoff_secs(60, None) <= RETRY_CAP_SECS);
    }

    #[test]
    fn only_transient_statuses_are_retried() {
        for code in [429, 408, 500, 502, 503, 599] {
            assert!(should_retry(code), "{code} should retry");
        }
        for code in [200, 201, 400, 401, 403, 404, 422] {
            assert!(!should_retry(code), "{code} must not retry");
        }
    }

    #[test]
    fn a_transient_failure_is_retried_then_succeeds() {
        let url = serve(r#"{"ok":true}"#);
        let calls = std::cell::Cell::new(0u32);
        let slept = std::cell::RefCell::new(Vec::new());
        let v = call_retrying(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    return Err(Box::new(ureq::Error::Status(
                        503,
                        ureq::Response::new(503, "Service Unavailable", "busy").expect("resp"),
                    )));
                }
                ureq::get(&url).call().map_err(Box::new)
            },
            1 << 20,
            |s| slept.borrow_mut().push(s),
        )
        .expect("second attempt succeeds");
        assert_eq!(v["ok"], true);
        assert_eq!(calls.get(), 2);
        assert_eq!(slept.borrow().as_slice(), &[1]);
    }

    #[test]
    fn a_permanent_failure_is_not_retried() {
        let calls = std::cell::Cell::new(0u32);
        let err = call_retrying(
            || {
                calls.set(calls.get() + 1);
                Err(Box::new(ureq::Error::Status(
                    403,
                    ureq::Response::new(403, "Forbidden", "nope").expect("resp"),
                )))
            },
            1 << 20,
            |_| panic!("a permanent failure must not sleep"),
        )
        .expect_err("must fail");
        assert!(err.starts_with("HTTP 403"), "{err}");
        assert_eq!(calls.get(), 1, "no retry on a permanent status");
    }

    #[test]
    fn retries_are_bounded_and_the_last_error_is_reported() {
        let calls = std::cell::Cell::new(0u32);
        let err = call_retrying(
            || {
                calls.set(calls.get() + 1);
                Err(Box::new(ureq::Error::Status(
                    429,
                    ureq::Response::new(429, "Too Many Requests", "slow down").expect("resp"),
                )))
            },
            1 << 20,
            |_| {},
        )
        .expect_err("must give up");
        assert_eq!(calls.get(), RETRY_ATTEMPTS);
        assert!(err.starts_with("HTTP 429"), "{err}");
    }

    #[test]
    fn the_cap_is_clamped_to_the_ceiling() {
        let url = serve(r#"{"ok":1}"#);
        assert!(read_json(get(&url), u64::MAX).is_ok());
        let url = serve(r#"{"ok":1}"#);
        assert!(read_json(get(&url), 0).is_err());
    }
}
