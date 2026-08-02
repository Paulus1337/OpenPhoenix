use std::process::{Command, Stdio};

use serde_json::{json, Value};

pub const EVENTS: &[&str] = &["turn_start", "turn_end", "tool_call", "command", "error"];

pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Hook {
    pub name: String,
    pub event: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub timeout_ms: u64,
}

impl Default for Hook {
    fn default() -> Self {
        Hook {
            name: String::new(),
            event: String::new(),
            command: String::new(),
            args: Vec::new(),
            enabled: true,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

pub fn known_event(name: &str) -> bool {
    EVENTS.contains(&name)
}

pub fn from_toml(root: &toml::Value) -> Vec<Hook> {
    let Some(table) = root.get("hooks").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut out: Vec<Hook> = table
        .iter()
        .filter_map(|(name, v)| {
            let t = v.as_table()?;
            let s = |key: &str| {
                t.get(key)
                    .and_then(toml::Value::as_str)
                    .map(crate::config::expand_env)
                    .unwrap_or_default()
            };
            Some(Hook {
                name: name.clone(),
                event: s("event"),
                command: s("command"),
                args: t
                    .get("args")
                    .and_then(toml::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(crate::config::expand_env))
                            .collect()
                    })
                    .unwrap_or_default(),
                enabled: t
                    .get("enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true),
                timeout_ms: t
                    .get("timeout_ms")
                    .and_then(toml::Value::as_integer)
                    .filter(|v| *v > 0)
                    .map(|v| v as u64)
                    .unwrap_or(DEFAULT_TIMEOUT_MS),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn problems(hooks: &[Hook]) -> Vec<String> {
    let mut out = Vec::new();
    for h in hooks {
        if h.command.trim().is_empty() {
            out.push(format!("hook '{}' has no command", h.name));
        }
        if h.event.trim().is_empty() {
            out.push(format!("hook '{}' has no event", h.name));
        } else if !known_event(&h.event) {
            out.push(format!(
                "hook '{}' listens for unknown event '{}': expected one of {EVENTS:?}",
                h.name, h.event
            ));
        }
    }
    out
}

pub fn payload(event: &str, detail: &Value) -> String {
    json!({
        "v": 1,
        "event": event,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "detail": detail,
    })
    .to_string()
}

pub fn fire(hooks: &[Hook], event: &str, detail: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let body = payload(event, detail);
    for h in hooks
        .iter()
        .filter(|h| h.enabled && h.event == event && !h.command.trim().is_empty())
    {
        if let Err(e) = run_one(h, &body) {
            problems.push(format!("hook '{}': {e}", h.name));
        }
    }
    problems
}

fn run_one(h: &Hook, body: &str) -> Result<(), String> {
    let mut cmd = Command::new(&h.command);
    cmd.args(&h.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, _) in std::env::vars() {
        if crate::mcp::is_secret_var(&k) {
            cmd.env_remove(&k);
        }
    }
    cmd.env("PHOENIX_HOOK_EVENT", &h.event);
    cmd.env("PHOENIX_HOOK_NAME", &h.name);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(body.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(h.timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("exited with {status}"))
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timed out after {}ms", h.timeout_ms));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    }
}

pub fn summary(hooks: &[Hook]) -> String {
    if hooks.is_empty() {
        return format!("no hooks configured; add [hooks.NAME] with event = one of {EVENTS:?}\n");
    }
    let on = hooks.iter().filter(|h| h.enabled).count();
    let mut out = format!("{} hooks, {on} enabled\n", hooks.len());
    for h in hooks {
        let mark = if h.enabled { "on " } else { "off" };
        out.push_str(&format!(
            "  {mark}  {:<16}{:<12}{}\n",
            h.name, h.event, h.command
        ));
    }
    for p in problems(hooks) {
        out.push_str(&format!("  warn  {p}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(body: &str, name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("phx-hook-{name}-{}", std::process::id()));
        std::fs::write(&p, body).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        p
    }

    #[test]
    fn config_parses_hooks_with_defaults() {
        let raw = r#"
[hooks.logger]
event = "tool_call"
command = "/usr/bin/tee"
args = ["-a", "/tmp/log"]

[hooks.off_one]
event = "turn_end"
command = "x"
enabled = false
timeout_ms = 250
"#;
        let root: toml::Value = toml::from_str(raw).expect("toml");
        let hooks = from_toml(&root);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].name, "logger");
        assert_eq!(hooks[0].event, "tool_call");
        assert_eq!(hooks[0].args, vec!["-a", "/tmp/log"]);
        assert!(hooks[0].enabled);
        assert_eq!(hooks[0].timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(!hooks[1].enabled);
        assert_eq!(hooks[1].timeout_ms, 250);
    }

    #[test]
    fn no_hooks_table_means_no_hooks_rather_than_an_error() {
        let root: toml::Value = toml::from_str("[provider]\nkind = \"nvidia\"\n").expect("toml");
        assert!(from_toml(&root).is_empty());
    }

    #[test]
    fn an_unknown_event_is_reported_not_silently_ignored() {
        let hooks = vec![Hook {
            name: "typo".into(),
            event: "turn_ended".into(),
            command: "/bin/true".into(),
            ..Hook::default()
        }];
        let p = problems(&hooks);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("unknown event"), "{:?}", p);
    }

    #[test]
    fn a_hook_missing_its_command_or_event_is_reported() {
        let hooks = vec![Hook {
            name: "empty".into(),
            ..Hook::default()
        }];
        let p = problems(&hooks);
        assert_eq!(p.len(), 2, "{p:?}");
    }

    #[test]
    fn payload_is_one_json_line_with_event_and_detail() {
        let raw = payload("tool_call", &json!({"tool": "shell"}));
        assert!(!raw.contains('\n'), "hooks read one line per event");
        let v: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(v["event"], "tool_call");
        assert_eq!(v["detail"]["tool"], "shell");
        assert_eq!(v["v"], 1);
        assert!(v["ts"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn a_hook_receives_the_payload_on_stdin() {
        let out = std::env::temp_dir().join(format!("phx-hookout-cat-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let sh = script(&format!("#!/bin/sh\ncat > {}\n", out.display()), "cat");
        let hooks = vec![Hook {
            name: "capture".into(),
            event: "turn_end".into(),
            command: sh.to_string_lossy().to_string(),
            ..Hook::default()
        }];
        let problems = fire(&hooks, "turn_end", &json!({"reply": "hi"}));
        assert!(problems.is_empty(), "{problems:?}");
        let got = std::fs::read_to_string(&out).unwrap_or_default();
        let v: Value = serde_json::from_str(got.trim()).expect("hook got json");
        assert_eq!(v["event"], "turn_end");
        assert_eq!(v["detail"]["reply"], "hi");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&sh);
    }

    #[test]
    fn only_hooks_for_that_event_fire() {
        let out = std::env::temp_dir().join(format!("phx-hookout-ev-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let sh = script(
            &format!("#!/bin/sh\necho fired >> {}\n", out.display()),
            "ev",
        );
        let hooks = vec![
            Hook {
                name: "a".into(),
                event: "turn_start".into(),
                command: sh.to_string_lossy().to_string(),
                ..Hook::default()
            },
            Hook {
                name: "b".into(),
                event: "turn_end".into(),
                command: sh.to_string_lossy().to_string(),
                ..Hook::default()
            },
        ];
        fire(&hooks, "turn_start", &json!({}));
        let got = std::fs::read_to_string(&out).unwrap_or_default();
        assert_eq!(got.lines().count(), 1, "only the turn_start hook may run");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&sh);
    }

    #[test]
    fn a_disabled_hook_never_runs() {
        let out = std::env::temp_dir().join(format!("phx-hookout-dis-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let sh = script(&format!("#!/bin/sh\necho x >> {}\n", out.display()), "dis");
        let hooks = vec![Hook {
            name: "off".into(),
            event: "turn_end".into(),
            command: sh.to_string_lossy().to_string(),
            enabled: false,
            ..Hook::default()
        }];
        assert!(fire(&hooks, "turn_end", &json!({})).is_empty());
        assert!(!out.exists(), "a disabled hook must not run");
        let _ = std::fs::remove_file(&sh);
    }

    #[test]
    fn a_failing_hook_is_reported_but_does_not_raise() {
        let hooks = vec![Hook {
            name: "missing".into(),
            event: "turn_end".into(),
            command: "/nonexistent/hook/binary".into(),
            ..Hook::default()
        }];
        let problems = fire(&hooks, "turn_end", &json!({}));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("cannot start"), "{:?}", problems);
    }

    #[test]
    fn a_nonzero_exit_is_reported() {
        let hooks = vec![Hook {
            name: "fails".into(),
            event: "error".into(),
            command: "/bin/false".into(),
            ..Hook::default()
        }];
        let problems = fire(&hooks, "error", &json!({}));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("exited"), "{:?}", problems);
    }

    #[test]
    fn a_hanging_hook_is_killed_at_its_timeout() {
        let hooks = vec![Hook {
            name: "sleepy".into(),
            event: "turn_end".into(),
            command: "/bin/sleep".into(),
            args: vec!["30".into()],
            timeout_ms: 200,
            ..Hook::default()
        }];
        let start = std::time::Instant::now();
        let problems = fire(&hooks, "turn_end", &json!({}));
        let took = start.elapsed();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("timed out"), "{:?}", problems);
        assert!(
            took < std::time::Duration::from_secs(5),
            "the agent waited {took:?} on a hung hook"
        );
    }

    #[test]
    fn summary_lists_every_hook_and_its_problems() {
        let hooks = vec![Hook {
            name: "typo".into(),
            event: "nope".into(),
            command: "/bin/true".into(),
            ..Hook::default()
        }];
        let text = summary(&hooks);
        assert!(text.contains("1 hooks, 1 enabled"), "{text}");
        assert!(text.contains("unknown event"), "{text}");
    }

    #[test]
    fn an_empty_list_says_where_to_configure_one() {
        assert!(summary(&[]).contains("[hooks.NAME]"));
    }
}
