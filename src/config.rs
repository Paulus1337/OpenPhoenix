use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const PRIVACY_MODES: [&str; 3] = ["ghost", "session", "recall"];
pub const LEAN_LEVELS: [&str; 3] = ["off", "lean", "grunt"];

pub const HEARTBEAT_PROMPT: &str = "Read HEARTBEAT.md in the workspace if it \
exists and follow it. If nothing needs attention, reply exactly HEARTBEAT_OK.";

pub const SAMPLE_CONFIG: &str = r#"# OpenPhoenix - https://github.com/Paulus1337/OpenPhoenix
# Secrets may live in env instead: PHOENIX_API_KEY, PHOENIX_TELEGRAM_TOKEN.
# No PHOENIX_API_KEY? The provider's standard env var is picked up too:
# ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENROUTER_API_KEY, NVIDIA_API_KEY,
# GEMINI_API_KEY / GOOGLE_API_KEY.

[provider]
kind = "openai"             # anthropic | openai | openrouter | ollama | nvidia | google | custom
model = "gpt-5.6-sol"       # aliases work too: opus, sonnet, gpt, gemini, …
# api_key = ""              # prefer env: PHOENIX_API_KEY or the vars above
# base_url = ""             # ollama / custom OpenAI-compatible endpoints
# fallbacks = []            # models tried in order after provider errors
# api_keys = []             # extra keys rotated on rate limits (first = api_key)

[agent]
privacy = "session"          # ghost = no history, no disk | session | recall
lean = "off"                 # off | lean | grunt (max token savings)
max_turns = 24
workspace = "~/phoenix-work"
# sessions = false           # serve: keep per-chat history on disk (never in ghost)
# stream = false             # chat: print tokens as they arrive
# compact_after = 0          # summarize the oldest half when history exceeds N messages

[security]
confirm_shell = true
# approvals = true           # queue serve-mode shell commands for /approve (off by default)
allow_outside_workspace = false
deny_commands = []           # extra regexes on top of built-ins

[telegram]
# token = ""                 # prefer env: PHOENIX_TELEGRAM_TOKEN
allowed_chat_ids = []        # empty = refuse everyone (fail closed)
# group_mention_only = true  # groups: only answer when the bot is @mentioned

# [http]
# enabled = false            # POST /run on 127.0.0.1, bearer token required
# port = 8787
# token = ""                 # prefer env: PHOENIX_HTTP_TOKEN
# web = false                # embedded chat UI on GET / (off by default)
# username = ""              # required for the web UI (fail closed)
# password = ""              # "sha256:<hex>" preferred over plaintext
# headers = "strong"         # security headers; "minimal" to reduce
# allow_crawlers = []        # robots.txt allowlist; empty = deny all + X-Robots-Tag

# [whatsapp]                 # WhatsApp Business Cloud API channel
# token = ""                 # prefer env: PHOENIX_WHATSAPP_TOKEN
# phone_id = ""              # business phone number id
# verify_token = ""          # webhook verify token you choose
# webhook_port = 8788        # 127.0.0.1 webhook listener; proxy it publicly
# allowed_numbers = []       # E.164 without plus, empty = refuse everyone

# [discord]                  # Discord bot over the raw gateway websocket
# token = ""                 # prefer env: PHOENIX_DISCORD_TOKEN
# allowed_channel_ids = []   # empty = refuse everyone (fail closed)

# [slack]                    # Slack bot over Socket Mode (no public webhook)
# app_token = ""             # xapp-, prefer env: PHOENIX_SLACK_APP_TOKEN
# bot_token = ""             # xoxb-, prefer env: PHOENIX_SLACK_BOT_TOKEN
# allowed_channel_ids = []   # empty = refuse everyone (fail closed)

# [signal]                   # Signal via a supervised signal-cli daemon
# account = ""               # your E.164, like "+4915551234567"
# allowed_numbers = []       # empty = refuse everyone (fail closed)
# cli_path = "signal-cli"    # binary on PATH or absolute path
# http_port = 8789           # localhost JSON-RPC/SSE port for the daemon

# [dreaming]                 # serve: think while idle, write to the journal
# minutes = 0                # dream after N idle minutes; 0 = disabled
# prompt = ""                # empty = built-in reflective prompt

# [clawhub]                  # skill registry for `phoenix skill` commands
# url = "https://clawhub.ai"

# [heartbeat]                # serve: run a check-in prompt on a fixed cadence
# minutes = 0                # 0 = disabled
# prompt = ""                # empty = built-in HEARTBEAT.md prompt
# chat_ids = []              # empty = all telegram allowed chats

# [memory]                   # vector search over recall-mode notes
# embeddings = false         # rank recall results by embedding similarity
# embed_model = "text-embedding-3-small"
# embed_base_url = ""        # empty = provider base_url or api.openai.com/v1

# [browser]                  # drive a system Chromium over local CDP
# enabled = false            # registers the browser_* tools (off by default)
# cdp_url = ""               # attach to a running browser, localhost only
# binary = "/usr/bin/chromium"
# headless = true

# [audio]                    # telegram voice notes -> text
# transcribe = false
# model = "whisper-1"
# base_url = ""              # empty = provider base_url or api.openai.com/v1

# [media]                    # image generation + speech tools
# images = false             # enable the image_generate tool
# tts = false                # enable the speak tool
# video = false              # enable the video_generate tool
# music = false              # enable the music_generate tool
# image_model = "gpt-image-1"
# tts_model = "tts-1"
# tts_voice = "alloy"
# video_model = "sora-2"
# music_model = "music-1"

# [canvas]                   # rendered visual surface at GET /canvas
# enabled = false            # needs [http] enabled plus web credentials

# [board]                    # durable task cards (task_add/list/update)
# enabled = false

# [imessage]                 # macOS only: drives the imsg CLI
# enabled = false
# cli_path = "imsg"          # imsg binary (needs Full Disk Access)
# db_path = ""               # optional Messages chat.db override
# allowed_senders = []       # fail closed: empty list refuses to start

# [[jobs]]
# name = "morning-brief"
# cron = "0 7 * * *"
# prompt = "Summarize the TODO file in my workspace."
# chat_ids = []              # optional: deliver only to these chats
# webhook = ""               # optional: POST result JSON here instead
"#;

pub fn home_dir() -> PathBuf {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn home() -> PathBuf {
    match env::var("PHOENIX_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_dir().join(".openphoenix"),
    }
}

pub fn config_path() -> PathBuf {
    home().join("config.toml")
}

pub fn expanduser(raw: &str) -> PathBuf {
    if raw == "~" {
        home_dir()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(raw)
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub webhook: String,
    pub name: String,
    pub cron: String,
    pub prompt: String,
    pub chat_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub privacy: String,
    pub lean: String,
    pub max_turns: u32,
    pub workspace: PathBuf,
    pub confirm_shell: bool,
    pub approvals: bool,
    pub allow_outside_workspace: bool,
    pub deny_commands: Vec<String>,
    pub telegram_token: String,
    pub telegram_allowed: Vec<String>,
    pub tg_group_mention_only: bool,
    pub jobs: Vec<Job>,
    pub fallbacks: Vec<String>,
    pub sessions: bool,
    pub stream: bool,
    pub http_enabled: bool,
    pub http_port: u16,
    pub http_token: String,
    pub http_web: bool,
    pub http_headers: String,
    pub http_user: String,
    pub http_pass: String,
    pub http_allow_crawlers: Vec<String>,
    pub wa_token: String,
    pub wa_phone_id: String,
    pub wa_verify_token: String,
    pub wa_webhook_port: u16,
    pub wa_allowed: Vec<String>,
    pub discord_token: String,
    pub discord_allowed: Vec<String>,
    pub slack_app_token: String,
    pub slack_bot_token: String,
    pub slack_allowed: Vec<String>,
    pub signal_account: String,
    pub signal_allowed: Vec<String>,
    pub signal_cli_path: String,
    pub signal_http_port: u16,
    pub dream_minutes: u32,
    pub dream_prompt: String,
    pub clawhub_url: String,
    pub heartbeat_minutes: u32,
    pub heartbeat_prompt: String,
    pub heartbeat_chat_ids: Vec<String>,
    pub compact_after: u32,
    pub mem_embeddings: bool,
    pub mem_embed_model: String,
    pub mem_embed_base_url: String,
    pub api_keys: Vec<String>,
    pub audio_transcribe: bool,
    pub audio_model: String,
    pub audio_base_url: String,
    pub media_images: bool,
    pub media_tts: bool,
    pub media_video: bool,
    pub media_music: bool,
    pub media_image_model: String,
    pub media_tts_model: String,
    pub media_tts_voice: String,
    pub media_video_model: String,
    pub media_music_model: String,
    pub media_base_url: String,
    pub canvas_enabled: bool,
    pub board_enabled: bool,
    pub imessage_enabled: bool,
    pub imessage_cli_path: String,
    pub imessage_db_path: String,
    pub imessage_allowed: Vec<String>,
    pub browser_enabled: bool,
    pub browser_cdp_url: String,
    pub browser_binary: String,
    pub browser_headless: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: "openai".into(),
            model: "gpt-5.6-sol".into(),
            api_key: String::new(),
            base_url: String::new(),
            privacy: "session".into(),
            lean: "off".into(),
            max_turns: 24,
            workspace: home_dir().join("phoenix-work"),
            confirm_shell: true,
            approvals: false,
            allow_outside_workspace: false,
            deny_commands: Vec::new(),
            telegram_token: String::new(),
            telegram_allowed: Vec::new(),
            tg_group_mention_only: true,
            jobs: Vec::new(),
            fallbacks: Vec::new(),
            sessions: false,
            stream: false,
            http_enabled: false,
            http_port: 8787,
            http_token: String::new(),
            http_web: false,
            http_headers: "strong".to_string(),
            http_user: String::new(),
            http_pass: String::new(),
            http_allow_crawlers: Vec::new(),
            wa_token: String::new(),
            wa_phone_id: String::new(),
            wa_verify_token: String::new(),
            wa_webhook_port: 8788,
            wa_allowed: Vec::new(),
            discord_token: String::new(),
            discord_allowed: Vec::new(),
            slack_app_token: String::new(),
            slack_bot_token: String::new(),
            slack_allowed: Vec::new(),
            signal_account: String::new(),
            signal_allowed: Vec::new(),
            signal_cli_path: String::new(),
            signal_http_port: 8789,
            dream_minutes: 0,
            dream_prompt: String::new(),
            clawhub_url: "https://clawhub.ai".to_string(),
            heartbeat_minutes: 0,
            heartbeat_prompt: HEARTBEAT_PROMPT.to_string(),
            heartbeat_chat_ids: Vec::new(),
            compact_after: 0,
            mem_embeddings: false,
            mem_embed_model: "text-embedding-3-small".to_string(),
            mem_embed_base_url: String::new(),
            api_keys: Vec::new(),
            audio_transcribe: false,
            audio_model: "whisper-1".to_string(),
            audio_base_url: String::new(),
            media_images: false,
            media_tts: false,
            media_video: false,
            media_music: false,
            media_image_model: "gpt-image-1".to_string(),
            media_tts_model: "tts-1".to_string(),
            media_tts_voice: "alloy".to_string(),
            media_video_model: "sora-2".to_string(),
            media_music_model: "music-1".to_string(),
            media_base_url: String::new(),
            canvas_enabled: false,
            board_enabled: false,
            imessage_enabled: false,
            imessage_cli_path: "imsg".to_string(),
            imessage_db_path: String::new(),
            imessage_allowed: Vec::new(),
            browser_enabled: false,
            browser_cdp_url: String::new(),
            browser_binary: String::new(),
            browser_headless: true,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !PRIVACY_MODES.contains(&self.privacy.as_str()) {
            return Err(format!("privacy must be one of {PRIVACY_MODES:?}"));
        }
        if !LEAN_LEVELS.contains(&self.lean.as_str()) {
            return Err(format!("lean must be one of {LEAN_LEVELS:?}"));
        }
        if self.max_turns < 1 {
            return Err("max_turns must be >= 1".into());
        }
        Ok(())
    }
}

fn tbl<'a>(root: &'a toml::Value, name: &str) -> Option<&'a toml::value::Table> {
    root.get(name).and_then(|v| v.as_table())
}

fn get_str(t: Option<&toml::value::Table>, key: &str, default: &str) -> String {
    t.and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn get_bool(t: Option<&toml::value::Table>, key: &str, default: bool) -> bool {
    t.and_then(|t| t.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

fn get_str_list(t: Option<&toml::value::Table>, key: &str) -> Vec<String> {
    t.and_then(|t| t.get(key))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|v| match v {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse(raw: &str) -> Result<Config, String> {
    let root: toml::Value = if raw.trim().is_empty() {
        toml::Value::Table(toml::value::Table::new())
    } else {
        toml::from_str(raw).map_err(|e| format!("bad config: {e}"))?
    };
    let prov = tbl(&root, "provider");
    let agent = tbl(&root, "agent");
    let sec = tbl(&root, "security");
    let tg = tbl(&root, "telegram");
    let http = tbl(&root, "http");
    let wa = tbl(&root, "whatsapp");
    let dc = tbl(&root, "discord");
    let sl = tbl(&root, "slack");
    let sg = tbl(&root, "signal");
    let dr = tbl(&root, "dreaming");
    let ch = tbl(&root, "clawhub");
    let hb = tbl(&root, "heartbeat");
    let mem = tbl(&root, "memory");
    let audio = tbl(&root, "audio");
    let media = tbl(&root, "media");
    let browser = tbl(&root, "browser");

    let max_turns = agent
        .and_then(|t| t.get("max_turns"))
        .and_then(|v| v.as_integer())
        .unwrap_or(24);
    if max_turns < 1 {
        return Err("max_turns must be >= 1".into());
    }

    let workspace_raw = get_str(agent, "workspace", "~/phoenix-work");

    let deny_commands = sec
        .and_then(|t| t.get("deny_commands"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let telegram_allowed = get_str_list(tg, "allowed_chat_ids");

    let http_port = http
        .and_then(|t| t.get("port"))
        .and_then(|v| v.as_integer())
        .unwrap_or(8787);
    if !(1..=65535).contains(&http_port) {
        return Err("http.port must be between 1 and 65535".into());
    }

    let wa_webhook_port = wa
        .and_then(|t| t.get("webhook_port"))
        .and_then(|v| v.as_integer())
        .unwrap_or(8788);
    if !(1..=65535).contains(&wa_webhook_port) {
        return Err("whatsapp.webhook_port must be between 1 and 65535".into());
    }

    let signal_http_port = sg
        .and_then(|t| t.get("http_port"))
        .and_then(toml::Value::as_integer)
        .unwrap_or(8789);
    if !(1..=65535).contains(&signal_http_port) {
        return Err("signal.http_port must be between 1 and 65535".into());
    }
    let signal_http_port = signal_http_port as u16;
    let heartbeat_minutes = hb
        .and_then(|t| t.get("minutes"))
        .and_then(|v| v.as_integer())
        .unwrap_or(0);
    if heartbeat_minutes < 0 {
        return Err("heartbeat.minutes must be >= 0".into());
    }
    let heartbeat_prompt = get_str(hb, "prompt", "");
    let heartbeat_prompt = if heartbeat_prompt.is_empty() {
        HEARTBEAT_PROMPT.to_string()
    } else {
        heartbeat_prompt
    };

    let compact_after = agent
        .and_then(|t| t.get("compact_after"))
        .and_then(|v| v.as_integer())
        .unwrap_or(0);
    if compact_after < 0 {
        return Err("agent.compact_after must be >= 0".into());
    }

    let jobs = root
        .get("jobs")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_table())
                .map(|t| Job {
                    name: t
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("job")
                        .to_string(),
                    cron: t
                        .get("cron")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    prompt: t
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    chat_ids: get_str_list(Some(t), "chat_ids"),
                    webhook: t
                        .get("webhook")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let cfg = Config {
        provider: get_str(prov, "kind", "openai"),
        model: get_str(prov, "model", "gpt-5.6-sol"),
        api_key: get_str(prov, "api_key", ""),
        base_url: get_str(prov, "base_url", ""),
        privacy: get_str(agent, "privacy", "session"),
        lean: get_str(agent, "lean", "off"),
        max_turns: max_turns as u32,
        workspace: expanduser(&workspace_raw),
        confirm_shell: get_bool(sec, "confirm_shell", true),
        approvals: get_bool(sec, "approvals", false),
        allow_outside_workspace: get_bool(sec, "allow_outside_workspace", false),
        deny_commands,
        telegram_token: get_str(tg, "token", ""),
        telegram_allowed,
        tg_group_mention_only: get_bool(tg, "group_mention_only", true),
        jobs,
        fallbacks: get_str_list(prov, "fallbacks"),
        sessions: get_bool(agent, "sessions", false),
        stream: get_bool(agent, "stream", false),
        http_enabled: get_bool(http, "enabled", false),
        http_port: http_port as u16,
        http_web: get_bool(http, "web", false),
        http_headers: get_str(http, "headers", "strong"),
        http_user: get_str(http, "username", ""),
        http_pass: get_str(http, "password", ""),
        http_allow_crawlers: get_str_list(http, "allow_crawlers"),
        http_token: get_str(http, "token", ""),
        wa_token: get_str(wa, "token", ""),
        wa_phone_id: get_str(wa, "phone_id", ""),
        wa_verify_token: get_str(wa, "verify_token", ""),
        wa_webhook_port: wa_webhook_port as u16,
        wa_allowed: get_str_list(wa, "allowed_numbers"),
        discord_token: get_str(dc, "token", ""),
        discord_allowed: get_str_list(dc, "allowed_channel_ids"),
        slack_app_token: get_str(sl, "app_token", ""),
        slack_bot_token: get_str(sl, "bot_token", ""),
        slack_allowed: get_str_list(sl, "allowed_channel_ids"),
        signal_account: get_str(sg, "account", ""),
        signal_allowed: get_str_list(sg, "allowed_numbers"),
        signal_cli_path: get_str(sg, "cli_path", ""),
        signal_http_port,
        dream_minutes: dr
            .and_then(|t| t.get("minutes"))
            .and_then(toml::Value::as_integer)
            .unwrap_or(0)
            .max(0) as u32,
        dream_prompt: get_str(dr, "prompt", ""),
        clawhub_url: {
            let u = get_str(ch, "url", "https://clawhub.ai");
            u.trim_end_matches('/').to_string()
        },
        heartbeat_minutes: heartbeat_minutes as u32,
        heartbeat_prompt,
        heartbeat_chat_ids: get_str_list(hb, "chat_ids"),
        compact_after: compact_after as u32,
        api_keys: get_str_list(prov, "api_keys"),
        audio_transcribe: get_bool(audio, "transcribe", false),
        audio_model: get_str(audio, "model", "whisper-1"),
        audio_base_url: get_str(audio, "base_url", ""),
        media_images: get_bool(media, "images", false),
        media_tts: get_bool(media, "tts", false),
        media_video: get_bool(media, "video", false),
        media_music: get_bool(media, "music", false),
        media_image_model: get_str(media, "image_model", "gpt-image-1"),
        media_tts_model: get_str(media, "tts_model", "tts-1"),
        media_tts_voice: get_str(media, "tts_voice", "alloy"),
        media_video_model: get_str(media, "video_model", "sora-2"),
        media_music_model: get_str(media, "music_model", "music-1"),
        media_base_url: get_str(media, "base_url", ""),
        canvas_enabled: get_bool(tbl(&root, "canvas"), "enabled", false),
        board_enabled: get_bool(tbl(&root, "board"), "enabled", false),
        imessage_enabled: get_bool(tbl(&root, "imessage"), "enabled", false),
        imessage_cli_path: get_str(tbl(&root, "imessage"), "cli_path", "imsg"),
        imessage_db_path: get_str(tbl(&root, "imessage"), "db_path", ""),
        imessage_allowed: get_str_list(tbl(&root, "imessage"), "allowed_senders"),
        mem_embeddings: get_bool(mem, "embeddings", false),
        mem_embed_model: get_str(mem, "embed_model", "text-embedding-3-small"),
        mem_embed_base_url: get_str(mem, "embed_base_url", ""),
        browser_enabled: get_bool(browser, "enabled", false),
        browser_cdp_url: get_str(browser, "cdp_url", ""),
        browser_binary: get_str(browser, "binary", ""),
        browser_headless: get_bool(browser, "headless", true),
    };
    cfg.validate()?;
    Ok(cfg)
}

const SCHEMA: &[(&str, &[&str])] = &[
    (
        "provider",
        &[
            "kind",
            "model",
            "api_key",
            "base_url",
            "fallbacks",
            "api_keys",
        ],
    ),
    (
        "agent",
        &[
            "privacy",
            "lean",
            "max_turns",
            "workspace",
            "sessions",
            "stream",
            "compact_after",
        ],
    ),
    (
        "security",
        &[
            "confirm_shell",
            "approvals",
            "allow_outside_workspace",
            "deny_commands",
        ],
    ),
    (
        "telegram",
        &["token", "allowed_chat_ids", "group_mention_only"],
    ),
    (
        "http",
        &[
            "enabled",
            "port",
            "token",
            "web",
            "username",
            "password",
            "headers",
            "allow_crawlers",
        ],
    ),
    (
        "whatsapp",
        &[
            "token",
            "phone_id",
            "verify_token",
            "webhook_port",
            "allowed_numbers",
        ],
    ),
    ("discord", &["token", "allowed_channel_ids"]),
    ("slack", &["app_token", "bot_token", "allowed_channel_ids"]),
    (
        "signal",
        &["account", "allowed_numbers", "cli_path", "http_port"],
    ),
    ("dreaming", &["minutes", "prompt"]),
    ("clawhub", &["url"]),
    ("heartbeat", &["minutes", "prompt", "chat_ids"]),
    ("memory", &["embeddings", "embed_model", "embed_base_url"]),
    ("audio", &["transcribe", "model", "base_url"]),
    (
        "media",
        &[
            "images",
            "tts",
            "video",
            "music",
            "image_model",
            "tts_model",
            "tts_voice",
            "video_model",
            "music_model",
            "base_url",
        ],
    ),
    ("browser", &["enabled", "cdp_url", "binary", "headless"]),
    ("canvas", &["enabled"]),
    ("board", &["enabled"]),
    (
        "imessage",
        &["enabled", "cli_path", "db_path", "allowed_senders"],
    ),
];

const JOB_KEYS: &[&str] = &["name", "cron", "prompt", "chat_ids", "webhook"];

pub fn unknown_keys(raw: &str) -> Vec<String> {
    let Ok(root) = toml::from_str::<toml::Value>(raw) else {
        return Vec::new();
    };
    let Some(table) = root.as_table() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (tname, val) in table {
        if tname == "jobs" {
            if let Some(arr) = val.as_array() {
                for (i, job) in arr.iter().enumerate() {
                    if let Some(jt) = job.as_table() {
                        for key in jt.keys() {
                            if !JOB_KEYS.contains(&key.as_str()) {
                                out.push(format!("jobs[{i}].{key}"));
                            }
                        }
                    }
                }
            }
            continue;
        }
        match SCHEMA.iter().find(|(name, _)| *name == tname) {
            None => out.push(tname.clone()),
            Some((_, keys)) => {
                if let Some(t) = val.as_table() {
                    for key in t.keys() {
                        if !keys.contains(&key.as_str()) {
                            out.push(format!("{tname}.{key}"));
                        }
                    }
                }
            }
        }
    }
    out
}

pub fn load(path: Option<&Path>) -> Result<Config, String> {
    let p = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    let raw = if p.exists() {
        fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?
    } else {
        String::new()
    };
    let mut cfg = parse(&raw)?;
    if let Ok(v) = env::var("PHOENIX_API_KEY") {
        if !v.is_empty() {
            cfg.api_key = v;
        }
    }

    if cfg.api_key.is_empty() {
        for var in provider_key_vars(&cfg.provider) {
            if let Ok(v) = env::var(var) {
                if !v.is_empty() {
                    cfg.api_key = v;
                    break;
                }
            }
        }
    }
    if let Ok(v) = env::var("PHOENIX_TELEGRAM_TOKEN") {
        if !v.is_empty() {
            cfg.telegram_token = v;
        }
    }
    if let Ok(v) = env::var("PHOENIX_HTTP_TOKEN") {
        if !v.is_empty() {
            cfg.http_token = v;
        }
    }
    if let Ok(v) = env::var("PHOENIX_WHATSAPP_TOKEN") {
        if !v.is_empty() {
            cfg.wa_token = v;
        }
    }
    if let Ok(v) = env::var("PHOENIX_DISCORD_TOKEN") {
        if !v.is_empty() {
            cfg.discord_token = v;
        }
    }
    if let Ok(v) = env::var("PHOENIX_SLACK_APP_TOKEN") {
        if !v.is_empty() {
            cfg.slack_app_token = v;
        }
    }
    if let Ok(v) = env::var("PHOENIX_SLACK_BOT_TOKEN") {
        if !v.is_empty() {
            cfg.slack_bot_token = v;
        }
    }
    Ok(cfg)
}

pub fn provider_key_vars(kind: &str) -> &'static [&'static str] {
    match kind {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" | "custom" => &["OPENAI_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        _ => &[],
    }
}

pub fn init_config() -> Result<PathBuf, String> {
    let dir = home();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("config.toml");
    if !path.exists() {
        fs::write(&path, SAMPLE_CONFIG).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn provider_key_vars_cover_known_providers() {
        assert_eq!(provider_key_vars("anthropic"), ["ANTHROPIC_API_KEY"]);
        assert_eq!(provider_key_vars("openai"), ["OPENAI_API_KEY"]);
        assert_eq!(provider_key_vars("custom"), ["OPENAI_API_KEY"]);
        assert_eq!(provider_key_vars("openrouter"), ["OPENROUTER_API_KEY"]);
        assert_eq!(provider_key_vars("nvidia"), ["NVIDIA_API_KEY"]);
        assert_eq!(
            provider_key_vars("google"),
            ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
        );

        assert!(provider_key_vars("ollama").is_empty());
        assert!(provider_key_vars("weird").is_empty());
    }

    #[test]
    fn parses_v060_fields() {
        let cfg = parse(
            r#"
[provider]
kind = "openai"
api_keys = ["k2", "k3"]

[telegram]
allowed_chat_ids = ["1"]
group_mention_only = false

[audio]
transcribe = true
model = "whisper-large"
base_url = "http://local/v1"

[[jobs]]
name = "brief"
cron = "0 7 * * *"
prompt = "p"
webhook = "http://127.0.0.1:9/hook"
"#,
        )
        .unwrap();
        assert_eq!(cfg.api_keys, vec!["k2", "k3"]);
        assert!(!cfg.tg_group_mention_only);
        assert!(cfg.audio_transcribe);
        assert_eq!(cfg.audio_model, "whisper-large");
        assert_eq!(cfg.audio_base_url, "http://local/v1");
        assert_eq!(cfg.jobs[0].webhook, "http://127.0.0.1:9/hook");
    }

    #[test]
    fn v060_defaults() {
        let cfg = parse("").unwrap();
        assert!(cfg.api_keys.is_empty());
        assert!(cfg.tg_group_mention_only);
        assert!(!cfg.audio_transcribe);
        assert_eq!(cfg.audio_model, "whisper-1");
        let cfg = parse("[[jobs]]\nname = \"j\"\ncron = \"* * * * *\"\nprompt = \"p\"\n").unwrap();
        assert!(cfg.jobs[0].webhook.is_empty());
    }

    #[test]
    fn bad_privacy_rejected() {
        let cfg = Config {
            privacy: "loud".into(),
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn bad_lean_rejected() {
        let cfg = Config {
            lean: "max".into(),
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn zero_max_turns_rejected() {
        let cfg = Config {
            max_turns: 0,
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn parse_full_file() {
        let cfg = parse(
            r#"
[provider]
kind = "openrouter"
model = "gpt-x"
api_key = "file-key"
[agent]
privacy = "recall"
lean = "grunt"
max_turns = 5
workspace = "/tmp/px-ws"
[security]
confirm_shell = false
allow_outside_workspace = true
deny_commands = ["foo"]
[telegram]
token = "tok"
allowed_chat_ids = [123, "456"]
[http]
enabled = true
port = 9911
token = "htok"
[[jobs]]
name = "j1"
cron = "0 7 * * *"
prompt = "hello"
chat_ids = [777]
"#,
        )
        .unwrap();
        assert_eq!(cfg.provider, "openrouter");
        assert_eq!(cfg.model, "gpt-x");
        assert_eq!(cfg.api_key, "file-key");
        assert_eq!(cfg.privacy, "recall");
        assert_eq!(cfg.lean, "grunt");
        assert_eq!(cfg.max_turns, 5);
        assert_eq!(cfg.workspace, PathBuf::from("/tmp/px-ws"));
        assert!(!cfg.confirm_shell);
        assert!(cfg.allow_outside_workspace);
        assert_eq!(cfg.deny_commands, vec!["foo".to_string()]);
        assert_eq!(cfg.telegram_token, "tok");
        assert_eq!(
            cfg.telegram_allowed,
            vec!["123".to_string(), "456".to_string()]
        );
        assert_eq!(cfg.jobs.len(), 1);
        assert_eq!(cfg.jobs[0].cron, "0 7 * * *");
        assert_eq!(cfg.jobs[0].chat_ids, vec!["777".to_string()]);
        assert!(cfg.http_enabled);
        assert_eq!(cfg.http_port, 9911);
        assert_eq!(cfg.http_token, "htok");
    }

    #[test]
    fn parse_new_keys() {
        let cfg = parse(
            "[provider]\nfallbacks = [\"m2\", \"m3\"]\n[agent]\nsessions = true\nstream = true\n",
        )
        .unwrap();
        assert_eq!(cfg.fallbacks, vec!["m2".to_string(), "m3".to_string()]);
        assert!(cfg.sessions);
        assert!(cfg.stream);
        assert!(!cfg.http_enabled);
        assert_eq!(cfg.http_port, 8787);
    }

    #[test]
    fn bad_http_port_rejected() {
        assert!(parse("[http]\nport = 0\n").is_err());
        assert!(parse("[http]\nport = 70000\n").is_err());
        assert!(parse("[whatsapp]\nwebhook_port = 0\n").is_err());
    }

    #[test]
    fn parse_whatsapp_table() {
        let cfg = parse(
            "[whatsapp]\ntoken = \"wt\"\nphone_id = \"55\"\nverify_token = \"vv\"\nallowed_numbers = [4915551234567]\n",
        )
        .unwrap();
        assert_eq!(cfg.wa_token, "wt");
        assert_eq!(cfg.wa_phone_id, "55");
        assert_eq!(cfg.wa_verify_token, "vv");
        assert_eq!(cfg.wa_allowed, vec!["4915551234567".to_string()]);
        assert_eq!(cfg.wa_webhook_port, 8788);
    }

    #[test]
    fn parse_empty_gives_defaults() {
        let cfg = parse("").unwrap();

        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-5.6-sol");
        assert_eq!(cfg.privacy, "session");
        assert_eq!(cfg.max_turns, 24);
        assert!(cfg.telegram_allowed.is_empty());
    }

    #[test]
    fn approvals_default_off_opt_in() {
        assert!(!Config::default().approvals);
        assert!(!parse("").unwrap().approvals);
        assert!(parse("[security]\napprovals = true\n").unwrap().approvals);
    }

    #[test]
    fn parse_heartbeat_table() {
        let cfg = parse("[heartbeat]\nminutes = 30\nchat_ids = [42]\n").unwrap();
        assert_eq!(cfg.heartbeat_minutes, 30);
        assert_eq!(cfg.heartbeat_prompt, HEARTBEAT_PROMPT);
        assert_eq!(cfg.heartbeat_chat_ids, vec!["42".to_string()]);
        let custom = parse("[heartbeat]\nminutes = 5\nprompt = \"check disk\"\n").unwrap();
        assert_eq!(custom.heartbeat_prompt, "check disk");
        assert!(parse("[heartbeat]\nminutes = -1\n").is_err());
    }

    #[test]
    fn heartbeat_defaults_disabled() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.heartbeat_minutes, 0);
        assert_eq!(cfg.heartbeat_prompt, HEARTBEAT_PROMPT);
        assert!(cfg.heartbeat_chat_ids.is_empty());
    }

    #[test]
    fn parse_compact_after() {
        assert_eq!(parse("").unwrap().compact_after, 0);
        let cfg = parse("[agent]\ncompact_after = 40\n").unwrap();
        assert_eq!(cfg.compact_after, 40);
        assert!(parse("[agent]\ncompact_after = -2\n").is_err());
    }

    #[test]
    fn parse_memory_table() {
        let cfg = parse("").unwrap();
        assert!(!cfg.mem_embeddings);
        assert_eq!(cfg.mem_embed_model, "text-embedding-3-small");
        assert_eq!(cfg.mem_embed_base_url, "");
        let cfg = parse(
            "[memory]\nembeddings = true\nembed_model = \"m-embed\"\nembed_base_url = \"http://x.local/v1\"\n",
        )
        .unwrap();
        assert!(cfg.mem_embeddings);
        assert_eq!(cfg.mem_embed_model, "m-embed");
        assert_eq!(cfg.mem_embed_base_url, "http://x.local/v1");
    }

    #[test]
    fn parse_rejects_bad_values() {
        assert!(parse("[agent]\nprivacy = \"loud\"\n").is_err());
        assert!(parse("[agent]\nlean = \"max\"\n").is_err());
        assert!(parse("[agent]\nmax_turns = 0\n").is_err());
        assert!(parse("not valid toml [").is_err());
    }

    #[test]
    fn sample_config_parses() {
        assert!(parse(SAMPLE_CONFIG).is_ok());
    }

    #[test]
    fn browser_defaults_off() {
        let cfg = parse("").unwrap();
        assert!(!cfg.browser_enabled);
        assert!(cfg.browser_cdp_url.is_empty());
        assert!(cfg.browser_binary.is_empty());
        assert!(cfg.browser_headless);
    }

    #[test]
    fn parse_browser_table() {
        let cfg = parse(
            "[browser]\nenabled = true\ncdp_url = \"http://127.0.0.1:9222\"\nbinary = \"/opt/chrome\"\nheadless = false\n",
        )
        .unwrap();
        assert!(cfg.browser_enabled);
        assert_eq!(cfg.browser_cdp_url, "http://127.0.0.1:9222");
        assert_eq!(cfg.browser_binary, "/opt/chrome");
        assert!(!cfg.browser_headless);
    }

    #[test]
    fn expanduser_tilde() {
        assert_eq!(expanduser("~/x"), home_dir().join("x"));
        assert_eq!(expanduser("/abs"), PathBuf::from("/abs"));
    }

    #[test]
    fn sample_config_has_no_unknown_keys() {
        assert!(unknown_keys(SAMPLE_CONFIG).is_empty());
    }

    #[test]
    fn unknown_keys_catch_typos() {
        let raw = "[provider]\nmodle = \"oops\"\n\n[telegramm]\ntoken = \"x\"\n";
        let bad = unknown_keys(raw);
        assert!(bad.contains(&"provider.modle".to_string()), "{bad:?}");
        assert!(bad.contains(&"telegramm".to_string()), "{bad:?}");
    }

    #[test]
    fn unknown_keys_check_jobs_entries() {
        let raw =
            "[[jobs]]\nname = \"a\"\ncron = \"* * * * *\"\nprompt = \"p\"\nwebook = \"typo\"\n";
        let bad = unknown_keys(raw);
        assert_eq!(bad, vec!["jobs[0].webook".to_string()]);
    }

    #[test]
    fn unknown_keys_clean_on_full_valid_config() {
        let raw =
            "[provider]\nkind = \"openai\"\n\n[agent]\nmax_turns = 5\n\n[canvas]\nenabled = true\n";
        assert!(unknown_keys(raw).is_empty());
    }
}
