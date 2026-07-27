use std::fmt;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;

pub const RETRY_CODES: [u16; 6] = [408, 429, 500, 502, 503, 529];
const MAX_RETRIES: u32 = 3;
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

fn base_url_for(kind: &str) -> Option<&'static str> {
    match kind {
        "openai" => Some("https://api.openai.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "ollama" => Some("http://localhost:11434/v1"),

        "nvidia" => Some("https://integrate.api.nvidia.com/v1"),

        "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
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

fn post(
    url: &str,
    headers: &[(&str, String)],
    payload: &Value,
    retries: u32,
) -> Result<Value, ProviderError> {
    let body = payload.to_string();
    for attempt in 0..=retries {
        let mut req = ureq::post(url)
            .timeout(Duration::from_secs(180))
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
                let detail: String = resp
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(400)
                    .collect();
                if RETRY_CODES.contains(&code) && attempt < MAX_RETRIES {
                    let secs = std::cmp::min(2u64.pow(attempt) * 2, 30);
                    thread::sleep(Duration::from_secs(secs));
                    continue;
                }
                return Err(ProviderError(format!("HTTP {code}: {detail}")));
            }
            Err(e) => {
                if attempt < MAX_RETRIES {
                    thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                    continue;
                }
                return Err(ProviderError(e.to_string()));
            }
        }
    }
    Err(ProviderError("unreachable".into()))
}

pub enum Provider {
    Anthropic,
    OpenAICompat { url: String },
}

pub fn make(cfg: &Config) -> Result<Provider, ProviderError> {
    if cfg.provider == "anthropic" {
        return Ok(Provider::Anthropic);
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
    Ok(Provider::OpenAICompat {
        url: format!("{}/chat/completions", base.trim_end_matches('/')),
    })
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
    let mut payload = json!({
        "model": cfg.model, "max_tokens": 8192,
        "system": system, "messages": msgs
    });
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

fn anthropic_headers(key: &str) -> [(&'static str, String); 2] {
    [
        ("x-api-key", key.to_string()),
        ("anthropic-version", "2023-06-01".to_string()),
    ]
}

pub fn key_ring(cfg: &Config) -> Vec<String> {
    let mut ring: Vec<String> = Vec::new();
    if !cfg.api_key.is_empty() || cfg.api_keys.iter().all(String::is_empty) {
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

pub fn rotatable(err: &ProviderError) -> bool {
    let s = &err.0;
    s.starts_with("HTTP 429")
        || s.starts_with("HTTP 5")
        || s.starts_with("HTTP 408")
        || s.contains("overloaded_error")
        || s.contains("rate_limit_error")
}

fn chat_anthropic(
    cfg: &Config,
    system: &str,
    history: &[Msg],
    tools: &[Value],
) -> Result<Reply, ProviderError> {
    let payload = anthropic_payload(cfg, system, history, tools);
    let data = rotate_post(cfg, &payload, |key, body, retries| {
        post(ANTHROPIC_URL, &anthropic_headers(key), body, retries)
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

fn rotate_post<T>(
    cfg: &Config,
    payload: &Value,
    call: impl Fn(&str, &Value, u32) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let ring = key_ring(cfg);
    let last = ring.len() - 1;
    let retries = if last == 0 { MAX_RETRIES } else { 0 };
    for (i, key) in ring.iter().enumerate() {
        match call(key, payload, retries) {
            Ok(v) => return Ok(v),
            Err(e) if i < last && rotatable(&e) => {
                eprintln!(
                    "provider key {} failed ({}), rotating to key {}",
                    i + 1,
                    crate::security::redact(&e.0),
                    i + 2
                );
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("key ring is never empty")
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
        post(url, &openai_headers(key), body, retries)
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
            Provider::Anthropic => chat_anthropic(cfg, system, history, tools),
            Provider::OpenAICompat { url } => chat_openai(url, cfg, system, history, tools),
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
            Provider::Anthropic => {
                let mut payload = anthropic_payload(cfg, system, history, tools);
                payload["stream"] = Value::Bool(true);
                let reader = rotate_post(cfg, &payload, |key, body, retries| {
                    post_stream(ANTHROPIC_URL, &anthropic_headers(key), body, retries)
                })?;
                parse_anthropic_sse(std::io::BufReader::new(reader), on_text)
            }
            Provider::OpenAICompat { url } => {
                let mut payload = openai_payload(cfg, system, history, tools);
                payload["stream"] = Value::Bool(true);
                let reader = rotate_post(cfg, &payload, |key, body, retries| {
                    post_stream(url, &openai_headers(key), body, retries)
                })?;
                parse_openai_sse(std::io::BufReader::new(reader), on_text)
            }
        }
    }
}

fn post_stream(
    url: &str,
    headers: &[(&str, String)],
    payload: &Value,
    retries: u32,
) -> Result<Box<dyn std::io::Read + Send>, ProviderError> {
    let body = payload.to_string();
    for attempt in 0..=retries {
        let mut req = ureq::post(url)
            .timeout(Duration::from_secs(300))
            .set("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_string(&body) {
            Ok(resp) => return Ok(Box::new(resp.into_reader())),
            Err(ureq::Error::Status(code, resp)) => {
                let detail: String = resp
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(400)
                    .collect();
                if RETRY_CODES.contains(&code) && attempt < MAX_RETRIES {
                    let secs = std::cmp::min(2u64.pow(attempt) * 2, 30);
                    thread::sleep(Duration::from_secs(secs));
                    continue;
                }
                return Err(ProviderError(format!("HTTP {code}: {detail}")));
            }
            Err(e) => {
                if attempt < MAX_RETRIES {
                    thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                    continue;
                }
                return Err(ProviderError(e.to_string()));
            }
        }
    }
    Err(ProviderError("unreachable".into()))
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
        let Some(data) = line.strip_prefix("data: ") else {
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
    let tool_calls = blocks
        .into_values()
        .map(|(id, name, args)| ToolCall {
            id,
            name,
            args: serde_json::from_str(&args).unwrap_or_else(|_| json!({})),
        })
        .collect();
    Ok(Reply {
        text,
        tool_calls,
        usage,
    })
}

fn parse_openai_sse(
    reader: impl std::io::BufRead,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, ProviderError> {
    use std::collections::BTreeMap;
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut acc: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for line in reader.lines() {
        let line = line.map_err(|e| ProviderError(e.to_string()))?;
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
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
                    e.1.push_str(n);
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    e.2.push_str(a);
                }
            }
        }
    }
    let mut auto_id = 0usize;
    let tool_calls = acc
        .into_values()
        .map(|(id, name, args)| {
            let id = if id.is_empty() {
                auto_id += 1;
                format!("call_{}", auto_id - 1)
            } else {
                id
            };
            ToolCall {
                id,
                name,
                args: serde_json::from_str(&args).unwrap_or_else(|_| json!({})),
            }
        })
        .collect();
    Ok(Reply {
        text,
        tool_calls,
        usage,
    })
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
    fn key_ring_order_and_dedup() {
        let mut cfg = Config::default();
        assert_eq!(key_ring(&cfg), vec![String::new()]);
        cfg.api_key = "k1".into();
        cfg.api_keys = vec!["k2".into(), "k1".into(), "k3".into()];
        assert_eq!(key_ring(&cfg), vec!["k1", "k2", "k3"]);
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
    fn rotate_post_advances_on_rate_limit_only() {
        let cfg = Config {
            api_key: "bad".into(),
            api_keys: vec!["good".into()],
            ..Config::default()
        };
        let payload = json!({});

        let out = rotate_post(&cfg, &payload, |key, _, _| {
            if key == "bad" {
                Err(ProviderError("HTTP 429: slow down".into()))
            } else {
                Ok(key.to_string())
            }
        })
        .unwrap();
        assert_eq!(out, "good");

        let err = rotate_post(&cfg, &payload, |key, _, _| -> Result<String, _> {
            Err(ProviderError(format!("HTTP 401: {key}")))
        })
        .unwrap_err();
        assert_eq!(err.0, "HTTP 401: bad");

        let err = rotate_post(&cfg, &payload, |key, _, _| -> Result<String, _> {
            Err(ProviderError(format!("HTTP 429: {key}")))
        })
        .unwrap_err();
        assert_eq!(err.0, "HTTP 429: good");
    }

    #[test]
    fn make_selects_backend() {
        assert!(matches!(
            make(&cfg_with("anthropic", "")).unwrap(),
            Provider::Anthropic
        ));
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
