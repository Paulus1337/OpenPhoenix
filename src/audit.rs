use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const MAX_AUDIT_BYTES: u64 = 8 * 1024 * 1024;
const KEEP_GENERATIONS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Blocked,
    Error,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Blocked => "blocked",
            Outcome::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Audit {
    path: Option<PathBuf>,
    cap: u64,
}

impl Default for Audit {
    fn default() -> Self {
        Audit::disabled()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

impl Audit {
    pub fn disabled() -> Self {
        Audit {
            path: None,
            cap: MAX_AUDIT_BYTES,
        }
    }

    pub fn at(path: &Path) -> Self {
        Audit {
            path: Some(path.to_path_buf()),
            cap: MAX_AUDIT_BYTES,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_cap(path: &Path, cap: u64) -> Self {
        Audit {
            path: Some(path.to_path_buf()),
            cap: cap.max(1),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }

    pub fn tool(&self, name: &str, args: &Value, outcome: Outcome, detail: &str) {
        self.write(&json!({
            "kind": "tool",
            "tool": name,
            "args": redact_value(args),
            "outcome": outcome.as_str(),
            "detail": crate::security::one_line(&crate::security::redact(detail), 200),
        }));
    }

    pub fn auth(&self, surface: &str, peer: &str, outcome: Outcome, detail: &str) {
        self.write(&json!({
            "kind": "auth",
            "surface": surface,
            "peer": peer,
            "outcome": outcome.as_str(),
            "detail": crate::security::one_line(detail, 200),
        }));
    }

    pub fn turn(&self, channel: &str, session: &str, input_tokens: u64, output_tokens: u64) {
        self.write(&json!({
            "kind": "turn",
            "channel": channel,
            "session": session,
            "usage": {"input": input_tokens, "output": output_tokens},
            "outcome": Outcome::Ok.as_str(),
        }));
    }

    fn write(&self, fields: &Value) {
        let Some(path) = &self.path else { return };
        let mut record = json!({
            "v": 1,
            "ts": now_ms(),
            "pid": std::process::id(),
        });
        if let (Some(dst), Some(src)) = (record.as_object_mut(), fields.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        let mut line = record.to_string();
        line.push('\n');

        let _g = guard();
        if let Some(dir) = path.parent() {
            if fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        if fs::metadata(path).map(|m| m.len()).unwrap_or(0) + line.len() as u64 > self.cap {
            rotate(path);
        }
        let mut opts = fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut fh) = opts.open(path) {
            let _ = fh.write_all(line.as_bytes());
        }
    }
}

fn rotate(path: &Path) {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("audit");
    let gen = |i: usize| path.with_file_name(format!("{name}.{i}"));
    let _ = fs::remove_file(gen(KEEP_GENERATIONS));
    for i in (1..KEEP_GENERATIONS).rev() {
        let _ = fs::rename(gen(i), gen(i + 1));
    }
    let _ = fs::rename(path, gen(1));
}

fn redact_value(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            Value::String(crate::security::one_line(&crate::security::redact(s), 300))
        }
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, val)| {
                    let sensitive = ["token", "key", "secret", "password", "authorization"]
                        .iter()
                        .any(|s| k.to_ascii_lowercase().contains(s));
                    let out = if sensitive {
                        Value::String("[redacted]".into())
                    } else {
                        redact_value(val)
                    };
                    (k.clone(), out)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("phx-audit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn lines(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("every audit line must be valid JSON"))
            .collect()
    }

    #[test]
    fn every_record_is_one_valid_json_object_per_line() {
        let d = dir("jsonl");
        let path = d.join("audit.jsonl");
        let a = Audit::at(&path);
        a.tool("shell", &json!({"command": "ls"}), Outcome::Ok, "listed");
        a.auth("http", "10.0.0.1", Outcome::Blocked, "bad token");
        a.turn("telegram", "chat:1", 120, 45);

        let recs = lines(&path);
        assert_eq!(recs.len(), 3);
        for r in &recs {
            assert_eq!(r["v"], 1, "records must carry a schema version");
            assert!(r["ts"].as_u64().unwrap_or(0) > 0);
            assert!(r["kind"].is_string());
            assert!(r["outcome"].is_string());
        }
        assert_eq!(recs[0]["tool"], "shell");
        assert_eq!(recs[1]["peer"], "10.0.0.1");
        assert_eq!(recs[2]["usage"]["input"], 120);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn secrets_never_reach_the_audit_log() {
        let d = dir("redact");
        let path = d.join("audit.jsonl");
        let a = Audit::at(&path);
        let ghp = format!("ghp_{}", "a".repeat(36));
        a.tool(
            "http_get",
            &json!({"url": "https://x", "api_key": "***", "nested": {"token": "***"}}),
            Outcome::Ok,
            &format!("used {ghp}"),
        );

        let body = fs::read_to_string(&path).unwrap();
        assert!(!body.contains("super-secret-value"), "{body}");
        assert!(!body.contains("nested-secret"), "{body}");
        assert!(
            !body.contains(&ghp),
            "pattern secrets must be redacted: {body}"
        );
        assert!(body.contains("[redacted]"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_disabled_audit_writes_nothing() {
        let d = dir("off");
        let a = Audit::disabled();
        a.tool("shell", &json!({}), Outcome::Ok, "x");
        assert!(!a.enabled());
        assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_writers_never_interleave_a_line() {
        let d = dir("parallel");
        let path = d.join("audit.jsonl");
        let mut handles = Vec::new();
        for i in 0..12 {
            let p = path.clone();
            handles.push(std::thread::spawn(move || {
                let a = Audit::at(&p);
                a.tool("shell", &json!({"n": i}), Outcome::Ok, "done");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            lines(&path).len(),
            12,
            "each writer must contribute exactly one parseable line"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_single_huge_argument_cannot_bloat_one_record() {
        let d = dir("clip");
        let path = d.join("audit.jsonl");
        let a = Audit::at(&path);
        a.tool(
            "shell",
            &json!({"command": "x".repeat(200_000)}),
            Outcome::Ok,
            &"y".repeat(200_000),
        );
        let len = fs::metadata(&path).unwrap().len();
        assert!(len < 2_000, "one record grew to {len} bytes");
        assert_eq!(lines(&path).len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_log_rotates_instead_of_growing_without_bound() {
        let d = dir("rotate");
        let path = d.join("audit.jsonl");
        let a = Audit::with_cap(&path, 2_000);
        for i in 0..400 {
            a.tool("shell", &json!({"n": i}), Outcome::Ok, "ok");
        }
        assert!(
            fs::metadata(&path).unwrap().len() <= 2_000,
            "live log must stay under the cap"
        );
        assert!(d.join("audit.jsonl.1").exists(), "a generation must exist");
        assert!(
            !d.join("audit.jsonl.4").exists(),
            "history must stay bounded"
        );
        let _ = fs::remove_dir_all(&d);
    }
}
