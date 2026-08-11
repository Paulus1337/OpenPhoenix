use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    fn value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Error => 1,
            Self::Warn => 2,
            Self::Info => 3,
            Self::Debug => 4,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

#[derive(Default)]
pub struct Fields<'a> {
    session: Option<&'a str>,
    channel: Option<&'a str>,
    provider: Option<&'a str>,
    duration_ms: Option<u64>,
}

impl<'a> Fields<'a> {
    pub fn session(mut self, value: &'a str) -> Self {
        self.session = Some(value);
        self
    }

    pub fn channel(mut self, value: &'a str) -> Self {
        self.channel = Some(value);
        self
    }

    pub fn provider(mut self, value: &'a str) -> Self {
        self.provider = Some(value);
        self
    }

    pub fn duration_ms(mut self, value: u64) -> Self {
        self.duration_ms = Some(value);
        self
    }
}

struct JsonLogger<W: Write> {
    level: AtomicU8,
    secrets: RwLock<Vec<String>>,
    writer: Mutex<W>,
}

impl<W: Write> JsonLogger<W> {
    fn new(writer: W) -> Self {
        Self {
            level: AtomicU8::new(Level::Error.value()),
            secrets: RwLock::new(Vec::new()),
            writer: Mutex::new(writer),
        }
    }

    fn configure(&self, level: Level, secrets: Vec<String>) {
        self.level.store(level.value(), Ordering::Release);
        let mut stored = self.secrets.write().unwrap_or_else(|e| e.into_inner());
        *stored = secrets;
    }

    fn enabled(&self, level: Level) -> bool {
        level != Level::Off && self.level.load(Ordering::Acquire) >= level.value()
    }

    fn emit(&self, level: Level, module: &str, msg: &str, fields: &Fields<'_>) {
        self.emit_at(SystemTime::now(), level, module, msg, fields);
    }

    fn emit_at(&self, now: SystemTime, level: Level, module: &str, msg: &str, fields: &Fields<'_>) {
        if !self.enabled(level) {
            return;
        }
        let secrets = self.secrets.read().unwrap_or_else(|e| e.into_inner());
        let mut event = serde_json::Map::new();
        event.insert("ts".into(), serde_json::Value::String(timestamp(now)));
        event.insert(
            "level".into(),
            serde_json::Value::String(level.name().to_string()),
        );
        event.insert(
            "module".into(),
            serde_json::Value::String(clean_field(module, &secrets)),
        );
        event.insert(
            "msg".into(),
            serde_json::Value::String(clean_message(msg, &secrets)),
        );
        if let Some(value) = fields.session {
            event.insert(
                "session".into(),
                serde_json::Value::String(clean_field(value, &secrets)),
            );
        }
        if let Some(value) = fields.channel {
            event.insert(
                "channel".into(),
                serde_json::Value::String(clean_field(value, &secrets)),
            );
        }
        if let Some(value) = fields.provider {
            event.insert(
                "provider".into(),
                serde_json::Value::String(clean_field(value, &secrets)),
            );
        }
        if let Some(value) = fields.duration_ms {
            event.insert("duration_ms".into(), serde_json::Value::from(value));
        }
        let Ok(mut line) = serde_json::to_vec(&serde_json::Value::Object(event)) else {
            return;
        };
        line.push(b'\n');
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writer.write_all(&line);
        let _ = writer.flush();
    }
}

static LOGGER: OnceLock<JsonLogger<io::Stderr>> = OnceLock::new();

fn global() -> &'static JsonLogger<io::Stderr> {
    LOGGER.get_or_init(|| JsonLogger::new(io::stderr()))
}

pub fn init(cfg: &crate::config::Config) {
    let level = Level::parse(&cfg.log_level).unwrap_or(Level::Error);
    global().configure(level, cfg.secret_values());
}

pub fn error(module: &str, msg: impl AsRef<str>) {
    emit(Level::Error, module, msg.as_ref(), &Fields::default());
}

pub fn error_with(module: &str, msg: impl AsRef<str>, fields: &Fields<'_>) {
    emit(Level::Error, module, msg.as_ref(), fields);
}

pub fn warn(module: &str, msg: impl AsRef<str>) {
    emit(Level::Warn, module, msg.as_ref(), &Fields::default());
}

pub fn warn_with(module: &str, msg: impl AsRef<str>, fields: &Fields<'_>) {
    emit(Level::Warn, module, msg.as_ref(), fields);
}

pub fn info(module: &str, msg: impl AsRef<str>) {
    emit(Level::Info, module, msg.as_ref(), &Fields::default());
}

pub fn info_with(module: &str, msg: impl AsRef<str>, fields: &Fields<'_>) {
    emit(Level::Info, module, msg.as_ref(), fields);
}

pub fn debug(module: &str, msg: impl AsRef<str>) {
    emit(Level::Debug, module, msg.as_ref(), &Fields::default());
}

pub fn debug_with(module: &str, msg: impl AsRef<str>, fields: &Fields<'_>) {
    emit(Level::Debug, module, msg.as_ref(), fields);
}

fn emit(level: Level, module: &str, msg: &str, fields: &Fields<'_>) {
    global().emit(level, module, msg, fields);
}

pub fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn timestamp(now: SystemTime) -> String {
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    let millis = duration.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let hour = day_seconds / 3_600;
    let minute = day_seconds.rem_euclid(3_600) / 60;
    let second = day_seconds.rem_euclid(60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_base = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year_base + 1 } else { year_base };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn clean_field(value: &str, secrets: &[String]) -> String {
    let value = mask_known(value, secrets);
    let value = crate::security::redact(&value);
    sanitize_paths(&value)
}

fn clean_message(value: &str, secrets: &[String]) -> String {
    let trimmed = value.trim();
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return "[redacted structured body]".to_string();
    }
    let value = mask_known(value, secrets);
    let value = redact_embedded_json(&value);
    let value = crate::security::redact(&value);
    let value = redact_labeled_values(&value);
    sanitize_paths(&value)
}

fn redact_embedded_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some((start, end)) = next_json_span(value, cursor) {
        out.push_str(value.get(cursor..start).unwrap_or(""));
        out.push_str("[redacted structured body]");
        cursor = end;
    }
    out.push_str(value.get(cursor..).unwrap_or(""));
    out
}

fn next_json_span(value: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    for start in from..bytes.len() {
        let close = match bytes[start] {
            b'{' => b'}',
            b'[' => b']',
            _ => continue,
        };
        let mut stack = vec![close];
        let mut quoted = false;
        let mut escaped = false;
        for (at, byte) in bytes.iter().copied().enumerate().skip(start + 1) {
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                }
                continue;
            }
            if byte == b'"' {
                quoted = true;
                continue;
            }
            match byte {
                b'{' => stack.push(b'}'),
                b'[' => stack.push(b']'),
                b'}' | b']' => {
                    if stack.pop() != Some(byte) {
                        break;
                    }
                    if stack.is_empty() {
                        let end = at + 1;
                        if serde_json::from_str::<serde_json::Value>(
                            value.get(start..end).unwrap_or(""),
                        )
                        .is_ok()
                        {
                            return Some((start, end));
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn mask_known(value: &str, secrets: &[String]) -> String {
    let mut known: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    known.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = value.to_string();
    for secret in known {
        if secret.chars().count() >= 4 {
            out = out.replace(secret.as_str(), "[redacted]");
        } else if out == *secret {
            out = "[redacted]".to_string();
        }
    }
    out
}

fn redact_labeled_values(value: &str) -> String {
    const LABELS: &[&str] = &[
        "authorization",
        "credential",
        "api_key",
        "apikey",
        "app_token",
        "bot_token",
        "access_token",
        "refresh_token",
        "verify_token",
        "password",
        "secret",
        "cookie",
        "token",
        "message body",
        "message",
        "content",
        "prompt",
        "input",
        "output",
        "body",
        "text",
        "args",
    ];
    let lower = value.to_ascii_lowercase();
    let mut hits = Vec::new();
    for label in LABELS {
        let mut from = 0usize;
        while let Some(rel) = lower.get(from..).and_then(|s| s.find(label)) {
            let start = from + rel;
            let end = start + label.len();
            let before_ok = start == 0
                || !lower
                    .as_bytes()
                    .get(start - 1)
                    .is_some_and(u8::is_ascii_alphanumeric);
            let after_ok = !lower
                .as_bytes()
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            if before_ok && after_ok {
                let mut at = end;
                while lower.as_bytes().get(at) == Some(&b' ') {
                    at += 1;
                }
                if matches!(lower.as_bytes().get(at), Some(b'=') | Some(b':')) {
                    at += 1;
                    while lower
                        .as_bytes()
                        .get(at)
                        .is_some_and(u8::is_ascii_whitespace)
                    {
                        at += 1;
                    }
                    let body_label = matches!(
                        *label,
                        "message body"
                            | "message"
                            | "content"
                            | "prompt"
                            | "input"
                            | "output"
                            | "body"
                            | "text"
                            | "args"
                    );
                    let finish = labeled_value_end(value, at, body_label);
                    if finish > at {
                        hits.push((at, finish));
                    }
                }
            }
            from = end;
        }
    }
    if hits.is_empty() {
        return value.to_string();
    }
    hits.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in hits {
        match merged.last_mut() {
            Some((_, prior_end)) if start <= *prior_end => *prior_end = (*prior_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut out = String::with_capacity(value.len());
    let mut at = 0usize;
    for (start, end) in merged {
        out.push_str(value.get(at..start).unwrap_or(""));
        out.push_str("[redacted]");
        at = end;
    }
    out.push_str(value.get(at..).unwrap_or(""));
    out
}

fn labeled_value_end(value: &str, start: usize, body_label: bool) -> usize {
    let bytes = value.as_bytes();
    let quote = bytes
        .get(start)
        .copied()
        .filter(|b| matches!(b, b'\'' | b'"'));
    if let Some(quote) = quote {
        return bytes
            .get(start + 1..)
            .and_then(|rest| rest.iter().position(|b| *b == quote))
            .map(|rel| start + rel + 2)
            .unwrap_or(value.len());
    }
    if body_label {
        return value.len();
    }
    bytes
        .get(start..)
        .and_then(|rest| {
            rest.iter()
                .position(|b| b.is_ascii_whitespace() || matches!(b, b',' | b';' | b'}' | b']'))
        })
        .map(|rel| start + rel)
        .unwrap_or(value.len())
}

fn sanitize_paths(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut at = 0usize;
    while at < value.len() {
        let rest = value.get(at..).unwrap_or("");
        if let Some((prefix, path_start)) = path_start(rest) {
            out.push_str(rest.get(..prefix).unwrap_or(""));
            out.push_str("[path]");
            let end = path_end(rest, path_start);
            at += end.max(path_start + 1);
        } else {
            out.push_str(rest);
            break;
        }
    }
    out
}

fn path_start(value: &str) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    for index in 0..bytes.len() {
        let prior = index.checked_sub(1).and_then(|at| bytes.get(at)).copied();
        let boundary = index == 0
            || prior.is_some_and(|byte| {
                byte.is_ascii_whitespace()
                    || matches!(byte, b'=' | b':' | b'"' | b'\'' | b'(' | b'[' | b'{')
            });
        if !boundary {
            continue;
        }
        let rest = value.get(index..).unwrap_or("");
        if rest.starts_with("file:///") {
            return Some((index, index + "file://".len()));
        }
        if rest.starts_with("~/") || (rest.starts_with('/') && !rest.starts_with("//")) {
            return Some((index, index));
        }
        if rest.as_bytes().get(1) == Some(&b':')
            && matches!(rest.as_bytes().get(2), Some(b'/') | Some(b'\\'))
            && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        {
            return Some((index, index));
        }
    }
    None
}

fn path_end(value: &str, start: usize) -> usize {
    value
        .get(start..)
        .unwrap_or("")
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | ',' | ';'))
        .map(|(relative, _)| start + relative)
        .unwrap_or(value.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn lines(logger: &JsonLogger<Vec<u8>>) -> Vec<serde_json::Value> {
        let bytes = logger.writer.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect()
    }

    #[test]
    fn default_level_is_error_and_filtering_obeys_every_level() {
        let logger = JsonLogger::new(Vec::new());
        logger.emit(Level::Warn, "test", "hidden", &Fields::default());
        logger.emit(Level::Error, "test", "shown", &Fields::default());
        assert_eq!(lines(&logger).len(), 1);
        logger.configure(Level::Info, Vec::new());
        logger.emit(Level::Debug, "test", "hidden", &Fields::default());
        logger.emit(Level::Info, "test", "info", &Fields::default());
        logger.emit(Level::Warn, "test", "warn", &Fields::default());
        logger.emit(Level::Error, "test", "error", &Fields::default());
        assert_eq!(lines(&logger).len(), 4);
        logger.configure(Level::Off, Vec::new());
        logger.emit(Level::Error, "test", "hidden", &Fields::default());
        assert_eq!(lines(&logger).len(), 4);
    }

    #[test]
    fn output_is_json_lines_with_required_and_optional_fields() {
        let logger = JsonLogger::new(Vec::new());
        logger.configure(Level::Debug, Vec::new());
        let fields = Fields::default()
            .session("s1")
            .channel("telegram")
            .provider("openai")
            .duration_ms(42);
        logger.emit_at(
            UNIX_EPOCH + Duration::from_millis(1_704_164_645_006),
            Level::Debug,
            "agent",
            "request completed",
            &fields,
        );
        let event = &lines(&logger)[0];
        assert_eq!(event["ts"], "2024-01-02T03:04:05.006Z");
        assert_eq!(event["level"], "debug");
        assert_eq!(event["module"], "agent");
        assert_eq!(event["msg"], "request completed");
        assert_eq!(event["session"], "s1");
        assert_eq!(event["channel"], "telegram");
        assert_eq!(event["provider"], "openai");
        assert_eq!(event["duration_ms"], 42);
    }

    #[test]
    fn credentials_message_bodies_raw_json_and_paths_are_redacted() {
        let logger = JsonLogger::new(Vec::new());
        logger.configure(Level::Debug, vec!["opaque-live-credential".into()]);
        logger.emit(
            Level::Error,
            "test",
            "token=odd-token password: hunter2 at /home/alice/private.txt path=/srv/private file:///var/private C:\\Users\\Alice\\private opaque-live-credential",
            &Fields::default(),
        );
        logger.emit(
            Level::Debug,
            "test",
            "message body: private words that must not survive",
            &Fields::default(),
        );
        logger.emit(
            Level::Debug,
            "test",
            r#"{"messages":[{"content":"private"}],"token":"odd"}"#,
            &Fields::default(),
        );
        logger.emit(
            Level::Error,
            "provider",
            r#"HTTP 400: {"error":{"message":"embedded private prompt"}} retrying"#,
            &Fields::default(),
        );
        let events = lines(&logger);
        let joined = serde_json::to_string(&events).expect("json");
        for secret in [
            "odd-token",
            "hunter2",
            "/home/alice/private.txt",
            "/srv/private",
            "/var/private",
            "C:\\Users\\Alice\\private",
            "opaque-live-credential",
            "private words",
            "private\"",
            "embedded private prompt",
        ] {
            assert!(!joined.contains(secret), "secret leaked: {joined}");
        }
        assert!(joined.contains("[redacted]"), "{joined}");
        assert!(joined.contains("[path]"), "{joined}");
        assert!(joined.contains("redacted structured body"), "{joined}");
    }

    #[test]
    fn concurrent_events_remain_one_valid_json_object_per_line() {
        let logger = Arc::new(JsonLogger::new(Vec::new()));
        logger.configure(Level::Info, Vec::new());
        let mut handles = Vec::new();
        for thread_id in 0..8 {
            let logger = Arc::clone(&logger);
            handles.push(std::thread::spawn(move || {
                for event_id in 0..100 {
                    logger.emit(
                        Level::Info,
                        "concurrent",
                        &format!("thread={thread_id} event={event_id}"),
                        &Fields::default(),
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().expect("writer thread");
        }
        let events = lines(&logger);
        assert_eq!(events.len(), 800);
        assert!(events.iter().all(|event| event["module"] == "concurrent"));
    }

    #[test]
    fn level_parser_accepts_only_the_contract_values() {
        for (raw, level) in [
            ("off", Level::Off),
            ("error", Level::Error),
            ("warn", Level::Warn),
            ("info", Level::Info),
            ("debug", Level::Debug),
        ] {
            assert_eq!(Level::parse(raw), Some(level));
        }
        assert_eq!(Level::parse("trace"), None);
        assert_eq!(Level::parse("INFO"), None);
    }
}
