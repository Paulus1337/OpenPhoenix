use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::httpd::{self, Req, Resp};
use crate::util;

pub struct Mock {
    pub server: httpd::Server,
    pub log: Arc<Mutex<Vec<Value>>>,
}

impl Mock {
    pub fn port(&self) -> u16 {
        self.server.port
    }

    pub fn log_text(&self) -> String {
        self.log
            .lock()
            .map(|v| Value::Array(v.clone()).to_string())
            .unwrap_or_default()
    }

    pub fn bodies_text(&self) -> String {
        self.log
            .lock()
            .map(|v| {
                Value::Array(
                    v.iter()
                        .map(|e| e.get("body").cloned().unwrap_or(Value::Null))
                        .collect(),
                )
                .to_string()
            })
            .unwrap_or_default()
    }

    pub fn count(&self, f: impl Fn(&Value) -> bool) -> usize {
        self.log
            .lock()
            .map(|v| v.iter().filter(|e| f(e)).count())
            .unwrap_or(0)
    }
}

#[derive(Default, Clone)]
pub struct ProviderOpts {
    pub fail_status: u16,
    pub fail_once: u16,
    pub fail_model: String,
    pub tool_call: String,
}

pub fn provider(opts: ProviderOpts) -> Result<Mock, String> {
    let log: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let state: Arc<Mutex<(u32, bool)>> = Arc::new(Mutex::new((0, false)));
    let handler = move |req: &Req| -> Resp {
        if req.method != "POST" {
            if req.path == "/health" {
                return Resp::json(200, &json!({"ok": true}));
            }
            return Resp::json(404, &json!({"error": "not found"}));
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_else(|_| json!({}));
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut headers = serde_json::Map::new();
        for (k, v) in &req.headers {
            headers.insert(k.to_ascii_lowercase(), Value::String(v.clone()));
        }
        if let Ok(mut l) = log2.lock() {
            l.push(json!({
                "path": req.path,
                "method": "POST",
                "headers": headers,
                "model": model,
                "body": body,
            }));
        }
        let (n, tool_done) = match state.lock() {
            Ok(mut s) => {
                s.0 += 1;
                (s.0, s.1)
            }
            Err(_) => return Resp::json(500, &json!({"error": "state poisoned"})),
        };
        if opts.fail_status != 0 {
            return Resp::json(
                opts.fail_status,
                &json!({"error": {"message": "mock failure"}}),
            );
        }
        if opts.fail_once != 0 && n == 1 {
            return Resp::json(
                opts.fail_once,
                &json!({"error": {"message": "mock transient failure"}}),
            );
        }
        if !opts.fail_model.is_empty() && model == opts.fail_model {
            return Resp::json(429, &json!({"error": {"message": "mock rate limit"}}));
        }
        let text = format!("mock reply model={model}");
        if req.path.ends_with("/messages") {
            return Resp::json(
                200,
                &json!({
                    "content": [{"type": "text", "text": text}],
                    "usage": {"input_tokens": 1, "output_tokens": 1},
                }),
            );
        }
        if req.path.ends_with("/responses") {
            return Resp::json(
                200,
                &json!({
                    "output": [{"type": "message", "content": [{"type": "output_text", "text": text}]}],
                    "usage": {"input_tokens": 1, "output_tokens": 1},
                }),
            );
        }
        if !opts.tool_call.is_empty() && !tool_done {
            if let Ok(mut s) = state.lock() {
                s.1 = true;
            }
            let (name, args) = opts
                .tool_call
                .split_once(':')
                .unwrap_or((opts.tool_call.as_str(), "{}"));
            let args = if args.is_empty() { "{}" } else { args };
            return Resp::json(
                200,
                &json!({
                    "choices": [{"message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{"id": "call_1", "type": "function",
                                        "function": {"name": name, "arguments": args}}],
                    }}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                }),
            );
        }
        Resp::json(
            200,
            &json!({
                "choices": [{"message": {"role": "assistant", "content": text}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1},
            }),
        )
    };
    let server = httpd::start(Arc::new(handler))?;
    Ok(Mock { server, log })
}

pub fn telegram(chat: i64, text: &str, thread: Option<i64>) -> Result<Mock, String> {
    let log: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let served = Arc::new(Mutex::new(false));
    let text = text.to_string();
    let handler = move |req: &Req| -> Resp {
        let raw = String::from_utf8_lossy(&req.body).into_owned();
        let params = util::form_params(&raw);
        let method = req.path.rsplit('/').next().unwrap_or("").to_string();
        if let Ok(mut l) = log2.lock() {
            l.push(json!({"method": method, "path": req.path, "params": params}));
        }
        match method.as_str() {
            "getMe" => Resp::json(
                200,
                &json!({"ok": true, "result": {"id": 9, "is_bot": true, "username": "e2ebot"}}),
            ),
            "getUpdates" => {
                let first = match served.lock() {
                    Ok(mut s) => {
                        let was = *s;
                        *s = true;
                        !was
                    }
                    Err(_) => false,
                };
                if first {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let mut message = json!({
                        "message_id": 1,
                        "from": {"id": chat, "is_bot": false, "first_name": "E2E"},
                        "chat": {"id": chat, "type": "private"},
                        "date": now,
                        "text": text,
                    });
                    if let Some(tid) = thread {
                        message["chat"]["type"] = json!("supergroup");
                        message["chat"]["title"] = json!("e2e nest");
                        message["message_thread_id"] = json!(tid);
                        message["is_topic_message"] = json!(true);
                    }
                    let upd = json!({"update_id": 7, "message": message});
                    Resp::json(200, &json!({"ok": true, "result": [upd]}))
                } else {
                    std::thread::sleep(Duration::from_millis(300));
                    Resp::json(200, &json!({"ok": true, "result": []}))
                }
            }
            "sendMessage" => Resp::json(200, &json!({"ok": true, "result": {"message_id": 2}})),
            _ => Resp::json(200, &json!({"ok": true, "result": true})),
        }
    };
    let server = httpd::start(Arc::new(handler))?;
    Ok(Mock { server, log })
}

pub fn release(dir: PathBuf) -> Result<Mock, String> {
    let log: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let port_cell = Arc::new(AtomicU16::new(0));
    let port_cell2 = port_cell.clone();
    let handler = move |req: &Req| -> Resp {
        let port = port_cell2.load(Ordering::SeqCst);
        if req.path.contains("releases/latest") {
            let base = format!("http://127.0.0.1:{port}");
            let names = [
                "SHA256SUMS",
                "phoenix-linux-x86_64",
                "phoenix-linux-x86_64.sig",
            ];
            let assets: Vec<Value> = names
                .iter()
                .filter(|n| dir.join(n).is_file())
                .map(|n| json!({"name": n, "browser_download_url": format!("{base}/{n}")}))
                .collect();
            return Resp::json(200, &json!({"tag_name": "v9.9.9", "assets": assets}));
        }
        let name = req.path.rsplit('/').next().unwrap_or("");
        let p = dir.join(name);
        match std::fs::read(&p) {
            Ok(data) => Resp::bytes(200, "application/octet-stream", data),
            Err(_) => Resp::bytes(404, "text/plain", Vec::new()),
        }
    };
    let server = httpd::start(Arc::new(handler))?;
    port_cell.store(server.port, Ordering::SeqCst);
    Ok(Mock { server, log })
}

pub fn mcp_echo() -> Result<ExitCode, String> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let rid = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let reply = match method {
            "initialize" => {
                let proto = req
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .cloned()
                    .unwrap_or_else(|| Value::String("latest".to_string()));
                Some(json!({"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": proto,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "e2e-echo", "version": "1.0"},
                }}))
            }
            "tools/list" => Some(json!({"jsonrpc": "2.0", "id": rid, "result": {"tools": [{
                "name": "echo",
                "description": "echo the text back",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
            }]}})),
            "tools/call" => {
                let text = req
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .and_then(|a| a.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(json!({"jsonrpc": "2.0", "id": rid, "result": {
                    "content": [{"type": "text", "text": format!("mcp echo: {text}")}],
                    "isError": false,
                }}))
            }
            _ => rid
                .as_ref()
                .map(|r| json!({"jsonrpc": "2.0", "id": r, "result": {}})),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
    Ok(ExitCode::SUCCESS)
}
