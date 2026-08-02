use serde_json::Value;

use crate::security::sha256_hex;

const HISTORY_SIZE: usize = 30;
const WARNING_THRESHOLD: usize = 10;
const UNKNOWN_TOOL_THRESHOLD: usize = 10;
const CRITICAL_THRESHOLD: usize = 20;
const GLOBAL_CIRCUIT_BREAKER_THRESHOLD: usize = 30;
const WARNED_KEYS_MAX: usize = 64;

pub const LOOP_BLOCK_PREFIX: &str = "tool loop blocked:";

const POLL_TOOLS: [&str; 2] = ["bg_list", "bg_result"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    Ok,
    Warn(String),
    Block(String),
}

#[derive(Debug, Clone)]
struct Record {
    tool: String,
    args_hash: String,
    result_hash: Option<String>,
    unknown_tool: Option<String>,
}

#[derive(Debug, Default)]
pub struct LoopDetector {
    history: Vec<Record>,
    warned: Vec<String>,
}

fn stable_write(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*k).to_string()).to_string());
                out.push(':');
                if let Some(nested) = m.get(*k) {
                    stable_write(nested, out);
                }
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                stable_write(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

pub fn stable_json(v: &Value) -> String {
    let mut out = String::new();
    stable_write(v, &mut out);
    out
}

fn hash_call(tool: &str, args: &Value) -> String {
    format!("{tool}:{}", sha256_hex(stable_json(args).as_bytes()))
}

fn is_poll_tool(tool: &str) -> bool {
    POLL_TOOLS.contains(&tool)
}

fn unknown_tool_name(result: &str) -> Option<String> {
    let low = result.to_lowercase();
    let at = low.find("unknown tool")?;
    let rest = &low[at + "unknown tool".len()..];
    let name: String = rest
        .chars()
        .skip_while(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn unknown_tool_streak(history: &[Record], tool: &str) -> (usize, Option<String>) {
    let mut streak = 0usize;
    let mut repeated: Option<String> = None;
    for record in history.iter().rev() {
        if record.tool != tool {
            break;
        }
        let Some(name) = record.unknown_tool.as_ref() else {
            break;
        };
        match &repeated {
            None => {
                repeated = Some(name.clone());
                streak = 1;
            }
            Some(seen) if seen == name => streak += 1,
            Some(_) => break,
        }
    }
    (streak, repeated)
}

fn no_progress_streak(history: &[Record], tool: &str, args_hash: &str) -> (usize, Option<String>) {
    let mut streak = 0usize;
    let mut latest: Option<String> = None;
    for record in history.iter().rev() {
        if record.tool != tool || record.args_hash != args_hash {
            continue;
        }
        let Some(hash) = record.result_hash.as_ref() else {
            continue;
        };
        match &latest {
            None => {
                latest = Some(hash.clone());
                streak = 1;
            }
            Some(seen) if seen == hash => streak += 1,
            Some(_) => break,
        }
    }
    (streak, latest)
}

struct PingPong {
    count: usize,
    paired_signature: Option<String>,
    no_progress_evidence: bool,
}

fn ping_pong_streak(history: &[Record], current: &str) -> PingPong {
    let none = PingPong {
        count: 0,
        paired_signature: None,
        no_progress_evidence: false,
    };
    let Some(last) = history.last() else {
        return none;
    };
    let mut other: Option<&Record> = None;
    for record in history[..history.len() - 1].iter().rev() {
        if record.args_hash != last.args_hash {
            other = Some(record);
            break;
        }
    }
    let Some(other) = other else {
        return none;
    };

    let mut alternating = 0usize;
    for record in history.iter().rev() {
        let expected = if alternating.is_multiple_of(2) {
            &last.args_hash
        } else {
            &other.args_hash
        };
        if &record.args_hash != expected {
            break;
        }
        alternating += 1;
    }
    if alternating < 2 || current != other.args_hash {
        return none;
    }

    let tail_start = history.len().saturating_sub(alternating);
    let mut first_a: Option<&String> = None;
    let mut first_b: Option<&String> = None;
    let mut evidence = true;
    for record in &history[tail_start..] {
        let Some(hash) = record.result_hash.as_ref() else {
            evidence = false;
            break;
        };
        let slot = if record.args_hash == last.args_hash {
            &mut first_a
        } else if record.args_hash == other.args_hash {
            &mut first_b
        } else {
            evidence = false;
            break;
        };
        match slot {
            None => *slot = Some(hash),
            Some(seen) if *seen == hash => {}
            Some(_) => {
                evidence = false;
                break;
            }
        }
    }
    if first_a.is_none() || first_b.is_none() {
        evidence = false;
    }

    PingPong {
        count: alternating + 1,
        paired_signature: Some(other.args_hash.clone()),
        no_progress_evidence: evidence,
    }
}

fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

impl LoopDetector {
    pub fn new() -> Self {
        LoopDetector::default()
    }

    pub fn record(&mut self, tool: &str, args: &Value, result: &str) {
        let result_hash = if result.starts_with(LOOP_BLOCK_PREFIX) {
            None
        } else {
            Some(sha256_hex(result.as_bytes()))
        };
        let unknown_tool = if result_hash.is_some() {
            unknown_tool_name(result)
        } else {
            None
        };
        self.history.push(Record {
            tool: tool.to_string(),
            args_hash: hash_call(tool, args),
            result_hash,
            unknown_tool,
        });
        if self.history.len() > HISTORY_SIZE {
            let excess = self.history.len() - HISTORY_SIZE;
            self.history.drain(..excess);
        }
    }

    fn warn_once(&mut self, key: String, message: String) -> Detection {
        if self.warned.contains(&key) {
            return Detection::Ok;
        }
        self.warned.push(key);
        if self.warned.len() > WARNED_KEYS_MAX {
            let excess = self.warned.len() - WARNED_KEYS_MAX;
            self.warned.drain(..excess);
        }
        Detection::Warn(message)
    }

    pub fn detect(&mut self, tool: &str, args: &Value) -> Detection {
        let current = hash_call(tool, args);
        let (unknown_count, unknown_name) = unknown_tool_streak(&self.history, tool);
        let (no_progress, latest_result) = no_progress_streak(&self.history, tool, &current);
        let vetoed_repeats = self
            .history
            .iter()
            .filter(|r| r.tool == tool && r.args_hash == current && r.result_hash.is_none())
            .count();
        let total_no_progress = no_progress + vetoed_repeats;
        let poll = is_poll_tool(tool);
        let ping_pong = ping_pong_streak(&self.history, &current);

        if unknown_count >= UNKNOWN_TOOL_THRESHOLD {
            let name = unknown_name.unwrap_or_else(|| tool.to_string());
            return Detection::Block(format!(
                "{LOOP_BLOCK_PREFIX} attempted the unavailable tool '{name}' {unknown_count} \
times. Stop retrying that missing tool and answer without it."
            ));
        }

        if total_no_progress >= GLOBAL_CIRCUIT_BREAKER_THRESHOLD {
            return Detection::Block(format!(
                "{LOOP_BLOCK_PREFIX} '{tool}' produced identical no-progress outcomes \
{total_no_progress} times. Stopping to prevent a runaway loop; report what you have instead."
            ));
        }

        if poll && no_progress >= CRITICAL_THRESHOLD {
            return Detection::Block(format!(
                "{LOOP_BLOCK_PREFIX} '{tool}' polled with identical arguments and no progress \
{no_progress} times. Stopping the poll loop; report the task as stuck."
            ));
        }

        if poll && no_progress >= WARNING_THRESHOLD {
            let key = format!(
                "poll:{current}:{}",
                latest_result.clone().unwrap_or_default()
            );
            return self.warn_once(
                key,
                format!(
                    "loop warning: '{tool}' has returned the same result {no_progress} times. \
Wait longer between checks or report the task as stuck instead of polling again."
                ),
            );
        }

        let pp_key = match &ping_pong.paired_signature {
            Some(sig) => format!("pingpong:{}", pair_key(&current, sig)),
            None => format!("pingpong:{tool}:{current}"),
        };

        if ping_pong.count >= CRITICAL_THRESHOLD && ping_pong.no_progress_evidence {
            return Detection::Block(format!(
                "{LOOP_BLOCK_PREFIX} alternating between two tool calls for {} turns with no \
progress. Stopping the ping-pong loop; report what you have instead.",
                ping_pong.count
            ));
        }

        if ping_pong.count >= WARNING_THRESHOLD {
            return self.warn_once(
                pp_key,
                format!(
                    "loop warning: you are alternating between the same two tool calls ({} \
consecutive calls). Change approach or report the task as blocked.",
                    ping_pong.count
                ),
            );
        }

        if !poll && no_progress >= CRITICAL_THRESHOLD {
            return Detection::Block(format!(
                "{LOOP_BLOCK_PREFIX} '{tool}' was called with identical arguments and identical \
results {no_progress} times. Stopping to prevent a runaway loop."
            ));
        }

        if !poll {
            let repeats = self
                .history
                .iter()
                .filter(|r| r.tool == tool && r.args_hash == current)
                .count();
            if repeats >= WARNING_THRESHOLD {
                let key = format!("generic:{current}");
                return self.warn_once(
                    key,
                    format!(
                        "loop warning: '{tool}' has been called {repeats} times with identical \
arguments. If this is not making progress, stop and report instead of retrying."
                    ),
                );
            }
        }

        Detection::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args() -> Value {
        json!({"path": "a.txt"})
    }

    #[test]
    fn stable_json_is_key_order_independent() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"d":2,"c":3}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"c":3,"d":2},"b":1}"#).unwrap();
        assert_eq!(stable_json(&a), stable_json(&b));
        assert_eq!(hash_call("read_file", &a), hash_call("read_file", &b));
    }

    #[test]
    fn identical_args_with_changing_results_never_blocks() {
        let mut d = LoopDetector::new();
        for i in 0..40 {
            match d.detect("read_file", &args()) {
                Detection::Block(m) => panic!("blocked real progress at call {i}: {m}"),
                Detection::Warn(_) | Detection::Ok => {}
            }
            d.record("read_file", &args(), &format!("contents version {i}"));
        }
    }

    #[test]
    fn changing_results_warn_at_most_once_and_never_escalate() {
        let mut d = LoopDetector::new();
        let mut warnings = 0usize;
        for i in 0..40 {
            match d.detect("read_file", &args()) {
                Detection::Warn(_) => warnings += 1,
                Detection::Block(m) => panic!("blocked real progress at call {i}: {m}"),
                Detection::Ok => {}
            }
            d.record("read_file", &args(), &format!("contents version {i}"));
        }
        assert_eq!(
            warnings, 1,
            "one nudge, never a block, when work progresses"
        );
    }

    #[test]
    fn identical_args_and_results_warns_then_blocks() {
        let mut d = LoopDetector::new();
        let mut warned = false;
        let mut blocked_at = None;
        for i in 0..CRITICAL_THRESHOLD + 2 {
            match d.detect("read_file", &args()) {
                Detection::Ok => {}
                Detection::Warn(m) => {
                    assert!(m.contains("loop warning"), "{m}");
                    warned = true;
                }
                Detection::Block(m) => {
                    assert!(m.starts_with(LOOP_BLOCK_PREFIX), "{m}");
                    blocked_at = Some(i);
                    break;
                }
            }
            d.record("read_file", &args(), "same output");
        }
        assert!(warned, "expected a warning before the block");
        assert_eq!(blocked_at, Some(CRITICAL_THRESHOLD));
    }

    #[test]
    fn warning_is_emitted_only_once_per_key() {
        let mut d = LoopDetector::new();
        let mut warnings = 0usize;
        for _ in 0..CRITICAL_THRESHOLD {
            if let Detection::Warn(_) = d.detect("read_file", &args()) {
                warnings += 1;
            }
            d.record("read_file", &args(), "same output");
        }
        assert_eq!(warnings, 1);
    }

    #[test]
    fn different_args_do_not_accumulate() {
        let mut d = LoopDetector::new();
        for i in 0..40 {
            let a = json!({"path": format!("file-{i}.txt")});
            assert_eq!(d.detect("read_file", &a), Detection::Ok);
            d.record("read_file", &a, "same output");
        }
    }

    #[test]
    fn poll_tool_blocks_earlier_than_generic_and_says_poll() {
        let mut d = LoopDetector::new();
        let a = json!({"id": 1});
        let mut blocked = None;
        for i in 0..CRITICAL_THRESHOLD + 2 {
            if let Detection::Block(m) = d.detect("bg_result", &a) {
                assert!(m.contains("polled"), "{m}");
                blocked = Some(i);
                break;
            }
            d.record("bg_result", &a, "still running");
        }
        assert_eq!(blocked, Some(CRITICAL_THRESHOLD));
    }

    #[test]
    fn unknown_tool_repeat_blocks_with_the_tool_name() {
        let mut d = LoopDetector::new();
        let a = json!({});
        let mut blocked = None;
        for i in 0..UNKNOWN_TOOL_THRESHOLD + 2 {
            if let Detection::Block(m) = d.detect("nope", &a) {
                assert!(m.contains("nope"), "{m}");
                assert!(m.contains("unavailable tool"), "{m}");
                blocked = Some(i);
                break;
            }
            d.record("nope", &a, "error: unknown tool 'nope'");
        }
        assert_eq!(blocked, Some(UNKNOWN_TOOL_THRESHOLD));
    }

    #[test]
    fn unknown_tool_extraction_handles_quotes_and_case() {
        assert_eq!(
            unknown_tool_name("error: unknown tool 'read_file'"),
            Some("read_file".into())
        );
        assert_eq!(
            unknown_tool_name("Unknown tool: web-search"),
            Some("web-search".into())
        );
        assert_eq!(unknown_tool_name("some other failure"), None);
    }

    #[test]
    fn unknown_tool_extraction_is_utf8_safe() {
        assert_eq!(
            unknown_tool_name("ERROR from Ä: unknown tool 'grep'"),
            Some("grep".into())
        );
    }

    #[test]
    fn ping_pong_alternation_warns() {
        let mut d = LoopDetector::new();
        let a = json!({"n": 1});
        let b = json!({"n": 2});
        let mut warned = false;
        for i in 0..WARNING_THRESHOLD + 4 {
            let (tool, arg) = if i % 2 == 0 {
                ("list_dir", &a)
            } else {
                ("list_dir", &b)
            };
            match d.detect(tool, arg) {
                Detection::Warn(m) => {
                    assert!(m.contains("alternating"), "{m}");
                    warned = true;
                    break;
                }
                Detection::Block(_) => panic!("blocked too early at {i}"),
                Detection::Ok => {}
            }
            d.record(tool, arg, "same");
        }
        assert!(warned);
    }

    #[test]
    fn ping_pong_with_changing_results_does_not_block() {
        let mut d = LoopDetector::new();
        let a = json!({"n": 1});
        let b = json!({"n": 2});
        for i in 0..CRITICAL_THRESHOLD + 6 {
            let arg = if i % 2 == 0 { &a } else { &b };
            if let Detection::Block(m) = d.detect("list_dir", arg) {
                panic!("blocked on real progress at {i}: {m}");
            }
            d.record("list_dir", arg, &format!("result {i}"));
        }
    }

    #[test]
    fn block_result_does_not_reset_the_streak() {
        let mut d = LoopDetector::new();
        for _ in 0..CRITICAL_THRESHOLD {
            let _ = d.detect("read_file", &args());
            d.record("read_file", &args(), "same output");
        }
        let first = d.detect("read_file", &args());
        assert!(matches!(first, Detection::Block(_)));
        let msg = match first {
            Detection::Block(m) => m,
            _ => unreachable!(),
        };
        d.record("read_file", &args(), &msg);
        assert!(
            matches!(d.detect("read_file", &args()), Detection::Block(_)),
            "the block must stay sticky after its own veto is recorded"
        );
    }

    #[test]
    fn the_global_circuit_breaker_is_actually_reachable() {
        let mut d = LoopDetector::new();
        let mut blocks = 0usize;
        let mut saw_runaway = false;
        for _ in 0..GLOBAL_CIRCUIT_BREAKER_THRESHOLD + 15 {
            match d.detect("read_file", &args()) {
                Detection::Block(m) => {
                    blocks += 1;
                    if m.contains("runaway") {
                        saw_runaway = true;
                        break;
                    }
                    d.record("read_file", &args(), &m);
                }
                _ => d.record("read_file", &args(), "same output"),
            }
        }
        assert!(blocks > 0, "the critical block must fire first");
        assert!(
            saw_runaway,
            "vetoed calls must count toward the breaker, or it is dead code"
        );
    }

    #[test]
    fn warned_keys_are_bounded_over_a_long_session() {
        let mut d = LoopDetector::new();
        for round in 0..WARNED_KEYS_MAX * 2 {
            let a = json!({"round": round});
            for _ in 0..WARNING_THRESHOLD {
                let _ = d.detect("read_file", &a);
                d.record("read_file", &a, "same output");
            }
        }
        assert!(
            d.warned.len() <= WARNED_KEYS_MAX,
            "warned keys leaked: {}",
            d.warned.len()
        );
    }

    #[test]
    fn history_is_bounded() {
        let mut d = LoopDetector::new();
        for i in 0..HISTORY_SIZE * 3 {
            let a = json!({"i": i});
            d.record("read_file", &a, "x");
        }
        assert_eq!(d.history.len(), HISTORY_SIZE);
    }

    #[test]
    fn global_circuit_breaker_message_mentions_runaway() {
        let mut d = LoopDetector::new();
        for _ in 0..HISTORY_SIZE {
            d.record("bg_list", &args(), "same output");
        }
        match d.detect("bg_list", &args()) {
            Detection::Block(m) => assert!(m.contains("runaway"), "{m}"),
            other => panic!("expected a block, got {other:?}"),
        }
    }
}
