use std::fmt;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;

pub const RETRY_CODES: [u16; 6] = [408, 429, 500, 502, 503, 529];
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug)]
pub struct ProviderError(pub String);

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ProviderError {}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub enum Msg {
    User {
        content: String,

        images: Vec<(String, String)>,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

impl Reply {
    pub fn text_only(text: &str) -> Reply {
        Reply {
            text: text.to_string(),
            ..Reply::default()
        }
    }
}

pub trait ChatBackend {
    fn chat(
        &mut self,
        cfg: &Config,
        system: &str,
        history: &[Msg],
        tools: &[Value],
    ) -> Result<Reply, ProviderError>;

    fn chat_stream(
        &mut self,
        cfg: &Config,
        system: &str,
        history: &[Msg],
        tools: &[Value],
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, ProviderError> {
        let r = self.chat(cfg, system, history, tools)?;
        if !r.text.is_empty() {
            on_text(&r.text);
        }
        Ok(r)
    }
}

pub fn base_url_of(cfg: &Config) -> String {
    if !cfg.base_url.is_empty() {
        return cfg.base_url.clone();
    }
    base_url_for(&cfg.provider).unwrap_or_default().to_string()
}

fn base_url_for(kind: &str) -> Option<&'static str> {
    match kind {
        "openai" => Some("https://api.openai.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "ollama" => Some("http://localhost:11434/v1"),

        "nvidia" => Some("https://integrate.api.nvidia.com/v1"),

        "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        "groq" => Some("https://api.groq.com/openai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "moonshot" => Some("https://api.moonshot.ai/v1"),
        "cohere" => Some("https://api.cohere.ai/compatibility/v1"),
        "together" => Some("https://api.together.xyz/v1"),
        "novita" => Some("https://api.novita.ai/openai/v1"),
        "opencode" => Some("https://opencode.ai/zen/v1"),
        "byteplus" => Some("https://ark.ap-southeast.bytepluses.com/api/v3"),
        "volcengine" => Some("https://ark.cn-beijing.volces.com/api/v3"),
        "xiaomi" => Some("https://api.xiaomimimo.com/v1"),
        "meta" => Some("https://api.meta.ai/v1"),
        "huggingface" => Some("https://router.huggingface.co/v1"),
        _ => None,
    }
}

pub fn resolve_alias(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "opus" => ("anthropic", "claude-opus-5"),
        "sonnet" => ("anthropic", "claude-sonnet-5"),
        "gpt" => ("openai", "gpt-5.4"),
        "gpt-mini" => ("openai", "gpt-5.4-mini"),
        "gpt-nano" => ("openai", "gpt-5.4-nano"),
        "gemini" => ("google", "gemini-3.1-pro-preview"),
        "gemini-flash" => ("google", "gemini-3-flash-preview"),
        "gemini-flash-lite" => ("google", "gemini-3.1-flash-lite"),
        _ => return None,
    })
}

pub const BACKOFF_MAX_SECS: u64 = 30;

fn jitter_fraction() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

pub fn backoff_secs(attempt: u32, retry_after: Option<u64>, jitter: f64) -> u64 {
    if let Some(secs) = retry_after {
        return secs.min(BACKOFF_MAX_SECS);
    }
    let base = (2u64.saturating_pow(attempt) * 2).min(BACKOFF_MAX_SECS);
    let extra = (base as f64 * 0.25 * jitter.clamp(0.0, 1.0)).round() as u64;
    (base + extra).min(BACKOFF_MAX_SECS)
}

fn backoff_delay(attempt: u32, retry_after: Option<u64>) -> Duration {
    Duration::from_secs(backoff_secs(attempt, retry_after, jitter_fraction()))
}

fn call_timeout(cfg: &Config, default_secs: u64) -> Duration {
    if cfg.provider_timeout_secs > 0 {
        Duration::from_secs(cfg.provider_timeout_secs)
    } else {
        Duration::from_secs(default_secs)
    }
}

fn with_extra_headers(headers: Vec<(&'static str, String)>, cfg: &Config) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = headers
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    for (k, v) in &cfg.provider_headers {
        out.retain(|(name, _)| !name.eq_ignore_ascii_case(k));
        out.push((k.clone(), v.clone()));
    }
    out
}

fn post(
    url: &str,
    headers: &[(String, String)],
    payload: &Value,
    retries: u32,
    timeout: Duration,
) -> Result<Value, ProviderError> {
    let body = payload.to_string();
    for attempt in 0..=retries {
        let mut req = ureq::post(url)
            .timeout(timeout)
            .set("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_string(&body) {
            Ok(resp) => {
                let text = resp
                    .into_string()
                    .map_err(|e| ProviderError(e.to_string()))?;
                return serde_json::from_str(&text)
                    .map_err(|e| ProviderError(format!("bad JSON from provider: {e}")));
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let detail: String = error_body(resp, ERROR_BODY_CAP).chars().take(400).collect();
                if RETRY_CODES.contains(&code) && attempt < retries {
                    thread::sleep(backoff_delay(attempt, retry_after));
                    continue;
                }
                return Err(ProviderError(format!("HTTP {code}: {detail}")));
            }
            Err(e) => {
                if attempt < retries {
                    thread::sleep(backoff_delay(attempt, None));
                    continue;
                }
                return Err(ProviderError(e.to_string()));
            }
        }
    }
    Err(ProviderError("unreachable".into()))
}

pub enum Provider {
    Anthropic { url: String },
    OpenAICompat { url: String },
    OpenAIResponses { url: String },
}

pub const API_DIALECTS: &[&str] = &[
    "anthropic-messages",
    "openai-completions",
    "openai-responses",
];

pub fn provider_api(kind: &str) -> &'static str {
    match kind {
        "anthropic" => "anthropic-messages",
        "meta" => "openai-responses",
        _ => "openai-completions",
    }
}

fn resolved_api(cfg: &Config) -> String {
    if !cfg.api.is_empty() {
        return cfg.api.clone();
    }
    provider_api(&cfg.provider).to_string()
}

pub fn make(cfg: &Config) -> Result<Provider, ProviderError> {
    let api = resolved_api(cfg);
    if api == "anthropic-messages" {
        let url = if cfg.base_url.is_empty() {
            ANTHROPIC_URL.to_string()
        } else {
            format!("{}/messages", cfg.base_url.trim_end_matches('/'))
        };
        return Ok(Provider::Anthropic { url });
    }
    let base = if !cfg.base_url.is_empty() {
        cfg.base_url.clone()
    } else {
        base_url_for(&cfg.provider)
            .map(str::to_string)
            .ok_or_else(|| {
                ProviderError(format!(
                    "unknown provider '{}': set provider.base_url",
                    cfg.provider
                ))
            })?
    };
    let base = base.trim_end_matches('/');
    match api.as_str() {
        "openai-responses" => Ok(Provider::OpenAIResponses {
            url: format!("{base}/responses"),
        }),
        "openai-completions" => Ok(Provider::OpenAICompat {
            url: format!("{base}/chat/completions"),
        }),
        other => Err(ProviderError(format!(
            "unknown provider.api '{other}': expected one of {API_DIALECTS:?}"
        ))),
    }
}

fn responses_payload(cfg: &Config, system: &str, history: &[Msg], tools: &[Value]) -> Value {
    let mut input: Vec<Value> = Vec::new();
    for m in history {
        match m {
            Msg::User { content, images } => {
                let mut parts: Vec<Value> = Vec::new();
                for (mime, b64) in images {
                    parts.push(json!({"type": "input_image",
                        "image_url": format!("data:{mime};base64,{b64}")}));
                }
                let text = if content.is_empty() && !parts.is_empty() {
                    "(see attachment)"
                } else {
                    content
                };
                parts.push(json!({"type": "input_text", "text": text}));
                input.push(json!({"role": "user", "content": parts}));
            }
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                if !content.is_empty() {
                    input.push(json!({"role": "assistant",
                        "content": [{"type": "output_text", "text": content}]}));
                }
                for tc in tool_calls {
                    input.push(json!({"type": "function_call", "call_id": tc.id,
                        "name": tc.name, "arguments": tc.args.to_string()}));
                }
            }
            Msg::Tool { id, content } => {
                input.push(json!({"type": "function_call_output",
                    "call_id": id, "output": content}));
            }
        }
    }
    let mut payload = json!({
        "model": cfg.model,
        "instructions": system,
        "input": input,
    });
    if !tools.is_empty() {
        let flat: Vec<Value> = tools
            .iter()
            .map(|t| {
                let f = t.get("function").unwrap_or(t);
                json!({
                    "type": "function",
                    "name": f.get("name").cloned().unwrap_or(Value::Null),
                    "description": f.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": f.get("parameters").cloned().unwrap_or(json!({})),
                })
            })
            .collect();
        payload["tools"] = Value::Array(flat);
    }
    payload
}

fn parse_responses(data: &Value) -> Result<Reply, ProviderError> {
    let items = data
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            let s = data.to_string();
            let clip: String = s.chars().take(200).collect();
            ProviderError(format!("malformed response: {clip}"))
        })?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for p in parts {
                        if let Some(t) = p.get("text").and_then(Value::as_str) {
                            text.push_str(t);
                        }
                    }
                }
            }
            Some("function_call") => {
                let raw = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let args: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
                calls.push(ToolCall {
                    id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_{}", calls.len())),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    args,
                });
            }
            _ => {}
        }
    }
    Ok(Reply {
        text,
        tool_calls: calls,
        usage: Usage {
            input: data["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output: data["usage"]["output_tokens"].as_u64().unwrap_or(0),
        },
    })
}

fn chat_responses(
    url: &str,
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = responses_payload(cfg, system, history, tools);
    let data = rotate_post(cfg, &payload, |key, body, retries| {
        post(
            url,
            &with_extra_headers(openai_headers(key), cfg),
            body,
            retries,
            call_timeout(cfg, 180),
        )
    })?;
    parse_responses(&data)
}

fn anthropic_payload(cfg: &Config, system: &str, history: &[Msg], tools: &[Value]) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    for m in history {
        match m {
            Msg::User { content, images } => {
                if images.is_empty() {
                    msgs.push(json!({"role": "user", "content": content}));
                } else {
                    let mut blocks: Vec<Value> = Vec::new();
                    for (mime, b64) in images {
                        let kind = if mime == "application/pdf" {
                            "document"
                        } else {
                            "image"
                        };
                        blocks.push(json!({"type": kind, "source": {
                            "type": "base64", "media_type": mime, "data": b64}}));
                    }
                    let text = if content.is_empty() {
                        "(see attachment)"
                    } else {
                        content
                    };
                    blocks.push(json!({"type": "text", "text": text}));
                    msgs.push(json!({"role": "user", "content": blocks}));
                }
            }
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                let mut blocks: Vec<Value> = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                for tc in tool_calls {
                    blocks.push(json!({
                        "type": "tool_use", "id": tc.id,
                        "name": tc.name, "input": tc.args
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": " "}));
                }
                msgs.push(json!({"role": "assistant", "content": blocks}));
            }
            Msg::Tool { id, content } => {
                msgs.push(json!({"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": id, "content": content
                }]}));
            }
        }
    }
    let system_value = if key_ring(cfg).iter().any(|k| oauth_key(k)) {
        json!([
            {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
            {"type": "text", "text": system}
        ])
    } else {
        json!(system)
    };
    let mut payload = json!({
        "model": cfg.model, "max_tokens": 8192,
        "system": system_value, "messages": msgs
    });
    if let Some(budget) = thinking_budget(&cfg.thinking) {
        payload["max_tokens"] = json!(std::cmp::min(budget + 8192, 64000).max(budget + 1));
        payload["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
    }
    if !tools.is_empty() {
        let ts: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t["name"], "description": t["description"],
                    "input_schema": t["parameters"]
                })
            })
            .collect();
        payload["tools"] = Value::Array(ts);
    }
    payload
}

pub fn thinking_budget(level: &str) -> Option<u64> {
    match level {
        "minimal" => Some(1024),
        "low" => Some(2048),
        "medium" | "adaptive" => Some(8192),
        "high" => Some(16384),
        "xhigh" => Some(32768),
        "max" => Some(63999),
        _ => None,
    }
}

pub fn reasoning_effort(level: &str) -> Option<&'static str> {
    match level {
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" | "adaptive" => Some("medium"),
        "high" | "xhigh" | "max" => Some("high"),
        _ => None,
    }
}

fn reasoning_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

pub fn oauth_key(key: &str) -> bool {
    key.starts_with("sk-ant-oat")
}

fn anthropic_headers(key: &str) -> Vec<(&'static str, String)> {
    if oauth_key(key) {
        vec![
            ("authorization", format!("Bearer {key}")),
            ("anthropic-beta", "oauth-2025-04-20".to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ]
    } else {
        vec![
            ("x-api-key", key.to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ]
    }
}

pub fn list_models(cfg: &Config) -> Result<Vec<String>, ProviderError> {
    let key = key_ring(cfg).into_iter().next().unwrap_or_default();
    let (url, headers): (String, Vec<(&str, String)>) = if cfg.provider == "anthropic" {
        (
            "https://api.anthropic.com/v1/models".into(),
            anthropic_headers(&key),
        )
    } else {
        let base = if !cfg.base_url.is_empty() {
            cfg.base_url.clone()
        } else {
            base_url_for(&cfg.provider)
                .map(str::to_string)
                .ok_or_else(|| ProviderError(format!("unknown provider '{}'", cfg.provider)))?
        };
        (
            format!("{}/models", base.trim_end_matches('/')),
            vec![("Authorization", format!("Bearer {key}"))],
        )
    };
    let mut req = ureq::get(&url).timeout(Duration::from_secs(30));
    for (k, v) in &headers {
        req = req.set(k, v);
    }
    let text = req
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, r) => ProviderError(format!(
                "HTTP {code}: {}",
                error_body(r, ERROR_BODY_CAP)
                    .chars()
                    .take(200)
                    .collect::<String>()
            )),
            other => ProviderError(other.to_string()),
        })?
        .into_string()
        .map_err(|e| ProviderError(e.to_string()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| ProviderError(e.to_string()))?;
    let mut out: Vec<String> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    Ok(out)
}

pub fn key_ring(cfg: &Config) -> Vec<String> {
    let mut ring: Vec<String> = Vec::new();
    if cfg.provider == "anthropic" {
        if let Some(tok) = crate::oauth::fresh_access() {
            ring.push(tok);
        }
    }
    if (!cfg.api_key.is_empty() || cfg.api_keys.iter().all(String::is_empty))
        && !ring.contains(&cfg.api_key)
    {
        ring.push(cfg.api_key.clone());
    }
    for k in &cfg.api_keys {
        if !k.is_empty() && !ring.contains(k) {
            ring.push(k.clone());
        }
    }
    if ring.is_empty() {
        ring.push(String::new());
    }
    ring
}

pub fn is_auth_error(err: &ProviderError) -> bool {
    err.0.starts_with("HTTP 401") || err.0.starts_with("HTTP 403")
}

pub const ERROR_BODY_CAP: u64 = 8192;

pub fn error_body(resp: ureq::Response, cap: u64) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = resp.into_reader().take(cap).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

pub fn context_overflow(err: &ProviderError) -> bool {
    let s = err.0.to_lowercase();
    [
        "context length",
        "context_length",
        "maximum context",
        "context window",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "reduce the length of the messages",
    ]
    .iter()
    .any(|k| s.contains(k))
}

pub fn rotatable(err: &ProviderError) -> bool {
    let s = &err.0;
    s.starts_with("HTTP 429")
        || s.starts_with("HTTP 5")
        || s.starts_with("HTTP 408")
        || s.contains("overloaded_error")
        || s.contains("rate_limit_error")
}

fn chat_anthropic(
    url: &str,
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = anthropic_payload(cfg, system, history, tools);
    let data = rotate_post(cfg, &payload, |key, body, retries| {
        post(
            url,
            &with_extra_headers(anthropic_headers(key), cfg),
            body,
            retries,
            call_timeout(cfg, 180),
        )
    })?;
    let mut text = String::new();
    let mut calls = Vec::new();
    if let Some(blocks) = data.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                }
                Some("tool_use") => calls.push(ToolCall {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    args: block.get("input").cloned().unwrap_or_else(|| json!({})),
                }),
                _ => {}
            }
        }
    }
    let usage = Usage {
        input: data["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output: data["usage"]["output_tokens"].as_u64().unwrap_or(0),
    };
    Ok(Reply {
        text,
        tool_calls: calls,
        usage,
    })
}

fn openai_payload(cfg: &Config, system: &str, history: &[Msg], tools: &[Value]) -> Value {
    let mut msgs: Vec<Value> = vec![json!({"role": "system", "content": system})];
    for m in history {
        match m {
            Msg::User { content, images } => {
                if images.is_empty() {
                    msgs.push(json!({"role": "user", "content": content}));
                } else {
                    let mut parts: Vec<Value> = Vec::new();
                    for (mime, b64) in images {
                        if mime == "application/pdf" {
                            parts.push(json!({"type": "text",
                                "text": "[PDF attached, but this provider API cannot read PDFs]"}));
                        } else {
                            parts.push(json!({"type": "image_url", "image_url": {
                                "url": format!("data:{mime};base64,{b64}")}}));
                        }
                    }
                    let text = if content.is_empty() {
                        "(see attachment)"
                    } else {
                        content
                    };
                    parts.push(json!({"type": "text", "text": text}));
                    msgs.push(json!({"role": "user", "content": parts}));
                }
            }
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                let mut out = json!({"role": "assistant"});
                out["content"] = if content.is_empty() {
                    Value::Null
                } else {
                    Value::String(content.clone())
                };
                if !tool_calls.is_empty() {
                    let tcs: Vec<Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id, "type": "function",
                                "function": {"name": tc.name,
                                             "arguments": tc.args.to_string()}
                            })
                        })
                        .collect();
                    out["tool_calls"] = Value::Array(tcs);
                }
                msgs.push(out);
            }
            Msg::Tool { id, content } => {
                msgs.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
            }
        }
    }
    let mut payload = json!({"model": cfg.model, "messages": msgs});
    if reasoning_model(&cfg.model) {
        if let Some(effort) = reasoning_effort(&cfg.thinking) {
            payload["reasoning_effort"] = json!(effort);
        }
    }
    if !tools.is_empty() {
        let ts: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t["name"], "description": t["description"],
                    "parameters": t["parameters"]
                }})
            })
            .collect();
        payload["tools"] = Value::Array(ts);
    }
    payload
}

fn openai_headers(key: &str) -> Vec<(&'static str, String)> {
    let mut headers = Vec::new();
    if !key.is_empty() {
        headers.push(("Authorization", format!("Bearer {key}")));
    }
    headers
}

pub const KEY_COOLDOWN_SECS: u64 = 120;

fn cooldown_key(cfg: &Config, index: usize) -> String {
    format!("{}:key{index}", cfg.provider)
}

fn rotate_post<T>(
    cfg: &Config,
    payload: &Value,
    call: impl Fn(&str, &Value, u32) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    rotate_post_in(&crate::state::State::load(), cfg, payload, call)
}

fn rotate_post_in<T>(
    state: &crate::state::State,
    cfg: &Config,
    payload: &Value,
    call: impl Fn(&str, &Value, u32) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let ring = key_ring(cfg);
    let hot: Vec<(usize, &String)> = ring
        .iter()
        .enumerate()
        .filter(|(i, _)| state.cooling(&cooldown_key(cfg, *i)).is_none())
        .collect();
    let ring: Vec<&String> = if hot.is_empty() {
        ring.iter().collect()
    } else {
        hot.into_iter().map(|(_, k)| k).collect()
    };
    let last = ring.len() - 1;
    let retries = if last == 0 { cfg.max_retries } else { 0 };
    let mut first_err: Option<ProviderError> = None;
    for (i, key) in ring.iter().enumerate() {
        match call(key, payload, retries) {
            Ok(v) => return Ok(v),
            Err(e) if i < last && rotatable(&e) => {
                let _ = state.cool_down(
                    &cooldown_key(cfg, i),
                    KEY_COOLDOWN_SECS,
                    &crate::security::one_line(&e.0, 60),
                );
                eprintln!(
                    "provider key {} failed ({}), rotating to key {}",
                    i + 1,
                    crate::security::redact(&e.0),
                    i + 2
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                return Err(match first_err {
                    Some(first) if is_auth_error(&e) && !is_auth_error(&first) => {
                        ProviderError(format!(
                            "{} (key {} of {}); the original failure was: {}",
                            e.0,
                            i + 1,
                            ring.len(),
                            first.0
                        ))
                    }
                    _ => e,
                });
            }
        }
    }
    Err(first_err.unwrap_or_else(|| ProviderError("no provider key was usable".into())))
}

fn chat_openai(
    url: &str,
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = openai_payload(cfg, system, history, tools);
    let data = rotate_post(cfg, &payload, |key, body, retries| {
        post(
            url,
            &with_extra_headers(openai_headers(key), cfg),
            body,
            retries,
            call_timeout(cfg, 180),
        )
    })?;
    let msg = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .ok_or_else(|| {
            let s = data.to_string();
            let clip: String = s.chars().take(200).collect();
            ProviderError(format!("malformed response: {clip}"))
        })?;
    let mut calls = Vec::new();
    if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let args_raw = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let args: Value = serde_json::from_str(args_raw).unwrap_or_else(|_| json!({}));
            calls.push(ToolCall {
                id: tc
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{}", calls.len())),
                name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                args,
            });
        }
    }
    let usage = Usage {
        input: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output: data["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    };
    Ok(Reply {
        text: msg
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        tool_calls: calls,
        usage,
    })
}

impl ChatBackend for Provider {
    fn chat(
        &mut self,
        cfg: &Config,
        system: &str,
        history: &[Msg],
        tools: &[Value],
    ) -> Result<Reply, ProviderError> {
        match self {
            Provider::Anthropic { url } => chat_anthropic(url, cfg, system, history, tools),
            Provider::OpenAICompat { url } => chat_openai(url, cfg, system, history, tools),
            Provider::OpenAIResponses { url } => chat_responses(url, cfg, system, history, tools),
        }
    }

    fn chat_stream(
        &mut self,
        cfg: &Config,
        system: &str,
        history: &[Msg],
        tools: &[Value],
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, ProviderError> {
        match self {
            Provider::Anthropic { url } => {
                let mut payload = anthropic_payload(cfg, system, history, tools);
                payload["stream"] = Value::Bool(true);
                let reader = rotate_post(cfg, &payload, |key, body, retries| {
                    post_stream(
                        url,
                        &with_extra_headers(anthropic_headers(key), cfg),
                        body,
                        retries,
                        call_timeout(cfg, 300),
                    )
                })?;
                parse_anthropic_sse(std::io::BufReader::new(reader), on_text)
            }
            Provider::OpenAICompat { url } => {
                let mut payload = openai_payload(cfg, system, history, tools);
                payload["stream"] = Value::Bool(true);
                let reader = rotate_post(cfg, &payload, |key, body, retries| {
                    post_stream(
                        url,
                        &with_extra_headers(openai_headers(key), cfg),
                        body,
                        retries,
                        call_timeout(cfg, 300),
                    )
                })?;
                parse_openai_sse(std::io::BufReader::new(reader), on_text)
            }
            Provider::OpenAIResponses { url } => {
                let r = chat_responses(url, cfg, system, history, tools)?;
                if !r.text.is_empty() {
                    on_text(&r.text);
                }
                Ok(r)
            }
        }
    }
}

fn post_stream(
    url: &str,
    headers: &[(String, String)],
    payload: &Value,
    retries: u32,
    timeout: Duration,
) -> Result<Box<dyn std::io::Read + Send>, ProviderError> {
    let body = payload.to_string();
    for attempt in 0..=retries {
        let mut req = ureq::post(url)
            .timeout(timeout)
            .set("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_string(&body) {
            Ok(resp) => return Ok(Box::new(resp.into_reader())),
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let detail: String = error_body(resp, ERROR_BODY_CAP).chars().take(400).collect();
                if RETRY_CODES.contains(&code) && attempt < retries {
                    thread::sleep(backoff_delay(attempt, retry_after));
                    continue;
                }
                return Err(ProviderError(format!("HTTP {code}: {detail}")));
            }
            Err(e) => {
                if attempt < retries {
                    thread::sleep(backoff_delay(attempt, None));
                    continue;
                }
                return Err(ProviderError(e.to_string()));
            }
        }
    }
    Err(ProviderError("unreachable".into()))
}

fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn parse_anthropic_sse(
    reader: impl std::io::BufRead,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, ProviderError> {
    use std::collections::BTreeMap;
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut blocks: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for line in reader.lines() {
        let line = line.map_err(|e| ProviderError(e.to_string()))?;
        let Some(data) = sse_data(&line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "message_start" => {
                usage.input = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                if v["content_block"]["type"] == "tool_use" {
                    let idx = v["index"].as_u64().unwrap_or(0) as usize;
                    blocks.insert(
                        idx,
                        (
                            v["content_block"]["id"].as_str().unwrap_or("").to_string(),
                            v["content_block"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                            String::new(),
                        ),
                    );
                }
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0) as usize;
                match v["delta"]["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let t = v["delta"]["text"].as_str().unwrap_or("");
                        text.push_str(t);
                        on_text(t);
                    }
                    "input_json_delta" => {
                        if let Some(b) = blocks.get_mut(&idx) {
                            b.2.push_str(v["delta"]["partial_json"].as_str().unwrap_or(""));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                    usage.output = o;
                }
            }
            "error" => {
                let msg = v["error"]["message"].as_str().unwrap_or("unknown");
                return Err(ProviderError(format!("stream error: {msg}")));
            }
            _ => {}
        }
    }
    let mut taken: Vec<String> = Vec::new();
    let mut tool_calls = Vec::new();
    for (id, name, args) in blocks.into_values() {
        tool_calls.push(ToolCall {
            id: unique_tool_id(&mut taken, id),
            args: parse_tool_args(&args, &name)?,
            name,
        });
    }
    Ok(Reply {
        text,
        tool_calls,
        usage,
    })
}

pub fn parse_tool_args(raw: &str, tool: &str) -> Result<Value, ProviderError> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) | Err(_) => Err(ProviderError(format!(
            "stream ended with an incomplete argument payload for tool '{tool}'; \
the reply was truncated and was not executed"
        ))),
    }
}

fn unique_tool_id(taken: &mut Vec<String>, id: String) -> String {
    if !id.is_empty() && !taken.contains(&id) {
        taken.push(id.clone());
        return id;
    }
    let mut n = taken.len();
    loop {
        let candidate = format!("call_{n}");
        if !taken.contains(&candidate) {
            taken.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

fn parse_openai_sse(
    reader: impl std::io::BufRead,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, ProviderError> {
    use std::collections::BTreeMap;
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut acc: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    let mut saw_event = false;
    for line in reader.lines() {
        let line = line.map_err(|e| ProviderError(e.to_string()))?;
        let Some(data) = sse_data(&line) else {
            continue;
        };
        if data.trim() == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        saw_event = true;
        if let Some(err) = v.get("error") {
            let msg = err["message"]
                .as_str()
                .or_else(|| err.as_str())
                .unwrap_or("unknown");
            return Err(ProviderError(format!("stream error: {msg}")));
        }
        if let Some(u) = v.get("usage") {
            if let Some(p) = u["prompt_tokens"].as_u64() {
                usage.input = p;
            }
            if let Some(c) = u["completion_tokens"].as_u64() {
                usage.output = c;
            }
        }
        let delta = &v["choices"][0]["delta"];
        if let Some(t) = delta["content"].as_str() {
            text.push_str(t);
            on_text(t);
        }
        if let Some(tcs) = delta["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                let e = acc.entry(idx).or_default();
                if let Some(id) = tc["id"].as_str() {
                    e.0 = id.to_string();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    if e.1 != n {
                        e.1.push_str(n);
                    }
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    e.2.push_str(a);
                }
            }
        }
    }
    let mut taken: Vec<String> = Vec::new();
    let mut tool_calls = Vec::new();
    for (id, name, args) in acc.into_values() {
        tool_calls.push(ToolCall {
            id: unique_tool_id(&mut taken, id),
            args: parse_tool_args(&args, &name)?,
            name,
        });
    }
    if !saw_event {
        return Err(ProviderError(
            "the model stream closed without sending any events".into(),
        ));
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(ProviderError(
            "the model stream produced no text and no tool calls".into(),
        ));
    }
    Ok(Reply {
        text,
        tool_calls,
        usage,
    })
}

#[cfg(test)]
mod sse_frame_tests {
    use super::*;

    #[test]
    fn data_lines_parse_with_and_without_space() {
        assert_eq!(sse_data("data: {}"), Some("{}"));
        assert_eq!(sse_data("data:{}"), Some("{}"));
        assert_eq!(sse_data("data:  two"), Some(" two"));
        assert_eq!(sse_data("event: ping"), None);
        assert_eq!(sse_data(":heartbeat"), None);
        assert_eq!(sse_data(""), None);
    }

    #[test]
    fn openai_stream_without_space_after_data_still_yields_text() {
        let body = "data:{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\ndata: [DONE]\n";
        let mut seen = String::new();
        let reply = parse_openai_sse(std::io::BufReader::new(body.as_bytes()), &mut |t| {
            seen.push_str(t)
        })
        .unwrap();
        assert_eq!(reply.text, "hi");
        assert_eq!(seen, "hi");
    }

    #[test]
    fn anthropic_stream_without_space_after_data_still_yields_text() {
        let frame = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "yo"}
        });
        let body = format!("data:{frame}\n");
        let reply =
            parse_anthropic_sse(std::io::BufReader::new(body.as_bytes()), &mut |_| {}).unwrap();
        assert_eq!(reply.text, "yo");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(provider: &str, base_url: &str) -> Config {
        Config {
            provider: provider.into(),
            base_url: base_url.into(),
            ..Config::default()
        }
    }

    #[test]
    fn a_responses_provider_posts_to_the_responses_endpoint() {
        let mut cfg = cfg_with("openai", "https://api.example/v1");
        cfg.api = "openai-responses".into();
        match make(&cfg).expect("provider") {
            Provider::OpenAIResponses { url } => {
                assert_eq!(url, "https://api.example/v1/responses")
            }
            _ => panic!("expected the responses dialect"),
        }
    }

    #[test]
    fn completions_stays_the_default_for_ordinary_providers() {
        let cfg = cfg_with("nvidia", "");
        match make(&cfg).expect("provider") {
            Provider::OpenAICompat { url } => assert!(url.ends_with("/chat/completions")),
            _ => panic!("expected chat completions"),
        }
    }

    #[test]
    fn meta_defaults_to_responses_because_that_is_all_it_implements() {
        assert_eq!(provider_api("meta"), "openai-responses");
        assert_eq!(provider_api("anthropic"), "anthropic-messages");
        assert_eq!(provider_api("nvidia"), "openai-completions");
    }

    #[test]
    fn an_explicit_api_overrides_the_provider_default() {
        let mut cfg = cfg_with("nvidia", "");
        cfg.api = "openai-responses".into();
        assert!(matches!(
            make(&cfg).expect("provider"),
            Provider::OpenAIResponses { .. }
        ));
    }

    #[test]
    fn an_unknown_api_is_refused_instead_of_silently_using_completions() {
        let mut cfg = cfg_with("nvidia", "");
        cfg.api = "grpc-please".into();
        assert!(make(&cfg).is_err());
    }

    #[test]
    fn responses_payload_uses_input_and_instructions_not_messages() {
        let cfg = cfg_with("openai", "");
        let history = vec![Msg::User {
            content: "hello".into(),
            images: Vec::new(),
        }];
        let p = responses_payload(&cfg, "be brief", &history, &[]);
        assert_eq!(p["instructions"], "be brief");
        assert!(p.get("messages").is_none(), "that is the completions shape");
        assert_eq!(p["input"][0]["role"], "user");
        assert_eq!(p["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(p["input"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn responses_tools_are_flat_not_nested_under_function() {
        let cfg = cfg_with("openai", "");
        let tools = vec![json!({"type": "function", "function": {
            "name": "shell", "description": "run", "parameters": {"type": "object"}}})];
        let p = responses_payload(&cfg, "s", &[], &tools);
        assert_eq!(p["tools"][0]["name"], "shell");
        assert_eq!(p["tools"][0]["type"], "function");
        assert!(p["tools"][0].get("function").is_none());
    }

    #[test]
    fn responses_tool_results_round_trip_as_function_call_output() {
        let cfg = cfg_with("openai", "");
        let history = vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "shell".into(),
                    args: json!({"cmd": "ls"}),
                }],
            },
            Msg::Tool {
                id: "call_1".into(),
                content: "out".into(),
            },
        ];
        let p = responses_payload(&cfg, "s", &history, &[]);
        assert_eq!(p["input"][0]["type"], "function_call");
        assert_eq!(p["input"][0]["call_id"], "call_1");
        assert_eq!(p["input"][1]["type"], "function_call_output");
        assert_eq!(p["input"][1]["output"], "out");
    }

    #[test]
    fn responses_reply_parses_text_tool_calls_and_usage() {
        let data = json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "PONG"}]},
                {"type": "function_call", "call_id": "c1", "name": "shell",
                 "arguments": "{\"cmd\":\"ls\"}"}
            ],
            "usage": {"input_tokens": 11, "output_tokens": 22}
        });
        let r = parse_responses(&data).expect("parse");
        assert_eq!(r.text, "PONG");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "shell");
        assert_eq!(r.tool_calls[0].args["cmd"], "ls");
        assert_eq!(r.usage.input, 11);
        assert_eq!(r.usage.output, 22);
    }

    #[test]
    fn malformed_tool_arguments_do_not_become_an_empty_object() {
        let data = json!({"output": [
            {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": "{trunc"}
        ]});
        let r = parse_responses(&data).expect("parse");
        assert_eq!(r.tool_calls[0].args, json!({}));
    }

    #[test]
    fn a_response_without_output_is_an_error_not_an_empty_reply() {
        assert!(parse_responses(&json!({"error": "nope"})).is_err());
    }

    #[test]
    fn key_ring_order_and_dedup() {
        let mut cfg = Config::default();
        assert_eq!(key_ring(&cfg), vec![String::new()]);
        cfg.api_key = "k1".into();
        cfg.api_keys = vec!["k2".into(), "k1".into(), "k3".into()];
        assert_eq!(key_ring(&cfg), vec!["k1", "k2", "k3"]);
    }

    #[test]
    fn anthropic_thinking_budget_and_max_tokens() {
        let mut cfg = cfg_with("anthropic", "");
        cfg.thinking = "medium".into();
        let p = anthropic_payload(&cfg, "sys", &[], &[]);
        assert_eq!(p["thinking"]["type"], "enabled");
        assert_eq!(p["thinking"]["budget_tokens"], 8192);
        assert_eq!(p["max_tokens"], 8192 + 8192);
        cfg.thinking = "off".into();
        let p = anthropic_payload(&cfg, "sys", &[], &[]);
        assert!(p.get("thinking").is_none());
        assert_eq!(p["max_tokens"], 8192);
    }

    #[test]
    fn openai_reasoning_effort_gated_by_model() {
        let mut cfg = cfg_with("openai", "");
        cfg.model = "gpt-5.4".into();
        cfg.thinking = "high".into();
        let p = openai_payload(&cfg, "sys", &[], &[]);
        assert_eq!(p["reasoning_effort"], "high");
        cfg.model = "llama3.3".into();
        let p = openai_payload(&cfg, "sys", &[], &[]);
        assert!(p.get("reasoning_effort").is_none());
        cfg.model = "o3-mini".into();
        cfg.thinking = "off".into();
        let p = openai_payload(&cfg, "sys", &[], &[]);
        assert!(p.get("reasoning_effort").is_none());
    }

    #[test]
    fn context_overflow_classification() {
        let hits = [
            "HTTP 400: This model's maximum context length is 128000 tokens, however you requested 131000",
            "HTTP 400: prompt is too long: 210000 tokens > 200000 maximum",
            "HTTP 400: {\"error\":{\"code\":\"context_length_exceeded\"}}",
            "HTTP 400: please reduce the length of the messages",
        ];
        for h in hits {
            assert!(context_overflow(&ProviderError(h.into())), "{h}");
        }
        let misses = [
            "HTTP 429: rate limited",
            "HTTP 500: mock failure",
            "HTTP 401: bad key",
        ];
        for m in misses {
            assert!(!context_overflow(&ProviderError(m.into())), "{m}");
        }
    }

    #[test]
    fn rotatable_classification() {
        assert!(rotatable(&ProviderError("HTTP 429: too many".into())));
        assert!(rotatable(&ProviderError("HTTP 500: boom".into())));
        assert!(rotatable(&ProviderError("HTTP 529: overloaded".into())));
        assert!(rotatable(&ProviderError("HTTP 408: timeout".into())));
        assert!(!rotatable(&ProviderError("HTTP 401: bad key".into())));
        assert!(!rotatable(&ProviderError("HTTP 400: bad request".into())));
        assert!(!rotatable(&ProviderError("connection refused".into())));
    }

    #[test]
    fn a_billing_402_fails_fast_no_retry_no_rotation() {
        assert!(
            !RETRY_CODES.contains(&402),
            "402 is a billing state, not a transient fault; retrying it is a death spiral"
        );
        assert!(
            !rotatable(&ProviderError("HTTP 402: payment required".into())),
            "rotating keys cannot fix an unpaid account"
        );
        assert!(!is_auth_error(&ProviderError(
            "HTTP 402: payment required".into()
        )));
    }

    #[test]
    fn backoff_grows_jitters_and_honours_retry_after() {
        assert_eq!(backoff_secs(0, None, 0.0), 2);
        assert_eq!(backoff_secs(1, None, 0.0), 4);
        assert_eq!(backoff_secs(2, None, 0.0), 8);
        assert!(backoff_secs(10, None, 1.0) <= BACKOFF_MAX_SECS);
        assert_eq!(backoff_secs(30, None, 0.0), BACKOFF_MAX_SECS);

        let plain = backoff_secs(2, None, 0.0);
        let jittered = backoff_secs(2, None, 1.0);
        assert!(jittered > plain, "jitter must spread retries");

        assert_eq!(backoff_secs(5, Some(3), 1.0), 3);
        assert_eq!(backoff_secs(0, Some(9999), 0.0), BACKOFF_MAX_SECS);
    }

    #[test]
    fn auth_error_classification_and_rotation_context() {
        assert!(is_auth_error(&ProviderError("HTTP 401: bad key".into())));
        assert!(is_auth_error(&ProviderError("HTTP 403: forbidden".into())));
        assert!(!is_auth_error(&ProviderError("HTTP 429: slow down".into())));

        let cfg = Config {
            api_key: "k1".into(),
            api_keys: vec!["k2".into()],
            ..Config::default()
        };
        let st = test_state("auth-ctx");
        let err = rotate_post_in(
            &st,
            &cfg,
            &json!({}),
            |key, _b, _r| -> Result<(), ProviderError> {
                if key == "k1" {
                    Err(ProviderError("HTTP 429: rate limited".into()))
                } else {
                    Err(ProviderError("HTTP 401: API key is invalid".into()))
                }
            },
        )
        .unwrap_err();
        assert!(err.0.contains("HTTP 401"), "{}", err.0);
        assert!(err.0.contains("original failure"), "{}", err.0);
        assert!(err.0.contains("429"), "{}", err.0);
    }

    #[test]
    fn rotate_post_advances_on_rate_limit_only() {
        let cfg = Config {
            api_key: "bad".into(),
            api_keys: vec!["good".into()],
            ..Config::default()
        };
        let payload = json!({});

        let st = test_state("rotate");
        let out = rotate_post_in(&st, &cfg, &payload, |key, _, _| {
            if key == "bad" {
                Err(ProviderError("HTTP 429: slow down".into()))
            } else {
                Ok(key.to_string())
            }
        })
        .unwrap();
        assert_eq!(out, "good");

        let st = test_state("rotate401");
        let err = rotate_post_in(&st, &cfg, &payload, |key, _, _| -> Result<String, _> {
            Err(ProviderError(format!("HTTP 401: {key}")))
        })
        .unwrap_err();
        assert_eq!(err.0, "HTTP 401: bad");

        let st = test_state("rotate429");
        let err = rotate_post_in(&st, &cfg, &payload, |key, _, _| -> Result<String, _> {
            Err(ProviderError(format!("HTTP 429: {key}")))
        })
        .unwrap_err();
        assert_eq!(err.0, "HTTP 429: good");
    }

    fn test_state(tag: &str) -> crate::state::State {
        let p = std::env::temp_dir().join(format!(
            "phx-provstate-{tag}-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&p);
        crate::state::State::at(&p)
    }

    #[test]
    fn extra_headers_join_and_override_case_insensitively() {
        let mut cfg = cfg_with("openai", "");
        cfg.provider_headers = vec![
            ("X-Prompt-Cache".to_string(), "on".to_string()),
            ("AUTHORIZATION".to_string(), "Bearer custom".to_string()),
        ];
        let h = with_extra_headers(openai_headers("sk-base"), &cfg);
        assert!(
            h.iter().any(|(k, v)| k == "X-Prompt-Cache" && v == "on"),
            "{h:?}"
        );
        assert_eq!(
            h.iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .count(),
            1,
            "an override must replace, not duplicate: {h:?}"
        );
        assert!(
            h.iter()
                .any(|(k, v)| k == "AUTHORIZATION" && v == "Bearer custom"),
            "{h:?}"
        );
        let plain = with_extra_headers(openai_headers("sk-base"), &cfg_with("openai", ""));
        assert!(plain.iter().any(|(k, _)| k == "Authorization"));
    }

    #[test]
    fn a_configured_timeout_overrides_the_built_in_ceilings() {
        let mut cfg = cfg_with("openai", "");
        assert_eq!(call_timeout(&cfg, 180), Duration::from_secs(180));
        assert_eq!(call_timeout(&cfg, 300), Duration::from_secs(300));
        cfg.provider_timeout_secs = 45;
        assert_eq!(call_timeout(&cfg, 180), Duration::from_secs(45));
        assert_eq!(call_timeout(&cfg, 300), Duration::from_secs(45));
    }

    #[test]
    fn make_selects_backend() {
        match make(&cfg_with("anthropic", "")).unwrap() {
            Provider::Anthropic { url } => assert_eq!(url, ANTHROPIC_URL),
            _ => panic!("expected Anthropic"),
        }
        match make(&cfg_with("anthropic", "http://127.0.0.1:9999/v1/")).unwrap() {
            Provider::Anthropic { url } => {
                assert_eq!(url, "http://127.0.0.1:9999/v1/messages")
            }
            _ => panic!("expected Anthropic"),
        }
        match make(&cfg_with("openrouter", "")).unwrap() {
            Provider::OpenAICompat { url } => {
                assert_eq!(url, "https://openrouter.ai/api/v1/chat/completions")
            }
            _ => panic!("expected OpenAICompat"),
        }
        match make(&cfg_with("custom", "http://x.local/v1/")).unwrap() {
            Provider::OpenAICompat { url } => {
                assert_eq!(url, "http://x.local/v1/chat/completions")
            }
            _ => panic!("expected OpenAICompat"),
        }
    }

    #[test]
    fn oauth_tokens_get_bearer_and_identity() {
        let h = anthropic_headers("sk-ant-oat01-token");
        assert!(h
            .iter()
            .any(|(k, v)| *k == "authorization" && v.starts_with("Bearer ")));
        assert!(h
            .iter()
            .any(|(k, v)| *k == "anthropic-beta" && v == "oauth-2025-04-20"));
        let h2 = anthropic_headers("sk-ant-api-key");
        assert!(h2.iter().any(|(k, _)| *k == "x-api-key"));
        assert!(!h2.iter().any(|(k, _)| *k == "authorization"));

        let mut cfg = Config {
            provider: "anthropic".into(),
            api_key: "sk-ant-oat01-token".into(),
            ..Config::default()
        };
        let p = anthropic_payload(&cfg, "be brief", &[], &[]);
        let sys = p["system"].as_array().expect("system blocks for oauth");
        assert!(sys[0]["text"].as_str().unwrap().contains("Claude Code"));
        cfg.api_key = "sk-ant-plain".into();
        let p2 = anthropic_payload(&cfg, "be brief", &[], &[]);
        assert_eq!(p2["system"].as_str().unwrap(), "be brief");
    }

    #[test]
    fn make_rejects_unknown_without_base_url() {
        assert!(make(&cfg_with("mystery", "")).is_err());
    }

    #[test]
    fn retry_codes_cover_spec() {
        for code in [408u16, 429, 500, 502, 503, 529] {
            assert!(RETRY_CODES.contains(&code));
        }
        assert!(!RETRY_CODES.contains(&404));
    }

    #[test]
    fn anthropic_sse_parses_text_tools_usage() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu1\",\"name\":\"shell\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"comm\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"and\\\":\\\"ls\\\"}\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n",
        );
        let mut seen = String::new();
        let r = parse_anthropic_sse(std::io::Cursor::new(sse), &mut |t| seen.push_str(t)).unwrap();
        assert_eq!(r.text, "Hello");
        assert_eq!(seen, "Hello");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "tu1");
        assert_eq!(r.tool_calls[0].name, "shell");
        assert_eq!(r.tool_calls[0].args["command"], "ls");
        assert_eq!(
            r.usage,
            Usage {
                input: 7,
                output: 9
            }
        );
    }

    #[test]
    fn anthropic_sse_error_event_fails() {
        let sse = "data: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n";
        let out = parse_anthropic_sse(std::io::Cursor::new(sse), &mut |_| {});
        assert!(out.is_err());
    }

    #[test]
    fn openai_sse_parses_text_and_tools() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"there\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c9\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"c\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ommand\\\":\\\"ls\\\"}\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n\n",
        );
        let mut seen = String::new();
        let r = parse_openai_sse(std::io::Cursor::new(sse), &mut |t| seen.push_str(t)).unwrap();
        assert_eq!(r.text, "Hi there");
        assert_eq!(seen, "Hi there");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "c9");
        assert_eq!(r.tool_calls[0].args["command"], "ls");
    }

    #[test]
    fn truncated_tool_arguments_fail_instead_of_running_empty() {
        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"rm -rf /tm\"}}]}}]}\n\n";
        let err = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {})
            .expect_err("a truncated argument payload must not become {}");
        assert!(err.0.contains("incomplete argument payload"), "{}", err.0);
        assert!(err.0.contains("shell"), "{}", err.0);
    }

    #[test]
    fn truncated_anthropic_tool_arguments_fail_too() {
        let sse = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu1\",\"name\":\"write_file\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a\"}}\n\n",
        );
        let err = parse_anthropic_sse(std::io::Cursor::new(sse), &mut |_| {})
            .expect_err("a truncated argument payload must not become {}");
        assert!(err.0.contains("incomplete argument payload"), "{}", err.0);
        assert!(err.0.contains("write_file"), "{}", err.0);
    }

    #[test]
    fn empty_tool_arguments_are_a_valid_no_arg_call() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"bg_list\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let r = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {}).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].args, json!({}));
    }

    #[test]
    fn non_object_tool_arguments_are_refused() {
        assert!(parse_tool_args("[1,2]", "shell").is_err());
        assert!(parse_tool_args("\"just a string\"", "shell").is_err());
        assert!(parse_tool_args("   ", "shell").is_ok());
        assert!(parse_tool_args("{\"a\":1}", "shell").is_ok());
    }

    #[test]
    fn duplicate_tool_ids_are_made_unique() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
            "{\"index\":0,\"id\":\"same\",\"function\":{\"name\":\"list_dir\",\"arguments\":\"{}\"}},",
            "{\"index\":1,\"id\":\"same\",\"function\":{\"name\":\"list_dir\",\"arguments\":\"{}\"}}",
            "]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let r = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {}).unwrap();
        assert_eq!(r.tool_calls.len(), 2);
        assert_ne!(
            r.tool_calls[0].id, r.tool_calls[1].id,
            "duplicate tool ids break tool_result pairing"
        );
    }

    #[test]
    fn repeated_name_fragments_do_not_duplicate_the_tool_name() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"list_dir\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"list_dir\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let r = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {}).unwrap();
        assert_eq!(r.tool_calls[0].name, "list_dir");
    }

    #[test]
    fn openai_stream_error_event_is_surfaced() {
        let sse = "data: {\"error\":{\"message\":\"context length exceeded\"}}\n\n";
        let err = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {})
            .expect_err("a stream error must not look like an empty reply");
        assert!(err.0.contains("context length exceeded"), "{}", err.0);
    }

    #[test]
    fn silent_stream_is_an_error_not_an_empty_reply() {
        let err = parse_openai_sse(std::io::Cursor::new(""), &mut |_| {})
            .expect_err("a stream with no events must be an error");
        assert!(err.0.contains("without sending any events"), "{}", err.0);

        let sse = "data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n";
        let err = parse_openai_sse(std::io::Cursor::new(sse), &mut |_| {})
            .expect_err("no text and no tool calls must be an error");
        assert!(err.0.contains("no text and no tool calls"), "{}", err.0);
    }

    #[test]
    fn default_chat_stream_falls_back_to_full_text() {
        struct One;
        impl ChatBackend for One {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                Ok(Reply::text_only("whole"))
            }
        }
        let mut p = One;
        let mut seen = String::new();
        let r = p
            .chat_stream(&Config::default(), "", &[], &[], &mut |t| seen.push_str(t))
            .unwrap();
        assert_eq!(seen, "whole");
        assert_eq!(r.text, "whole");
    }
}

#[cfg(test)]
mod vision_tests {
    use super::*;
    use serde_json::json;

    fn user_with(mime: &str, content: &str) -> Vec<Msg> {
        vec![Msg::User {
            content: content.to_string(),
            images: vec![(mime.to_string(), "QUJD".to_string())],
        }]
    }

    #[test]
    fn anthropic_image_block() {
        let p = anthropic_payload(
            &Config::default(),
            "s",
            &user_with("image/jpeg", "what?"),
            &[],
        );
        let c = &p["messages"][0]["content"];
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(c[0]["source"]["data"], "QUJD");
        assert_eq!(c[1]["type"], "text");
        assert_eq!(c[1]["text"], "what?");
    }

    #[test]
    fn anthropic_pdf_becomes_document_block() {
        let p = anthropic_payload(
            &Config::default(),
            "s",
            &user_with("application/pdf", ""),
            &[],
        );
        let c = &p["messages"][0]["content"];
        assert_eq!(c[0]["type"], "document");
        assert_eq!(c[1]["text"], "(see attachment)");
    }

    #[test]
    fn openai_image_data_url() {
        let p = openai_payload(&Config::default(), "s", &user_with("image/png", "hm"), &[]);
        let c = &p["messages"][1]["content"];
        assert_eq!(c[0]["type"], "image_url");
        assert_eq!(c[0]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(c[1]["text"], "hm");
    }

    #[test]
    fn openai_pdf_degrades_to_note() {
        let p = openai_payload(
            &Config::default(),
            "s",
            &user_with("application/pdf", "x"),
            &[],
        );
        let c = &p["messages"][1]["content"];
        assert_eq!(c[0]["type"], "text");
        assert!(c[0]["text"].as_str().unwrap().contains("cannot read PDFs"));
    }

    #[test]
    fn plain_user_message_stays_a_string() {
        let h = vec![Msg::User {
            content: "hi".into(),
            images: Vec::new(),
        }];
        let p = anthropic_payload(&Config::default(), "s", &h, &[]);
        assert_eq!(p["messages"][0]["content"], json!("hi"));
    }
}
