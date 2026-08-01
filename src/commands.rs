use crate::config::{self, Config};

pub struct Spec {
    pub name: &'static str,
    pub summary: &'static str,
}

pub const COMMANDS: &[Spec] = &[
    Spec {
        name: "agent",
        summary: "run one agent turn and print the reply",
    },
    Spec {
        name: "agents",
        summary: "list, show, and remove named child agents",
    },
    Spec {
        name: "approvals",
        summary: "list and resolve queued shell approvals",
    },
    Spec {
        name: "attach",
        summary: "talk to a running phoenix gateway from this terminal",
    },
    Spec {
        name: "audit",
        summary: "read the audit log of tool and auth events",
    },
    Spec {
        name: "backup",
        summary: "archive the nest to a tarball",
    },
    Spec {
        name: "board",
        summary: "track work cards: add, list, update",
    },
    Spec {
        name: "canvas",
        summary: "present or clear HTML on the canvas",
    },
    Spec {
        name: "capability",
        summary: "probe what the current provider and model support",
    },
    Spec {
        name: "channels",
        summary: "show configured chat channels and their allowlists",
    },
    Spec {
        name: "chat",
        summary: "interactive REPL",
    },
    Spec {
        name: "commands",
        summary: "list every command and what it does",
    },
    Spec {
        name: "commitments",
        summary: "track follow-ups with a deadline",
    },
    Spec {
        name: "completion",
        summary: "print a shell completion script",
    },
    Spec {
        name: "config",
        summary: "read and validate config.toml",
    },
    Spec {
        name: "configure",
        summary: "interactive setup wizard",
    },
    Spec {
        name: "cron",
        summary: "list cron jobs and validate their schedules",
    },
    Spec {
        name: "daemon",
        summary: "manage the background service",
    },
    Spec {
        name: "dashboard",
        summary: "open the web UI",
    },
    Spec {
        name: "directory",
        summary: "show every allowlisted chat id",
    },
    Spec {
        name: "docs",
        summary: "print documentation links",
    },
    Spec {
        name: "doctor",
        summary: "audit config, permissions, and risky settings",
    },
    Spec {
        name: "exec-approvals",
        summary: "alias for approvals",
    },
    Spec {
        name: "exec-policy",
        summary: "show the shell command deny policy",
    },
    Spec {
        name: "gateway",
        summary: "inspect the HTTP gateway",
    },
    Spec {
        name: "health",
        summary: "one-line health summary for monitors",
    },
    Spec {
        name: "help",
        summary: "print usage",
    },
    Spec {
        name: "hooks",
        summary: "list lifecycle hooks and fire a test event",
    },
    Spec {
        name: "infer",
        summary: "alias for capability",
    },
    Spec {
        name: "init",
        summary: "write a sample config",
    },
    Spec {
        name: "logs",
        summary: "tail service logs",
    },
    Spec {
        name: "mcp",
        summary: "list MCP servers and probe their tools",
    },
    Spec {
        name: "media",
        summary: "generate an image from a prompt",
    },
    Spec {
        name: "memory",
        summary: "search and manage long-term memory",
    },
    Spec {
        name: "message",
        summary: "send a message through a configured channel",
    },
    Spec {
        name: "migrate",
        summary: "convert another agent's config",
    },
    Spec {
        name: "models",
        summary: "list models for the current provider",
    },
    Spec {
        name: "oauth",
        summary: "show and refresh stored OAuth tokens",
    },
    Spec {
        name: "onboard",
        summary: "alias for configure",
    },
    Spec {
        name: "proxy",
        summary: "record and inspect provider traffic",
    },
    Spec {
        name: "reset",
        summary: "reset config and state",
    },
    Spec {
        name: "run",
        summary: "one-shot task",
    },
    Spec {
        name: "schema",
        summary: "print the config JSON Schema",
    },
    Spec {
        name: "secrets",
        summary: "encrypted secret store: set, list, rm, export",
    },
    Spec {
        name: "security",
        summary: "report only the security findings",
    },
    Spec {
        name: "serve",
        summary: "go live on every configured channel",
    },
    Spec {
        name: "sessions",
        summary: "list stored sessions",
    },
    Spec {
        name: "setup",
        summary: "alias for configure",
    },
    Spec {
        name: "skills",
        summary: "search and install skills",
    },
    Spec {
        name: "status",
        summary: "one screen: config, model, channels, service",
    },
    Spec {
        name: "system",
        summary: "runtime and build information",
    },
    Spec {
        name: "tasks",
        summary: "inspect background tasks",
    },
    Spec {
        name: "terminal",
        summary: "alias for chat",
    },
    Spec {
        name: "transcribe",
        summary: "turn an audio file into text",
    },
    Spec {
        name: "transcripts",
        summary: "print a stored session transcript",
    },
    Spec {
        name: "tui",
        summary: "alias for chat",
    },
    Spec {
        name: "uninstall",
        summary: "remove the service",
    },
    Spec {
        name: "update",
        summary: "self-update to the latest release",
    },
    Spec {
        name: "webhooks",
        summary: "show configured webhook endpoints",
    },
    Spec {
        name: "worktrees",
        summary: "create, list, and remove git worktrees",
    },
];

pub const NOT_BUILT: &[(&str, &str)] = &[
    ("acp", "needs the Agent Client Protocol transport"),
    ("clawbot", "an OpenClaw legacy alias; nothing to build"),
    (
        "crestodian",
        "an OpenClaw-specific helper; doctor covers repair here",
    ),
    ("devices", "needs companion apps (issue #21)"),
    ("dns", "needs Tailscale and CoreDNS integration"),
    ("node", "needs the node host service"),
    ("nodes", "needs companion apps (issue #21)"),
    ("pairing", "needs a pairing handshake"),
    (
        "plugins",
        "needs a plugin loader; phoenix is one static binary by design",
    ),
    ("promos", "needs a promo catalog service"),
    ("qr", "needs companion apps (issue #21)"),
    ("sandbox", "needs container orchestration"),
];

#[cfg(test)]
pub fn find(name: &str) -> Option<&'static Spec> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn list_text(json: bool) -> String {
    if json {
        let cmds: Vec<serde_json::Value> = COMMANDS
            .iter()
            .map(|c| serde_json::json!({"name": c.name, "summary": c.summary}))
            .collect();
        let missing: Vec<serde_json::Value> = NOT_BUILT
            .iter()
            .map(|(name, why)| serde_json::json!({"name": name, "reason": why}))
            .collect();
        let v = serde_json::json!({"commands": cmds, "not_built": missing});
        let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
        s.push('\n');
        return s;
    }
    let mut out = format!("{} commands\n\n", COMMANDS.len());
    for c in COMMANDS {
        out.push_str(&format!("  {:<16}{}\n", c.name, c.summary));
    }
    out.push_str(&format!(
        "\n{} OpenClaw commands are not built here (reasons: phoenix commands --json):\n",
        NOT_BUILT.len()
    ));
    for (name, _) in NOT_BUILT {
        out.push_str(&format!("  {name}\n"));
    }
    out
}

pub fn docs_text() -> String {
    "OpenPhoenix documentation\n  wiki      https://github.com/Paulus1337/OpenPhoenix/wiki\n  source    https://github.com/Paulus1337/OpenPhoenix\n  issues    https://github.com/Paulus1337/OpenPhoenix/issues\n\nNo API key yet? ollama needs none; google and nvidia have free tiers.\n  phoenix configure    set one up now\n"
        .to_string()
}

pub fn system_text() -> String {
    format!(
        "openphoenix {}\n  target      {}\n  config      {}\n  nest        {}\n  commands    {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        config::config_path().display(),
        config::home().display(),
        COMMANDS.len(),
    )
}

pub fn channels_text(cfg: &Config) -> String {
    let rows: Vec<(&str, bool, usize)> = vec![
        (
            "telegram",
            !cfg.telegram_token.is_empty(),
            cfg.telegram_allowed.len(),
        ),
        (
            "discord",
            !cfg.discord_token.is_empty(),
            cfg.discord_allowed.len(),
        ),
        (
            "slack",
            !cfg.slack_bot_token.is_empty(),
            cfg.slack_allowed.len(),
        ),
        (
            "signal",
            !cfg.signal_account.is_empty(),
            cfg.signal_allowed.len(),
        ),
        ("irc", !cfg.irc_server.is_empty(), cfg.irc_allowed.len()),
        (
            "matrix",
            !cfg.matrix_token.is_empty(),
            cfg.matrix_allowed.len(),
        ),
        (
            "mattermost",
            !cfg.mattermost_token.is_empty(),
            cfg.mattermost_allowed.len(),
        ),
        ("imessage", cfg.imessage_enabled, cfg.imessage_allowed.len()),
    ];
    let live = rows.iter().filter(|(_, on, _)| *on).count();
    let mut out = format!("{live} of {} channels configured\n", rows.len());
    for (name, on, allowed) in rows {
        if on {
            let who = if allowed == 0 {
                "no allowlist: refuses everyone".to_string()
            } else {
                format!("{allowed} allowed")
            };
            out.push_str(&format!("  on   {name:<12}{who}\n"));
        } else {
            out.push_str(&format!("  off  {name}\n"));
        }
    }
    out
}

pub fn directory_text(cfg: &Config) -> String {
    let groups: Vec<(&str, &Vec<String>)> = vec![
        ("telegram", &cfg.telegram_allowed),
        ("discord", &cfg.discord_allowed),
        ("slack", &cfg.slack_allowed),
        ("signal", &cfg.signal_allowed),
        ("irc", &cfg.irc_allowed),
        ("matrix", &cfg.matrix_allowed),
        ("mattermost", &cfg.mattermost_allowed),
        ("imessage", &cfg.imessage_allowed),
    ];
    let total: usize = groups.iter().map(|(_, v)| v.len()).sum();
    if total == 0 {
        return "no allowlisted ids anywhere; every channel refuses all senders\n".to_string();
    }
    let mut out = format!("{total} allowlisted ids\n");
    for (name, ids) in groups {
        for id in ids.iter() {
            out.push_str(&format!("  {name:<12}{id}\n"));
        }
    }
    out
}

pub fn exec_policy_text(cfg: &Config) -> String {
    let mut out = String::from("shell command policy\n");
    out.push_str(&format!(
        "  confirm before running   {}\n  queue for approval       {}\n  outside the workspace    {}\n",
        if cfg.confirm_shell { "yes" } else { "no" },
        if cfg.approvals { "yes" } else { "no" },
        if cfg.allow_outside_workspace { "allowed" } else { "refused" },
    ));
    if cfg.deny_commands.is_empty() {
        out.push_str("  extra deny patterns      none beyond the built-ins\n");
    } else {
        out.push_str(&format!(
            "  extra deny patterns      {}\n",
            cfg.deny_commands.len()
        ));
        for p in &cfg.deny_commands {
            out.push_str(&format!("      {p}\n"));
        }
    }
    out
}

pub fn gateway_text(cfg: &Config) -> String {
    if !cfg.http_enabled {
        return "http gateway is off; enable [http] in config.toml to turn it on\n".to_string();
    }
    format!(
        "http gateway\n  listening   {}:{}\n  web ui      {}\n  auth        {}\n",
        cfg.http_bind,
        cfg.http_port,
        if cfg.http_web { "on" } else { "off" },
        if cfg.http_token.is_empty() {
            "NO TOKEN SET"
        } else {
            "bearer token"
        },
    )
}

pub fn webhooks_text(cfg: &Config) -> String {
    let hooks: Vec<&crate::config::Job> =
        cfg.jobs.iter().filter(|j| !j.webhook.is_empty()).collect();
    if !cfg.http_enabled {
        return "webhooks need the http gateway; enable [http] in config.toml\n".to_string();
    }
    if hooks.is_empty() {
        return "no webhook jobs configured\n".to_string();
    }
    let mut out = format!("{} webhook endpoints\n", hooks.len());
    for j in hooks {
        out.push_str(&format!("  POST /hook/{}\n", j.webhook));
    }
    out
}

pub fn capability_text(cfg: &Config) -> String {
    let mut out = format!("{}/{}\n", cfg.provider, cfg.model);
    out.push_str(&format!(
        "  wire format   {}\n",
        crate::providers::provider_api(&cfg.provider)
    ));
    match crate::catalog::lookup(&cfg.provider, &cfg.model) {
        Some(i) => {
            out.push_str(&format!("  context       {} tokens\n", i.context_window));
            out.push_str(&format!("  max output    {} tokens\n", i.max_tokens));
            if i.cost_in > 0.0 || i.cost_out > 0.0 {
                out.push_str(&format!(
                    "  price         ${:.2} in / ${:.2} out per million tokens\n",
                    i.cost_in, i.cost_out
                ));
            } else {
                out.push_str("  price         not published\n");
            }
        }
        None => out.push_str("  context       unknown model; limits are guessed from the name\n"),
    }
    let has_key = !cfg.api_key.is_empty() || cfg.provider == "ollama";
    out.push_str(&format!(
        "  key           {}\n",
        if has_key { "present" } else { "MISSING" }
    ));
    if has_key {
        match crate::providers::list_models(cfg) {
            Ok(m) => out.push_str(&format!("  live models   {}\n", m.len())),
            Err(e) => out.push_str(&format!("  live models   query failed: {e}\n")),
        }
    }
    out
}

pub fn completion_script(shell: &str) -> Result<String, String> {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    let joined = names.join(" ");
    match shell {
        "bash" => Ok(format!(
            "_phoenix() {{\n  COMPREPLY=($(compgen -W \"{joined}\" -- \"${{COMP_WORDS[COMP_CWORD]}}\"))\n}}\ncomplete -F _phoenix phoenix\n"
        )),
        "zsh" => Ok(format!(
            "#compdef phoenix\n_arguments '1:command:({joined})'\n"
        )),
        "fish" => Ok(names
            .iter()
            .map(|n| format!("complete -c phoenix -a {n}\n"))
            .collect::<String>()),
        other => Err(format!(
            "unknown shell '{other}': expected bash, zsh, or fish"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_sorted() {
        let mut seen = std::collections::BTreeSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.name), "duplicate command: {}", c.name);
            assert!(!c.summary.is_empty(), "{} has no summary", c.name);
        }
        let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "keep COMMANDS alphabetical");
    }

    #[test]
    fn nothing_is_advertised_and_unbuilt_at_the_same_time() {
        for (name, why) in NOT_BUILT {
            assert!(
                find(name).is_none(),
                "{name} is offered as a command but also listed as not built"
            );
            assert!(!why.is_empty(), "{name} does not say what it needs");
        }
    }

    #[test]
    fn completion_covers_every_command_and_refuses_unknown_shells() {
        for shell in ["bash", "zsh", "fish"] {
            let s = completion_script(shell).expect(shell);
            assert!(s.contains("doctor"), "{shell} script lost a command");
        }
        assert!(completion_script("powershell").is_err());
    }

    #[test]
    fn channels_report_an_empty_allowlist_as_refusing_everyone() {
        let mut cfg = Config {
            telegram_token: "t".into(),
            ..Config::default()
        };
        cfg.telegram_allowed.clear();
        assert!(channels_text(&cfg).contains("refuses everyone"));
    }

    #[test]
    fn a_gateway_without_a_token_is_called_out() {
        let cfg = Config {
            http_enabled: true,
            http_token: String::new(),
            ..Config::default()
        };
        assert!(gateway_text(&cfg).contains("NO TOKEN SET"));
    }

    #[test]
    fn capability_reports_the_wire_format_and_context() {
        let cfg = Config {
            provider: "nvidia".into(),
            model: "nvidia/nemotron-3-super-120b-a12b".into(),
            ..Config::default()
        };
        let t = capability_text(&cfg);
        assert!(t.contains("openai-completions"), "{t}");
        assert!(t.contains("1048576"), "{t}");
    }
}
