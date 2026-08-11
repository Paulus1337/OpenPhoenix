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

pub fn key_signup_url(kind: &str) -> Option<&'static str> {
    match kind {
        "anthropic" => Some("https://console.anthropic.com/settings/keys"),
        "openai" => Some("https://platform.openai.com/api-keys"),
        "openrouter" => Some("https://openrouter.ai/keys"),
        "nvidia" => Some("https://build.nvidia.com"),
        "google" => Some("https://aistudio.google.com/apikey"),
        "groq" => Some("https://console.groq.com/keys"),
        "mistral" => Some("https://console.mistral.ai/api-keys"),
        "deepseek" => Some("https://platform.deepseek.com/api_keys"),
        "xai" => Some("https://console.x.ai"),
        "moonshot" => Some("https://platform.moonshot.ai/console/api-keys"),
        "cohere" => Some("https://dashboard.cohere.com/api-keys"),
        "together" => Some("https://api.together.xyz/settings/api-keys"),
        "novita" => Some("https://novita.ai/settings/key-management"),
        "huggingface" => Some("https://huggingface.co/settings/tokens"),
        "ollama" => None,
        _ => None,
    }
}

pub fn free_key_routes() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "ollama",
            "runs models on this machine, no key and no account",
        ),
        (
            "google",
            "https://aistudio.google.com/apikey (free tier, no card)",
        ),
        (
            "nvidia",
            "https://build.nvidia.com (free credits on signup)",
        ),
        (
            "openrouter",
            "https://openrouter.ai/keys (has :free models)",
        ),
    ]
}

pub fn no_key_anywhere_help() -> String {
    let mut s = String::from("No API key found yet. Free ways to get running:\n");
    for (kind, how) in free_key_routes() {
        s.push_str(&format!("  {kind:<12}{how}\n"));
    }
    s.push_str("Then: phoenix configure");
    s
}

pub fn key_help_line(kind: &str) -> Option<String> {
    let url = key_signup_url(kind)?;
    let var = config::provider_key_vars(kind)
        .first()
        .copied()
        .unwrap_or("PHOENIX_API_KEY");
    Some(format!(
        "No key yet? Make one at {url} then paste it here, or export {var} and press Enter."
    ))
}

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

pub const CHANNELS: &[(&str, &str, &str)] = &[
    ("telegram", "Telegram", "bot token from @BotFather"),
    ("whatsapp", "WhatsApp", "Meta Cloud API token + phone id"),
    ("discord", "Discord", "bot token + channel ids"),
    ("slack", "Slack", "app + bot tokens, Socket Mode"),
    ("signal", "Signal", "signal-cli and your number"),
    ("irc", "IRC", "server, nick, channels"),
    ("matrix", "Matrix", "homeserver + access token"),
    ("mattermost", "Mattermost", "server url + token"),
    ("imessage", "iMessage", "a Mac with the imsg CLI"),
];

pub const FEATURES: &[(&str, &str)] = &[
    ("browser", "browser automation, drives a local Chromium"),
    ("images", "image generation"),
    ("tts", "voice replies"),
    ("audio", "transcribe incoming voice notes"),
    ("board", "task board cards"),
    ("canvas", "rendered pages at /canvas"),
    ("memory", "smarter recall with embeddings"),
    ("dreaming", "journal its day while idle"),
    ("heartbeat", "periodic check-in message"),
    ("audit_log", "append a JSONL record of every action"),
];

pub const BINDS: &[(&str, &str)] = &[
    ("127.0.0.1", "this machine only, safest"),
    ("0.0.0.0", "every IPv4 interface, reachable on your network"),
    ("::", "every interface, IPv4 and IPv6"),
];

pub fn weak_password(pass: &str) -> Option<&'static str> {
    const JUNK: &[&str] = &[
        "password", "passwd", "admin", "root", "phoenix", "changeme", "letmein", "secret", "test",
        "temp", "demo", "guest", "user", "login", "qwerty",
    ];
    let t = pass.trim();
    if t.len() < 12 {
        return Some("shorter than 12 characters");
    }
    let low = t.to_ascii_lowercase();
    if JUNK.iter().any(|j| low == *j || low.starts_with(j)) {
        return Some("built on a well-known word");
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return Some("all digits");
    }
    if t.chars().collect::<std::collections::BTreeSet<_>>().len() < 5 {
        return Some("too few distinct characters");
    }
    None
}

pub fn parse_multi(input: &str, max: usize) -> Result<Vec<usize>, String> {
    let t = input.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") || t == "0" {
        return Ok(Vec::new());
    }
    if t.eq_ignore_ascii_case("all") {
        return Ok((0..max).collect());
    }
    let mut out: Vec<usize> = Vec::new();
    for piece in t.split([',', ' ']).filter(|s| !s.trim().is_empty()) {
        let n: usize = piece
            .trim()
            .parse()
            .map_err(|_| format!("'{}' is not a number", piece.trim()))?;
        if n < 1 || n > max {
            return Err(format!("{n} is outside 1 to {max}"));
        }
        if !out.contains(&(n - 1)) {
            out.push(n - 1);
        }
    }
    Ok(out)
}

pub fn model_menu(models: &[String], default_model: &str) -> Vec<String> {
    let mut ranked: Vec<String> = Vec::new();
    if models.iter().any(|m| m == default_model) {
        ranked.push(default_model.to_string());
    }
    for m in models {
        if !ranked.contains(m) {
            ranked.push(m.clone());
        }
    }
    ranked
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub enum ModelAnswer {
    Keep,
    Spec(String),
    Junk,
}

pub fn model_answer(ans: &str) -> ModelAnswer {
    let t = ans.trim();
    if t.is_empty() {
        return ModelAnswer::Keep;
    }
    let low = t.to_ascii_lowercase();
    if matches!(low.as_str(), "y" | "yes") {
        return ModelAnswer::Keep;
    }
    if t.len() < 3 || matches!(low.as_str(), "n" | "no") {
        return ModelAnswer::Junk;
    }
    ModelAnswer::Spec(t.to_string())
}

fn pick_one(
    r: &mut impl BufRead,
    w: &mut impl Write,
    title: &str,
    items: &[crate::menu::Item],
    default: usize,
) -> Result<Option<usize>, String> {
    if let Ok(choice) = crate::menu::select(title, items, default) {
        return Ok(choice);
    }
    let _ = writeln!(w, "{title}");
    for (i, it) in items.iter().enumerate() {
        let _ = writeln!(w, "  {}) {}  {}", i + 1, it.label, it.hint);
    }
    loop {
        let ans = ask(r, w, &format!("\nChoice [{}]: ", default + 1))?;
        match parse_choice(&ans, items.len(), default) {
            Some(i) => return Ok(Some(i)),
            None => {
                let _ = writeln!(w, "pick a number between 1 and {}", items.len());
            }
        }
    }
}

fn pick_many(
    r: &mut impl BufRead,
    w: &mut impl Write,
    title: &str,
    items: &[crate::menu::Item],
    prompt: &str,
) -> Result<Vec<usize>, String> {
    if let Ok(choice) = crate::menu::multi_select(title, items) {
        return Ok(choice.unwrap_or_default());
    }
    let _ = writeln!(w, "{title}");
    for (i, it) in items.iter().enumerate() {
        let _ = writeln!(w, "  {:>2}) {:<11} {}", i + 1, it.label, it.hint);
    }
    loop {
        let a = ask(r, w, prompt)?;
        match parse_multi(&a, items.len()) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let _ = writeln!(w, "{e}");
            }
        }
    }
}

fn pick_yn(
    r: &mut impl BufRead,
    w: &mut impl Write,
    title: &str,
    default_yes: bool,
) -> Result<bool, String> {
    if let Ok(choice) = crate::menu::confirm(title, default_yes) {
        return Ok(choice.unwrap_or(false));
    }
    let suffix = if default_yes { " [Y/n] " } else { " [y/N] " };
    let ans = ask_or(
        r,
        w,
        &format!("{title}{suffix}"),
        if default_yes { "y" } else { "n" },
    );
    Ok(parse_yn(&ans, default_yes))
}

fn ask(r: &mut impl BufRead, w: &mut impl Write, prompt: &str) -> Result<String, String> {
    write!(w, "{prompt}")
        .and_then(|_| w.flush())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("input ended before setup finished".into());
    }
    Ok(line.trim().to_string())
}

pub type ChannelPlan = (String, Vec<(String, String)>, Vec<String>);

#[derive(Default)]
pub struct Plan {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub key_var: Option<String>,
    pub fallbacks: Vec<String>,
    pub approvals: bool,
    pub channels: Vec<ChannelPlan>,
    pub features: Vec<String>,
    pub http: Option<HttpPlan>,
}

pub struct HttpPlan {
    pub port: u16,
    pub bind: String,
    pub token: String,
    pub web: bool,
    pub user: String,
    pub pass: String,
}

fn allow_key_for(channel: &str) -> &'static str {
    match channel {
        "telegram" => "allowed_chat_ids",
        "whatsapp" | "signal" => "allowed_numbers",
        "discord" | "slack" => "allowed_channel_ids",
        "irc" => "allowed_nicks",
        "matrix" | "mattermost" => "allowed_users",
        _ => "allowed_senders",
    }
}

pub fn build_config(plan: &Plan) -> String {
    let mut out = String::from("# Written by the phoenix first-flight wizard.\n");
    out.push_str("\n[provider]\n");
    out.push_str(&format!("kind = {}\n", toml_str(&plan.provider)));
    out.push_str(&format!("model = {}\n", toml_str(&plan.model)));
    if !plan.api_key.is_empty() {
        out.push_str(&format!("api_key = {}\n", toml_str(&plan.api_key)));
    } else if let Some(var) = plan.key_var.as_deref() {
        out.push_str(&format!("# api_key comes from {var} in the environment.\n"));
    } else if plan.provider != "ollama" {
        out.push_str("# api_key: export PHOENIX_API_KEY (or the provider's standard var).\n");
    }
    if !plan.fallbacks.is_empty() {
        let list: Vec<String> = plan.fallbacks.iter().map(|s| toml_str(s)).collect();
        out.push_str(&format!("fallbacks = [{}]\n", list.join(", ")));
    }

    let audit = plan.features.iter().any(|f| f == "audit_log");
    if plan.approvals || audit {
        out.push_str("\n[security]\n");
        if plan.approvals {
            out.push_str("approvals = true\n");
        }
        if audit {
            out.push_str("audit_log = true\n");
        }
    }

    for (name, fields, allow) in &plan.channels {
        out.push_str(&format!("\n[{name}]\n"));
        for (k, v) in fields {
            if v.is_empty() {
                continue;
            }
            out.push_str(&format!("{k} = {}\n", toml_str(v)));
        }
        if name == "imessage" {
            out.push_str("enabled = true\n");
        }
        let list: Vec<String> = allow.iter().map(|s| toml_str(s)).collect();
        out.push_str(&format!(
            "{} = [{}]\n",
            allow_key_for(name),
            list.join(", ")
        ));
        if allow.is_empty() {
            out.push_str("# empty list refuses everyone: add yourself before serve.\n");
        }
    }

    if let Some(h) = &plan.http {
        out.push_str("\n[http]\nenabled = true\n");
        out.push_str(&format!("port = {}\n", h.port));
        out.push_str(&format!("bind = {}\n", toml_str(&h.bind)));
        if h.token.is_empty() {
            out.push_str("# token comes from the PHOENIX_HTTP_TOKEN env var.\n");
        } else {
            out.push_str(&format!("token = {}\n", toml_str(&h.token)));
        }
        if h.web {
            out.push_str("web = true\n");
            out.push_str(&format!("username = {}\n", toml_str(&h.user)));
            out.push_str(&format!("password = {}\n", toml_str(&h.pass)));
        }
    }

    let has = |name: &str| plan.features.iter().any(|f| f == name);
    if has("browser") {
        out.push_str("\n[browser]\nenabled = true\n");
    }
    if has("images") || has("tts") {
        out.push_str("\n[media]\n");
        if has("images") {
            out.push_str("images = true\n");
        }
        if has("tts") {
            out.push_str("tts = true\n");
        }
    }
    if has("audio") {
        out.push_str("\n[audio]\ntranscribe = true\n");
    }
    if has("board") {
        out.push_str("\n[board]\nenabled = true\n");
    }
    if has("canvas") {
        out.push_str("\n[canvas]\nenabled = true\n");
    }
    if has("memory") {
        out.push_str("\n[memory]\nembeddings = true\n");
    }
    if has("dreaming") {
        out.push_str("\n[dreaming]\nminutes = 45\n");
    }
    if has("heartbeat") {
        out.push_str("\n[heartbeat]\nminutes = 180\n");
    }
    out
}

fn ask_or(r: &mut impl BufRead, w: &mut impl Write, prompt: &str, fallback: &str) -> String {
    match ask(r, w, prompt) {
        Ok(v) => v,
        Err(_) => {
            let _ = writeln!(w);
            fallback.to_string()
        }
    }
}

fn offer_beacon(r: &mut impl BufRead, w: &mut impl Write) -> Result<(), String> {
    if !crate::service::systemd_available() {
        return Ok(());
    }
    let ans = ask_or(
        r,
        w,
        "Light the beacon - install and start the phoenix service now, so \
the gateway runs in the background? [Y/n] ",
        "n",
    );
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

const CONFIG_BACKUPS: usize = 5;

fn rotate_config_backups(path: &Path) {
    if !path.exists() {
        return;
    }
    let base = |i: usize| -> std::path::PathBuf {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config");
        path.with_file_name(if i == 0 {
            format!("{name}.bak")
        } else {
            format!("{name}.bak.{i}")
        })
    };
    let _ = fs::remove_file(base(CONFIG_BACKUPS - 1));
    for i in (1..CONFIG_BACKUPS - 1).rev() {
        let _ = fs::rename(base(i), base(i + 1));
    }
    let _ = fs::rename(base(0), base(1));
    if let Ok(body) = fs::read(path) {
        let _ = crate::security::write_atomic(&base(0), &body, Some(0o600));
    }
}

pub(crate) fn write_config(path: &Path, toml: &str) -> Result<(), String> {
    rotate_config_backups(path);
    crate::security::write_atomic(path, toml.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

fn offer_migration(
    src: &Path,
    cfg_path: &Path,
    r: &mut impl BufRead,
    w: &mut impl Write,
) -> Result<Option<bool>, String> {
    let _ = writeln!(w, "I see a legacy gateway config at {}.", src.display());
    let ans = ask(
        r,
        w,
        "Migrate it? Model, workspace, and chat allowlists carry over; \
secrets stay in env. [Y/n] ",
    )?;
    if !parse_yn(&ans, true) {
        return Ok(None);
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
    let keys = src.parent().map(migrate::collect_keys).unwrap_or_default();
    let mut carried = false;
    if !keys.is_empty() {
        let summary: Vec<String> = keys
            .iter()
            .map(|(p, k)| format!("{p} ({})", k.len()))
            .collect();
        let ans = ask(
            r,
            w,
            &format!(
                "Found API keys in the old nest: {}. Carry them all over? [Y/n] ",
                summary.join(", ")
            ),
        )?;
        carried = parse_yn(&ans, true);
    }

    let primary_raw = v["agents"]["defaults"]["model"]["primary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut chosen_spec = primary_raw.clone();
    if !primary_raw.is_empty() {
        let ans = ask(r, w, &format!("Default model [{primary_raw}]: "))?;
        match model_answer(&ans) {
            ModelAnswer::Keep => {}
            ModelAnswer::Spec(s) => chosen_spec = s,
            ModelAnswer::Junk => {
                let _ = writeln!(
                    w,
                    "'{}' does not look like a model; keeping {primary_raw}",
                    ans.trim()
                );
            }
        }
    }

    let old_fallbacks: Vec<String> = v["agents"]["defaults"]["model"]["fallbacks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let fb_default = if old_fallbacks.is_empty() {
        "none".to_string()
    } else {
        old_fallbacks.join(", ")
    };
    let ans = ask(
        r,
        w,
        &format!("Fallback models, comma-separated provider/model [{fb_default}]: "),
    )?;
    let chosen_fallbacks: Vec<String> = {
        let t = ans.trim();
        let low = t.to_ascii_lowercase();
        if t.is_empty() || matches!(low.as_str(), "y" | "yes") {
            old_fallbacks.clone()
        } else if matches!(low.as_str(), "none" | "n" | "no") {
            Vec::new()
        } else {
            ans.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| s.len() >= 3)
                .collect()
        }
    };

    let provider = match chosen_spec.split_once('/') {
        Some((k, _)) if crate::config::known_kind(k) => k.to_string(),
        _ => migrate::primary_provider(&v),
    };
    let covered = carried && keys.iter().any(|(p, k)| *p == provider && !k.is_empty());
    let mut api_key = String::new();
    if !covered && !provider.is_empty() && provider != "ollama" {
        if let Some(dir) = src.parent() {
            if let Some(k) = migrate::resolve_api_key(&v, dir, &provider) {
                let ans = ask(
                    r,
                    w,
                    &format!(
                        "Found your {provider} API key in the old nest. Carry it over \
too? [Y/n] "
                    ),
                )?;
                if parse_yn(&ans, true) {
                    api_key = k;
                }
            }
        }
        if api_key.is_empty() && detected_key_var(&provider).is_none() {
            if let Some(help) = key_help_line(&provider) {
                let _ = writeln!(w, "{help}");
            }
            let ans = ask(
                r,
                w,
                &format!(
                    "API key for {provider} (input is visible; Enter skips - export \
PHOENIX_API_KEY later): "
                ),
            )?;
            let t = ans.trim();
            let low = t.to_ascii_lowercase();
            if t.len() >= 8 && !matches!(low.as_str(), "y" | "yes" | "n" | "no" | "none") {
                api_key = t.to_string();
            } else if !t.is_empty() {
                let _ = writeln!(w, "'{t}' does not look like an API key; skipping.");
            }
        }
    }
    let has_key = covered
        || !api_key.is_empty()
        || provider == "ollama"
        || (!provider.is_empty() && detected_key_var(&provider).is_some());

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
    let mut toml = m.toml.clone();
    if !chosen_spec.is_empty() && chosen_spec != primary_raw {
        toml = migrate::set_primary(&toml, &chosen_spec);
    }
    toml = migrate::set_fallbacks(&toml, &chosen_fallbacks);
    if carried && !keys.is_empty() {
        crate::secrets::stash_provider_keys(&keys)
            .map_err(|e| format!("cannot encrypt the carried keys: {e}"))?;
    } else if !api_key.is_empty() {
        crate::secrets::stash_provider_keys(&[(provider.clone(), vec![api_key.clone()])])
            .map_err(|e| format!("cannot encrypt the carried key: {e}"))?;
    }
    write_config(cfg_path, &toml)?;
    let _ = writeln!(w, "\nReborn. Config written to {}.", cfg_path.display());
    if let Some(dir) = src.parent() {
        let ws = migrate::gateway_workspace(&v, dir);
        let workspace = crate::config::parse(&toml)
            .map(|c| c.workspace)
            .unwrap_or_else(|_| crate::config::home_dir().join("phoenix"));
        for n in migrate::carry_persona(&ws, &workspace) {
            let _ = writeln!(w, "{n}");
        }
    }
    let stored = covered || !api_key.is_empty();
    if carried && !keys.is_empty() {
        let summary: Vec<String> = keys
            .iter()
            .map(|(p, k)| format!("{p} ({})", k.len()))
            .collect();
        let _ = writeln!(
            w,
            "API keys encrypted in the secret store: {}.",
            summary.join(", ")
        );
    } else if !api_key.is_empty() {
        let _ = writeln!(w, "API key encrypted in the secret store.");
    }
    let notes: Vec<&String> = m
        .notes
        .iter()
        .filter(|n| !stored || !n.starts_with("export PHOENIX_API_KEY"))
        .collect();
    if !notes.is_empty() {
        let _ = writeln!(w, "\nnext steps:");
        for n in notes {
            let _ = writeln!(w, "  - {n}");
        }
    }
    Ok(Some(has_key))
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
            Ok(Some(has_key)) => {
                if offer_service {
                    if has_key {
                        offer_beacon(r, w)?;
                    } else {
                        let _ = writeln!(
                            w,
                            "beacon on hold: no API key yet. Set one, then run \
`phoenix service install`."
                        );
                    }
                }
                return Ok(true);
            }
            Ok(None) => {
                let _ = writeln!(w, "Fresh start it is.\n");
            }
            Err(e) => {
                let _ = writeln!(w, "migration failed ({e}); setting up fresh instead.\n");
            }
        }
    }

    let mut plan = Plan::default();

    let mut prov_items: Vec<crate::menu::Item> = PROVIDERS
        .iter()
        .map(|(kind, model, blurb)| {
            let found = detected_key_var(kind)
                .map(|v| format!(" [{v} found]"))
                .unwrap_or_default();
            crate::menu::Item::new(*kind, format!("{blurb} ({model}){found}"))
        })
        .collect();
    prov_items.push(crate::menu::Item::new(
        "custom",
        "any OpenAI-compatible endpoint",
    ));
    let default = default_provider_index();
    let idx = match pick_one(
        r,
        w,
        "Step 1 of 5: where should the brain come from?",
        &prov_items,
        default,
    )? {
        Some(i) => i,
        None => {
            let _ = writeln!(w, "\nsetup cancelled; nothing was written.");
            return Ok(false);
        }
    };

    let mut base_url = String::new();
    let (kind, default_model): (String, String) = if idx == PROVIDERS.len() {
        let url = loop {
            let a = ask(r, w, "Base URL (like http://localhost:1234/v1): ")?;
            if !a.is_empty() {
                break a;
            }
            let _ = writeln!(w, "a base URL is required for a custom endpoint");
        };
        base_url = url;
        let m = ask(r, w, "Model name: ")?;
        ("custom".to_string(), m)
    } else {
        let (k, m, _) = PROVIDERS[idx];
        (k.to_string(), m.to_string())
    };
    plan.provider = kind.clone();
    plan.key_var = detected_key_var(&kind).map(str::to_string);

    if kind == "ollama" {
        let _ = writeln!(w, "\nollama runs locally, no key needed.");
    } else if let Some(var) = plan.key_var.as_deref() {
        let _ = writeln!(
            w,
            "\nUsing {var} from your environment, nothing gets stored."
        );
    } else {
        let _ = writeln!(
            w,
            "\nPaste an API key. Input is visible. Enter skips and you can export \
PHOENIX_API_KEY later."
        );
        if let Some(help) = key_help_line(&kind) {
            let _ = writeln!(w, "{help}");
        }
        plan.api_key = ask(r, w, "API key: ")?;
    }

    let _ = writeln!(w, "\nStep 2 of 5: which model?");
    let mut probe = config::Config {
        provider: kind.clone(),
        model: default_model.clone(),
        api_key: plan.api_key.clone(),
        base_url: base_url.clone(),
        ..config::Config::default()
    };
    if plan.api_key.is_empty() {
        if let Some(var) = plan.key_var.as_deref() {
            probe.api_key = env::var(var).unwrap_or_default();
        }
    }
    let live = if probe.api_key.is_empty() && kind != "ollama" && kind != "custom" {
        Vec::new()
    } else {
        let _ = writeln!(w, "  asking {kind} what it can run...");
        crate::providers::list_models(&probe).unwrap_or_default()
    };

    plan.model = if live.is_empty() {
        let _ = writeln!(
            w,
            "  no live list available, type a model name (Enter keeps {default_model})"
        );
        let a = ask(r, w, &format!("Model [{default_model}]: "))?;
        if a.is_empty() {
            default_model.clone()
        } else {
            a
        }
    } else {
        let menu = model_menu(&live, &default_model);
        let shown = menu.len().min(30);
        let _ = writeln!(w, "  {} models available:\n", menu.len());
        for (i, m) in menu.iter().take(shown).enumerate() {
            let tag = if *m == default_model {
                "  (suggested)"
            } else {
                ""
            };
            let _ = writeln!(w, "  {:>2}) {m}{tag}", i + 1);
        }
        if menu.len() > shown {
            let _ = writeln!(
                w,
                "  ... and {} more, type any name instead",
                menu.len() - shown
            );
        }
        loop {
            let a = ask(r, w, "\nNumber, or a model name [1]: ")?;
            if a.is_empty() {
                break menu[0].clone();
            }
            if let Ok(n) = a.parse::<usize>() {
                if n >= 1 && n <= shown {
                    break menu[n - 1].clone();
                }
                let _ = writeln!(w, "pick 1 to {shown}, or type a model name");
                continue;
            }
            break a;
        }
    };

    if !live.is_empty() {
        let _ = writeln!(
            w,
            "\nA fallback keeps you talking when the first model errors or rate limits."
        );
        let a = ask_or(r, w, "Fallback model (Enter = none): ", "");
        if !a.is_empty() {
            plan.fallbacks.push(a);
        }
    }

    let chan_items: Vec<crate::menu::Item> = CHANNELS
        .iter()
        .map(|(_, label, need)| crate::menu::Item::new(*label, *need))
        .collect();
    let picks = pick_many(
        r,
        w,
        "\nStep 3 of 5: chat apps.",
        &chan_items,
        "\nChannels [none]: ",
    )?;

    for i in picks {
        let (name, label, _) = CHANNELS[i];
        let _ = writeln!(w, "\n{label}:");
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut prompts: Vec<(&str, &str)> = match name {
            "telegram" => vec![(
                "token",
                "Bot token (Enter = PHOENIX_TELEGRAM_TOKEN env var)",
            )],
            "whatsapp" => vec![
                (
                    "token",
                    "Cloud API token (Enter = PHOENIX_WHATSAPP_TOKEN env var)",
                ),
                ("phone_id", "Business phone number id"),
                ("verify_token", "Webhook verify token you choose"),
            ],
            "discord" => vec![("token", "Bot token (Enter = PHOENIX_DISCORD_TOKEN env var)")],
            "slack" => vec![
                (
                    "app_token",
                    "App token, xapp- (Enter = PHOENIX_SLACK_APP_TOKEN)",
                ),
                (
                    "bot_token",
                    "Bot token, xoxb- (Enter = PHOENIX_SLACK_BOT_TOKEN)",
                ),
            ],
            "signal" => vec![("account", "Your number in E.164, like +4915551234567")],
            "irc" => vec![
                ("server", "Server host, like irc.libera.chat"),
                ("nick", "Nickname"),
            ],
            "matrix" => vec![
                ("homeserver", "Homeserver URL"),
                ("token", "Access token"),
                ("user_id", "Your user id, like @you:matrix.org"),
            ],
            "mattermost" => vec![("url", "Server URL"), ("token", "Bot token")],
            _ => Vec::new(),
        };
        if name == "imessage" {
            prompts.push(("cli_path", "Path to the imsg binary [imsg]"));
        }
        for (key, prompt) in prompts {
            let v = ask(r, w, &format!("  {prompt}: "))?;
            fields.push((key.to_string(), v));
        }
        if name == "irc" {
            let ch = ask(r, w, "  Channels to join, comma-separated: ")?;
            let list: Vec<String> = ch
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                fields.push(("__channels".to_string(), list.join(",")));
            }
        }
        let who = match name {
            "telegram" => "chat ids (@userinfobot tells you yours)",
            "whatsapp" | "signal" => "phone numbers in E.164",
            "discord" | "slack" => "channel ids",
            "irc" => "nicknames",
            "matrix" | "mattermost" => "user ids",
            _ => "sender handles",
        };
        let ids = ask(
            r,
            w,
            &format!("  Allowed {who}, comma-separated (empty refuses everyone): "),
        )?;
        let allow: Vec<String> = ids
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if allow.is_empty() {
            let _ = writeln!(
                w,
                "  noted: this channel stays closed until you add someone."
            );
        }
        plan.channels.push((name.to_string(), fields, allow));
    }

    if pick_yn(
        r,
        w,
        "\nStep 4 of 5: a web UI and HTTP API on this machine?",
        false,
    )? {
        let port = loop {
            let p = ask(r, w, "  Port [8787]: ")?;
            if p.is_empty() {
                break 8787u16;
            }
            match p.parse::<u16>() {
                Ok(n) if n > 0 => break n,
                _ => {
                    let _ = writeln!(w, "  pick a port between 1 and 65535");
                }
            }
        };
        let bind_items: Vec<crate::menu::Item> = BINDS
            .iter()
            .map(|(addr, blurb)| crate::menu::Item::new(*addr, *blurb))
            .collect();
        let bi =
            pick_one(r, w, "\n  Who should be able to reach it?", &bind_items, 0)?.unwrap_or(0);
        let bind = BINDS[bi].0.to_string();
        let token = ask_or(
            r,
            w,
            "  API token (Enter = PHOENIX_HTTP_TOKEN env var): ",
            "",
        );
        let a = ask_or(r, w, "  Browser chat UI as well? [y/N] ", "n");
        let mut web = parse_yn(&a, false);
        let mut user = String::new();
        let mut pass = String::new();
        let exposed = bind != "127.0.0.1";
        if web {
            user = ask_or(r, w, "  UI username: ", "");
            pass = ask_or(r, w, "  UI password: ", "");
            if user.is_empty() || pass.is_empty() {
                let _ = writeln!(
                    w,
                    "  the UI needs both a username and password, leaving it off for now"
                );
                web = false;
            }
            while web && exposed {
                match weak_password(&pass) {
                    None => break,
                    Some(why) => {
                        let _ = writeln!(
                            w,
                            "  that password is {why}, and {bind} is reachable from your \
network. Pick a stronger one (12+ characters), or Enter to keep the UI off."
                        );
                        pass = ask_or(r, w, "  UI password: ", "");
                        if pass.is_empty() {
                            let _ = writeln!(w, "  leaving the browser UI off.");
                            web = false;
                        }
                    }
                }
            }
        }
        if exposed {
            let _ = writeln!(
                w,
                "  heads up: {bind} is reachable from your network. Put it behind HTTPS."
            );
        }
        plan.http = Some(HttpPlan {
            port,
            bind,
            token,
            web,
            user,
            pass,
        });
    }

    let feat_items: Vec<crate::menu::Item> = FEATURES
        .iter()
        .map(|(name, blurb)| crate::menu::Item::new(*name, *blurb))
        .collect();
    let feats = pick_many(
        r,
        w,
        "\nStep 5 of 5: extras.",
        &feat_items,
        "\nExtras [none]: ",
    )?;
    for i in feats {
        plan.features.push(FEATURES[i].0.to_string());
    }

    let a = ask_or(
        r,
        w,
        "\nShould shell commands wait for your /approve first? [Y/n] ",
        "y",
    );
    plan.approvals = parse_yn(&a, true);

    let mut toml = build_config(&plan);
    if !base_url.is_empty() {
        toml = toml.replace(
            "\n[agent]",
            &format!("base_url = {}\n\n[agent]", toml_str(&base_url)),
        );
        if !toml.contains("base_url") {
            let anchor = format!("model = {}\n", toml_str(&plan.model));
            toml = toml.replace(
                &anchor,
                &format!("{anchor}base_url = {}\n", toml_str(&base_url)),
            );
        }
    }
    for (name, fields, _) in &plan.channels {
        if let Some((_, chans)) = fields.iter().find(|(k, _)| k == "__channels") {
            let list: Vec<String> = chans.split(',').map(toml_str).collect();
            toml = toml.replace(
                &format!("[{name}]\n"),
                &format!("[{name}]\nchannels = [{}]\n", list.join(", ")),
            );
        }
    }
    toml = toml.replace("__channels = ", "# channels handled above: ");
    write_config(cfg_path, &toml)?;

    let _ = writeln!(w, "\nNest built: {} (mode 600)", cfg_path.display());
    let _ = writeln!(w, "  model     {}/{}", plan.provider, plan.model);
    if !plan.channels.is_empty() {
        let names: Vec<&str> = plan.channels.iter().map(|(n, _, _)| n.as_str()).collect();
        let _ = writeln!(w, "  channels  {}", names.join(", "));
    }
    if let Some(h) = &plan.http {
        let _ = writeln!(
            w,
            "  http      {}:{}{}",
            h.bind,
            h.port,
            if h.web { " with browser UI" } else { "" }
        );
    }
    if !plan.features.is_empty() {
        let _ = writeln!(w, "  extras    {}", plan.features.join(", "));
    }

    let has_key = plan.provider == "ollama"
        || !plan.api_key.is_empty()
        || plan.key_var.is_some()
        || env::var("PHOENIX_API_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    let has_channel = !plan.channels.is_empty();
    if offer_service && has_channel && has_key {
        offer_beacon(r, w)?;
    }

    let _ = writeln!(w, "\nSpread your wings:");
    if has_key {
        let _ = writeln!(w, "  phoenix           chat right here");
    } else {
        let key_hint = config::provider_key_vars(&plan.provider)
            .first()
            .copied()
            .unwrap_or("PHOENIX_API_KEY");
        let _ = writeln!(w, "  export {key_hint}=your-key    then: phoenix chat");
        match key_signup_url(&plan.provider) {
            Some(url) => {
                let _ = writeln!(w, "  need a key first? {url}");
            }
            None => {
                let _ = writeln!(w, "\n{}", no_key_anywhere_help());
            }
        }
    }
    if has_channel {
        let _ = writeln!(w, "  phoenix serve     go live on your channels");
        let _ = writeln!(w, "  phoenix service   run it in the background");
    }
    let _ = writeln!(w, "  phoenix doctor    check the nest\n");
    Ok(true)
}

#[cfg(test)]
mod backup_tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("phx-bak-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rewriting_config_keeps_a_rotating_history() {
        let d = dir("rotate");
        let cfg = d.join("config.toml");

        for i in 0..8 {
            write_config(&cfg, &format!("version = {i}\n")).unwrap();
        }

        assert_eq!(fs::read_to_string(&cfg).unwrap(), "version = 7\n");
        assert_eq!(
            fs::read_to_string(d.join("config.toml.bak")).unwrap(),
            "version = 6\n",
            "the newest backup must be the previous version"
        );
        assert_eq!(
            fs::read_to_string(d.join("config.toml.bak.1")).unwrap(),
            "version = 5\n"
        );
        assert!(
            d.join("config.toml.bak.4").exists(),
            "the full history depth must be kept"
        );
        assert!(
            !d.join("config.toml.bak.5").exists(),
            "history must stay bounded at CONFIG_BACKUPS entries"
        );
        assert_eq!(
            fs::read_to_string(d.join("config.toml.bak.4")).unwrap(),
            "version = 2\n",
            "the oldest kept backup must be the correct generation"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_first_write_creates_no_backup_and_is_owner_only() {
        let d = dir("first");
        let cfg = d.join("config.toml");
        write_config(&cfg, "first = true\n").unwrap();

        assert!(!d.join("config.toml.bak").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&cfg).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} must be owner only", cfg.display());
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg(unix)]
    fn backups_never_leak_secrets_to_other_users() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("perms");
        let cfg = d.join("config.toml");
        write_config(&cfg, "api_key = \"first\"\n").unwrap();
        write_config(&cfg, "api_key = \"second\"\n").unwrap();

        let bak = d.join("config.toml.bak");
        assert!(bak.exists(), "a backup must exist");
        let mode = fs::metadata(&bak).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "backups hold the same secrets as the config");
        let _ = fs::remove_dir_all(&d);
    }
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
    fn every_keyed_provider_says_where_to_get_one() {
        for (kind, _, _) in PROVIDERS {
            if *kind == "ollama" {
                assert!(
                    key_signup_url(kind).is_none(),
                    "ollama needs no key, so it must not advertise a signup page"
                );
                assert!(key_help_line(kind).is_none());
                continue;
            }
            let url = key_signup_url(kind)
                .unwrap_or_else(|| panic!("{kind} takes a key but offers no signup page"));
            assert!(
                url.starts_with("https://"),
                "{kind} signup page must be https, got {url}"
            );
        }
    }

    #[test]
    fn the_key_help_line_names_both_the_page_and_the_variable() {
        let line = key_help_line("anthropic").unwrap_or_default();
        assert!(
            line.contains("https://console.anthropic.com/settings/keys"),
            "help must link the real page: {line}"
        );
        assert!(
            line.contains("ANTHROPIC_API_KEY"),
            "help must name the variable so the env route stays obvious: {line}"
        );
    }

    #[test]
    fn an_unknown_provider_gets_no_invented_signup_page() {
        assert!(key_signup_url("custom").is_none());
        assert!(key_help_line("custom").is_none());
        assert!(key_signup_url("nonsense").is_none());
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
        let toml = build_config(&Plan {
            provider: "openai".into(),
            model: "gpt-5.6-sol".into(),
            api_key: "sk-test".into(),
            approvals: true,
            channels: vec![(
                "telegram".into(),
                vec![("token".into(), "123:abc".into())],
                vec!["42".into()],
            )],
            ..Plan::default()
        });
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
        let toml = build_config(&Plan {
            provider: "nvidia".into(),
            model: "m".into(),
            key_var: Some("NVIDIA_API_KEY".into()),
            ..Plan::default()
        });
        assert!(toml.contains("# api_key comes from NVIDIA_API_KEY"));
        let cfg = config::parse(&toml).unwrap();
        assert_eq!(cfg.provider, "nvidia");
        assert!(cfg.api_key.is_empty());
    }

    #[test]
    fn every_channel_writes_a_parsable_section_with_an_allowlist() {
        for (name, _, _) in CHANNELS {
            let toml = build_config(&Plan {
                provider: "ollama".into(),
                model: "llama3.3".into(),
                channels: vec![(
                    (*name).into(),
                    vec![
                        ("token".into(), "t".into()),
                        ("account".into(), "+15551234567".into()),
                        ("phone_id".into(), "1".into()),
                        ("verify_token".into(), "v".into()),
                        ("app_token".into(), "xapp-1".into()),
                        ("bot_token".into(), "xoxb-1".into()),
                        ("server".into(), "irc.example.org".into()),
                        ("nick".into(), "phx".into()),
                        ("homeserver".into(), "https://m.example.org".into()),
                        ("user_id".into(), "@a:b".into()),
                        ("url".into(), "https://mm.example.org".into()),
                        ("cli_path".into(), "imsg".into()),
                    ],
                    vec!["someone".into()],
                )],
                ..Plan::default()
            });
            let cfg = config::parse(&toml)
                .unwrap_or_else(|e| panic!("channel {name} produced invalid config: {e}"));
            let _ = cfg;
            assert!(
                toml.contains(allow_key_for(name)),
                "channel {name} must write its allowlist key"
            );
        }
    }

    #[test]
    fn every_feature_toggle_produces_a_valid_config() {
        for (name, _) in FEATURES {
            let toml = build_config(&Plan {
                provider: "ollama".into(),
                model: "llama3.3".into(),
                features: vec![(*name).into()],
                ..Plan::default()
            });
            config::parse(&toml)
                .unwrap_or_else(|e| panic!("feature {name} produced invalid config: {e}"));
        }
    }

    #[test]
    fn every_bind_choice_is_accepted_by_the_parser() {
        for (addr, _) in BINDS {
            let toml = build_config(&Plan {
                provider: "ollama".into(),
                model: "llama3.3".into(),
                http: Some(HttpPlan {
                    port: 8787,
                    bind: (*addr).into(),
                    token: "t".into(),
                    web: true,
                    user: "u".into(),
                    pass: "p".into(),
                }),
                ..Plan::default()
            });
            let cfg = config::parse(&toml)
                .unwrap_or_else(|e| panic!("bind {addr} produced invalid config: {e}"));
            assert_eq!(cfg.http_bind, *addr);
            assert!(cfg.http_enabled);
        }
    }

    #[test]
    fn multi_select_parsing_handles_none_all_lists_and_junk() {
        assert_eq!(parse_multi("", 9).unwrap(), Vec::<usize>::new());
        assert_eq!(parse_multi("none", 9).unwrap(), Vec::<usize>::new());
        assert_eq!(parse_multi("all", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_multi("1,3", 9).unwrap(), vec![0, 2]);
        assert_eq!(parse_multi("2 2 1", 9).unwrap(), vec![1, 0]);
        assert!(parse_multi("12", 9).is_err());
        assert!(parse_multi("abc", 9).is_err());
    }

    #[test]
    fn the_model_menu_puts_the_suggested_model_first_without_duplicating_it() {
        let live = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let menu = model_menu(&live, "b");
        assert_eq!(menu[0], "b");
        assert_eq!(menu.len(), 3, "suggested model must not be duplicated");
        let menu = model_menu(&live, "zz");
        assert_eq!(menu, live, "unknown suggestion must not be invented");
    }

    #[test]
    fn every_answer_the_wizard_accepts_survives_the_round_trip_to_disk() {
        let path = tmp("roundtrip");
        let script = b"6\nllama3.3\n1\n123:abcdef\n42,99\n\
y\n9911\n1\nhttp-tok\ny\nadmin\nT3rn-Owl-Rises\n1,3\ny\n"
            .to_vec();
        let mut input = Cursor::new(script);
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();

        let body = fs::read_to_string(&path).unwrap();
        let cfg = config::parse(&body).unwrap();

        assert_eq!(cfg.provider, "ollama", "chosen provider must persist");
        assert_eq!(cfg.model, "llama3.3", "chosen model must persist");
        assert_eq!(
            cfg.telegram_token, "123:abcdef",
            "channel token must persist"
        );
        assert_eq!(
            cfg.telegram_allowed,
            vec!["42".to_string(), "99".to_string()],
            "allowlist must persist exactly"
        );
        assert!(cfg.http_enabled, "http choice must persist");
        assert_eq!(cfg.http_port, 9911, "chosen port must persist");
        assert_eq!(cfg.http_bind, "127.0.0.1", "chosen bind must persist");
        assert_eq!(cfg.http_token, "http-tok", "api token must persist");
        assert!(cfg.http_web, "web ui choice must persist");
        assert_eq!(cfg.http_user, "admin", "ui user must persist");
        assert_eq!(cfg.http_pass, "T3rn-Owl-Rises", "ui password must persist");
        assert!(cfg.browser_enabled, "extra 1 must persist");
        assert!(cfg.media_tts, "extra 3 must persist");
        assert!(!cfg.media_images, "an unpicked extra must stay off");
        assert!(!cfg.board_enabled, "an unpicked extra must stay off");
        assert!(cfg.approvals, "approval choice must persist");

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ollama/llama3.3"),
            "summary must match: {text}"
        );
        assert!(
            text.contains("127.0.0.1:9911"),
            "summary must match: {text}"
        );
    }

    #[test]
    fn the_path_the_wizard_reports_is_the_path_it_actually_wrote() {
        let path = tmp("reported-path");
        let mut input = Cursor::new(b"6\nllama3.3\n\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(&path.display().to_string()),
            "the reported path must be the real one: {text}"
        );
        assert!(path.exists(), "the reported file must exist on disk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "the config holds secrets and must be owner-only"
            );
        }
    }

    #[test]
    fn wizard_fresh_setup_is_five_clear_steps_and_defaults_to_nothing_extra() {
        let path = tmp("fresh");

        let mut input = Cursor::new(b"6\nllama3.3\n\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(None, &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.provider, "ollama");
        assert_eq!(cfg.model, "llama3.3");
        assert!(!cfg.http_enabled, "nothing extra unless asked");
        assert!(
            cfg.telegram_token.is_empty(),
            "no channel forced on the user"
        );
        let text = String::from_utf8(out).unwrap();
        for step in [
            "Step 1 of 5",
            "Step 2 of 5",
            "Step 3 of 5",
            "Step 4 of 5",
            "Step 5 of 5",
        ] {
            assert!(text.contains(step), "missing {step}: {text}");
        }
        assert!(text.contains("Nest built"));
    }

    #[test]
    fn wizard_offers_every_channel_not_just_telegram() {
        let path = tmp("channels-listed");
        let mut input = Cursor::new(b"6\nllama3.3\n\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        for (_, label, _) in CHANNELS {
            assert!(text.contains(label), "channel {label} must be offered");
        }
    }

    #[test]
    fn wizard_can_enable_the_web_ui_on_a_chosen_bind_address() {
        let path = tmp("webui");
        let mut input =
            Cursor::new(b"6\nllama3.3\n\ny\n9911\n2\ntok\ny\nadmin\nT3rn-Owl-Rises\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(cfg.http_enabled);
        assert_eq!(cfg.http_port, 9911);
        assert_eq!(cfg.http_bind, "0.0.0.0");
        assert!(cfg.http_web);
        assert_eq!(cfg.http_user, "admin");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("reachable from your network"),
            "exposing it must be called out: {text}"
        );
    }

    #[test]
    fn a_weak_password_cannot_guard_a_network_reachable_ui() {
        let path = tmp("webui-weak");
        let mut input =
            Cursor::new(b"6\nllama3.3\n\ny\n8899\n2\ntok\ny\nadmin\npw123\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        let cfg = config::parse(&body).unwrap();
        assert!(cfg.http_enabled, "the token-guarded API may still run");
        assert!(
            !cfg.http_web,
            "a browser UI on 0.0.0.0 must never be written with a weak password"
        );
        assert!(
            !body.contains("pw123"),
            "the weak password must not reach the config file: {body}"
        );
    }

    #[test]
    fn a_weak_password_is_accepted_on_loopback_where_it_is_the_users_business() {
        let path = tmp("webui-loopback");
        let mut input =
            Cursor::new(b"6\nllama3.3\n\ny\n8787\n1\ntok\ny\nadmin\npw123\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.http_bind, "127.0.0.1");
        assert!(cfg.http_web, "loopback must not be nagged");
    }

    #[test]
    fn weak_password_rules_catch_the_shapes_that_show_up_in_incidents() {
        assert!(weak_password("pw123").is_some(), "short");
        assert!(weak_password("123456789012").is_some(), "all digits");
        assert!(weak_password("password1234").is_some(), "well-known word");
        assert!(weak_password("aaaaaaaaaaaa").is_some(), "too few distinct");
        assert!(weak_password("admin").is_some(), "well-known and short");
        assert!(weak_password("temp").is_some(), "the drama-file password");
        assert!(
            weak_password("T3rn-Owl-Rises").is_none(),
            "a real passphrase must pass"
        );
    }

    #[test]
    fn wizard_web_ui_without_credentials_refuses_to_turn_on() {
        let path = tmp("webui-nocreds");
        let mut input = Cursor::new(b"6\nllama3.3\n\ny\n\n1\ntok\ny\n\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(cfg.http_enabled, "the API can still run");
        assert!(!cfg.http_web, "the UI must stay off without a password");
    }

    #[test]
    fn wizard_enables_only_the_extras_that_were_picked() {
        let path = tmp("extras");
        let mut input = Cursor::new(b"6\nllama3.3\n\n\n1,5\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        first_run(None, &path, false, &mut input, &mut out).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        let cfg = config::parse(&body).unwrap();
        assert!(cfg.browser_enabled, "picked browser must be on");
        assert!(cfg.board_enabled, "picked board must be on");
        assert!(!cfg.media_images, "unpicked extras must stay off");
        assert!(!cfg.canvas_enabled);
    }

    #[test]
    fn wizard_declined_migration_falls_through() {
        let path = tmp("declined");
        let src = path.with_file_name("gateway.json");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, "{}").unwrap();

        let mut input = Cursor::new(b"n\n6\nllama3.3\n\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("I see a legacy gateway config"));
        assert!(text.contains("Fresh start"));
    }

    fn mig_dir(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = tmp(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (dir.join("config.toml"), dir.join("gateway.json"))
    }

    struct SecretHome(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Drop for SecretHome {
        fn drop(&mut self) {
            std::env::remove_var("PHOENIX_STATE_DIR");
            std::env::remove_var(crate::secrets::KEY_VAR);
        }
    }

    fn secret_home(cfg_path: &std::path::Path) -> SecretHome {
        let g = crate::secrets::test_env_lock();
        let dir = cfg_path.parent().unwrap();
        std::env::set_var("PHOENIX_STATE_DIR", dir);
        std::env::set_var(crate::secrets::KEY_VAR, "test-passphrase");
        let _ = fs::remove_file(dir.join("secrets.enc"));
        SecretHome(g)
    }

    #[test]
    fn wizard_accepts_migration() {
        let (path, src) = mig_dir("migrate");
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"anthropic/claude-sonnet-5"}}}}"#,
        )
        .unwrap();
        let mut input = Cursor::new(b"\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-5");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Reborn"));
        assert!(text.contains("Default model [anthropic/claude-sonnet-5]"));
        assert!(text.contains("Fallback models"));
    }

    #[test]
    fn wizard_migration_carries_gateway_key() {
        let (path, src) = mig_dir("migrate-key");
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"openrouter/some-model"}}}}"#,
        )
        .unwrap();
        fs::write(
            src.parent().unwrap().join("gateway.systemd.env"),
            "OPENROUTER_API_KEY=carried-or-key\n",
        )
        .unwrap();
        let mut input = Cursor::new(b"\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("carried-or-key"),
            "a carried key must never be written to config.toml: {raw}"
        );
        let cfg = config::parse(&raw).unwrap();
        assert!(cfg.api_key.is_empty(), "api_key stays out of the config");
        assert_eq!(
            crate::secrets::Store::at(&crate::secrets::Store::default_path())
                .get("OPENROUTER_API_KEY"),
            Some("carried-or-key".to_string()),
            "the key belongs in the encrypted store"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("encrypted in the secret store"),
            "a key found in the old nest is carried and encrypted without asking: {text}"
        );
    }

    #[test]
    fn wizard_migration_without_key_holds_the_beacon() {
        let (path, src) = mig_dir("migrate-nokey");
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"customprov/some-model"}}}}"#,
        )
        .unwrap();
        let mut input = Cursor::new(b"\n\n\n\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, true, &mut input, &mut out).unwrap();
        assert!(ok);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("beacon on hold"));
    }

    #[test]
    fn wizard_yes_answers_never_become_models() {
        let (path, src) = mig_dir("migrate-yn");
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"anthropic/claude-sonnet-5"}}}}"#,
        )
        .unwrap();
        fs::write(
            src.parent().unwrap().join("secrets.json"),
            r#"{"authProfiles":{"main":{"anthropic:default":{"key":"key-anthro"}}}}"#,
        )
        .unwrap();
        let mut input = Cursor::new(b"Y\nY\nY\nY\nY\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let cfg = config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.model, "claude-sonnet-5");
        assert!(cfg.fallbacks.is_empty());
    }

    #[test]
    fn wizard_migration_carries_all_keys_and_picks_models() {
        let (path, src) = mig_dir("migrate-allkeys");
        fs::write(
            &src,
            r#"{"agents":{"defaults":{"model":{"primary":"anthropic/claude-sonnet-5"}}}}"#,
        )
        .unwrap();
        fs::write(
            src.parent().unwrap().join("secrets.json"),
            r#"{"authProfiles":{"main":{
                "anthropic:default":{"key":"key-anthro"},
                "openrouter:key0":{"key":"or-zero"},
                "openrouter:key1":{"key":"or-one"},
                "nvidia:key":{"key":"nv-key"}}}}"#,
        )
        .unwrap();
        let mut input = Cursor::new(b"\n\n\nopenrouter/meta/llama-3.3-70b\n".to_vec());
        let mut out = Vec::new();
        let _g = secret_home(&path);
        let ok = first_run(Some(&src), &path, false, &mut input, &mut out).unwrap();
        assert!(ok);
        let raw = fs::read_to_string(&path).unwrap();
        for secret in ["key-anthro", "or-zero", "or-one", "nv-key"] {
            assert!(
                !raw.contains(secret),
                "{secret} must never be written to config.toml: {raw}"
            );
        }
        let cfg = config::parse(&raw).unwrap();
        assert!(cfg.api_key.is_empty());
        assert!(cfg.provider_keys.is_empty());
        assert_eq!(
            cfg.fallbacks,
            vec!["openrouter/meta/llama-3.3-70b".to_string()]
        );
        let store = crate::secrets::Store::at(&crate::secrets::Store::default_path());
        assert_eq!(
            store.get("ANTHROPIC_API_KEY"),
            Some("key-anthro".to_string())
        );
        assert_eq!(store.get("OPENROUTER_API_KEY"), Some("or-zero".to_string()));
        assert_eq!(
            store.get("OPENROUTER_API_KEY_2"),
            Some("or-one".to_string())
        );
        assert_eq!(store.get("NVIDIA_API_KEY"), Some("nv-key".to_string()));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("anthropic (1), nvidia (1), openrouter (2)"));
        assert!(text.contains("encrypted in the secret store"));
    }
}
