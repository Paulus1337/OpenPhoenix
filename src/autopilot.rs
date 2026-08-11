use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::{self, Config};
use crate::migrate;
use crate::onboard;
use crate::providers::{self, ChatBackend, Msg};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub provider: String,
    pub model: String,
    pub key: String,
    pub source: String,
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Origin {
    OldNest,
    Detected,
}

impl Candidate {
    pub fn spec(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
    fn key_from_env(&self) -> bool {
        self.source.starts_with('$')
    }
}

pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub keys: Vec<(String, Vec<String>)>,
    pub gateway: Option<serde_json::Value>,
    pub notes: Vec<String>,
}

pub fn default_model_for(kind: &str) -> Option<&'static str> {
    if let Some((_, m, _)) = onboard::PROVIDERS.iter().find(|(k, _, _)| *k == kind) {
        return Some(m);
    }
    match kind {
        "groq" => Some("llama-3.3-70b-versatile"),
        "mistral" => Some("mistral-large-latest"),
        "deepseek" => Some("deepseek-chat"),
        _ => None,
    }
}

fn env_key(kind: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<(String, String)> {
    config::provider_key_vars(kind).iter().find_map(|v| {
        env(v)
            .filter(|s| !s.is_empty())
            .map(|s| (s, format!("${v}")))
    })
}

pub fn ollama_alive() -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = ([127, 0, 0, 1], 11434).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn split_spec(spec: &str, fallback_provider: &str) -> (String, String) {
    match spec.split_once('/') {
        Some((k, m)) if config::known_kind(k) => (k.to_string(), m.to_string()),
        _ if !fallback_provider.is_empty() => (fallback_provider.to_string(), spec.to_string()),
        _ => (String::new(), spec.to_string()),
    }
}

pub fn gather(
    gateway_src: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    ollama_up: bool,
) -> Scan {
    let mut notes = Vec::new();
    let mut keys: Vec<(String, Vec<String>)> = Vec::new();
    let mut gateway = None;
    let mut specs: Vec<(String, String, String, Origin)> = Vec::new();

    if let Some(src) = gateway_src {
        if let Ok(raw) = std::fs::read_to_string(src) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(dir) = src.parent() {
                    keys = migrate::collect_keys(dir);
                }
                let primary = v["agents"]["defaults"]["model"]["primary"]
                    .as_str()
                    .unwrap_or("");
                if !primary.is_empty() {
                    let (p, m) = split_spec(primary, &migrate::primary_provider(&v));
                    if !p.is_empty() {
                        specs.push((p, m, "old nest primary".into(), Origin::OldNest));
                    }
                }
                if let Some(a) = v["agents"]["defaults"]["model"]["fallbacks"].as_array() {
                    for f in a.iter().filter_map(|x| x.as_str()) {
                        let (p, m) = split_spec(f, "");
                        if !p.is_empty() {
                            specs.push((p, m, "old nest fallback".into(), Origin::OldNest));
                        }
                    }
                }
                notes.push(format!("legacy gateway config: {}", src.display()));
                gateway = Some(v);
            }
        }
    }
    if !keys.is_empty() {
        let s: Vec<String> = keys
            .iter()
            .map(|(p, k)| format!("{p} ({})", k.len()))
            .collect();
        notes.push(format!("keys found: {}", s.join(", ")));
    }

    for (kind, ..) in onboard::PROVIDERS.iter() {
        if *kind == "ollama" {
            continue;
        }
        if let Some(m) = default_model_for(kind) {
            specs.push((
                kind.to_string(),
                m.to_string(),
                "detected key".into(),
                Origin::Detected,
            ));
        }
    }
    for (p, _) in keys.iter() {
        if let Some(m) = default_model_for(p) {
            specs.push((
                p.clone(),
                m.to_string(),
                "carried key".into(),
                Origin::Detected,
            ));
        }
    }
    if ollama_up {
        notes.push("ollama alive on :11434".into());
        specs.push((
            "ollama".into(),
            default_model_for("ollama").unwrap_or("llama3.3").into(),
            "local".into(),
            Origin::Detected,
        ));
    }

    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for (p, m, why, origin) in specs {
        let spec = format!("{p}/{m}");
        if !seen.insert(spec) {
            continue;
        }
        let (key, source) = if p == "ollama" {
            (String::new(), "local".to_string())
        } else if let Some((_, ring)) = keys.iter().find(|(kp, r)| *kp == p && !r.is_empty()) {
            (ring[0].clone(), why.clone())
        } else if let Some((k, var)) = env_key(&p, env) {
            (k, var)
        } else {
            continue;
        };
        candidates.push(Candidate {
            provider: p,
            model: m,
            key,
            source,
            origin,
        });
    }
    Scan {
        candidates,
        keys,
        gateway,
        notes,
    }
}

pub fn probe_live(c: &Candidate) -> Result<(), String> {
    let cfg = Config {
        provider: c.provider.clone(),
        model: c.model.clone(),
        api_key: c.key.clone(),
        max_retries: 0,
        ..Config::default()
    };
    let mut p = providers::make(&cfg).map_err(|e| e.0)?;
    let history = vec![Msg::User {
        content: "Reply with the single word: PONG".into(),
        images: Vec::new(),
    }];
    let r = p
        .chat(&cfg, "You are a connectivity check.", &history, &[])
        .map_err(|e| e.0)?;
    if r.text.trim().is_empty() && r.tool_calls.is_empty() {
        return Err("empty reply".into());
    }
    Ok(())
}

const MAX_PROBES: usize = 6;

pub struct FoundNest {
    pub telegram: bool,
    pub keys: usize,
    pub persona: usize,
}

pub fn inspect_nest(src: &Path) -> Option<FoundNest> {
    let raw = std::fs::read_to_string(src).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let dir = src.parent()?;
    let keys = migrate::collect_keys(dir)
        .iter()
        .map(|(_, k)| k.len())
        .sum();
    let telegram = migrate::resolve_secret_token(&v, dir).is_some();
    let ws = migrate::gateway_workspace(&v, dir);
    let persona = ["SOUL.md", "AGENTS.md", "IDENTITY.md", "USER.md", "TOOLS.md"]
        .iter()
        .filter(|n| ws.join(n).is_file())
        .count();
    Some(FoundNest {
        telegram,
        keys,
        persona,
    })
}

pub fn ask_migrate(
    src: &Path,
    found: &FoundNest,
    r: &mut impl BufRead,
    w: &mut impl Write,
) -> bool {
    let _ = writeln!(w, "\nFound an existing gateway setup at {}", src.display());
    let mut parts: Vec<String> = Vec::new();
    if found.keys > 0 {
        parts.push(format!("{} API key(s)", found.keys));
    }
    if found.telegram {
        parts.push("the Telegram bot token".to_string());
    }
    if found.persona > 0 {
        parts.push(format!("{} persona file(s)", found.persona));
    }
    if parts.is_empty() {
        parts.push("its model settings".to_string());
    }
    let _ = writeln!(w, "Migrating would carry over: {}.", parts.join(", "));
    let _ = writeln!(
        w,
        "Starting fresh leaves the old gateway untouched and sets up new."
    );
    let _ = write!(w, "Migrate it? [Y/n] ");
    let _ = w.flush();
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        let _ = writeln!(w, "\nno answer; starting fresh.");
        return false;
    }
    let yes = matches!(line.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes");
    let _ = writeln!(
        w,
        "{}",
        if yes {
            "migrating your old gateway setup."
        } else {
            "starting fresh; the old gateway is left untouched."
        }
    );
    yes
}

pub fn auto_first_run(
    gateway_src: Option<&Path>,
    cfg_path: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    ollama_up: bool,
    probe: &mut dyn FnMut(&Candidate) -> Result<(), String>,
    w: &mut impl Write,
) -> Result<bool, String> {
    let scan = gather(gateway_src, env, ollama_up);
    if scan.candidates.is_empty() {
        return Err("no keys in env, no old nest, no local ollama".into());
    }
    let _ = writeln!(w, "scanning the nest…");
    for n in &scan.notes {
        let _ = writeln!(w, "  {n}");
    }
    let _ = writeln!(w, "probing for a live model…");
    let mut winner: Option<usize> = None;
    let mut tried = 0usize;
    for (i, c) in scan.candidates.iter().enumerate().take(MAX_PROBES) {
        tried += 1;
        let t = Instant::now();
        match probe(c) {
            Ok(()) => {
                let _ = writeln!(
                    w,
                    "  ✓ {} ({}) {:.1}s",
                    c.spec(),
                    c.source,
                    t.elapsed().as_secs_f32()
                );
                winner = Some(i);
                break;
            }
            Err(e) => {
                let short: String = e.lines().next().unwrap_or("").chars().take(90).collect();
                let _ = writeln!(w, "  ✗ {}: {short}", c.spec());
            }
        }
    }
    let Some(wi) = winner else {
        return Err(format!("no provider answered ({tried} probed)"));
    };
    let win = scan.candidates[wi].clone();

    let from_nest = scan.gateway.is_some();
    let mut fallbacks: Vec<String> = Vec::new();
    for (i, c) in scan.candidates.iter().enumerate() {
        if i == wi || c.provider == win.provider || fallbacks.len() >= 3 {
            continue;
        }
        if from_nest && c.origin != Origin::OldNest {
            continue;
        }
        if fallbacks
            .iter()
            .any(|s| s.split('/').next() == Some(c.provider.as_str()))
        {
            continue;
        }
        fallbacks.push(c.spec());
    }

    let mut telegram_carried = false;
    let mut persona_src: Option<std::path::PathBuf> = None;
    let toml = if let Some(mut v) = scan.gateway {
        if let Some(dir) = gateway_src.and_then(Path::parent) {
            if let Some(tok) = migrate::resolve_secret_token(&v, dir) {
                v["channels"]["telegram"]["botToken"] = serde_json::Value::String(tok);
                telegram_carried = true;
            }
            persona_src = Some(migrate::gateway_workspace(&v, dir));
        }
        let m = migrate::from_gateway(&v);
        if !m.secrets.is_empty() {
            let carried: Vec<(String, Vec<String>)> = m
                .secrets
                .iter()
                .map(|(n, s)| (n.clone(), vec![s.clone()]))
                .collect();
            crate::secrets::stash_named(&carried)
                .map_err(|e| format!("cannot encrypt the carried token: {e}"))?;
        }
        let mut t = migrate::set_primary(&m.toml, &win.spec());
        t = migrate::set_fallbacks(&t, &fallbacks);
        if !scan.keys.is_empty() {
            crate::secrets::stash_provider_keys(&scan.keys)
                .map_err(|e| format!("cannot encrypt the carried keys: {e}"))?;
        } else if !win.key.is_empty() && !win.key_from_env() {
            crate::secrets::stash_provider_keys(&[(win.provider.clone(), vec![win.key.clone()])])
                .map_err(|e| format!("cannot encrypt the carried key: {e}"))?;
        }
        t
    } else {
        let key_var = win
            .key_from_env()
            .then(|| win.source.trim_start_matches('$'));
        let stored = if win.key_from_env() {
            ""
        } else {
            win.key.as_str()
        };
        let mut t = onboard::build_config(&onboard::Plan {
            provider: win.provider.clone(),
            model: win.model.clone(),
            api_key: stored.to_string(),
            key_var: key_var.map(str::to_string),
            ..onboard::Plan::default()
        });
        if !fallbacks.is_empty() {
            t = migrate::set_fallbacks(&t, &fallbacks);
        }
        t
    };
    onboard::write_config(cfg_path, &toml)?;
    let anthropic_oauth = crate::oauth::import_from_claude_cli()
        .and_then(|t| crate::oauth::save(&t).ok().map(|()| t.refresh.is_empty()));
    let persona_notes = persona_src
        .map(|ws| {
            let workspace = crate::config::parse(&toml)
                .map(|c| c.workspace)
                .unwrap_or_else(|_| crate::config::home_dir().join("phoenix"));
            migrate::carry_persona(&ws, &workspace)
        })
        .unwrap_or_default();

    let _ = writeln!(w, "\nReborn. {} (mode 600)", cfg_path.display());
    for n in &persona_notes {
        let _ = writeln!(w, "  {n}");
    }
    let _ = writeln!(w, "  model     {}", win.spec());
    if !fallbacks.is_empty() {
        let _ = writeln!(w, "  fallbacks {}", fallbacks.join(", "));
    }
    if !scan.keys.is_empty() {
        let s: Vec<String> = scan
            .keys
            .iter()
            .map(|(p, k)| format!("{p} ({})", k.len()))
            .collect();
        let _ = writeln!(w, "  keys      {} (encrypted store)", s.join(", "));
    }
    if telegram_carried {
        let _ = writeln!(w, "  telegram  token + allowlist carried");
    }
    if let Some(access_only) = anthropic_oauth {
        let tail = if access_only {
            "access token (sign in again when it expires)"
        } else {
            "auto-refreshing"
        };
        let _ = writeln!(w, "  anthropic OAuth carried from the Claude CLI ({tail})");
    }
    let _ = writeln!(
        w,
        "\nnothing was asked because nothing needed asking. Change any of it: {}",
        cfg_path.display()
    );
    if telegram_carried && crate::service::systemd_available() {
        if other_gateway_running() {
            let _ = writeln!(
                w,
                "beacon on hold: another gateway is running and would \
fight for the bot. When ready: stop the other gateway, then run phoenix service install"
            );
        } else {
            match crate::service::install() {
                Ok(out) => {
                    let _ = writeln!(w, "beacon lit: gateway runs at boot:\n{out}");
                }
                Err(e) => {
                    let _ = writeln!(w, "beacon failed ({e}); later: phoenix service install");
                }
            }
        }
    } else {
        let _ = writeln!(w, "background gateway: phoenix service install");
    }
    let _ = writeln!(w);
    Ok(true)
}

fn other_gateway_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", "gateway.*serve"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("phx-auto-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn migration_prompt_defaults_to_yes_on_bare_enter() {
        let d = tmp("ask-yes");
        let src = d.join("gateway.json");
        fs::write(&src, "{}").unwrap();
        let found = FoundNest {
            telegram: true,
            keys: 2,
            persona: 5,
        };
        let mut out = Vec::new();
        let yes = ask_migrate(&src, &found, &mut "\n".as_bytes(), &mut out);
        let text = String::from_utf8_lossy(&out);
        assert!(yes, "bare Enter must accept the [Y/n] default");
        assert!(text.contains("2 API key(s)"), "{text}");
        assert!(text.contains("Telegram bot token"), "{text}");
        assert!(text.contains("5 persona file(s)"), "{text}");
        assert!(text.contains("[Y/n]"), "{text}");
    }

    #[test]
    fn migration_prompt_honours_no_and_leaves_old_gateway_alone() {
        let d = tmp("ask-no");
        let src = d.join("gateway.json");
        fs::write(&src, "{}").unwrap();
        let found = FoundNest {
            telegram: false,
            keys: 0,
            persona: 0,
        };
        for answer in ["n\n", "N\n", "no\n"] {
            let mut out = Vec::new();
            let yes = ask_migrate(&src, &found, &mut answer.as_bytes(), &mut out);
            assert!(!yes, "{answer:?} must decline");
            let text = String::from_utf8_lossy(&out);
            assert!(text.contains("starting fresh"), "{text}");
            assert!(text.contains("untouched"), "{text}");
        }
    }

    #[test]
    fn migration_prompt_starts_fresh_when_stdin_is_closed() {
        let d = tmp("ask-eof");
        let src = d.join("gateway.json");
        fs::write(&src, "{}").unwrap();
        let found = FoundNest {
            telegram: true,
            keys: 1,
            persona: 0,
        };
        let mut out = Vec::new();
        let yes = ask_migrate(&src, &found, &mut "".as_bytes(), &mut out);
        assert!(!yes, "EOF must never silently migrate");
    }

    #[test]
    fn nest_inspection_counts_what_would_be_carried() {
        let d = tmp("inspect");
        let ws = d.join("workspace");
        fs::create_dir_all(&ws).unwrap();
        for n in ["SOUL.md", "USER.md"] {
            fs::write(ws.join(n), "x").unwrap();
        }
        let src = d.join("gateway.json");
        fs::write(
            &src,
            serde_json::json!({ "agents": { "defaults": { "workspace": ws.to_string_lossy() } } })
                .to_string(),
        )
        .unwrap();
        let found = inspect_nest(&src).expect("nest");
        assert_eq!(found.persona, 2, "must count persona files it would carry");
    }

    fn gateway_json(
        dir: &std::path::Path,
        primary: &str,
        fallbacks: &[&str],
    ) -> std::path::PathBuf {
        let fb: Vec<String> = fallbacks.iter().map(|s| format!("\"{s}\"")).collect();
        let j = format!(
            "{{\"agents\":{{\"defaults\":{{\"model\":{{\"primary\":\"{primary}\",\
\"fallbacks\":[{}]}}}}}}}}",
            fb.join(",")
        );
        let p = dir.join("gateway.json");
        fs::write(&p, j).unwrap();
        p
    }

    #[test]
    fn gather_orders_old_nest_first() {
        let d = tmp("order");
        let gw = gateway_json(&d, "anthropic/claude-x", &["nvidia/foo"]);
        let env = |k: &str| {
            matches!(k, "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY").then(|| "sk-test-123".to_string())
        };
        let s = gather(Some(&gw), &env, false);
        assert_eq!(s.candidates[0].spec(), "anthropic/claude-x");
        assert_eq!(s.candidates[1].spec(), "nvidia/foo");
        assert!(s.candidates.iter().all(|c| c.provider != "ollama"));
    }

    #[test]
    fn gather_env_only() {
        let env = |k: &str| (k == "OPENROUTER_API_KEY").then(|| "sk-or-abcdefgh".to_string());
        let s = gather(None, &env, false);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(s.candidates[0].provider, "openrouter");
        assert!(s.candidates[0].key_from_env());
    }

    #[test]
    fn gather_offers_local_ollama() {
        let s = gather(None, &no_env, true);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(s.candidates[0].provider, "ollama");
        assert!(s.candidates[0].key.is_empty());
    }

    #[test]
    fn auto_errs_with_nothing() {
        let d = tmp("nothing");
        let cfg = d.join("config.toml");
        let mut out = Vec::new();
        let r = auto_first_run(None, &cfg, &no_env, false, &mut |_| Ok(()), &mut out);
        assert!(r.is_err());
        assert!(!cfg.exists());
    }

    #[test]
    fn auto_picks_second_when_first_dead() {
        let d = tmp("second");
        let gw = gateway_json(&d, "anthropic/claude-x", &["nvidia/foo"]);
        let cfg = d.join("config.toml");
        let env = |k: &str| {
            matches!(k, "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY").then(|| "sk-test-123".to_string())
        };
        let mut out = Vec::new();
        let mut n = 0;
        let r = auto_first_run(
            Some(&gw),
            &cfg,
            &env,
            false,
            &mut |_| {
                n += 1;
                if n == 1 {
                    Err("HTTP 401: dead key".into())
                } else {
                    Ok(())
                }
            },
            &mut out,
        );
        assert_eq!(r, Ok(true));
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("kind = \"nvidia\""), "{text}");
        assert!(text.contains("model = \"foo\""), "{text}");
        assert!(
            text.contains("fallbacks = [\"anthropic/claude-x\"]"),
            "{text}"
        );
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("✗ anthropic/claude-x"));
        assert!(printed.contains("✓ nvidia/foo"));
    }

    #[test]
    fn env_key_stays_out_of_config() {
        let d = tmp("envkey");
        let cfg = d.join("config.toml");
        let env = |k: &str| (k == "OPENROUTER_API_KEY").then(|| "sk-or-abcdefgh".to_string());
        let mut out = Vec::new();
        let r = auto_first_run(None, &cfg, &env, false, &mut |_| Ok(()), &mut out);
        assert_eq!(r, Ok(true));
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(!text.contains("sk-or-abcdefgh"), "{text}");
        assert!(text.contains("OPENROUTER_API_KEY"), "{text}");
    }

    #[test]
    fn migration_never_invents_a_fallback_the_old_nest_did_not_have() {
        let d = tmp("nofabricate");
        let gw = gateway_json(&d, "anthropic/claude-x", &[]);
        let cfg = d.join("config.toml");
        let env = |k: &str| {
            matches!(k, "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY").then(|| "sk-test-123".to_string())
        };
        let mut out = Vec::new();
        let r = auto_first_run(Some(&gw), &cfg, &env, true, &mut |_| Ok(()), &mut out);
        assert_eq!(r, Ok(true));
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(
            !text.contains("fallbacks"),
            "migration carries settings, it must not invent fallbacks from stray env keys or a local ollama: {text}"
        );
        let printed = String::from_utf8(out).unwrap();
        assert!(!printed.contains("fallbacks"), "{printed}");
    }

    #[test]
    fn migration_keeps_the_fallbacks_the_old_nest_did_have() {
        let d = tmp("keepfallbacks");
        let gw = gateway_json(&d, "anthropic/claude-x", &["nvidia/foo"]);
        let cfg = d.join("config.toml");
        let env = |k: &str| {
            matches!(
                k,
                "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY" | "OPENROUTER_API_KEY"
            )
            .then(|| "sk-test-123".to_string())
        };
        let mut out = Vec::new();
        let r = auto_first_run(Some(&gw), &cfg, &env, true, &mut |_| Ok(()), &mut out);
        assert_eq!(r, Ok(true));
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("fallbacks = [\"nvidia/foo\"]"), "{text}");
        assert!(
            !text.contains("openrouter"),
            "an unrelated env key is not a fallback the user chose: {text}"
        );
    }

    #[test]
    fn a_fresh_start_may_still_use_detected_keys_as_fallbacks() {
        let d = tmp("freshfallback");
        let cfg = d.join("config.toml");
        let env = |k: &str| {
            matches!(k, "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY").then(|| "sk-test-123".to_string())
        };
        let mut out = Vec::new();
        let r = auto_first_run(None, &cfg, &env, false, &mut |_| Ok(()), &mut out);
        assert_eq!(r, Ok(true));
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(
            text.contains("fallbacks"),
            "with no old nest there is nothing to carry, so detected keys are all we have to offer: {text}"
        );
    }

    #[test]
    fn probe_failure_reports_short_reason() {
        let d = tmp("allfail");
        let gw = gateway_json(&d, "anthropic/claude-x", &[]);
        let cfg = d.join("config.toml");
        let env = |k: &str| (k == "ANTHROPIC_API_KEY").then(|| "sk-test-123".to_string());
        let mut out = Vec::new();
        let r = auto_first_run(
            Some(&gw),
            &cfg,
            &env,
            false,
            &mut |_| Err("HTTP 401: nope\nlong body".into()),
            &mut out,
        );
        assert!(r.is_err());
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("HTTP 401: nope"));
        assert!(!printed.contains("long body"));
        assert!(!cfg.exists());
    }
}
