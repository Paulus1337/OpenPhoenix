#[cfg(test)]
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const DEFAULT_TIMEOUT_MS: u64 = 20_000;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerCfg {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
    pub enabled: bool,
    pub timeout_ms: u64,
}

impl Default for ServerCfg {
    fn default() -> Self {
        ServerCfg {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: String::new(),
            enabled: true,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub server: String,
    pub name: String,
    pub description: String,
    pub schema: Value,
}

impl Tool {
    pub fn exposed_name(&self) -> String {
        format!("mcp_{}_{}", sanitize(&self.server), sanitize(&self.name))
    }
}

pub fn sanitize(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(48);
    out
}

pub fn split_exposed<'a>(exposed: &'a str, tools: &'a [Tool]) -> Option<&'a Tool> {
    tools.iter().find(|t| t.exposed_name() == exposed)
}

pub fn encode_request(id: u64, method: &str, params: Option<&Value>) -> String {
    let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if let Some(p) = params {
        msg["params"] = p.clone();
    }
    msg.to_string()
}

pub fn encode_notification(method: &str, params: Option<&Value>) -> String {
    let mut msg = json!({"jsonrpc": "2.0", "method": method});
    if let Some(p) = params {
        msg["params"] = p.clone();
    }
    msg.to_string()
}

pub fn is_response_for(line: &Value, id: u64) -> bool {
    line.get("id").and_then(Value::as_u64) == Some(id)
}

pub fn parse_result(line: &Value) -> Result<Value, String> {
    if let Some(err) = line.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("server error {code}: {msg}"));
    }
    match line.get("result") {
        Some(r) => Ok(r.clone()),
        None => Err("response carried neither result nor error".into()),
    }
}

pub fn parse_tools(server: &str, result: &Value) -> Vec<Tool> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(Value::as_str)?;
                    if name.is_empty() {
                        return None;
                    }
                    Some(Tool {
                        server: server.to_string(),
                        name: name.to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object"})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_tool_result(result: &Value) -> (String, bool) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut out = String::new();
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(t);
                    }
                }
                Some(other) => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format!("[{other} content omitted]"));
                }
                None => {}
            }
        }
    }
    if out.is_empty() {
        out = "(no content)".to_string();
    }
    (out, is_error)
}

pub fn is_secret_var(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.ends_with("_API_KEY")
        || n.ends_with("_TOKEN")
        || n.ends_with("_SECRET")
        || n.ends_with("_PASSWORD")
        || n == "PHOENIX_API_KEY"
}

pub struct Server {
    name: String,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    timeout: Duration,
    pub server_info: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    pub fn start(cfg: &ServerCfg) -> Result<Server, String> {
        if cfg.command.trim().is_empty() {
            return Err(format!("mcp server '{}' has no command", cfg.name));
        }
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, _) in std::env::vars() {
            if is_secret_var(&k) {
                cmd.env_remove(&k);
            }
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        if !cfg.cwd.is_empty() {
            cmd.current_dir(&cfg.cwd);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start mcp server '{}': {e}", cfg.name))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "child stdin was not piped".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "child stdout was not piped".to_string())?;
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.len() > MAX_LINE_BYTES {
                            break;
                        }
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Server {
            name: cfg.name.clone(),
            child,
            stdin,
            lines: rx,
            next_id: 0,
            timeout: Duration::from_millis(cfg.timeout_ms.max(1)),
            server_info: String::new(),
        })
    }

    fn send(&mut self, payload: &str) -> Result<(), String> {
        self.stdin
            .write_all(payload.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("mcp server '{}' closed its input: {e}", self.name))
    }

    fn request(&mut self, method: &str, params: Option<&Value>) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&encode_request(id, method, params))?;
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(format!(
                    "mcp server '{}' did not answer {method} within {:?}",
                    self.name, self.timeout
                ));
            }
            let line = self.lines.recv_timeout(left).map_err(|_| {
                format!(
                    "mcp server '{}' did not answer {method} within {:?}",
                    self.name, self.timeout
                )
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if is_response_for(&v, id) {
                return parse_result(&v);
            }
        }
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "openphoenix", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = self.request("initialize", Some(&params))?;
        let name = result
            .get("serverInfo")
            .and_then(|s| s.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let version = result
            .get("serverInfo")
            .and_then(|s| s.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("");
        self.server_info = format!("{name} {version}").trim().to_string();
        self.send(&encode_notification("notifications/initialized", None))?;
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<Tool>, String> {
        let result = self.request("tools/list", None)?;
        let name = self.name.clone();
        Ok(parse_tools(&name, &result))
    }

    pub fn call_tool(&mut self, tool: &str, args: &Value) -> Result<(String, bool), String> {
        let params = json!({"name": tool, "arguments": args});
        let result = self.request("tools/call", Some(&params))?;
        Ok(parse_tool_result(&result))
    }
}

pub fn connect_all(servers: &[ServerCfg]) -> (Vec<(String, Server)>, Vec<Tool>, Vec<String>) {
    let mut live = Vec::new();
    let mut tools = Vec::new();
    let mut problems = Vec::new();
    for cfg in servers.iter().filter(|s| s.enabled) {
        match Server::start(cfg).and_then(|mut s| {
            s.initialize()?;
            let t = s.list_tools()?;
            Ok((s, t))
        }) {
            Ok((s, t)) => {
                tools.extend(t);
                live.push((cfg.name.clone(), s));
            }
            Err(e) => problems.push(e),
        }
    }
    (live, tools, problems)
}

#[cfg(test)]
pub fn to_function_specs(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.exposed_name(),
                    "description": if t.description.is_empty() {
                        format!("{} tool from mcp server {}", t.name, t.server)
                    } else {
                        t.description.clone()
                    },
                    "parameters": t.schema,
                }
            })
        })
        .collect()
}

pub fn from_toml(root: &toml::Value) -> Vec<ServerCfg> {
    let Some(servers) = root
        .get("mcp")
        .and_then(|m| m.get("servers"))
        .and_then(toml::Value::as_table)
    else {
        return Vec::new();
    };
    let mut out: Vec<ServerCfg> = servers
        .iter()
        .map(|(name, v)| {
            let t = v.as_table();
            let s = |key: &str| {
                t.and_then(|t| t.get(key))
                    .and_then(toml::Value::as_str)
                    .map(crate::config::expand_env)
                    .unwrap_or_default()
            };
            let args = t
                .and_then(|t| t.get("args"))
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(crate::config::expand_env))
                        .collect()
                })
                .unwrap_or_default();
            let env: Vec<(String, String)> = t
                .and_then(|t| t.get("env"))
                .and_then(toml::Value::as_table)
                .map(|e| {
                    e.iter()
                        .map(|(k, v)| {
                            let raw = match v {
                                toml::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            (k.clone(), crate::config::expand_env(&raw))
                        })
                        .collect()
                })
                .unwrap_or_default();
            ServerCfg {
                name: name.clone(),
                command: s("command"),
                args,
                env,
                cwd: s("cwd"),
                enabled: t
                    .and_then(|t| t.get("enabled"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true),
                timeout_ms: t
                    .and_then(|t| t.get("timeout_ms"))
                    .and_then(toml::Value::as_integer)
                    .filter(|v| *v > 0)
                    .map(|v| v as u64)
                    .unwrap_or(DEFAULT_TIMEOUT_MS),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn summary(servers: &[ServerCfg]) -> String {
    if servers.is_empty() {
        return "no mcp servers configured; add [mcp.servers.NAME] to config.toml\n".to_string();
    }
    let on = servers.iter().filter(|s| s.enabled).count();
    let mut out = format!("{} mcp servers, {on} enabled\n", servers.len());
    for s in servers {
        let mark = if s.enabled { "on " } else { "off" };
        let argline = if s.args.is_empty() {
            String::new()
        } else {
            format!(" {}", s.args.join(" "))
        };
        out.push_str(&format!("  {mark}  {:<16}{}{argline}\n", s.name, s.command));
    }
    out
}

#[cfg(test)]
pub fn env_map(cfg: &ServerCfg) -> BTreeMap<String, String> {
    cfg.env.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_carries_jsonrpc_id_and_method() {
        let raw = encode_request(7, "tools/list", None);
        let v: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/list");
        assert!(v.get("params").is_none(), "absent params must stay absent");
    }

    #[test]
    fn a_notification_has_no_id_because_no_reply_is_coming() {
        let raw = encode_notification("notifications/initialized", None);
        let v: Value = serde_json::from_str(&raw).expect("json");
        assert!(v.get("id").is_none());
        assert_eq!(v["method"], "notifications/initialized");
    }

    #[test]
    fn only_a_matching_id_counts_as_our_response() {
        let mine = json!({"jsonrpc": "2.0", "id": 3, "result": {}});
        let theirs = json!({"jsonrpc": "2.0", "id": 4, "result": {}});
        let note = json!({"jsonrpc": "2.0", "method": "notifications/message"});
        assert!(is_response_for(&mine, 3));
        assert!(!is_response_for(&theirs, 3));
        assert!(
            !is_response_for(&note, 3),
            "a server notification must never be mistaken for our answer"
        );
    }

    #[test]
    fn an_error_response_becomes_an_error_not_an_empty_result() {
        let v = json!({"jsonrpc": "2.0", "id": 1,
            "error": {"code": -32601, "message": "method not found"}});
        let e = parse_result(&v).expect_err("must be an error");
        assert!(e.contains("-32601"), "{e}");
        assert!(e.contains("method not found"), "{e}");
    }

    #[test]
    fn a_response_with_neither_result_nor_error_is_refused() {
        assert!(parse_result(&json!({"jsonrpc": "2.0", "id": 1})).is_err());
    }

    #[test]
    fn tools_are_parsed_and_nameless_entries_dropped() {
        let result = json!({"tools": [
            {"name": "search", "description": "find things",
             "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}},
            {"description": "no name so unusable"},
            {"name": ""}
        ]});
        let tools = parse_tools("files", &result);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[0].server, "files");
        assert_eq!(tools[0].schema["properties"]["q"]["type"], "string");
    }

    #[test]
    fn a_tool_without_a_schema_still_gets_a_valid_object_schema() {
        let tools = parse_tools("s", &json!({"tools": [{"name": "ping"}]}));
        assert_eq!(tools[0].schema, json!({"type": "object"}));
    }

    #[test]
    fn exposed_names_are_namespaced_so_two_servers_can_share_a_tool_name() {
        let a = Tool {
            server: "files".into(),
            name: "search".into(),
            description: String::new(),
            schema: json!({}),
        };
        let b = Tool {
            server: "web".into(),
            name: "search".into(),
            description: String::new(),
            schema: json!({}),
        };
        assert_ne!(a.exposed_name(), b.exposed_name());
        assert_eq!(a.exposed_name(), "mcp_files_search");
    }

    #[test]
    fn names_providers_would_reject_are_sanitized() {
        let t = Tool {
            server: "my server!".into(),
            name: "read/file".into(),
            description: String::new(),
            schema: json!({}),
        };
        let exposed = t.exposed_name();
        assert!(
            exposed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{exposed}"
        );
    }

    #[test]
    fn a_tool_result_flattens_text_and_reports_the_error_flag() {
        let (text, err) = parse_tool_result(&json!({
            "content": [{"type": "text", "text": "line one"},
                        {"type": "text", "text": "line two"}]}));
        assert_eq!(text, "line one\nline two");
        assert!(!err);
        let (_, err2) = parse_tool_result(&json!({"content": [], "isError": true}));
        assert!(err2);
    }

    #[test]
    fn non_text_content_is_named_rather_than_silently_dropped() {
        let (text, _) = parse_tool_result(&json!({"content": [{"type": "image"}]}));
        assert!(text.contains("image"), "{text}");
    }

    #[test]
    fn an_empty_result_says_so_instead_of_returning_nothing() {
        let (text, _) = parse_tool_result(&json!({"content": []}));
        assert_eq!(text, "(no content)");
    }

    #[test]
    fn secret_shaped_variables_are_recognised_by_shape_not_by_a_fixed_list() {
        assert!(is_secret_var("ANTHROPIC_API_KEY"));
        assert!(is_secret_var("SOME_FUTURE_PROVIDER_API_KEY"));
        assert!(is_secret_var("GITHUB_TOKEN"));
        assert!(is_secret_var("db_password"));
        assert!(!is_secret_var("PATH"));
        assert!(!is_secret_var("HOME"));
    }

    #[test]
    fn config_parses_servers_with_args_env_and_defaults() {
        let raw = r#"
[mcp.servers.files]
command = "mcp-files"
args = ["--root", "/tmp"]
env = { LOG = "debug" }

[mcp.servers.off_one]
command = "x"
enabled = false
timeout_ms = 500
"#;
        let root: toml::Value = toml::from_str(raw).expect("toml");
        let servers = from_toml(&root);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "files");
        assert_eq!(servers[0].command, "mcp-files");
        assert_eq!(servers[0].args, vec!["--root", "/tmp"]);
        assert_eq!(
            env_map(&servers[0]).get("LOG").map(String::as_str),
            Some("debug")
        );
        assert!(servers[0].enabled);
        assert_eq!(servers[0].timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(!servers[1].enabled);
        assert_eq!(servers[1].timeout_ms, 500);
    }

    #[test]
    fn no_mcp_table_means_no_servers_rather_than_an_error() {
        let root: toml::Value = toml::from_str("[provider]\nkind = \"nvidia\"\n").expect("toml");
        assert!(from_toml(&root).is_empty());
    }

    #[test]
    fn function_specs_carry_the_schema_the_server_declared() {
        let tools = vec![Tool {
            server: "files".into(),
            name: "read".into(),
            description: "read a file".into(),
            schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let specs = to_function_specs(&tools);
        assert_eq!(specs[0]["type"], "function");
        assert_eq!(specs[0]["function"]["name"], "mcp_files_read");
        assert_eq!(specs[0]["function"]["description"], "read a file");
        assert_eq!(
            specs[0]["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[test]
    fn a_tool_without_a_description_still_gets_one() {
        let tools = vec![Tool {
            server: "s".into(),
            name: "t".into(),
            description: String::new(),
            schema: json!({}),
        }];
        let specs = to_function_specs(&tools);
        assert!(!specs[0]["function"]["description"]
            .as_str()
            .unwrap_or("")
            .is_empty());
    }

    #[test]
    fn an_exposed_name_maps_back_to_its_tool() {
        let tools = parse_tools("files", &json!({"tools": [{"name": "read"}]}));
        assert!(split_exposed("mcp_files_read", &tools).is_some());
        assert!(split_exposed("mcp_files_missing", &tools).is_none());
    }

    #[test]
    fn a_server_with_no_command_is_refused_before_spawning() {
        let cfg = ServerCfg {
            name: "broken".into(),
            ..ServerCfg::default()
        };
        assert!(Server::start(&cfg).is_err());
    }

    #[test]
    fn summary_names_every_server_and_its_state() {
        let servers = vec![
            ServerCfg {
                name: "files".into(),
                command: "mcp-files".into(),
                ..ServerCfg::default()
            },
            ServerCfg {
                name: "web".into(),
                command: "mcp-web".into(),
                enabled: false,
                ..ServerCfg::default()
            },
        ];
        let text = summary(&servers);
        assert!(text.contains("2 mcp servers, 1 enabled"), "{text}");
        assert!(text.contains("files"));
        assert!(text.contains("off"));
    }

    #[test]
    fn an_empty_list_tells_the_operator_where_to_configure_one() {
        assert!(summary(&[]).contains("[mcp.servers.NAME]"));
    }
}
