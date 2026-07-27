use std::env;
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use crate::config;
use crate::migrate;

pub const PROVIDERS: &[(&str, &str, &str)] = &[
    ("anthropic", "claude-sonnet-5", "Claude"),
    ("openai", "gpt-5.6-sol", "GPT"),
    (
        "openrouter",
        "anthropic/claude-sonnet-5",
        "many models, one key",
    ),
    (
        "nvidia",
        "deepseek-ai/deepseek-v4-flash",
        "build.nvidia.com",
    ),
    ("google", "gemini-3.1-pro-preview", "Gemini"),
    ("ollama", "llama3.3", "local, no key needed"),
];

fn detected_key_var(kind: &str) -> Option<&'static str> {
    config::provider_key_vars(kind)
        .iter()
        .find(|v| env::var(v).map(|s| !s.is_empty()).unwrap_or(false))
        .copied()
}

pub fn default_provider_index() -> usize {
    PROVIDERS
        .iter()
        .position(|(kind, _, _)| detected_key_var(kind).is_some())
        .unwrap_or(0)
}

pub fn parse_choice(input: &str, max: usize, default: usize) -> Option<usize> {
    let t = input.trim();
    if t.is_empty() {
        return Some(default);
    }
    match t.parse::<usize>() {
        Ok(n) if (1..=max).contains(&n) => Some(n - 1),
        _ => None,
    }
}

pub fn parse_yn(input: &str, default_yes: bool) -> bool {
    match input.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn ask(r: &mut impl BufRead, w: &mut impl Write, prompt: &str) -> Result<String, String> {
    write!(w, "{prompt}")
        .and_then(|_| w.flush())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn build_config(
    provider: &str,
    model: &str,
    api_key: &str,
    key_var: Option<&str>,
    tg_token: &str,
    tg_ids: &[String],
    telegram: bool,
    approvals: bool,
) -> String {
    let mut out = String::from("# Written by the phoenix first-flight wizard.\n");
    out.push_str("\n[provider]\n");
    out.push_str(&format!("kind = {}\n", toml_str(provider)));
    out.push_str(&format!("model = {}\n", toml_str(model)));
    if !api_key.is_empty() {
        out.push_str(&format!("api_key = {}\n", toml_str(api_key)));
    } else if let Some(var) = key_var {
        out.push_str(&format!("# api_key comes from {var} in the environment.\n"));
    } else if provider != "ollama" {
        out.push_str("# api_key: export PHOENIX_API_KEY (or the provider's standard var).\n");
    }
    if approvals {
        out.push_str("\n[security]\napprovals = true\n");
    }
    if telegram {
        out.push_str("\n[telegram]\n");
        if !tg_token.is_empty() {
            out.push_str(&format!("token = {}\n", toml_str(tg_token)));
        } else {
            out.push_str("# token comes from the PHOENIX_TELEGRAM_TOKEN env var.\n");
        }
        let ids: Vec<String> = tg_ids.iter().map(|s| toml_str(s)).collect();
        out.push_str(&format!("allowed_chat_ids = [{}]\n", ids.join(", ")));
    }
    out
}

fn offer_beacon(r: &mut impl BufRead, w: &mut impl Write) -> Result<(), String> {
    if !crate::service::systemd_available() {
        return Ok(());
    }
    let ans = ask(
        r,
        w,
        "Light the beacon - install and start the phoenix service now, so \
the gateway runs in the background? [Y/n] ",
    )?;
    if !parse_yn(&ans, true) {
        let _ = writeln!(w, "later: phoenix service install");
        return Ok(());
    }
    match crate::service::install() {
        Ok(out) => {
            let _ = writeln!(w, "{out}");
        }
        Err(e) => {
            let _ = writeln!(
                w,
                "could not start the service ({e}); later: phoenix service install"
            );
        }
    }
    Ok(())
}

fn write_config(path: &Path, toml: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, toml).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn offer_migration(
    src: &Path,
    cfg_path: &Path,
    r: &mut impl BufRead,
    w: &mut impl Write,
) -> Result<bool, String> {
    let _ = writeln!(w, "I see an OpenClaw config at {}.", src.display());
    let ans = ask(
        r,
        w,
        "Migrate it? Model, workspace, and chat allowlists carry over; \
secrets stay in env. [Y/n] ",
    )?;
    if !parse_yn(&ans, true) {
        return Ok(false);
    }
    let raw = fs::read_to_string(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let mut v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("{} is not valid JSON: {e}", src.display()))?;

    if let Some(dir) = src.parent() {
        if let Some(tok) = migrate::resolve_secret_token(&v, dir) {
            let ans = ask(
                r,
                w,
                "Carry your Telegram bot token over too, so phoenix can fly \
the instant you switch? [Y/n] ",
            )?;
            if parse_yn(&ans, true) {
                v["channels"]["telegram"]["botToken"] = serde_json::Value::String(tok);
            }
        }
    }
    let m = migrate::from_gateway(&v);
    write_config(cfg_path, &m.toml)?;
    let _ = writeln!(w, "\nReborn. Config written to {}.", cfg_path.display());
    if !m.notes.is_empty() {
        let _ = writeln!(w, "\nnext steps:");
        for n in &m.notes {
            let _ = writeln!(w, "  - {n}");
        }
    }
    Ok(true)
}

pub fn first_run(
    gateway_src: Option<&Path>,
    cfg_path: &Path,
    offer_service: bool,
    r: &mut impl BufRead,
    w: &mut impl Write,
) -> Result<bool, String> {
    let _ = writeln!(
        w,
        "\n🔥 openphoenix {} - first flight\n",
        env!("CARGO_PKG_VERSION")
    );

    if let Some(src) = gateway_src {
        match offer_migration(src, cfg_path, r, w) {
            Ok(true) => {
                if offer_service {
                    offer_beacon(r, w)?;
                }
                return Ok(true);
            }
            Ok(false) => {
                let _ = writeln!(w, "Fresh start it is.\n");
            }
            Err(e) => {
                let _ = writeln!(w, "migration failed ({e}); setting up fresh instead.\n");
            }
        }
    }

    let _ = writeln!(w, "Pick your model provider:");
    for (i, (kind, model, blurb)) in PROVIDERS.iter().enumerate() {
        let found = detected_key_var(kind)
            .map(|v| format!("  [{v} found ✓]"))
            .unwrap_or_default();
        let _ = writeln!(w, "  {}) {kind:<10} {blurb} ({model}){found}", i + 1);
    }
    let default = default_provider_index();
    let idx = loop {
        let ans = ask(r, w, &format!("Choice [{}]: ", default + 1))?;
        match parse_choice(&ans, PROVIDERS.len(), default) {
            Some(i) => break i,
            None => {
                let _ = writeln!(w, "pick a number between 1 and {}", PROVIDERS.len());
            }
        }
    };
    let (kind, default_model, _) = PROVIDERS[idx];

    let ans = ask(r, w, &format!("Model [{default_model}]: "))?;
    let model = if ans.is_empty() {
        default_model.to_string()
    } else {
        ans
    };

    let key_var = detected_key_var(kind);
    let mut api_key = String::new();
    if kind == "ollama" {
        let _ = writeln!(w, "ollama is local - no key needed.");
    } else if let Some(var) = key_var {
        let _ = writeln!(
            w,
            "Using {var} from your environment - nothing gets stored."
        );
    } else {
        api_key = ask(
            r,
            w,
            "API key (input is visible; Enter skips - export PHOENIX_API_KEY \
or the provider's standard var later): ",
        )?;
    }

    let ans = ask(
        r,
        w,
        "Add Telegram? A bot token from @BotFather puts phoenix in your \
pocket. [y/N] ",
    )?;
    let telegram = parse_yn(&ans, false);
    let mut tg_token = String::new();
    let mut tg_ids: Vec<String> = Vec::new();
    let mut approvals = false;
    if telegram {
        tg_token = ask(
            r,
            w,
            "Bot token (Enter = use the PHOENIX_TELEGRAM_TOKEN env var): ",
        )?;
        let ids = ask(
            r,
            w,
            "Allowed chat ids, comma-separated (@userinfobot tells you yours; \
empty = refuse everyone): ",
        )?;
        tg_ids = ids
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if tg_ids.is_empty() {
            let _ = writeln!(
                w,
                "note: empty allowlist fails closed - add allowed_chat_ids before serve."
            );
        }
        let ans = ask(
            r,
            w,
            "Shell guard - should chat commands wait for your /approve before \
touching this system? [Y/n] ",
        )?;
        approvals = parse_yn(&ans, true);
    }

    let toml = build_config(
        kind, &model, &api_key, key_var, &tg_token, &tg_ids, telegram, approvals,
    );
    write_config(cfg_path, &toml)?;
    let _ = writeln!(w, "\nNest built: {} (mode 600)", cfg_path.display());
    if offer_service && telegram {
        offer_beacon(r, w)?;
    }
    let _ = writeln!(w, "\nSpread your wings:");
    let _ = writeln!(w, "  phoenix           chat right here");
    if telegram {
        let _ = writeln!(w, "  phoenix serve     go live on Telegram (foreground)");
    } else {
        let _ = writeln!(
            w,
            "  phoenix serve     go live once a channel is configured"
        );
    }
    let _ = writeln!(w, "  phoenix service   run the gateway in the background");
    let _ = writeln!(w, "  phoenix doctor    check the nest\n");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("phx-onboard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d.join("config.toml")
    }

    #[test]
    fn choice_parsing() {
        assert_eq!(parse_choice("", 6, 3), Some(3));
        assert_eq!(parse_choice("1", 6, 0), Some(0));
        assert_eq!(parse_choice("6", 6, 0), Some(5));
        assert_eq!(parse_choice("7", 6, 0), None);
        assert_eq!(parse_choice("0", 6, 0), None);
        assert_eq!(parse_choice("x", 6, 0), None);
    }

    #[test]
    fn yn_parsing() {
        assert!(parse_yn("", true));
        assert!(!parse_yn("", false));
        assert!(parse_yn("y", false));
        assert!(parse_yn("YES", false));
        assert!(!parse_yn("n", true));
        assert!(!parse_yn("whatever", true));
    }

    #[test]
    fn config_builds_with_stored_key_and_telegram() {
        let toml = build_config(
            "openai",
            "gpt-5.6-sol",
            "sk-test",
            None,
            "123:abc",
            &["42".into()],
            true,
            true,
        );
        let cfg = config::parse(&toml).unwrap();
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-5.6-sol");
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.telegram_token, "123:abc");
        assert_eq!(cfg.telegram_allowed, vec!["42".to_string()]);
        assert!(cfg.approvals);
    }

    #[test]
    fn config_builds_keyless_for_env_var() {
        let toml = build_config(
            "nvidia",
            "m",
            "",
            Some("NVIDIA_API_KEY"),
            "",
            &[],
            false,
            false,
        );
        assert!(toml.contains("# api_key comes from NVIDIA_API_KEY"));
        let cfg = config::parse(&toml).unwrap();
        assert_eq!(cfg.provider, "nvidia");
        assert!(cfg.api_key.is_empty());
    }

    #[test]
    fn wizard_fresh_setup_ollama_no_telegram() {
        let path = tmp("fresh");

        let mut input = Cursor::new(b"6\n\nn\n".to_vec());
        let mut out = Vec::new();
        let ok = first_run(None, &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.provider, "ollama");
        assert_eq!(cfg.model, "llama3.3");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("first flight"));
        assert!(text.contains("Nest built"));
    }

    #[test]
    fn wizard_declined_migration_falls_through() {
        let path = tmp("declined");
        let src = path.with_file_name("openclaw.json");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, "{}").unwrap();

        let mut input = Cursor::new(b"n\n6\n\nn\n".to_vec());
        let mut out = Vec::new();
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("I see an OpenClaw config"));
        assert!(text.contains("Fresh start"));
    }

    #[test]
    fn wizard_accepts_migration() {
        let path = tmp("migrate");
        let src = path.with_file_name("openclaw.json");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"anthropic/claude-sonnet-5"}}}}"#,
        )
        .unwrap();
        let mut input = Cursor::new(b"\n".to_vec());
        let mut out = Vec::new();
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-5");
        assert!(String::from_utf8(out).unwrap().contains("Reborn"));
    }
}
