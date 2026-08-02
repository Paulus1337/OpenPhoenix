use std::io::{BufRead, Write};

use serde_json::{json, Value};

pub const PROTOCOL_VERSION: u64 = 1;
pub const AGENT_NAME: &str = "openphoenix";

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Malformed(String),
}

pub fn classify(line: &str) -> Incoming {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Incoming::Malformed("not json".into());
    };
    if v.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Incoming::Malformed("jsonrpc must be \"2.0\"".into());
    }
    let Some(method) = v.get("method").and_then(Value::as_str) else {
        return Incoming::Malformed("no method".into());
    };
    let params = v.get("params").cloned().unwrap_or_else(|| json!({}));
    match v.get("id") {
        Some(id) if !id.is_null() => Incoming::Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        },
        _ => Incoming::Notification {
            method: method.to_string(),
            params,
        },
    }
}

pub fn result(id: &Value, value: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": value}).to_string()
}

pub fn error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

pub fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": true
            }
        },
        "authMethods": [],
        "_meta": {"agent": AGENT_NAME, "version": crate::VERSION}
    })
}

pub fn session_id(seed: u64) -> String {
    format!("acp-{seed:016x}")
}

pub fn prompt_text(params: &Value) -> String {
    let Some(blocks) = params.get("prompt").and_then(Value::as_array) else {
        return String::new();
    };
    let mut out = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    out.push(t.to_string());
                }
            }
            Some("resource") => {
                let uri = b
                    .get("resource")
                    .and_then(|r| r.get("uri"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = b
                    .get("resource")
                    .and_then(|r| r.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !text.is_empty() {
                    out.push(format!("<<<resource {uri}>>>\n{text}\n<<<end resource>>>"));
                } else if !uri.is_empty() {
                    out.push(format!("(resource {uri})"));
                }
            }
            Some("resource_link") => {
                if let Some(uri) = b.get("uri").and_then(Value::as_str) {
                    out.push(format!("(resource link {uri})"));
                }
            }
            _ => {}
        }
    }
    out.join("\n")
}

pub fn agent_chunk(session: &str, text: &str) -> String {
    notification(
        "session/update",
        json!({
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }
        }),
    )
}

pub fn available_commands(session: &str) -> String {
    let cmds: Vec<Value> = crate::commands::COMMANDS
        .iter()
        .filter(|c| {
            matches!(
                c.name,
                "status" | "models" | "memory" | "sessions" | "doctor"
            )
        })
        .map(|c| json!({"name": c.name, "description": c.summary, "input": null}))
        .collect();
    notification(
        "session/update",
        json!({
            "sessionId": session,
            "update": {"sessionUpdate": "available_commands_update", "availableCommands": cmds}
        }),
    )
}

pub struct Bridge {
    pub initialized: bool,
    pub sessions: Vec<String>,
    pub next_seed: u64,
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge {
            initialized: false,
            sessions: Vec::new(),
            next_seed: 1,
        }
    }
}

pub enum Reply {
    One(String),
    Stream {
        pre: Vec<String>,
        prompt: String,
        session: String,
        id: Value,
    },
    None,
}

impl Bridge {
    pub fn handle(&mut self, line: &str) -> Reply {
        match classify(line) {
            Incoming::Malformed(why) => {
                Reply::One(error(&Value::Null, -32700, &format!("parse error: {why}")))
            }
            Incoming::Notification { .. } => Reply::None,
            Incoming::Request { id, method, params } => self.request(&id, &method, &params),
        }
    }

    fn request(&mut self, id: &Value, method: &str, params: &Value) -> Reply {
        match method {
            "initialize" => {
                self.initialized = true;
                Reply::One(result(id, initialize_result()))
            }
            _ if !self.initialized => Reply::One(error(id, -32002, "initialize must come first")),
            "session/new" => {
                let sid = session_id(self.next_seed);
                self.next_seed += 1;
                self.sessions.push(sid.clone());
                Reply::Stream {
                    pre: vec![
                        result(id, json!({"sessionId": sid})),
                        available_commands(&sid),
                    ],
                    prompt: String::new(),
                    session: sid,
                    id: id.clone(),
                }
            }
            "session/load" => Reply::One(error(id, -32601, "loadSession is not supported")),
            "session/prompt" => {
                let sid = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !self.sessions.contains(&sid) {
                    return Reply::One(error(id, -32602, "unknown sessionId"));
                }
                let text = prompt_text(params);
                if text.trim().is_empty() {
                    return Reply::One(error(id, -32602, "prompt carried no text"));
                }
                Reply::Stream {
                    pre: Vec::new(),
                    prompt: text,
                    session: sid,
                    id: id.clone(),
                }
            }
            "session/cancel" => Reply::One(result(id, json!({}))),
            "authenticate" => Reply::One(result(id, json!({}))),
            other => Reply::One(error(id, -32601, &format!("method not found: {other}"))),
        }
    }
}

pub fn serve<R: BufRead, W: Write>(
    input: R,
    out: &mut W,
    run: &mut dyn FnMut(&str) -> String,
) -> Result<(), String> {
    let mut bridge = Bridge::default();
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        match bridge.handle(&line) {
            Reply::None => {}
            Reply::One(msg) => {
                writeln!(out, "{msg}").map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
            }
            Reply::Stream {
                pre,
                prompt,
                session,
                id,
            } => {
                for msg in pre {
                    writeln!(out, "{msg}").map_err(|e| e.to_string())?;
                }
                if !prompt.is_empty() {
                    let reply = run(&prompt);
                    writeln!(out, "{}", agent_chunk(&session, &reply))
                        .map_err(|e| e.to_string())?;
                    let stop = if reply.starts_with("provider error:") {
                        "refusal"
                    } else {
                        "end_turn"
                    };
                    writeln!(out, "{}", result(&id, json!({"stopReason": stop})))
                        .map_err(|e| e.to_string())?;
                }
                out.flush().map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, method: &str, params: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn a_request_a_notification_and_junk_are_told_apart() {
        assert!(matches!(
            classify(&req(1, "initialize", json!({}))),
            Incoming::Request { .. }
        ));
        assert!(matches!(
            classify(&json!({"jsonrpc": "2.0", "method": "x"}).to_string()),
            Incoming::Notification { .. }
        ));
        assert!(matches!(classify("{oops"), Incoming::Malformed(_)));
        assert!(matches!(
            classify(&json!({"jsonrpc": "1.0", "method": "x", "id": 1}).to_string()),
            Incoming::Malformed(_)
        ));
        assert!(matches!(
            classify(&json!({"jsonrpc": "2.0", "id": 1}).to_string()),
            Incoming::Malformed(_)
        ));
    }

    #[test]
    fn a_null_id_is_a_notification_not_a_request() {
        assert!(matches!(
            classify(&json!({"jsonrpc": "2.0", "id": null, "method": "x"}).to_string()),
            Incoming::Notification { .. }
        ));
    }

    #[test]
    fn nothing_works_before_initialize() {
        let mut b = Bridge::default();
        let Reply::One(msg) = b.handle(&req(1, "session/new", json!({}))) else {
            panic!("expected a single reply");
        };
        assert_eq!(parse(&msg)["error"]["code"], -32002);
    }

    #[test]
    fn initialize_advertises_the_protocol_version_and_the_agent() {
        let mut b = Bridge::default();
        let Reply::One(msg) = b.handle(&req(1, "initialize", json!({"protocolVersion": 1}))) else {
            panic!("expected a single reply");
        };
        let v = parse(&msg);
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["result"]["_meta"]["agent"], AGENT_NAME);
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn a_new_session_gets_an_id_and_the_command_list() {
        let mut b = Bridge::default();
        b.handle(&req(1, "initialize", json!({})));
        let Reply::Stream { pre, session, .. } = b.handle(&req(2, "session/new", json!({}))) else {
            panic!("expected a stream reply");
        };
        assert_eq!(pre.len(), 2);
        assert_eq!(parse(&pre[0])["result"]["sessionId"], session);
        let update = parse(&pre[1]);
        assert_eq!(update["method"], "session/update");
        assert_eq!(
            update["params"]["update"]["sessionUpdate"],
            "available_commands_update"
        );
        assert!(update["params"]["update"]["availableCommands"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false));
    }

    #[test]
    fn session_ids_are_unique_per_session() {
        let mut b = Bridge::default();
        b.handle(&req(1, "initialize", json!({})));
        b.handle(&req(2, "session/new", json!({})));
        b.handle(&req(3, "session/new", json!({})));
        assert_eq!(b.sessions.len(), 2);
        assert_ne!(b.sessions.first(), b.sessions.get(1));
    }

    #[test]
    fn prompting_an_unknown_session_is_refused() {
        let mut b = Bridge::default();
        b.handle(&req(1, "initialize", json!({})));
        let Reply::One(msg) = b.handle(&req(
            2,
            "session/prompt",
            json!({"sessionId": "nope", "prompt": [{"type": "text", "text": "hi"}]}),
        )) else {
            panic!("expected a single reply");
        };
        assert_eq!(parse(&msg)["error"]["code"], -32602);
    }

    #[test]
    fn text_blocks_and_embedded_resources_flatten_into_one_prompt() {
        let params = json!({"prompt": [
            {"type": "text", "text": "explain"},
            {"type": "resource", "resource": {"uri": "file:///a.rs", "text": "fn main() {}"}},
            {"type": "resource_link", "uri": "file:///b.rs"},
            {"type": "image", "data": "ignored"}
        ]});
        let text = prompt_text(&params);
        assert!(text.contains("explain"), "{text}");
        assert!(text.contains("fn main() {}"), "{text}");
        assert!(text.contains("file:///a.rs"), "{text}");
        assert!(text.contains("file:///b.rs"), "{text}");
        assert!(!text.contains("ignored"), "{text}");
    }

    #[test]
    fn an_empty_prompt_is_refused_rather_than_sent_to_the_model() {
        let mut b = Bridge::default();
        b.handle(&req(1, "initialize", json!({})));
        let Reply::Stream { session, .. } = b.handle(&req(2, "session/new", json!({}))) else {
            panic!("expected a stream reply");
        };
        let Reply::One(msg) = b.handle(&req(
            3,
            "session/prompt",
            json!({"sessionId": session, "prompt": []}),
        )) else {
            panic!("expected a single reply");
        };
        assert_eq!(parse(&msg)["error"]["code"], -32602);
    }

    #[test]
    fn unknown_methods_and_load_session_answer_with_method_not_found() {
        let mut b = Bridge::default();
        b.handle(&req(1, "initialize", json!({})));
        for method in ["session/load", "does/not/exist"] {
            let Reply::One(msg) = b.handle(&req(9, method, json!({}))) else {
                panic!("expected a single reply for {method}");
            };
            assert_eq!(parse(&msg)["error"]["code"], -32601, "{method}");
        }
    }

    #[test]
    fn a_full_turn_streams_a_chunk_then_an_end_turn_stop_reason() {
        let input = [
            req(1, "initialize", json!({})),
            req(2, "session/new", json!({})),
            json!({"jsonrpc": "2.0", "method": "session/cancelled"}).to_string(),
        ]
        .join("\n");
        let mut out: Vec<u8> = Vec::new();
        let mut calls = 0;
        serve(input.as_bytes(), &mut out, &mut |_p| {
            calls += 1;
            "answer".to_string()
        })
        .unwrap();
        assert_eq!(calls, 0);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 3);

        let session = parse(text.lines().nth(1).unwrap())["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        let input2 = [
            req(1, "initialize", json!({})),
            req(2, "session/new", json!({})),
            req(
                3,
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}),
            ),
        ]
        .join("\n");
        let mut out2: Vec<u8> = Vec::new();
        let mut seen = String::new();
        serve(input2.as_bytes(), &mut out2, &mut |p| {
            seen = p.to_string();
            "answer".into()
        })
        .unwrap();
        let text2 = String::from_utf8(out2).unwrap();
        assert_eq!(seen, "hi");
        let last = parse(text2.lines().last().unwrap());
        assert_eq!(last["result"]["stopReason"], "end_turn");
        assert!(text2.contains("agent_message_chunk"), "{text2}");
        assert!(text2.contains("answer"), "{text2}");
    }

    #[test]
    fn a_provider_error_ends_the_turn_with_a_refusal() {
        let input = [
            req(1, "initialize", json!({})),
            req(2, "session/new", json!({})),
        ]
        .join("\n");
        let mut probe: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut probe, &mut |_| String::new()).unwrap();
        let session = parse(
            String::from_utf8(probe)
                .unwrap()
                .lines()
                .nth(1)
                .unwrap_or("{}"),
        )["result"]["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let full = [
            req(1, "initialize", json!({})),
            req(2, "session/new", json!({})),
            req(
                3,
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}),
            ),
        ]
        .join("\n");
        let mut out: Vec<u8> = Vec::new();
        serve(full.as_bytes(), &mut out, &mut |_| {
            "provider error: no key".into()
        })
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let last = parse(text.lines().last().unwrap_or("{}"));
        assert_eq!(last["result"]["stopReason"], "refusal");
    }

    #[test]
    fn junk_lines_get_a_parse_error_and_do_not_stop_the_bridge() {
        let input = format!("{{oops\n{}", req(1, "initialize", json!({})));
        let mut out: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut out, &mut |_| String::new()).unwrap();
        let text = String::from_utf8(out).unwrap();
        let first = parse(text.lines().next().unwrap_or("{}"));
        assert_eq!(first["error"]["code"], -32700);
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn blank_lines_are_skipped_silently() {
        let mut out: Vec<u8> = Vec::new();
        serve("\n\n   \n".as_bytes(), &mut out, &mut |_| String::new()).unwrap();
        assert!(out.is_empty());
    }
}
