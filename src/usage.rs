use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;

const MAX_USAGE_BYTES: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_SECS: u64 = 60;
pub const COLAB_DELEGATION_PERCENT: f64 = 98.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaState {
    Ready,
    Low,
    Exhausted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    pub reset_at_ms: Option<u64>,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Confirmed,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialPool {
    pub total: usize,
    pub ready: usize,
    pub low: usize,
    pub exhausted: usize,
    pub unknown: usize,
    pub delegated: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaCapability {
    Subscription,
    Organization,
    Credential,
    Balance,
    Reactive,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    pub main_percent: u8,
    pub partner_percent: u8,
    pub guidance: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Billing {
    pub label: String,
    pub amount: f64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub provider: String,
    pub source: String,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub billing: Vec<Billing>,
    pub available: Option<bool>,
    pub error: Option<String>,
    pub observed_at_ms: u64,
    pub max_age_ms: u64,
    pub confidence: Confidence,
    pub pool: Option<CredentialPool>,
}

impl Snapshot {
    fn empty(provider: &str, source: &str) -> Snapshot {
        Snapshot {
            provider: provider.to_string(),
            source: source.to_string(),
            plan: None,
            windows: Vec::new(),
            billing: Vec::new(),
            available: None,
            error: None,
            observed_at_ms: now_ms(),
            max_age_ms: CACHE_SECS.saturating_mul(1000),
            confidence: Confidence::Confirmed,
            pool: None,
        }
    }

    fn unknown(provider: &str, source: &str, error: impl Into<String>) -> Snapshot {
        let mut snapshot = Snapshot::empty(provider, source);
        snapshot.error = Some(error.into());
        snapshot.confidence = Confidence::Unknown;
        snapshot
    }

    pub fn stale(&self) -> bool {
        now_ms().saturating_sub(self.observed_at_ms) > self.max_age_ms
    }

    pub fn state(&self) -> QuotaState {
        self.state_for_model("")
    }

    pub fn should_delegate_for_model(&self, model: &str) -> bool {
        if self.stale() || self.confidence == Confidence::Unknown {
            return false;
        }
        if let Some(pool) = self.pool {
            return pool.total > 0 && pool.delegated == pool.total;
        }
        self.available == Some(false)
            || self.windows.iter().any(|window| {
                window_applies(window, model) && window.used_percent > COLAB_DELEGATION_PERCENT
            })
    }

    pub fn state_for_model(&self, model: &str) -> QuotaState {
        if self.stale() || self.confidence == Confidence::Unknown {
            return QuotaState::Unknown;
        }
        if let Some(pool) = self.pool {
            if pool.ready > 0 {
                return QuotaState::Ready;
            }
            if pool.unknown > 0 {
                return QuotaState::Unknown;
            }
            if pool.low > 0 {
                return QuotaState::Low;
            }
            if pool.total > 0 && pool.exhausted == pool.total {
                return QuotaState::Exhausted;
            }
        }
        let relevant: Vec<&Window> = self
            .windows
            .iter()
            .filter(|window| window_applies(window, model))
            .collect();
        if self.available == Some(false)
            || relevant.iter().any(|window| window.used_percent >= 100.0)
        {
            return QuotaState::Exhausted;
        }
        if relevant.iter().any(|window| window.used_percent >= 90.0) {
            return QuotaState::Low;
        }
        if self.available == Some(true) || !relevant.is_empty() {
            return QuotaState::Ready;
        }
        if !self.billing.is_empty() {
            return if self.billing.iter().any(|balance| balance.amount > 0.0) {
                QuotaState::Ready
            } else {
                QuotaState::Exhausted
            };
        }
        QuotaState::Unknown
    }

    pub fn short(&self) -> String {
        let mut parts = Vec::new();
        if let Some(plan) = &self.plan {
            parts.push(plan.clone());
        }
        for window in &self.windows {
            let reset = window
                .reset_at_ms
                .and_then(|at| at.checked_sub(now_ms()))
                .map(|left| format!(", resets in {}", crate::scheduler::time_ago(left / 1000)))
                .unwrap_or_default();
            parts.push(format!(
                "{} {:.0}% used{reset}",
                window.label, window.used_percent
            ));
        }
        for balance in &self.billing {
            parts.push(format!(
                "{} {:.2} {}",
                balance.label, balance.amount, balance.unit
            ));
        }
        if let Some(pool) = self.pool {
            parts.push(format!(
                "credential pool {} ready, {} low, {} unknown, {} exhausted",
                pool.ready, pool.low, pool.unknown, pool.exhausted
            ));
        }
        if self.available == Some(false) {
            parts.push("unavailable".to_string());
        }
        if self.stale() {
            parts.push("stale".to_string());
        }
        if parts.is_empty() {
            match &self.error {
                Some(error) => format!("{} limits unknown ({error})", self.provider),
                None => format!("{} limits unknown", self.provider),
            }
        } else {
            format!("{}: {}", self.provider, parts.join(" | "))
        }
    }
}

pub fn allocation(
    main: &Snapshot,
    main_model: &str,
    partner: &Snapshot,
    partner_model: &str,
) -> Allocation {
    let main_state = main.state_for_model(main_model);
    let partner_state = partner.state_for_model(partner_model);
    let (main_percent, partner_percent, guidance) = match (main_state, partner_state) {
        (QuotaState::Unknown, _) | (_, QuotaState::Unknown) => (
            50,
            50,
            "At least one usage signal is unknown. Keep the split balanced and do not treat unknown usage as zero.",
        ),
        (QuotaState::Low, QuotaState::Ready) => (
            35,
            65,
            "The primary quota is low. Give it the smaller review-focused share and give the partner the larger implementation share.",
        ),
        (QuotaState::Ready, QuotaState::Low) => (
            65,
            35,
            "The partner quota is low. Give it the smaller review-focused share and give the primary the larger implementation share.",
        ),
        (QuotaState::Low, QuotaState::Low) => (
            50,
            50,
            "Both quotas are low. Keep the split balanced, narrow, and free of duplicated work.",
        ),
        _ => (
            50,
            50,
            "Both seats have room. Keep the split balanced and capability-aware.",
        ),
    };
    Allocation {
        main_percent,
        partner_percent,
        guidance: guidance.to_string(),
    }
}

fn normalized_words(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| {
            !word.is_empty()
                && !word.chars().all(|character| character.is_ascii_digit())
                && !matches!(*word, "latest" | "model" | "limit")
        })
        .map(str::to_string)
        .collect()
}

fn window_applies(window: &Window, model: &str) -> bool {
    if window.models.is_empty() || model.is_empty() {
        return true;
    }
    let model_words = normalized_words(model);
    window.models.iter().any(|scope| {
        let scope_words = normalized_words(scope);
        !scope_words.is_empty()
            && scope_words
                .iter()
                .all(|word| model_words.iter().any(|candidate| candidate == word))
    })
}

static CACHE: OnceLock<Mutex<HashMap<String, (u64, Snapshot)>>> = OnceLock::new();

pub fn fetch(cfg: &Config) -> Snapshot {
    #[cfg(test)]
    if std::thread::current()
        .name()
        .is_some_and(|name| name != "main")
    {
        let mut snapshot = Snapshot::empty(&cfg.provider, "test");
        snapshot.available = Some(true);
        return snapshot;
    }
    let key = cache_key(cfg);
    let now = crate::scheduler::now_epoch();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .filter(|(at, _)| now.saturating_sub(*at) < CACHE_SECS)
        .map(|(_, snapshot)| snapshot.clone())
    {
        return hit;
    }
    let snapshot = fetch_uncached(cfg);
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key, (now, snapshot.clone()));
    snapshot
}

pub fn text(cfg: &Config) -> String {
    fetch(cfg).short()
}

fn cache_key(cfg: &Config) -> String {
    let identity = if cfg.provider == "anthropic" {
        crate::oauth::load().map(|tokens| tokens.access)
    } else if cfg.provider == "openai" {
        crate::codex::load().map(|tokens| tokens.access)
    } else if cfg.provider == "google" {
        std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN").ok()
    } else {
        Some(cfg.api_key.clone())
    }
    .unwrap_or_default();
    let mut identities = vec![identity];
    identities.extend(cfg.api_keys.iter().cloned());
    let joined = identities.join("\0");
    let digest = crate::security::sha256_hex(joined.as_bytes());
    format!("{}:{}", cfg.provider, &digest[..16])
}

pub fn quota_capability(cfg: &Config) -> QuotaCapability {
    let has_api_key = !cfg.api_key.is_empty() || cfg.api_keys.iter().any(|key| !key.is_empty());
    match cfg.provider.as_str() {
        "anthropic" if !has_api_key => QuotaCapability::Subscription,
        "openai" if !has_api_key && cfg.base_url.is_empty() => QuotaCapability::Subscription,
        "google" if !has_api_key => QuotaCapability::Subscription,
        "openrouter" => QuotaCapability::Credential,
        "deepseek" => QuotaCapability::Balance,
        "openai" => QuotaCapability::Organization,
        "ollama" => QuotaCapability::Local,
        _ => QuotaCapability::Reactive,
    }
}

fn fetch_uncached(cfg: &Config) -> Snapshot {
    match quota_capability(cfg) {
        QuotaCapability::Subscription => match cfg.provider.as_str() {
            "anthropic" => fetch_anthropic(),
            "openai" => fetch_codex(),
            "google" => fetch_google(),
            provider => Snapshot::unknown(
                provider,
                "subscription",
                "no subscription adapter is registered",
            ),
        },
        QuotaCapability::Organization => Snapshot::unknown(
            &cfg.provider,
            "admin-api",
            "organization usage needs a separately scoped admin key",
        ),
        QuotaCapability::Credential if cfg.provider == "openrouter" => {
            best_key_snapshot(cfg, fetch_openrouter)
        }
        QuotaCapability::Balance if cfg.provider == "deepseek" => {
            best_key_snapshot(cfg, fetch_deepseek)
        }
        QuotaCapability::Credential | QuotaCapability::Balance => Snapshot::unknown(
            &cfg.provider,
            "api",
            "no proactive credential adapter is registered",
        ),
        QuotaCapability::Local => {
            let mut snapshot = Snapshot::empty(&cfg.provider, "local");
            snapshot.available = Some(true);
            snapshot
        }
        QuotaCapability::Reactive => Snapshot::unknown(
            &cfg.provider,
            "live-response",
            "limits are learned from live responses and credential cooldowns",
        ),
    }
}

fn fetch_anthropic() -> Snapshot {
    let Some(token) = crate::oauth::fresh_access() else {
        return fetch_claude_web().unwrap_or_else(|| {
            Snapshot::unknown(
                "anthropic",
                "oauth",
                "sign in with phoenix anthropic login to read subscription limits",
            )
        });
    };
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Accept".to_string(), "application/json".to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
        (
            "User-Agent".to_string(),
            format!("phoenix/{}", crate::VERSION),
        ),
    ];
    match request_json(
        "GET",
        "https://api.anthropic.com/api/oauth/usage",
        &headers,
        None,
    ) {
        Ok(value) => parse_anthropic(&value, "oauth"),
        Err((Some(403), error)) if error.contains("scope requirement user:profile") => {
            fetch_claude_web().unwrap_or_else(|| Snapshot::unknown("anthropic", "oauth", error))
        }
        Err((_, error)) => Snapshot::unknown("anthropic", "oauth", error),
    }
}

fn web_session_key() -> Option<String> {
    for name in ["CLAUDE_AI_SESSION_KEY", "CLAUDE_WEB_SESSION_KEY"] {
        if let Ok(value) = std::env::var(name) {
            if value.starts_with("sk-ant-") {
                return Some(value);
            }
        }
    }
    let cookie = std::env::var("CLAUDE_WEB_COOKIE").ok()?;
    cookie
        .trim()
        .trim_start_matches("Cookie:")
        .split(';')
        .find_map(|part| part.trim().strip_prefix("sessionKey="))
        .filter(|value| value.starts_with("sk-ant-"))
        .map(str::to_string)
}

fn fetch_claude_web() -> Option<Snapshot> {
    let session = web_session_key()?;
    let headers = vec![
        ("Cookie".to_string(), format!("sessionKey={session}")),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let organizations =
        request_json("GET", "https://claude.ai/api/organizations", &headers, None).ok()?;
    let org = organizations
        .as_array()?
        .iter()
        .find_map(|entry| entry["uuid"].as_str())?;
    let usage = request_json(
        "GET",
        &format!(
            "https://claude.ai/api/organizations/{}/usage",
            percent_encode(org)
        ),
        &headers,
        None,
    )
    .ok()?;
    Some(parse_anthropic(&usage, "claude-web"))
}

fn parse_anthropic(value: &Value, source: &str) -> Snapshot {
    let mut snapshot = Snapshot::empty("anthropic", source);
    for (key, label) in [
        ("five_hour", "5h"),
        ("seven_day", "Week"),
        ("seven_day_sonnet", "Sonnet"),
        ("seven_day_opus", "Opus"),
    ] {
        if let Some(window) = value.get(key).and_then(Value::as_object) {
            if let Some(percent) = finite(window.get("utilization")) {
                snapshot.windows.push(Window {
                    label: label.to_string(),
                    used_percent: clamp_percent(percent),
                    reset_at_ms: window.get("resets_at").and_then(parse_reset_ms),
                    models: match label {
                        "Sonnet" | "Opus" => vec![label.to_string()],
                        _ => Vec::new(),
                    },
                });
            }
        }
    }
    if let Some(limits) = value["limits"].as_array() {
        for limit in limits {
            if limit["is_active"].as_bool() == Some(false) {
                continue;
            }
            let Some(percent) = finite(limit.get("percent")) else {
                continue;
            };
            let label = limit["scope"]["model"]["display_name"]
                .as_str()
                .or_else(|| limit["scope"]["model"]["id"].as_str())
                .unwrap_or("Model limit");
            if snapshot.windows.iter().any(|window| {
                window.label.eq_ignore_ascii_case(label)
                    || (label.contains("Sonnet") && window.label == "Sonnet")
                    || (label.contains("Opus") && window.label == "Opus")
            }) {
                continue;
            }
            snapshot.windows.push(Window {
                label: label.to_string(),
                used_percent: clamp_percent(percent),
                reset_at_ms: limit.get("resets_at").and_then(parse_reset_ms),
                models: vec![label.to_string()],
            });
        }
    }
    if let Some(extra) = value["extra_usage"].as_object() {
        if extra.get("is_enabled").and_then(Value::as_bool) == Some(true) {
            if let (Some(used), Some(limit)) = (
                non_negative(extra.get("used_credits")),
                non_negative(extra.get("monthly_limit")),
            ) {
                snapshot.billing.push(Billing {
                    label: format!("extra usage {:.2}/{:.2}", used / 100.0, limit / 100.0),
                    amount: (limit - used).max(0.0) / 100.0,
                    unit: extra
                        .get("currency")
                        .and_then(Value::as_str)
                        .unwrap_or("USD")
                        .to_ascii_uppercase(),
                });
            } else if let Some(percent) = finite(extra.get("utilization")) {
                snapshot.windows.push(Window {
                    label: "Extra usage".to_string(),
                    used_percent: clamp_percent(percent),
                    reset_at_ms: None,
                    models: Vec::new(),
                });
            }
        }
    }
    if snapshot.windows.is_empty() && snapshot.billing.is_empty() {
        snapshot.error = Some("usage response contained no recognized limits".to_string());
    }
    snapshot
}

fn fetch_codex() -> Snapshot {
    let Some(tokens) = crate::codex::fresh_auth() else {
        return Snapshot::unknown(
            "openai",
            "chatgpt-wham",
            "sign in with phoenix codex login to read subscription limits",
        );
    };
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", tokens.access),
        ),
        ("Accept".to_string(), "application/json".to_string()),
        ("originator".to_string(), "phoenix".to_string()),
        (
            "User-Agent".to_string(),
            format!("phoenix/{}", crate::VERSION),
        ),
    ];
    if !tokens.account_id.is_empty() {
        headers.push(("ChatGPT-Account-Id".to_string(), tokens.account_id));
    }
    match request_json(
        "GET",
        "https://chatgpt.com/backend-api/wham/usage",
        &headers,
        None,
    ) {
        Ok(value) => parse_codex(&value),
        Err((_, error)) => Snapshot::unknown("openai", "chatgpt-wham", error),
    }
}

fn parse_codex(value: &Value) -> Snapshot {
    let mut snapshot = Snapshot::empty("openai", "chatgpt-wham");
    snapshot.plan = value["plan_type"].as_str().map(str::to_string);
    snapshot.available = value["rate_limit"]["limit_reached"]
        .as_bool()
        .map(|reached| !reached);
    let primary_reset = value["rate_limit"]["primary_window"]["reset_at"].as_u64();
    for (key, primary) in [("primary_window", true), ("secondary_window", false)] {
        let Some(window) = value["rate_limit"][key].as_object() else {
            continue;
        };
        let seconds = window
            .get("limit_window_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(if primary { 10_800 } else { 86_400 });
        let hours = seconds.div_ceil(3600);
        let reset_seconds = window.get("reset_at").and_then(Value::as_u64);
        let weekly_cadence = !primary
            && hours == 24
            && reset_seconds
                .zip(primary_reset)
                .is_some_and(|(secondary, first)| secondary.saturating_sub(first) >= 259_200);
        let label = if !primary && (hours >= 168 || weekly_cadence) {
            "Week".to_string()
        } else if !primary && hours >= 24 {
            "Day".to_string()
        } else {
            format!("{hours}h")
        };
        let reset_at_ms = reset_seconds
            .map(|seconds| seconds.saturating_mul(1000))
            .or_else(|| {
                window
                    .get("reset_after_seconds")
                    .and_then(Value::as_u64)
                    .map(|seconds| now_ms().saturating_add(seconds.saturating_mul(1000)))
            });
        snapshot.windows.push(Window {
            label,
            used_percent: clamp_percent(finite(window.get("used_percent")).unwrap_or(0.0)),
            reset_at_ms,
            models: Vec::new(),
        });
    }
    if let Some(balance) = finite(value["credits"].get("balance")) {
        if balance >= 0.0 {
            snapshot.billing.push(Billing {
                label: "credit balance".to_string(),
                amount: balance,
                unit: "credits".to_string(),
            });
        }
    }
    if snapshot.windows.is_empty() && snapshot.billing.is_empty() {
        snapshot.error = Some("usage response contained no recognized limits".to_string());
    }
    snapshot
}

fn quota_rank(state: QuotaState) -> u8 {
    match state {
        QuotaState::Ready => 3,
        QuotaState::Unknown => 2,
        QuotaState::Low => 1,
        QuotaState::Exhausted => 0,
    }
}

fn best_key_snapshot(cfg: &Config, fetch_one: impl Fn(&str) -> Snapshot) -> Snapshot {
    let mut keys = Vec::new();
    if !cfg.api_key.is_empty() {
        keys.push(cfg.api_key.as_str());
    }
    for key in &cfg.api_keys {
        if !key.is_empty() && !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut pool = CredentialPool::default();
    let mut best: Option<(u8, f64, Snapshot)> = None;
    for key in keys {
        let snapshot = fetch_one(key);
        let state = snapshot.state();
        let pressure = snapshot
            .windows
            .iter()
            .filter(|window| window_applies(window, &cfg.model))
            .map(|window| window.used_percent)
            .fold(0.0, f64::max);
        pool.total += 1;
        if snapshot.should_delegate_for_model(&cfg.model) {
            pool.delegated += 1;
        }
        match state {
            QuotaState::Ready => pool.ready += 1,
            QuotaState::Low => pool.low += 1,
            QuotaState::Exhausted => pool.exhausted += 1,
            QuotaState::Unknown => pool.unknown += 1,
        }
        let rank = quota_rank(state);
        if best
            .as_ref()
            .is_none_or(|(current_rank, current_pressure, _)| {
                rank > *current_rank || (rank == *current_rank && pressure < *current_pressure)
            })
        {
            best = Some((rank, pressure, snapshot));
        }
    }
    let mut snapshot = best.map(|(_, _, snapshot)| snapshot).unwrap_or_else(|| {
        Snapshot::unknown(
            &cfg.provider,
            "api",
            "no API key is available for proactive limit lookup",
        )
    });
    snapshot.pool = Some(pool);
    snapshot
}

fn fetch_openrouter(key: &str) -> Snapshot {
    if key.is_empty() {
        return Snapshot::unknown("openrouter", "api", "no OpenRouter API key is available");
    }
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {key}")),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let credits = request_json(
        "GET",
        "https://openrouter.ai/api/v1/credits",
        &headers,
        None,
    );
    let key_usage = request_json("GET", "https://openrouter.ai/api/v1/key", &headers, None);
    let mut snapshot = Snapshot::empty("openrouter", "api");
    if let Ok(root) = &credits {
        let data = &root["data"];
        if let (Some(total), Some(used)) = (
            finite(data.get("total_credits")),
            finite(data.get("total_usage")),
        ) {
            snapshot.billing.push(Billing {
                label: "account balance".to_string(),
                amount: (total - used).max(0.0),
                unit: "USD".to_string(),
            });
        }
    }
    if let Ok(root) = &key_usage {
        let data = &root["data"];
        snapshot.plan = data["label"].as_str().map(str::to_string);
        if let Some(limit) = non_negative(data.get("limit")) {
            let period = data["limit_reset"].as_str().unwrap_or("key");
            let period_usage = match period {
                "daily" => non_negative(data.get("usage_daily")),
                "weekly" => non_negative(data.get("usage_weekly")),
                "monthly" => non_negative(data.get("usage_monthly")),
                _ => non_negative(data.get("usage")),
            };
            let used = non_negative(data.get("limit_remaining"))
                .map(|remaining| (limit - remaining).max(0.0))
                .or(period_usage);
            if let Some(used) = used {
                snapshot.windows.push(Window {
                    label: format!("{} key budget", title(period)),
                    used_percent: if limit <= 0.0 {
                        100.0
                    } else {
                        clamp_percent(used / limit * 100.0)
                    },
                    reset_at_ms: None,
                    models: Vec::new(),
                });
            }
        }
    }
    if snapshot.windows.is_empty() && snapshot.billing.is_empty() {
        let error = credits
            .err()
            .or_else(|| key_usage.err())
            .map(|(_, error)| error)
            .unwrap_or_else(|| "usage response contained no recognized limits".to_string());
        snapshot.error = Some(error);
    }
    snapshot
}

fn fetch_deepseek(key: &str) -> Snapshot {
    if key.is_empty() {
        return Snapshot::unknown("deepseek", "api", "no DeepSeek API key is available");
    }
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {key}")),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    match request_json(
        "GET",
        "https://api.deepseek.com/user/balance",
        &headers,
        None,
    ) {
        Ok(value) => {
            let mut snapshot = Snapshot::empty("deepseek", "api");
            snapshot.available = value["is_available"].as_bool();
            if let Some(balances) = value["balance_infos"].as_array() {
                for balance in balances {
                    if let Some(amount) = non_negative(balance.get("total_balance")) {
                        snapshot.billing.push(Billing {
                            label: "balance".to_string(),
                            amount,
                            unit: balance["currency"]
                                .as_str()
                                .unwrap_or("credits")
                                .to_ascii_uppercase(),
                        });
                    }
                }
            }
            if snapshot.billing.is_empty() && snapshot.available.is_none() {
                snapshot.error = Some("balance response contained no recognized data".to_string());
            }
            snapshot
        }
        Err((_, error)) => Snapshot::unknown("deepseek", "api", error),
    }
}

fn fetch_google() -> Snapshot {
    let Ok(token) = std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN") else {
        return Snapshot::unknown(
            "google",
            "cloud-code",
            "quota lookup needs GOOGLE_OAUTH_ACCESS_TOKEN; API keys expose limits only on live responses",
        );
    };
    if token.is_empty() {
        return Snapshot::unknown("google", "cloud-code", "GOOGLE_OAUTH_ACCESS_TOKEN is empty");
    }
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    match request_json(
        "POST",
        "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota",
        &headers,
        Some("{}"),
    ) {
        Ok(value) => parse_google(&value),
        Err((_, error)) => Snapshot::unknown("google", "cloud-code", error),
    }
}

fn parse_google(value: &Value) -> Snapshot {
    let mut pro: Option<f64> = None;
    let mut flash: Option<f64> = None;
    if let Some(buckets) = value["buckets"].as_array() {
        for bucket in buckets {
            let model = bucket["modelId"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase();
            let remaining = finite(bucket.get("remainingFraction")).unwrap_or(1.0);
            let slot = if model.contains("pro") {
                &mut pro
            } else if model.contains("flash") {
                &mut flash
            } else {
                continue;
            };
            *slot = Some(slot.map_or(remaining, |current| current.min(remaining)));
        }
    }
    let mut snapshot = Snapshot::empty("google", "cloud-code");
    for (label, remaining) in [("Pro", pro), ("Flash", flash)] {
        if let Some(remaining) = remaining {
            snapshot.windows.push(Window {
                label: label.to_string(),
                used_percent: clamp_percent((1.0 - remaining) * 100.0),
                reset_at_ms: None,
                models: vec![label.to_string()],
            });
        }
    }
    if snapshot.windows.is_empty() {
        snapshot.error = Some("quota response contained no Pro or Flash buckets".to_string());
    }
    snapshot
}

fn request_json(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Result<Value, (Option<u16>, String)> {
    let mut request = if method == "POST" {
        ureq::post(url)
    } else {
        ureq::get(url)
    }
    .timeout(TIMEOUT);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = match body {
        Some(body) => request.send_string(body),
        None => request.call(),
    };
    match response {
        Ok(response) => crate::net::read_json(response, MAX_USAGE_BYTES)
            .map_err(|error| (None, compact_error(&error))),
        Err(ureq::Error::Status(status, response)) => {
            let detail = crate::net::read_json(response, 4096)
                .ok()
                .and_then(|value| {
                    value["error"]["message"]
                        .as_str()
                        .or_else(|| value["message"].as_str())
                        .map(|message| crate::security::one_line(message, 120))
                });
            let message = detail
                .map(|detail| format!("usage endpoint returned HTTP {status}: {detail}"))
                .unwrap_or_else(|| format!("usage endpoint returned HTTP {status}"));
            Err((Some(status), message))
        }
        Err(error) => Err((None, compact_error(&error.to_string()))),
    }
}

fn compact_error(error: &str) -> String {
    crate::security::one_line(&crate::security::redact(error), 120)
}

fn finite(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

fn non_negative(value: Option<&Value>) -> Option<f64> {
    finite(value).filter(|number| *number >= 0.0)
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

fn title(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "API".to_string(),
    }
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn now_ms() -> u64 {
    crate::scheduler::now_epoch().saturating_mul(1000)
}

fn parse_reset_ms(value: &Value) -> Option<u64> {
    if let Some(seconds) = value.as_u64() {
        return Some(if seconds < 10_000_000_000 {
            seconds.saturating_mul(1000)
        } else {
            seconds
        });
    }
    parse_rfc3339_ms(value.as_str()?)
}

fn parse_rfc3339_ms(value: &str) -> Option<u64> {
    let (date, time) = value.trim().split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let zone_at = time
        .char_indices()
        .skip(1)
        .find(|(_, character)| matches!(character, 'Z' | '+' | '-'))
        .map(|(index, _)| index)?;
    let (clock, zone) = time.split_at(zone_at);
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second_text = clock_parts.next()?;
    let (second, millis): (i64, u64) = match second_text.split_once('.') {
        Some((seconds, fraction)) => {
            let digits: String = fraction.chars().take(3).collect();
            let padded = format!("{digits:0<3}");
            (seconds.parse().ok()?, padded.parse::<u64>().ok()?)
        }
        None => (second_text.parse().ok()?, 0),
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let offset = if zone == "Z" {
        0i64
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let mut parts = zone.get(1..)?.split(':');
        let hours: i64 = parts.next()?.parse().ok()?;
        let minutes: i64 = parts.next()?.parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    };
    let days = days_from_civil(year, month, day);
    let seconds = days
        .saturating_mul(86_400)
        .saturating_add(hour * 3600 + minute * 60 + second)
        .saturating_sub(offset);
    u64::try_from(seconds)
        .ok()
        .map(|seconds| seconds.saturating_mul(1000).saturating_add(millis))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_windows_and_model_limits_normalize() {
        let snapshot = parse_anthropic(
            &json!({
                "five_hour": {"utilization": 18, "resets_at": "2026-01-08T00:00:00Z"},
                "seven_day": {"utilization": 140},
                "limits": [{
                    "is_active": true,
                    "percent": 27,
                    "resets_at": "2026-01-12T00:00:00+00:00",
                    "scope": {"model": {"display_name": "Fable"}}
                }]
            }),
            "oauth",
        );
        assert_eq!(snapshot.windows.len(), 3);
        assert_eq!(snapshot.windows[0].label, "5h");
        assert_eq!(snapshot.windows[1].used_percent, 100.0);
        assert_eq!(snapshot.windows[2].label, "Fable");
        assert!(snapshot.windows[2].reset_at_ms.is_some());
        assert_eq!(snapshot.state(), QuotaState::Exhausted);
        assert_eq!(
            snapshot.state_for_model("claude-fable-5"),
            QuotaState::Exhausted
        );
    }

    #[test]
    fn model_windows_match_real_display_names_by_family() {
        let mut snapshot = Snapshot::empty("anthropic", "test");
        snapshot.windows.push(Window {
            label: "Claude 3.7 Sonnet".into(),
            used_percent: 100.0,
            reset_at_ms: None,
            models: vec!["Sonnet".into()],
        });
        assert_eq!(
            snapshot.state_for_model("claude-3-7-sonnet-latest"),
            QuotaState::Exhausted
        );
        assert_eq!(
            snapshot.state_for_model("claude-opus-5"),
            QuotaState::Unknown
        );
    }

    #[test]
    fn codex_windows_plan_credits_and_weekly_cadence_normalize() {
        let primary = 1_700_000_000u64;
        let snapshot = parse_codex(&json!({
            "plan_type": "Plus",
            "rate_limit": {
                "limit_reached": false,
                "primary_window": {
                    "limit_window_seconds": 10800,
                    "used_percent": 35.5,
                    "reset_at": primary
                },
                "secondary_window": {
                    "limit_window_seconds": 86400,
                    "used_percent": 75,
                    "reset_at": primary + 5 * 86400
                }
            },
            "credits": {"balance": "12.5"}
        }));
        assert_eq!(snapshot.plan.as_deref(), Some("Plus"));
        assert_eq!(snapshot.windows[0].label, "3h");
        assert_eq!(snapshot.windows[1].label, "Week");
        assert_eq!(snapshot.billing[0].amount, 12.5);
        assert_eq!(snapshot.state(), QuotaState::Ready);
    }

    #[test]
    fn codex_limit_reached_is_exhausted_even_without_percent() {
        let snapshot = parse_codex(&json!({
            "rate_limit": {"limit_reached": true}
        }));
        assert_eq!(snapshot.state(), QuotaState::Exhausted);
    }

    #[test]
    fn google_uses_the_lowest_remaining_fraction_per_model_family() {
        let snapshot = parse_google(&json!({
            "buckets": [
                {"modelId": "gemini-pro-a", "remainingFraction": 0.8},
                {"modelId": "gemini-pro-b", "remainingFraction": 0.3},
                {"modelId": "gemini-flash", "remainingFraction": 0.9}
            ]
        }));
        assert_eq!(snapshot.windows[0].used_percent, 70.0);
        assert!((snapshot.windows[1].used_percent - 10.0).abs() < 0.0001);
    }

    #[test]
    fn key_ring_uses_a_healthy_secondary_instead_of_pausing() {
        let cfg = Config {
            provider: "openrouter".into(),
            api_key: "spent".into(),
            api_keys: vec!["healthy".into()],
            ..Config::default()
        };
        let snapshot = best_key_snapshot(&cfg, |key| {
            let mut snapshot = Snapshot::empty("openrouter", "test");
            snapshot.available = Some(key == "healthy");
            snapshot
        });
        assert_eq!(snapshot.state(), QuotaState::Ready);
        assert_eq!(
            snapshot.pool,
            Some(CredentialPool {
                total: 2,
                ready: 1,
                exhausted: 1,
                delegated: 1,
                ..CredentialPool::default()
            })
        );
    }

    #[test]
    fn key_ring_checks_past_a_low_key_for_a_ready_key() {
        let cfg = Config {
            provider: "openrouter".into(),
            api_key: "low".into(),
            api_keys: vec!["ready".into()],
            ..Config::default()
        };
        let snapshot = best_key_snapshot(&cfg, |key| {
            let mut snapshot = Snapshot::empty("openrouter", "test");
            snapshot.windows.push(Window {
                label: "key".into(),
                used_percent: if key == "low" { 95.0 } else { 25.0 },
                reset_at_ms: None,
                models: Vec::new(),
            });
            snapshot
        });
        assert_eq!(snapshot.state(), QuotaState::Ready);
        assert_eq!(snapshot.pool.map(|pool| pool.low), Some(1));
        assert_eq!(snapshot.pool.map(|pool| pool.ready), Some(1));
    }

    #[test]
    fn a_nearly_spent_key_does_not_delegate_when_another_low_key_has_room() {
        let cfg = Config {
            provider: "openrouter".into(),
            model: "model".into(),
            api_key: "nearly-spent".into(),
            api_keys: vec!["lower-pressure".into()],
            ..Config::default()
        };
        let snapshot = best_key_snapshot(&cfg, |key| {
            let mut snapshot = Snapshot::empty("openrouter", "test");
            snapshot.windows.push(Window {
                label: "key".into(),
                used_percent: if key == "nearly-spent" { 99.0 } else { 91.0 },
                reset_at_ms: None,
                models: Vec::new(),
            });
            snapshot
        });
        assert_eq!(snapshot.state(), QuotaState::Low);
        assert!(!snapshot.should_delegate_for_model("model"));
        assert_eq!(snapshot.windows[0].used_percent, 91.0);
        assert_eq!(snapshot.pool.map(|pool| pool.delegated), Some(1));
    }

    #[test]
    fn stale_or_unknown_evidence_never_forces_delegation() {
        let mut snapshot = Snapshot::empty("test", "test");
        snapshot.windows.push(Window {
            label: "all".into(),
            used_percent: 100.0,
            reset_at_ms: None,
            models: Vec::new(),
        });
        snapshot.observed_at_ms = 0;
        snapshot.max_age_ms = 1;
        assert_eq!(snapshot.state(), QuotaState::Unknown);
        assert!(!snapshot.should_delegate_for_model("model"));
        snapshot.observed_at_ms = now_ms();
        snapshot.confidence = Confidence::Unknown;
        assert_eq!(snapshot.state(), QuotaState::Unknown);
        assert!(!snapshot.should_delegate_for_model("model"));
    }

    #[test]
    fn quota_capabilities_preserve_specialized_sources() {
        let mut cfg = Config {
            provider: "google".into(),
            ..Config::default()
        };
        assert_eq!(quota_capability(&cfg), QuotaCapability::Subscription);
        cfg.api_key = "key".into();
        assert_eq!(quota_capability(&cfg), QuotaCapability::Reactive);
        cfg.provider = "openrouter".into();
        assert_eq!(quota_capability(&cfg), QuotaCapability::Credential);
        cfg.provider = "deepseek".into();
        assert_eq!(quota_capability(&cfg), QuotaCapability::Balance);
        cfg.provider = "ollama".into();
        assert_eq!(quota_capability(&cfg), QuotaCapability::Local);
    }

    #[test]
    fn allocation_shifts_work_away_from_a_low_seat_without_silencing_it() {
        let mut low = Snapshot::empty("low", "test");
        low.windows.push(Window {
            label: "week".into(),
            used_percent: 95.0,
            reset_at_ms: None,
            models: Vec::new(),
        });
        let mut ready = Snapshot::empty("ready", "test");
        ready.available = Some(true);
        let split = allocation(&low, "a", &ready, "b");
        assert_eq!((split.main_percent, split.partner_percent), (35, 65));
        assert!(split.guidance.contains("review-focused"));
        let reversed = allocation(&ready, "a", &low, "b");
        assert_eq!((reversed.main_percent, reversed.partner_percent), (65, 35));
    }

    #[test]
    fn allocation_keeps_unknown_balanced_and_explicit() {
        let unknown = Snapshot::unknown("unknown", "test", "no signal");
        let mut ready = Snapshot::empty("ready", "test");
        ready.available = Some(true);
        let split = allocation(&unknown, "a", &ready, "b");
        assert_eq!((split.main_percent, split.partner_percent), (50, 50));
        assert!(split.guidance.contains("not treat unknown usage as zero"));
        let mut low = Snapshot::empty("low", "test");
        low.windows.push(Window {
            label: "week".into(),
            used_percent: 95.0,
            reset_at_ms: None,
            models: Vec::new(),
        });
        let mixed = allocation(&low, "a", &unknown, "b");
        assert_eq!((mixed.main_percent, mixed.partner_percent), (50, 50));
        assert!(mixed.guidance.contains("unknown"));
    }

    #[test]
    fn colab_delegates_only_above_ninety_eight_percent() {
        let mut snapshot = Snapshot::empty("test", "test");
        snapshot.windows.push(Window {
            label: "Week".into(),
            used_percent: 98.0,
            reset_at_ms: None,
            models: Vec::new(),
        });
        assert!(!snapshot.should_delegate_for_model("any-model"));
        snapshot.windows[0].used_percent = 98.01;
        assert!(snapshot.should_delegate_for_model("any-model"));
        snapshot.windows.clear();
        assert!(!snapshot.should_delegate_for_model("any-model"));
        snapshot.available = Some(false);
        assert!(snapshot.should_delegate_for_model("any-model"));
    }

    #[test]
    fn delegation_threshold_respects_model_scoped_windows() {
        let mut snapshot = Snapshot::empty("anthropic", "test");
        snapshot.windows.push(Window {
            label: "Opus".into(),
            used_percent: 99.0,
            reset_at_ms: None,
            models: vec!["Opus".into()],
        });
        assert!(snapshot.should_delegate_for_model("claude-opus-5"));
        assert!(!snapshot.should_delegate_for_model("claude-sonnet-5"));
    }

    #[test]
    fn quota_states_fail_open_when_unknown_and_warn_when_low() {
        assert_eq!(
            Snapshot::unknown("custom", "provider", "no endpoint").state(),
            QuotaState::Unknown
        );
        let mut snapshot = Snapshot::empty("test", "test");
        snapshot.windows.push(Window {
            label: "Week".into(),
            used_percent: 90.0,
            reset_at_ms: None,
            models: Vec::new(),
        });
        assert_eq!(snapshot.state(), QuotaState::Low);
    }

    #[test]
    fn rfc3339_parser_handles_zulu_offsets_and_milliseconds() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2024-01-02T03:04:05.006Z"),
            Some(1_704_164_645_006)
        );
        assert_eq!(parse_rfc3339_ms("not-a-date"), None);
    }
}
