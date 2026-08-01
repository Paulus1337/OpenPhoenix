use std::path::Path;

use crate::config::Config;

#[derive(Debug)]
pub struct Finding {
    pub level: &'static str,
    pub msg: String,
}

fn f(level: &'static str, msg: impl Into<String>) -> Finding {
    Finding {
        level,
        msg: msg.into(),
    }
}

fn key_in_file(raw: &str, key: &str) -> bool {
    raw.lines().any(|l| {
        let t = l.trim_start();
        if t.starts_with('#') || !t.starts_with(key) {
            return false;
        }
        match t.split_once('=') {
            Some((k, v)) => {
                k.trim() == key && !v.trim().trim_matches('"').trim_matches('\'').is_empty()
            }
            None => false,
        }
    })
}

#[cfg(unix)]
fn loose_perms(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        Some(mode)
    } else {
        None
    }
}

#[cfg(test)]
mod disk_tests {
    use super::*;

    #[test]
    fn byte_formatting_scales() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2 * 1024), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[cfg(unix)]
    #[test]
    fn free_space_probe_reads_filesystem_and_rejects_missing_paths() {
        assert!(free_bytes(Path::new("/")).is_some(), "statvfs on / works");
        assert!(free_bytes(Path::new("/definitely/not/here")).is_none());
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;
    if bytes >= G {
        format!("{:.1} GB", bytes as f64 / G as f64)
    } else if bytes >= M {
        format!("{} MB", bytes / M)
    } else if bytes >= K {
        format!("{} KB", bytes / K)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    let avail_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb.saturating_mul(1024))
}

pub fn check(cfg: &Config, cfg_path: &Path, raw: &str, memory_path: &Path) -> Vec<Finding> {
    let mut out = Vec::new();

    let unknown = crate::config::unknown_keys(raw);
    if unknown.is_empty() {
        out.push(f("ok", "config uses only known keys (schema 1.0)"));
    } else {
        let mut msg = format!("unknown config keys (typo?): {}", unknown.join(", "));
        let hints: Vec<String> = unknown
            .iter()
            .filter_map(|k| crate::config::misplaced_hint(k))
            .collect();
        if !hints.is_empty() {
            msg.push_str(&format!("; {}", hints.join("; ")));
        }
        out.push(f("warn", msg));
    }

    if cfg_path.exists() {
        #[cfg(unix)]
        match loose_perms(cfg_path) {
            Some(mode) => out.push(f(
                "warn",
                format!(
                    "config is mode {mode:o}, expected 600: chmod 600 {}",
                    cfg_path.display()
                ),
            )),
            None => out.push(f("ok", "config file permissions are tight")),
        }
    } else {
        out.push(f("warn", "no config file; run phoenix init"));
    }

    let st = crate::state::State::load();
    let report = st.report();
    if let Some(cools) = report["cooldowns"].as_array() {
        for c in cools {
            out.push(f(
                "warn",
                format!(
                    "provider key {} is cooling for {}s after: {}",
                    c["key"].as_str().unwrap_or("?"),
                    c["seconds_left"].as_u64().unwrap_or(0),
                    c["reason"].as_str().unwrap_or("")
                ),
            ));
        }
    }
    let tracked = report["tracked_chats"].as_u64().unwrap_or(0);
    if tracked > 0 {
        out.push(f("ok", format!("state: {tracked} chat(s) remembered")));
    }

    let serve = crate::daemon::report(&crate::daemon::default_path());
    if serve["running"] == serde_json::json!(true) {
        out.push(f(
            "ok",
            format!(
                "serve: running as pid {} for {}",
                serve["pid"].as_u64().unwrap_or(0),
                crate::scheduler::time_ago(serve["uptime_secs"].as_u64().unwrap_or(0))
            ),
        ));
    } else if serve["stale"] == serde_json::json!(true) {
        out.push(f(
            "warn",
            format!(
                "serve: stale lock from dead pid {}; the next start reclaims it",
                serve["pid"].as_u64().unwrap_or(0)
            ),
        ));
    }
    if serve["running"] == serde_json::json!(true) && cfg.http_enabled {
        let bind = match cfg.http_bind.as_str() {
            "" | "0.0.0.0" => "127.0.0.1",
            b => b,
        };
        let url = format!("http://{}:{}/health", bind, cfg.http_port);
        let started = std::time::Instant::now();
        match ureq::get(&url)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            Ok(_) => out.push(f(
                "ok",
                format!(
                    "http: /health answers in {} ms on {}:{}",
                    started.elapsed().as_millis(),
                    bind,
                    cfg.http_port
                ),
            )),
            Err(e) => out.push(f(
                "FAIL",
                format!(
                    "serve is running but http did not answer on {}:{}: {}",
                    bind,
                    cfg.http_port,
                    crate::security::one_line(&crate::security::redact(&e.to_string()), 120)
                ),
            )),
        }
    }
    let sess = crate::sessions::list(&crate::config::home().join("sessions"));
    if sess.is_empty() {
        out.push(f("ok", "sessions: none stored"));
    } else {
        let msgs: usize = sess.iter().map(|(_, n)| n).sum();
        out.push(f(
            "ok",
            format!(
                "sessions: {} stored holding {} message(s)",
                sess.len(),
                msgs
            ),
        ));
    }
    if memory_path.is_file() {
        let notes = std::fs::read_to_string(memory_path)
            .map(|s| {
                s.lines()
                    .filter(|l| l.trim_start().starts_with("- ["))
                    .count()
            })
            .unwrap_or(0);
        let emb = if cfg.mem_embeddings { "on" } else { "off" };
        out.push(f(
            "ok",
            format!("memory: {notes} note(s), embeddings {emb}"),
        ));
    }

    let tasks_path = crate::tasks::default_path();
    crate::tasks::reap(&tasks_path);
    crate::tasks::prune(&tasks_path, crate::tasks::DEFAULT_KEEP);
    let treport = crate::tasks::report(&tasks_path);
    let active = treport["active"].as_u64().unwrap_or(0);
    if active > 0 {
        out.push(f("ok", format!("tasks: {active} running in background")));
    }
    let lost = treport["by_status"]["lost"].as_u64().unwrap_or(0);
    if lost > 0 {
        out.push(f(
            "warn",
            format!("tasks: {lost} died without reporting; see phoenix tasks"),
        ));
    }

    let store = crate::secrets::Store::at(&crate::secrets::Store::default_path());
    if store.locked() {
        out.push(f(
            "warn",
            format!(
                "encrypted secret store found but {} is not set; secrets stay locked",
                crate::secrets::KEY_VAR
            ),
        ));
    } else if store.exists() {
        out.push(f(
            "ok",
            format!(
                "secret store unlocked: {} entries",
                store.names().map(|n| n.len()).unwrap_or(0)
            ),
        ));
    }
    if key_in_file(raw, "api_key") {
        out.push(f(
            "warn",
            "api_key sits in plain text in the config; move it with `phoenix secret set api_key`",
        ));
    } else {
        out.push(f("ok", "no API key in the config file"));
    }
    if key_in_file(raw, "token") {
        out.push(f(
            "warn",
            "a token is stored in the config file; prefer PHOENIX_TELEGRAM_TOKEN / PHOENIX_HTTP_TOKEN",
        ));
    }

    for (name, raw) in [
        ("telegram.allowed_chat_ids", &cfg.telegram_allowed),
        ("discord.allowed_channel_ids", &cfg.discord_allowed),
        ("slack.allowed_channel_ids", &cfg.slack_allowed),
        ("signal.allowed_numbers", &cfg.signal_allowed),
        ("whatsapp.allowed_numbers", &cfg.wa_allowed),
        ("irc.allowed_nicks", &cfg.irc_allowed),
        ("matrix.allowed_users", &cfg.matrix_allowed),
        ("mattermost.allowed_users", &cfg.mattermost_allowed),
    ] {
        if crate::allowlist::Allowlist::new(raw).open_to_everyone() {
            out.push(f(
                "warn",
                format!("{name} contains *, so this channel answers everyone"),
            ));
        }
    }
    if !cfg.telegram_token.is_empty() && cfg.telegram_allowed.is_empty() {
        out.push(f(
            "warn",
            "telegram token set but allowed_chat_ids is empty; serve will refuse to start",
        ));
    }
    if cfg.allow_outside_workspace {
        out.push(f(
            "warn",
            "allow_outside_workspace is on; file tools can reach the whole filesystem",
        ));
    }
    if !cfg.confirm_shell {
        out.push(f(
            "warn",
            "confirm_shell is off; shell commands run without an interactive prompt",
        ));
    }
    out.push(f(
        "ok",
        if cfg.approvals {
            "approvals: on; serve-mode shell commands and gated tools queue for /approve"
        } else {
            "approvals: off; serve-mode shell commands run directly (set security.approvals = true to queue)"
        },
    ));
    if !cfg.confirm_tools.is_empty() {
        if cfg.approvals {
            out.push(f(
                "ok",
                format!(
                    "confirm_tools: {} need a yes first: {}",
                    cfg.confirm_tools.len(),
                    cfg.confirm_tools.join(", ")
                ),
            ));
        } else {
            out.push(f(
                "warn",
                format!(
                    "confirm_tools lists {} tool(s) but approvals is off; serve-mode calls \
will be refused outright, not queued",
                    cfg.confirm_tools.len()
                ),
            ));
        }
    }
    if !cfg.allow_domains.is_empty() || !cfg.deny_domains.is_empty() {
        out.push(f(
            "ok",
            format!(
                "egress: {} allow rule(s), {} deny rule(s) on web tool domains",
                cfg.allow_domains.len(),
                cfg.deny_domains.len()
            ),
        ));
    }
    if cfg.http_enabled && cfg.http_token.is_empty() {
        out.push(f(
            "fail",
            "http.enabled is on but no token is set; serve will refuse to start the HTTP API",
        ));
    }
    if cfg.http_enabled && !crate::http::is_loopback_ip(&cfg.http_bind) {
        out.push(f(
            "warn",
            format!(
                "http.bind is {}: reachable from the network; 127.0.0.1 is the closed-door default, so make sure this door is open on purpose",
                cfg.http_bind
            ),
        ));
    }
    if cfg.http_enabled && cfg.http_web {
        if cfg.http_user.is_empty() || cfg.http_pass.is_empty() {
            out.push(f(
                "fail",
                "http.web is on but username/password are not set; the web UI stays dark (403) until both exist",
            ));
        } else if !cfg.http_pass.starts_with("sha256:") {
            out.push(f(
                "warn",
                "http.password is plaintext in the config; prefer password = \"sha256:<hex>\" (echo -n PASS | sha256sum)",
            ));
        }
        if !cfg.http_allow_crawlers.is_empty() {
            out.push(f(
                "warn",
                "http.allow_crawlers permits indexing for listed user-agents; robots.txt and X-Robots-Tag protection is reduced",
            ));
        }
    }

    #[cfg(unix)]
    {
        let dir = cfg_path.parent().unwrap_or(Path::new("/"));
        if let Some(free) = free_bytes(dir) {
            const CRITICAL: u64 = 100 * 1024 * 1024;
            const WARNING: u64 = 500 * 1024 * 1024;
            let human = format_bytes(free);
            if free < CRITICAL {
                out.push(f(
                    "fail",
                    format!(
                        "only {human} free on {}; config writes, sessions, and memory can fail silently",
                        dir.display()
                    ),
                ));
            } else if free < WARNING {
                out.push(f(
                    "warn",
                    format!("{human} free on {}; running low", dir.display()),
                ));
            } else {
                out.push(f("ok", format!("disk space healthy ({human} free)")));
            }
        }
    }

    if crate::whatsapp::WhatsApp::wanted(cfg) {
        if let Err(e) = crate::whatsapp::WhatsApp::new(cfg) {
            out.push(f(
                "fail",
                format!("whatsapp is partially configured; serve will refuse to start: {e}"),
            ));
        } else {
            out.push(f("ok", "whatsapp channel fully configured (fail closed)"));
        }
    }

    if cfg.browser_enabled {
        out.push(f(
            "warn",
            "browser is enabled; a real browser is a larger attack surface (pages can probe \
 localhost, downloads, redirects). Keep it headless and close sessions when done",
        ));
        if !cfg.browser_cdp_url.is_empty() {
            if let Err(e) = crate::browser::check_cdp_host(&cfg.browser_cdp_url) {
                out.push(f("fail", format!("browser.cdp_url: {e}")));
            } else {
                out.push(f("ok", "browser.cdp_url points at localhost"));
            }
        }
    }

    if cfg.canvas_enabled {
        if !cfg.http_enabled {
            out.push(f(
                "warn",
                "canvas is enabled but the HTTP server is off; nothing will serve /canvas \
until [http] enabled = true",
            ));
        } else if cfg.http_user.is_empty() || cfg.http_pass.is_empty() {
            out.push(f(
                "warn",
                "canvas is enabled but http.username/http.password are unset; the surface \
stays dark (fail closed) until credentials exist",
            ));
        } else {
            out.push(f(
                "ok",
                "canvas is served at /canvas behind web credentials",
            ));
        }
    }

    if cfg.imessage_enabled {
        if cfg.imessage_allowed.is_empty() {
            out.push(f(
                "fail",
                "imessage is enabled with an empty allowed_senders list; serve will refuse \
to start (fail closed). Add sender handles to [imessage] allowed_senders",
            ));
        } else {
            out.push(f(
                "ok",
                format!(
                    "imessage is enabled ({} allowed sender(s)); needs macOS, the imsg CLI, \
and Full Disk Access for chat.db",
                    cfg.imessage_allowed.len()
                ),
            ));
        }
    }

    if memory_path.exists() {
        #[cfg(unix)]
        if let Some(mode) = loose_perms(memory_path) {
            out.push(f(
                "warn",
                format!(
                    "memory file is mode {mode:o}, expected 600: chmod 600 {}",
                    memory_path.display()
                ),
            ));
        }
    }

    out.push(f("ok", format!("workspace: {}", cfg.workspace.display())));
    {
        let dirs = [
            cfg.workspace.join("skills"),
            crate::config::home().join("skills"),
        ];
        let mut loaded = 0usize;
        let mut problems: Vec<String> = Vec::new();
        for d in &dirs {
            let (skills, bad) = crate::skills::scan_dir(d);
            loaded += skills.len();
            problems.extend(bad);
        }
        if problems.is_empty() {
            out.push(f("ok", format!("skills: {loaded} loaded")));
        } else {
            for p in problems.iter().take(5) {
                out.push(f("warn", format!("skill ignored: {p}")));
            }
            out.push(f("ok", format!("skills: {loaded} loaded")));
        }
    }
    out.push(f(
        "ok",
        format!(
            "extra deny patterns: {}, fallback models: {}",
            cfg.deny_commands.len(),
            cfg.fallbacks.len()
        ),
    ));
    match crate::catalog::lookup(&cfg.provider, &cfg.model) {
        Some(i) => out.push(f(
            "ok",
            format!(
                "model {}/{}: {} token context, max output {}",
                cfg.provider, cfg.model, i.context_window, i.max_tokens
            ),
        )),
        None => out.push(f(
            "warn",
            format!(
                "model {}/{} is not in the catalog; context limits are guessed from the name",
                cfg.provider, cfg.model
            ),
        )),
    }
    out
}

pub fn has_failures(findings: &[Finding]) -> bool {
    findings.iter().any(|x| x.level == "fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn base_cfg() -> Config {
        Config::default()
    }

    #[test]
    fn clean_default_has_no_failures() {
        let cfg = base_cfg();
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent/config.toml"),
            "",
            &PathBuf::from("/nonexistent/memory.md"),
        );
        assert!(!has_failures(&out));
    }

    #[test]
    fn inventory_lines_cover_memory_and_sessions() {
        let dir = std::env::temp_dir().join(format!("phx-doc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mem = dir.join("memory.md");
        std::fs::write(
            &mem,
            "- [2026-08-01 09:00] [operator] a\n- [2026-08-01 09:01] [agent] b\n",
        )
        .unwrap();
        let out = check(&base_cfg(), &PathBuf::from("/nonexistent"), "", &mem);
        let msgs: Vec<&String> = out.iter().map(|x| &x.msg).collect();
        assert!(
            msgs.iter().any(|m| m.contains("memory: 2 note(s)")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.starts_with("sessions:")), "{msgs:?}");
    }

    #[test]
    fn warns_on_secrets_in_file() {
        let raw = "[provider]\napi_key = \"sk-xyz\"\n[telegram]\ntoken = \"t\"\n";
        let out = check(
            &base_cfg(),
            &PathBuf::from("/nonexistent"),
            raw,
            &PathBuf::from("/nonexistent"),
        );
        let warns: Vec<_> = out.iter().filter(|x| x.level == "warn").collect();
        assert!(warns.iter().any(|x| x.msg.contains("api_key")));
        assert!(warns.iter().any(|x| x.msg.contains("token")));
    }

    #[test]
    fn commented_keys_do_not_warn() {
        let raw = "# api_key = \"x\"\n# token = \"y\"\n";
        assert!(!key_in_file(raw, "api_key"));
        assert!(!key_in_file(raw, "token"));
    }

    #[test]
    fn a_public_bind_is_flagged() {
        let cfg = Config {
            http_enabled: true,
            http_token: "t".into(),
            http_bind: "0.0.0.0".into(),
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "warn" && x.msg.contains("reachable from the network")));
    }

    #[test]
    fn http_without_token_fails() {
        let cfg = Config {
            http_enabled: true,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(has_failures(&out));
    }

    #[test]
    fn risky_toggles_warn() {
        let cfg = Config {
            allow_outside_workspace: true,
            confirm_shell: false,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        let warns = out.iter().filter(|x| x.level == "warn").count();
        assert!(warns >= 2);
    }

    #[test]
    fn canvas_posture_warns_until_served_with_creds() {
        let cfg = Config {
            canvas_enabled: true,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "warn" && x.msg.contains("HTTP server is off")));
        let cfg = Config {
            canvas_enabled: true,
            http_enabled: true,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "warn" && x.msg.contains("fail closed")));
        let cfg = Config {
            canvas_enabled: true,
            http_enabled: true,
            http_user: "bob".into(),
            http_pass: "sha256:abc".into(),
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "ok" && x.msg.contains("/canvas")));
    }

    #[test]
    fn imessage_posture_fails_on_empty_allowlist() {
        let cfg = Config {
            imessage_enabled: true,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "fail" && x.msg.contains("allowed_senders")));
        let cfg = Config {
            imessage_enabled: true,
            imessage_allowed: vec!["+15550001".into()],
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "ok" && x.msg.contains("Full Disk Access")));
    }

    #[test]
    fn browser_enabled_warns_and_remote_cdp_fails() {
        let cfg = Config {
            browser_enabled: true,
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(out
            .iter()
            .any(|x| x.level == "warn" && x.msg.contains("attack surface")));
        let cfg = Config {
            browser_enabled: true,
            browser_cdp_url: "http://10.1.2.3:9222".into(),
            ..base_cfg()
        };
        let out = check(
            &cfg,
            &PathBuf::from("/nonexistent"),
            "",
            &PathBuf::from("/nonexistent"),
        );
        assert!(has_failures(&out));
    }

    #[test]
    fn approvals_reported_as_ok_either_way() {
        for (approvals, needle) in [(true, "approvals: on"), (false, "approvals: off")] {
            let cfg = Config {
                approvals,
                ..base_cfg()
            };
            let out = check(
                &cfg,
                &PathBuf::from("/nonexistent"),
                "",
                &PathBuf::from("/nonexistent"),
            );
            let hit = out
                .iter()
                .find(|x| x.msg.contains("approvals:"))
                .expect("approvals line present");
            assert_eq!(hit.level, "ok");
            assert!(hit.msg.contains(needle), "got: {}", hit.msg);
            assert!(!out
                .iter()
                .any(|x| x.level == "warn" && x.msg.contains("approvals")));
        }
    }
}
