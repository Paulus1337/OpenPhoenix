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
    pub thinking: String,
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

pub trait ChatBackend: Send {
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

pub fn busy_backoff(attempt: u32) -> Duration {
    backoff_delay(attempt, None)
}

pub fn retry_after_hint(err: &ProviderError) -> Option<u64> {
    let s = &err.0;
    let at = s.find("retry after ")? + "retry after ".len();
    let digits: String = s[at..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
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
            Ok(resp) => match resp.into_string() {
                Ok(text) => {
                    return serde_json::from_str(&text)
                        .map_err(|e| ProviderError(format!("bad JSON from provider: {e}")))
                }
                Err(_) if attempt < retries => {
                    thread::sleep(backoff_delay(attempt, None));
                    continue;
                }
                Err(e) => return Err(ProviderError(format!("provider response read failed: {e}"))),
            },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    Unavailable,
    AdapterDependent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None,
    ApiKey,
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub wire_format: &'static str,
    pub streaming: Support,
    pub tools: Support,
    pub images: Support,
    pub reasoning_control: Support,
    pub response_usage: Support,
    pub authentication: Vec<AuthMode>,
    pub quota: crate::usage::QuotaCapability,
}

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

pub fn capabilities(cfg: &Config) -> BackendCapabilities {
    let resolved = resolved_api(cfg);
    let wire_format = match resolved.as_str() {
        "anthropic-messages" => "anthropic-messages",
        "openai-responses" => "openai-responses",
        _ => "openai-completions",
    };
    let concrete_adapter = cfg.provider != "custom";
    let adapter_support = if concrete_adapter {
        Support::Supported
    } else {
        Support::AdapterDependent
    };
    let images = match resolved.as_str() {
        "anthropic-messages" | "openai-responses" if concrete_adapter => Support::Supported,
        "anthropic-messages" | "openai-responses" | "openai-completions" => {
            Support::AdapterDependent
        }
        _ => Support::Unavailable,
    };
    let authentication = match cfg.provider.as_str() {
        "ollama" => vec![AuthMode::None],
        "anthropic" => vec![AuthMode::ApiKey, AuthMode::OAuth],
        "openai" if cfg.base_url.is_empty() => vec![AuthMode::ApiKey, AuthMode::OAuth],
        _ => vec![AuthMode::ApiKey],
    };
    let reasoning_control = if !concrete_adapter {
        Support::AdapterDependent
    } else if thinking_levels_for(cfg).len() > 2 {
        Support::Supported
    } else {
        Support::Unavailable
    };
    BackendCapabilities {
        wire_format,
        streaming: adapter_support,
        tools: adapter_support,
        images,
        reasoning_control,
        response_usage: adapter_support,
        authentication,
        quota: crate::usage::quota_capability(cfg),
    }
}

pub fn codex_active(cfg: &Config) -> bool {
    cfg.provider == "openai"
        && cfg.api.is_empty()
        && cfg.base_url.is_empty()
        && cfg.api_key.is_empty()
        && crate::codex::load().is_some()
}

pub fn has_credential(cfg: &Config) -> bool {
    if cfg.provider == "ollama" {
        return true;
    }
    if !cfg.api_key.is_empty() || cfg.api_keys.iter().any(|k| !k.is_empty()) {
        return true;
    }
    if cfg.provider == "anthropic" && crate::oauth::fresh_access().is_some() {
        return true;
    }
    if cfg.provider == "openai" && crate::codex::load().is_some() {
        return true;
    }
    false
}

pub fn make(cfg: &Config) -> Result<Provider, ProviderError> {
    if codex_active(cfg) {
        return Ok(Provider::OpenAIResponses {
            url: format!("{}/responses", crate::codex::BACKEND_URL),
        });
    }
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
    if reasoning_model(&cfg.model) {
        if let Some(effort) = reasoning_effort(&cfg.thinking) {
            payload["reasoning"] = json!({"effort": effort, "summary": "auto"});
        }
    }
    if codex_active(cfg) {
        payload["store"] = json!(false);
        payload["stream"] = json!(true);
    }
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

fn tool_call_from_item(item: &Value, idx: usize) -> ToolCall {
    let raw = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let args: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
    ToolCall {
        id: item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{idx}")),
        name: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        args,
    }
}

fn message_text_from_item(item: &Value) -> String {
    let mut text = String::new();
    if let Some(parts) = item.get("content").and_then(Value::as_array) {
        for p in parts {
            if let Some(t) = p.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
        }
    }
    text
}

fn reasoning_text_from_item(item: &Value) -> String {
    let mut text = String::new();
    if let Some(sum) = item.get("summary").and_then(Value::as_array) {
        for s in sum {
            if let Some(t) = s.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
        }
    }
    text
}

fn responses_usage(data: &Value) -> Usage {
    Usage {
        input: data["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output: data["usage"]["output_tokens"].as_u64().unwrap_or(0),
    }
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
    let mut thinking = String::new();
    let mut calls = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => text.push_str(&message_text_from_item(item)),
            Some("reasoning") => thinking.push_str(&reasoning_text_from_item(item)),
            Some("function_call") => calls.push(tool_call_from_item(item, calls.len())),
            _ => {}
        }
    }
    if text.trim().is_empty() && calls.is_empty() {
        return Err(ProviderError(EMPTY_REPLY_ERROR.into()));
    }
    Ok(Reply {
        text,
        thinking,
        tool_calls: calls,
        usage: responses_usage(data),
    })
}

fn oauth_retry<T>(
    cfg: &Config,
    call: impl Fn() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    match call() {
        Err(e) if is_auth_error(&e) => {
            let refreshed = if codex_active(cfg) {
                crate::codex::force_refresh().is_some()
            } else if cfg.provider == "anthropic" {
                crate::oauth::force_refresh().is_some()
            } else {
                false
            };
            if refreshed {
                call()
            } else {
                Err(e)
            }
        }
        other => other,
    }
}

fn chat_responses(
    url: &str,
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = responses_payload(cfg, system, history, tools);
    oauth_retry(cfg, || {
        if codex_active(cfg) {
            let reader = rotate_post(cfg, &payload, |key, body, retries| {
                post_stream(
                    url,
                    &with_extra_headers(openai_auth_headers(cfg, key), cfg),
                    body,
                    retries,
                    call_timeout(cfg, 300),
                )
            })?;
            return parse_responses_sse(std::io::BufReader::new(reader), &mut |_| {});
        }
        let data = rotate_post(cfg, &payload, |key, body, retries| {
            post(
                url,
                &with_extra_headers(openai_auth_headers(cfg, key), cfg),
                body,
                retries,
                call_timeout(cfg, 180),
            )
        })?;
        parse_responses(&data)
    })
}

fn parse_responses_sse(
    reader: impl std::io::BufRead,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, ProviderError> {
    let mut text = String::new();
    let mut item_text = String::new();
    let mut thinking = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut usage = Usage::default();
    let mut fallback: Option<Reply> = None;
    let mut last_error: Option<String> = None;
    let mut incomplete_reason: Option<String> = None;
    let mut stream_error: Option<String> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        };
        let Some(data) = sse_data(&line) else {
            continue;
        };
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(t) = v["delta"].as_str() {
                    text.push_str(t);
                    on_text(t);
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(t) = v["delta"].as_str() {
                    thinking.push_str(t);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    match item.get("type").and_then(Value::as_str) {
                        Some("function_call") => calls.push(tool_call_from_item(item, calls.len())),
                        Some("message") => {
                            let t = message_text_from_item(item);
                            if !t.is_empty() {
                                item_text = t;
                            }
                        }
                        Some("reasoning") if thinking.is_empty() => {
                            thinking.push_str(&reasoning_text_from_item(item))
                        }
                        _ => {}
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                if let Some(resp) = v.get("response") {
                    usage = responses_usage(resp);
                    fallback = parse_responses(resp).ok();
                    if v["type"] == "response.incomplete" {
                        incomplete_reason = resp["incomplete_details"]["reason"]
                            .as_str()
                            .map(str::to_string);
                    }
                }
            }
            "response.failed" | "error" => {
                let msg = if v["response"]["error"].is_object() {
                    stream_error_message(&v["response"]["error"])
                } else if v["error"].is_object() {
                    stream_error_message(&v["error"])
                } else {
                    v["message"].as_str().unwrap_or("stream failed").to_string()
                };
                last_error = Some(msg);
            }
            _ => {}
        }
    }
    let final_text = if text.trim().is_empty() {
        item_text
    } else {
        text
    };
    if !final_text.trim().is_empty() || !calls.is_empty() {
        let usage = if usage == Usage::default() {
            fallback.as_ref().map(|r| r.usage).unwrap_or_default()
        } else {
            usage
        };
        let text = if stream_error.is_some() && !final_text.trim().is_empty() {
            partial_stream_text(final_text)
        } else {
            final_text
        };
        return Ok(Reply {
            text,
            thinking,
            tool_calls: calls,
            usage,
        });
    }
    if let Some(reply) = fallback {
        return Ok(reply);
    }
    if let Some(e) = last_error {
        return Err(ProviderError(e));
    }
    if let Some(reason) = incomplete_reason {
        return Err(ProviderError(format!(
            "the model stopped early ({reason}) before sending any content"
        )));
    }
    if let Some(error) = stream_error {
        return Err(ProviderError(format!(
            "provider stream interrupted: {error}"
        )));
    }
    Err(ProviderError(EMPTY_REPLY_ERROR.into()))
}

fn partial_stream_text(mut text: String) -> String {
    text.push_str(
        "\n\n[The provider connection ended early. This is the text received before it reconnected.]",
    );
    text
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
                let block = json!({
                    "type": "tool_result", "tool_use_id": id, "content": content
                });
                let appended = msgs.last_mut().is_some_and(|message| {
                    if message["role"] != "user" {
                        return false;
                    }
                    let Some(blocks) = message["content"].as_array_mut() else {
                        return false;
                    };
                    if !blocks.iter().all(|item| item["type"] == "tool_result") {
                        return false;
                    }
                    blocks.push(block.clone());
                    true
                });
                if !appended {
                    msgs.push(json!({"role": "user", "content": [block]}));
                }
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
        const CAP: u64 = 64000;
        const MIN_OUTPUT: u64 = 8192;
        let budget = budget.min(CAP - MIN_OUTPUT);
        payload["max_tokens"] = json!(budget + MIN_OUTPUT);
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

pub fn thinking_levels_for(cfg: &Config) -> Vec<&'static str> {
    let api = resolved_api(cfg);
    if api == "anthropic-messages" {
        return vec![
            "default", "off", "minimal", "low", "medium", "adaptive", "high", "xhigh", "max",
        ];
    }
    if api == "openai-completions" && reasoning_model(&cfg.model) {
        return vec![
            "default", "off", "minimal", "low", "medium", "adaptive", "high",
        ];
    }
    vec!["default", "off"]
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
    if codex_active(cfg) {
        if let Some(tok) = crate::codex::fresh_access() {
            ring.push(tok);
        }
    }
    if !cfg.api_key.is_empty() && !ring.contains(&cfg.api_key) {
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

pub const EMPTY_REPLY_ERROR: &str = "the model returned no text and no tool calls";

pub fn is_empty_reply(err: &ProviderError) -> bool {
    err.0.contains(EMPTY_REPLY_ERROR) || err.0.contains("without sending any events")
}

pub fn transient_transport(err: &ProviderError) -> bool {
    let message = err.0.to_ascii_lowercase();
    [
        "error while decoding chunks",
        "provider stream interrupted",
        "provider response read failed",
        "connection reset",
        "connection aborted",
        "broken pipe",
        "unexpected eof",
        "timed out",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

pub fn stream_error_message(err: &Value) -> String {
    let msg = err["message"]
        .as_str()
        .or_else(|| err.as_str())
        .unwrap_or("unknown");
    match err["type"].as_str().filter(|t| !t.is_empty()) {
        Some(kind) if !msg.contains(kind) => format!("{kind}: {msg}"),
        _ => msg.to_string(),
    }
}

pub fn rotatable(err: &ProviderError) -> bool {
    let s = &err.0;
    s.starts_with("HTTP 429")
        || s.starts_with("HTTP 5")
        || s.starts_with("HTTP 408")
        || s.contains("overloaded_error")
        || s.contains("rate_limit_error")
        || transient_transport(err)
}

fn chat_anthropic(
    url: &str,
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = anthropic_payload(cfg, system, history, tools);
    let data = oauth_retry(cfg, || {
        rotate_post(cfg, &payload, |key, body, retries| {
            post(
                url,
                &with_extra_headers(anthropic_headers(key), cfg),
                body,
                retries,
                call_timeout(cfg, 180),
            )
        })
    })?;
    let mut text = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    if let Some(blocks) = data.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                }
                Some("thinking") => {
                    thinking.push_str(block.get("thinking").and_then(Value::as_str).unwrap_or(""))
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
    if text.trim().is_empty() && calls.is_empty() {
        return Err(ProviderError(EMPTY_REPLY_ERROR.into()));
    }
    Ok(Reply {
        text,
        thinking,
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

fn openai_auth_headers(cfg: &Config, key: &str) -> Vec<(&'static str, String)> {
    let mut headers = openai_headers(key);
    if codex_active(cfg) {
        if let Some(id) = crate::codex::account_id() {
            headers.push(("chatgpt-account-id", id));
        }
        headers.push(("originator", "phoenix".to_string()));
        headers.push(("OpenAI-Beta", "responses=experimental".to_string()));
    }
    headers
}

pub const KEY_COOLDOWN_SECS: u64 = 120;

fn credential_id(cfg: &Config, key: &str) -> String {
    let source = if (cfg.provider == "anthropic" && oauth_key(key)) || codex_active(cfg) {
        "oauth"
    } else {
        "api"
    };
    let digest = crate::security::sha256_hex(key.as_bytes());
    format!("{}:{source}:{}", cfg.provider, &digest[..16])
}

fn cooldown_key(cfg: &Config, key: &str) -> String {
    format!("{}:credential:{}", cfg.provider, credential_id(cfg, key))
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
    let full_ring = key_ring(cfg);
    let hot: Vec<&String> = full_ring
        .iter()
        .filter(|key| state.cooling(&cooldown_key(cfg, key)).is_none())
        .collect();
    let ring: Vec<&String> = if hot.is_empty() {
        full_ring.iter().collect()
    } else {
        hot
    };
    let last = ring.len().saturating_sub(1);
    let mut first_err: Option<ProviderError> = None;
    for (index, key) in ring.iter().enumerate() {
        match call(key, payload, cfg.max_retries) {
            Ok(value) => return Ok(value),
            Err(error) if rotatable(&error) || is_auth_error(&error) => {
                let _ = state.cool_down(
                    &cooldown_key(cfg, key),
                    KEY_COOLDOWN_SECS,
                    &crate::security::one_line(&error.0, 60),
                );
                if index < last {
                    crate::log::warn_with(
                        "providers",
                        format!(
                            "provider credential {} of {} failed ({}); trying the next one",
                            index + 1,
                            ring.len(),
                            if is_auth_error(&error) {
                                "not accepted"
                            } else {
                                "temporarily unavailable"
                            }
                        ),
                        &crate::log::Fields::default().provider(&cfg.provider),
                    );
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                }
                return Err(match first_err {
                    Some(first) if is_auth_error(&error) && !is_auth_error(&first) => {
                        ProviderError(format!(
                            "{} (credential {} of {}); the original failure was: {}",
                            error.0,
                            index + 1,
                            ring.len(),
                            first.0
                        ))
                    }
                    _ => error,
                });
            }
            Err(error) => return Err(error),
        }
    }
    Err(first_err.unwrap_or_else(|| ProviderError("no provider credential was usable".into())))
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
            &with_extra_headers(openai_auth_headers(cfg, key), cfg),
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
    let text = msg
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() && calls.is_empty() {
        return Err(ProviderError(EMPTY_REPLY_ERROR.into()));
    }
    Ok(Reply {
        text,
        thinking: String::new(),
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
                let fetch = || {
                    rotate_post(cfg, &payload, |key, body, retries| {
                        post_stream(
                            url,
                            &with_extra_headers(anthropic_headers(key), cfg),
                            body,
                            retries,
                            call_timeout(cfg, 300),
                        )
                    })
                };
                let reader = match fetch() {
                    Err(e)
                        if is_auth_error(&e)
                            && cfg.provider == "anthropic"
                            && crate::oauth::force_refresh().is_some() =>
                    {
                        fetch()?
                    }
                    other => other?,
                };
                parse_anthropic_sse(std::io::BufReader::new(reader), on_text)
            }
            Provider::OpenAICompat { url } => {
                let mut payload = openai_payload(cfg, system, history, tools);
                payload["stream"] = Value::Bool(true);
                let reader = rotate_post(cfg, &payload, |key, body, retries| {
                    post_stream(
                        url,
                        &with_extra_headers(openai_auth_headers(cfg, key), cfg),
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
    let mut thinking = String::new();
    let mut usage = Usage::default();
    let mut blocks: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    let mut stream_error: Option<String> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        };
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
                    "thinking_delta" => {
                        thinking.push_str(v["delta"]["thinking"].as_str().unwrap_or(""));
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
                let msg = stream_error_message(&v["error"]);
                return Err(ProviderError(format!("stream error: {msg}")));
            }
            _ => {}
        }
    }
    if let Some(error) = stream_error {
        if !text.trim().is_empty() {
            return Ok(Reply {
                text: partial_stream_text(text),
                thinking,
                tool_calls: Vec::new(),
                usage,
            });
        }
        return Err(ProviderError(format!(
            "provider stream interrupted: {error}"
        )));
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
    if text.trim().is_empty() && tool_calls.is_empty() {
        return Err(ProviderError(EMPTY_REPLY_ERROR.into()));
    }
    Ok(Reply {
        text,
        thinking,
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
    let mut stream_error: Option<String> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        };
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
            let msg = stream_error_message(err);
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
    if let Some(error) = stream_error {
        if !text.trim().is_empty() {
            return Ok(Reply {
                text: partial_stream_text(text),
                thinking: String::new(),
                tool_calls: Vec::new(),
                usage,
            });
        }
        return Err(ProviderError(format!(
            "provider stream interrupted: {error}"
        )));
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
    if text.trim().is_empty() && tool_calls.is_empty() {
        return Err(ProviderError(EMPTY_REPLY_ERROR.into()));
    }
    Ok(Reply {
        text,
        thinking: String::new(),
        tool_calls,
        usage,
    })
}

#[cfg(test)]
mod interrupted_stream_tests {
    use super::*;
    use std::io::{self, BufRead, Read};

    struct BrokenLines {
        lines: std::vec::IntoIter<Result<String, io::Error>>,
    }

    impl BrokenLines {
        fn new(lines: Vec<Result<String, io::Error>>) -> Self {
            Self {
                lines: lines.into_iter(),
            }
        }
    }

    impl Read for BrokenLines {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl BufRead for BrokenLines {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            Ok(&[])
        }

        fn consume(&mut self, _amt: usize) {}

        fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
            match self.lines.next() {
                Some(Ok(line)) => {
                    buf.push_str(&line);
                    Ok(line.len())
                }
                Some(Err(error)) => Err(error),
                None => Ok(0),
            }
        }
    }

    fn broken() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "Error while decoding chunks")
    }

    #[test]
    fn openai_partial_text_survives_a_chunk_decoder_failure() {
        let event = json!({"choices": [{"delta": {"content": "useful partial"}}]});
        let reader = BrokenLines::new(vec![Ok(format!("data: {event}\n")), Err(broken())]);
        let reply = parse_openai_sse(reader, &mut |_| {}).expect("partial reply");
        assert!(reply.text.contains("useful partial"));
        assert!(reply.text.contains("connection ended early"));
        assert!(reply.tool_calls.is_empty());
    }

    #[test]
    fn anthropic_partial_text_survives_a_chunk_decoder_failure() {
        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "useful partial"}
        });
        let reader = BrokenLines::new(vec![Ok(format!("data: {event}\n")), Err(broken())]);
        let reply = parse_anthropic_sse(reader, &mut |_| {}).expect("partial reply");
        assert!(reply.text.contains("useful partial"));
        assert!(reply.text.contains("connection ended early"));
        assert!(reply.tool_calls.is_empty());
    }

    #[test]
    fn responses_partial_text_survives_a_chunk_decoder_failure() {
        let event = json!({"type": "response.output_text.delta", "delta": "useful partial"});
        let reader = BrokenLines::new(vec![Ok(format!("data: {event}\n")), Err(broken())]);
        let reply = parse_responses_sse(reader, &mut |_| {}).expect("partial reply");
        assert!(reply.text.contains("useful partial"));
        assert!(reply.text.contains("connection ended early"));
    }

    #[test]
    fn a_decoder_failure_before_content_is_retryable() {
        let reader = BrokenLines::new(vec![Err(broken())]);
        let error = parse_openai_sse(reader, &mut |_| {}).expect_err("no partial reply");
        assert!(transient_transport(&error), "{}", error.0);
        assert!(rotatable(&error));
    }
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

    #[test]
    fn anthropic_stream_captures_thinking_separately_from_text() {
        let think = json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "let me reason: 17*23"}
        });
        let answer = json!({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "text_delta", "text": "391"}
        });
        let body = format!("data: {think}\ndata: {answer}\n");
        let reply =
            parse_anthropic_sse(std::io::BufReader::new(body.as_bytes()), &mut |_| {}).unwrap();
        assert_eq!(reply.text, "391");
        assert!(
            reply.thinking.contains("let me reason"),
            "{}",
            reply.thinking
        );
    }

    #[test]
    fn codex_stream_captures_text_when_completed_output_is_empty() {
        let delta = json!({"type": "response.output_text.delta", "delta": "Hello"});
        let done = json!({
            "type": "response.completed",
            "response": {"output": [], "usage": {"input_tokens": 23, "output_tokens": 5}}
        });
        let body = format!("data: {delta}\ndata: {done}\n");
        let mut seen = String::new();
        let reply = parse_responses_sse(std::io::BufReader::new(body.as_bytes()), &mut |t| {
            seen.push_str(t)
        })
        .unwrap();
        assert_eq!(reply.text, "Hello");
        assert_eq!(seen, "Hello");
        assert_eq!(reply.usage.output, 5);
        assert_eq!(reply.usage.input, 23);
    }

    #[test]
    fn codex_stream_captures_a_tool_call_when_completed_output_is_empty() {
        let item_done = json!({
            "type": "response.output_item.done",
            "item": {
                "id": "fc_1", "type": "function_call", "status": "completed",
                "arguments": "{\"city\":\"Paris\"}", "call_id": "call_abc", "name": "get_weather"
            }
        });
        let done = json!({
            "type": "response.completed",
            "response": {"output": [], "usage": {"input_tokens": 71, "output_tokens": 18}}
        });
        let body = format!("data: {item_done}\ndata: {done}\n");
        let reply =
            parse_responses_sse(std::io::BufReader::new(body.as_bytes()), &mut |_| {}).unwrap();
        assert_eq!(reply.tool_calls.len(), 1, "the tool call must survive");
        assert_eq!(reply.tool_calls[0].name, "get_weather");
        assert_eq!(reply.tool_calls[0].id, "call_abc");
        assert_eq!(reply.tool_calls[0].args["city"], "Paris");
    }

    #[test]
    fn codex_stream_truly_empty_is_an_empty_reply_error() {
        let done = json!({"type": "response.completed", "response": {"output": [], "usage": {}}});
        let body = format!("data: {done}\n");
        let err =
            parse_responses_sse(std::io::BufReader::new(body.as_bytes()), &mut |_| {}).unwrap_err();
        assert!(is_empty_reply(&err), "{}", err.0);
    }

    #[test]
    fn codex_stream_reports_an_incomplete_reason() {
        let inc = json!({
            "type": "response.incomplete",
            "response": {"output": [], "incomplete_details": {"reason": "max_output_tokens"}}
        });
        let body = format!("data: {inc}\n");
        let err =
            parse_responses_sse(std::io::BufReader::new(body.as_bytes()), &mut |_| {}).unwrap_err();
        assert!(err.0.contains("max_output_tokens"), "{}", err.0);
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
    fn an_empty_api_key_never_shadows_a_configured_key_ring() {
        let cfg = Config {
            provider: "openrouter".into(),
            api_key: String::new(),
            api_keys: vec!["ring-1".into(), "ring-2".into()],
            ..Config::default()
        };
        assert_eq!(key_ring(&cfg), vec!["ring-1", "ring-2"]);
    }

    #[test]
    fn a_rejected_credential_is_rotatable_like_a_busy_one() {
        assert!(is_auth_error(&ProviderError("HTTP 401: nope".into())));
        assert!(is_auth_error(&ProviderError("HTTP 403: nope".into())));
        assert!(!is_auth_error(&ProviderError("HTTP 429: slow".into())));
    }

    #[test]
    fn key_ring_order_and_dedup() {
        let mut cfg = Config {
            provider: "nvidia".into(),
            ..Config::default()
        };
        assert_eq!(key_ring(&cfg), vec![String::new()]);
        cfg.api_key = "k1".into();
        cfg.api_keys = vec!["k2".into(), "k1".into(), "k3".into()];
        assert_eq!(key_ring(&cfg), vec!["k1", "k2", "k3"]);
    }

    #[test]
    fn has_credential_counts_api_keys_and_ollama() {
        let mut cfg = cfg_with("openai", "");
        cfg.api_key = String::new();
        cfg.api_keys = Vec::new();
        let ol = Config {
            provider: "ollama".into(),
            ..cfg.clone()
        };
        assert!(has_credential(&ol));
        let mut keyed = cfg.clone();
        keyed.api_key = "sk-test".into();
        assert!(has_credential(&keyed));
        let mut ring = cfg.clone();
        ring.api_keys = vec![String::new(), "sk-ring".into()];
        assert!(has_credential(&ring));
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
        cfg.thinking = "max".into();
        let p = anthropic_payload(&cfg, "sys", &[], &[]);
        let budget = p["thinking"]["budget_tokens"].as_u64().unwrap();
        let max_tokens = p["max_tokens"].as_u64().unwrap();
        assert!(
            max_tokens.saturating_sub(budget) >= 8192,
            "want >=8192 output room, got budget {budget} / max_tokens {max_tokens}"
        );
        assert!(max_tokens <= 64000);
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
    fn thinking_levels_match_what_each_dialect_actually_sends() {
        let mut cfg = cfg_with("anthropic", "");
        cfg.model = "claude-sonnet-5".into();
        let levels = thinking_levels_for(&cfg);
        assert!(levels.contains(&"max"), "{levels:?}");
        assert!(levels.contains(&"xhigh"), "{levels:?}");

        let mut cfg = cfg_with("openai", "");
        cfg.model = "gpt-5.4".into();
        let levels = thinking_levels_for(&cfg);
        assert!(levels.contains(&"high"), "{levels:?}");
        assert!(
            !levels.contains(&"xhigh"),
            "openai reasoning caps at high, xhigh would mislead: {levels:?}"
        );

        cfg.model = "llama3.3".into();
        assert_eq!(
            thinking_levels_for(&cfg),
            vec!["default", "off"],
            "a model with no thinking control offers only default and off"
        );

        let mut cfg = cfg_with("meta", "");
        cfg.model = "llama-4".into();
        assert_eq!(thinking_levels_for(&cfg), vec!["default", "off"]);
    }

    #[test]
    fn capabilities_keep_adapter_differences_explicit() {
        let mut anthropic = cfg_with("anthropic", "");
        anthropic.model = "claude-opus-5".into();
        let anthropic_capabilities = capabilities(&anthropic);
        assert_eq!(anthropic_capabilities.wire_format, "anthropic-messages");
        assert_eq!(anthropic_capabilities.images, Support::Supported);
        assert_eq!(anthropic_capabilities.reasoning_control, Support::Supported);
        assert_eq!(
            anthropic_capabilities.authentication,
            vec![AuthMode::ApiKey, AuthMode::OAuth]
        );

        let custom = cfg_with("custom", "https://example.invalid/v1");
        let custom_capabilities = capabilities(&custom);
        assert_eq!(custom_capabilities.images, Support::AdapterDependent);
        assert_eq!(
            custom_capabilities.reasoning_control,
            Support::AdapterDependent
        );
        assert_eq!(custom_capabilities.authentication, vec![AuthMode::ApiKey]);

        let local = cfg_with("ollama", "");
        assert_eq!(capabilities(&local).authentication, vec![AuthMode::None]);
    }

    #[test]
    fn backend_capabilities_expose_quota_mode_without_claiming_universal_reporting() {
        let mut google = cfg_with("google", "");
        assert_eq!(
            capabilities(&google).quota,
            crate::usage::QuotaCapability::Subscription
        );
        google.api_key = "key".into();
        assert_eq!(
            capabilities(&google).quota,
            crate::usage::QuotaCapability::Reactive
        );
        let local = cfg_with("ollama", "");
        assert_eq!(
            capabilities(&local).quota,
            crate::usage::QuotaCapability::Local
        );
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
    fn an_overload_stream_event_keeps_its_machine_readable_type() {
        let payload = json!({
            "type": "overloaded_error",
            "message": "Our servers are currently overloaded. Please try again later."
        });
        let flat = stream_error_message(&payload);
        assert!(flat.contains("overloaded_error"), "got: {flat}");
        assert!(
            rotatable(&ProviderError(flat.clone())),
            "an overload must be retryable, got: {flat}"
        );
    }

    #[test]
    fn a_typeless_stream_error_still_reads_cleanly() {
        let payload = json!({"message": "something broke"});
        assert_eq!(stream_error_message(&payload), "something broke");
        let bare = json!("plain string error");
        assert_eq!(stream_error_message(&bare), "plain string error");
    }

    #[test]
    fn a_permanent_stream_error_type_stays_permanent() {
        let payload = json!({
            "type": "authentication_error",
            "message": "invalid x-api-key"
        });
        let flat = stream_error_message(&payload);
        assert!(
            !rotatable(&ProviderError(flat.clone())),
            "auth errors must not be retried: {flat}"
        );
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
    fn rejected_credentials_have_stable_secret_scoped_health_ids() {
        let cfg = Config {
            provider: "anthropic".into(),
            api_key: "bad".into(),
            api_keys: vec!["good".into()],
            ..Config::default()
        };
        let state = test_state("stable-auth-health");
        let _ = rotate_post_in(&state, &cfg, &json!({}), |key, _, _| -> Result<(), _> {
            Err(ProviderError(format!("HTTP 401: {key}")))
        });
        assert!(state.cooling(&cooldown_key(&cfg, "bad")).is_some());
        assert!(state.cooling(&cooldown_key(&cfg, "good")).is_some());
        let swapped = Config {
            api_key: "good".into(),
            api_keys: vec!["bad".into()],
            ..cfg.clone()
        };
        assert_eq!(cooldown_key(&cfg, "bad"), cooldown_key(&swapped, "bad"));
        assert_ne!(cooldown_key(&cfg, "bad"), cooldown_key(&cfg, "good"));
    }

    #[test]
    fn rotate_post_advances_past_a_rejected_or_busy_credential() {
        let cfg = Config {
            api_key: "bad".into(),
            api_keys: vec!["good".into()],
            ..Config::default()
        };
        let payload = json!({});

        let st = test_state("rotate");
        let out = rotate_post_in(&st, &cfg, &payload, |key, _, retries| {
            assert_eq!(
                retries, cfg.max_retries,
                "each distinct credential must receive bounded transient retries before rotation"
            );
            if key == "bad" {
                Err(ProviderError("HTTP 429: slow down".into()))
            } else {
                Ok(key.to_string())
            }
        })
        .unwrap();
        assert_eq!(out, "good");

        let st = test_state("rotate401");
        let out = rotate_post_in(&st, &cfg, &payload, |key, _, _| {
            if key == "bad" {
                Err(ProviderError("HTTP 401: not accepted".into()))
            } else {
                Ok(key.to_string())
            }
        })
        .unwrap();
        assert_eq!(out, "good", "a rejected credential must not end the turn");

        let st = test_state("rotate401all");
        let err = rotate_post_in(&st, &cfg, &payload, |key, _, _| -> Result<String, _> {
            Err(ProviderError(format!("HTTP 401: {key}")))
        })
        .unwrap_err();
        assert!(err.0.contains("401"), "{}", err.0);

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
        let iso = std::env::temp_dir().join(format!("phx-oauth-iso-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&iso);
        std::env::set_var("PHOENIX_STATE_DIR", &iso);
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
    fn anthropic_groups_tool_results_into_one_immediate_user_turn() {
        let history = vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "a".into(),
                        name: "shell".into(),
                        args: json!({}),
                    },
                    ToolCall {
                        id: "b".into(),
                        name: "read_file".into(),
                        args: json!({}),
                    },
                ],
            },
            Msg::Tool {
                id: "a".into(),
                content: "one".into(),
            },
            Msg::Tool {
                id: "b".into(),
                content: "two".into(),
            },
        ];
        let payload = anthropic_payload(&Config::default(), "s", &history, &[]);
        let messages = payload["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 2);
        let results = messages[1]["content"].as_array().expect("results");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "a");
        assert_eq!(results[1]["tool_use_id"], "b");
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

#[cfg(test)]
mod paulus_repro {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_exact_reported_overload_is_now_retryable_end_to_end() {
        let sse = json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Our servers are currently overloaded. Please try again later."
            }
        });
        let flat = format!("stream error: {}", stream_error_message(&sse["error"]));
        let as_turn = format!("provider error: {flat}");
        assert!(crate::colab::turn_failed(&as_turn));
        assert!(
            rotatable(&ProviderError(flat.clone())),
            "the reported overload must classify as transient: {flat}"
        );
        assert!(!is_auth_error(&ProviderError(flat)));
    }
}
