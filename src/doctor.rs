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

pub fn check(cfg: &Config, cfg_path: &Path, raw: &str, memory_path: &Path) -> Vec<Finding> {
    let mut out = Vec::new();

    let unknown = crate::config::unknown_keys(raw);
    if unknown.is_empty() {
        out.push(f("ok", "config uses only known keys (schema 1.0)"));
    } else {
        out.push(f(
            "warn",
            format!("unknown config keys (typo?): {}", unknown.join(", ")),
        ));
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

    if key_in_file(raw, "api_key") {
        out.push(f(
            "warn",
            "api_key is stored in the config file; prefer the PHOENIX_API_KEY env var",
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
            "approvals: on; serve-mode shell commands queue for /approve"
        } else {
            "approvals: off; serve-mode shell commands run directly (set security.approvals = true to queue)"
        },
    ));
    if cfg.http_enabled && cfg.http_token.is_empty() {
        out.push(f(
            "fail",
            "http.enabled is on but no token is set; serve will refuse to start the HTTP API",
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
    out.push(f(
        "ok",
        format!(
            "extra deny patterns: {}, fallback models: {}",
            cfg.deny_commands.len(),
            cfg.fallbacks.len()
        ),
    ));
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
