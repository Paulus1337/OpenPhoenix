use std::io::Write;

use serde_json::{json, Value};

use crate::config::Config;
use crate::memory::Memory;
use crate::prompts;
use crate::providers::{self, ChatBackend, Msg, ProviderError, Reply, Usage};
use crate::security::redact;
use crate::skills::{self, Skill};
use crate::tools::Toolbox;

pub struct Agent {
    pub cfg: Config,
    provider: Box<dyn ChatBackend>,
    pub toolbox: Toolbox,
    pub history: Vec<Msg>,
    pub usage: Usage,
    pub skills: Vec<Skill>,
    pub stream_stdout: bool,
    pub streamed_last: bool,
    fallbacks: Vec<String>,
    depth: u8,

    agents_dir: Option<std::path::PathBuf>,
}

const AGENT_TOOLS: [&str; 4] = ["agent_spawn", "agent_send", "agent_list", "agent_history"];

const AGENT_NAME_MAX: usize = 32;

fn valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= AGENT_NAME_MAX
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn agent_tool_schemas() -> Vec<Value> {
    vec![
        json!({"name": "agent_spawn",
            "description": "Create a named persistent child agent and give it a first \
task. The child keeps its own conversation history across later agent_send calls \
but has no access to this conversation. Returns the child's answer.",
            "parameters": {"type": "object",
                "properties": {"name": {"type": "string",
                                        "description": "short id: lowercase letters, digits, - and _"},
                               "task": {"type": "string",
                                        "description": "first task for the child agent"}},
                "required": ["name", "task"]}}),
        json!({"name": "agent_send",
            "description": "Send a follow-up message to a named child agent created with \
agent_spawn. The child remembers its previous work. Returns its answer.",
            "parameters": {"type": "object",
                "properties": {"name": {"type": "string"},
                               "message": {"type": "string"}},
                "required": ["name", "message"]}}),
        json!({"name": "agent_list",
            "description": "List named child agents and their message counts.",
            "parameters": {"type": "object", "properties": {}, "required": []}}),
        json!({"name": "agent_history",
            "description": "Show the most recent messages of a named child agent.",
            "parameters": {"type": "object",
                "properties": {"name": {"type": "string"},
                               "limit": {"type": "integer",
                                         "description": "messages to show, default 10"}},
                "required": ["name"]}}),
    ]
}

fn subtask_schema() -> Value {
    json!({"name": "subtask",
        "description": "Run a bounded side task in a fresh ghost agent and \
return its final answer. The child has its own tools and turn budget but \
no access to this conversation. Use for isolated research or side work.",
        "parameters": {"type": "object",
            "properties": {"prompt": {"type": "string",
                                       "description": "task for the child agent"}},
            "required": ["prompt"]}})
}

impl Agent {
    pub fn new(cfg: Config, provider: Box<dyn ChatBackend>, toolbox: Toolbox) -> Self {
        let fallbacks = cfg.fallbacks.clone();
        Agent {
            cfg,
            provider,
            toolbox,
            history: Vec::new(),
            usage: Usage::default(),
            skills: Vec::new(),
            stream_stdout: false,
            streamed_last: false,
            fallbacks,
            depth: 0,
            agents_dir: None,
        }
    }

    fn agents_dir(&self) -> std::path::PathBuf {
        self.agents_dir
            .clone()
            .unwrap_or_else(|| crate::config::home().join("agents"))
    }

    fn make_child(&self, privacy: &str) -> Result<Agent, String> {
        let mut cfg = self.cfg.clone();
        cfg.privacy = privacy.to_string();

        cfg.approvals = false;
        cfg.max_turns = std::cmp::max(self.cfg.max_turns / 2, 1);
        let toolbox =
            Toolbox::new(&cfg, Memory::new("ghost"), None, None).map_err(|e| e.to_string())?;
        let provider = providers::make(&cfg).map_err(|e| e.to_string())?;
        let mut child = Agent::new(cfg, Box::new(provider), toolbox);
        child.depth = 1;
        Ok(child)
    }

    fn run_agent_tool(&self, name: &str, args: &Value) -> String {
        match name {
            "agent_spawn" => self.agent_spawn(args),
            "agent_send" => self.agent_send(args),
            "agent_list" => self.agent_list(),
            "agent_history" => self.agent_history(args),
            _ => format!("error: unknown tool '{name}'"),
        }
    }

    fn agent_spawn(&self, args: &Value) -> String {
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return "error: bad arguments for agent_spawn: missing required 'name'".into();
        };
        let Some(task) = args.get("task").and_then(Value::as_str) else {
            return "error: bad arguments for agent_spawn: missing required 'task'".into();
        };
        if !valid_agent_name(name) {
            return format!(
                "error: bad agent name '{name}': use 1-{AGENT_NAME_MAX} chars of \
lowercase letters, digits, - or _, starting with a letter or digit"
            );
        }
        let dir = self.agents_dir();
        if !crate::sessions::load(&dir, name).is_empty() {
            return format!("error: agent '{name}' already exists; use agent_send");
        }
        let mut child = match self.make_child("session") {
            Ok(c) => c,
            Err(e) => return format!("error: {e}"),
        };
        let answer = child.run(task);
        if let Err(e) = crate::sessions::save(&dir, name, &child.history) {
            return format!("error: agent '{name}' ran but saving failed: {e}");
        }
        format!("[agent {name}] {answer}")
    }

    fn agent_send(&self, args: &Value) -> String {
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return "error: bad arguments for agent_send: missing required 'name'".into();
        };
        let Some(message) = args.get("message").and_then(Value::as_str) else {
            return "error: bad arguments for agent_send: missing required 'message'".into();
        };
        if !valid_agent_name(name) {
            return format!("error: bad agent name '{name}'");
        }
        let dir = self.agents_dir();
        let history = crate::sessions::load(&dir, name);
        if history.is_empty() {
            return format!("error: no agent named '{name}'; use agent_spawn first");
        }
        let mut child = match self.make_child("session") {
            Ok(c) => c,
            Err(e) => return format!("error: {e}"),
        };
        child.history = history;
        let answer = child.run(message);
        if let Err(e) = crate::sessions::save(&dir, name, &child.history) {
            return format!("error: agent '{name}' ran but saving failed: {e}");
        }
        format!("[agent {name}] {answer}")
    }

    fn agent_list(&self) -> String {
        let entries = crate::sessions::list(&self.agents_dir());
        if entries.is_empty() {
            return "(no named agents)".into();
        }
        entries
            .iter()
            .map(|(name, count)| format!("{name}: {count} messages"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn agent_history(&self, args: &Value) -> String {
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return "error: bad arguments for agent_history: missing required 'name'".into();
        };
        if !valid_agent_name(name) {
            return format!("error: bad agent name '{name}'");
        }
        let history = crate::sessions::load(&self.agents_dir(), name);
        if history.is_empty() {
            return format!("error: no agent named '{name}'");
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n.clamp(1, 100) as usize)
            .unwrap_or(10);
        let start = history.len().saturating_sub(limit);
        history[start..]
            .iter()
            .map(|m| {
                let (role, text) = match m {
                    Msg::User { content, .. } => ("user", content.as_str()),
                    Msg::Assistant {
                        content,
                        tool_calls,
                    } => {
                        if content.is_empty() && !tool_calls.is_empty() {
                            ("assistant", "(tool call)")
                        } else {
                            ("assistant", content.as_str())
                        }
                    }
                    Msg::Tool { content, .. } => ("tool", content.as_str()),
                };
                let one_line = text.replace('\n', " ");
                let clipped: String = one_line.chars().take(240).collect();
                format!("{role}: {}", redact(&clipped))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn run_subtask(&self, args: &Value) -> String {
        let Some(prompt) = args.get("prompt").and_then(Value::as_str) else {
            return "error: bad arguments for subtask: missing required 'prompt'".into();
        };
        let mut child = match self.make_child("ghost") {
            Ok(c) => c,
            Err(e) => return format!("error: {e}"),
        };
        child.run(prompt)
    }

    pub fn wipe(&mut self) {
        self.history.clear();
    }

    pub fn run(&mut self, user_text: &str) -> String {
        self.run_with_media(user_text, Vec::new())
    }

    pub fn run_with_media(&mut self, user_text: &str, images: Vec<(String, String)>) -> String {
        let mut system = prompts::build(&self.cfg);
        let extra = skills::inject(&self.skills, user_text);
        if !extra.is_empty() {
            system.push_str(&extra);
        }
        self.history.push(Msg::User {
            content: user_text.to_string(),
            images,
        });
        self.streamed_last = false;
        let mut reply = Reply::text_only("(no reply)");
        let mut provider_err: Option<ProviderError> = None;
        let mut finished = false;
        for _ in 0..self.cfg.max_turns {
            let mut schemas = self.toolbox.schemas();
            if self.depth == 0 {
                schemas.push(subtask_schema());
                schemas.extend(agent_tool_schemas());
            }
            let result = if self.stream_stdout {
                let mut out = std::io::stdout();
                self.provider.chat_stream(
                    &self.cfg,
                    &system,
                    &self.history,
                    &schemas,
                    &mut |t: &str| {
                        let _ = write!(out, "{t}");
                        let _ = out.flush();
                    },
                )
            } else {
                self.provider
                    .chat(&self.cfg, &system, &self.history, &schemas)
            };
            match result {
                Ok(r) => reply = r,
                Err(e) => {
                    if !self.fallbacks.is_empty() {
                        let next = self.fallbacks.remove(0);
                        eprintln!(
                            "provider error: {}; retrying with fallback model {next}",
                            redact(&e.to_string())
                        );
                        self.cfg.model = next;
                        continue;
                    }
                    provider_err = Some(e);
                    finished = true;
                    break;
                }
            }
            self.usage.input += reply.usage.input;
            self.usage.output += reply.usage.output;
            if reply.tool_calls.is_empty() {
                self.history.push(Msg::Assistant {
                    content: reply.text.clone(),
                    tool_calls: Vec::new(),
                });
                finished = true;
                break;
            }
            self.history.push(Msg::Assistant {
                content: reply.text.clone(),
                tool_calls: reply.tool_calls.clone(),
            });
            for tc in reply.tool_calls.clone() {
                let result = if tc.name == "subtask" && self.depth == 0 {
                    self.run_subtask(&tc.args)
                } else if AGENT_TOOLS.contains(&tc.name.as_str()) && self.depth == 0 {
                    self.run_agent_tool(&tc.name, &tc.args)
                } else {
                    self.toolbox.run(&tc.name, &tc.args)
                };
                self.history.push(Msg::Tool {
                    id: tc.id,
                    content: result,
                });
            }
        }
        self.streamed_last =
            self.stream_stdout && finished && provider_err.is_none() && !reply.text.is_empty();
        let final_text = if let Some(e) = provider_err {
            if matches!(self.history.last(), Some(Msg::User { .. })) {
                self.history.pop();
            }
            format!("provider error: {}", redact(&e.to_string()))
        } else if !finished {
            let text = if reply.text.is_empty() {
                "(stopped: tool-loop budget reached)".to_string()
            } else {
                reply.text.clone()
            };
            self.history.push(Msg::Assistant {
                content: text.clone(),
                tool_calls: Vec::new(),
            });
            text
        } else {
            reply.text.clone()
        };
        if self.cfg.privacy == "ghost" {
            self.wipe();
        }
        self.compact_if_needed();
        if final_text.is_empty() {
            "(empty reply)".into()
        } else {
            final_text
        }
    }

    fn compact_if_needed(&mut self) {
        if self.cfg.compact_after == 0 || self.cfg.privacy == "ghost" {
            return;
        }
        let len = self.history.len();
        if len <= self.cfg.compact_after as usize {
            return;
        }

        let mut cut = len / 2;
        while cut < len && matches!(self.history[cut], Msg::Tool { .. }) {
            cut += 1;
        }
        if cut == 0 || cut >= len {
            return;
        }
        let oldest: Vec<Msg> = self.history[..cut].to_vec();
        let system = "Summarize this conversation compactly, preserving \
facts, decisions, file paths, and open tasks.";
        match self.provider.chat(&self.cfg, system, &oldest, &[]) {
            Ok(r) if !r.text.is_empty() => {
                let summary = Msg::User {
                    content: format!("[conversation summary]\n{}", r.text),
                    images: Vec::new(),
                };
                self.history.splice(..cut, [summary]);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Memory;
    use crate::providers::{Reply, ToolCall};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-agent-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_cfg(privacy: &str) -> Config {
        Config {
            workspace: tmpdir(),
            confirm_shell: false,
            privacy: privacy.into(),
            ..Config::default()
        }
    }

    struct FakeProvider {
        calls: usize,
    }

    impl ChatBackend for FakeProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            _system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            self.calls += 1;
            if self.calls == 1 {
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "t1".into(),
                        name: "write_file".into(),
                        args: json!({"path": "out.txt", "content": "hello"}),
                    }],
                    usage: Usage {
                        input: 10,
                        output: 5,
                    },
                })
            } else {
                Ok(Reply {
                    text: "done".into(),
                    tool_calls: vec![],
                    usage: Usage {
                        input: 20,
                        output: 8,
                    },
                })
            }
        }
    }

    struct ErrProvider;

    impl ChatBackend for ErrProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            _system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            Err(ProviderError("HTTP 500: boom".into()))
        }
    }

    struct LoopProvider;

    impl ChatBackend for LoopProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            _system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            Ok(Reply {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "x".into(),
                    name: "list_dir".into(),
                    args: json!({}),
                }],
                usage: Usage::default(),
            })
        }
    }

    fn build(cfg: &Config, provider: Box<dyn ChatBackend>) -> Agent {
        let mem = Memory::with_home(&cfg.privacy, &cfg.workspace);
        let toolbox = Toolbox::new(cfg, mem, None, None).unwrap();
        Agent::new(cfg.clone(), provider, toolbox)
    }

    #[test]
    fn tool_loop_and_session_history() {
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        assert_eq!(agent.run("make a file"), "done");
        assert_eq!(
            std::fs::read_to_string(cfg.workspace.join("out.txt")).unwrap(),
            "hello"
        );
        assert!(!agent.history.is_empty());
        assert_eq!(
            agent.usage,
            Usage {
                input: 30,
                output: 13
            }
        );
    }

    #[test]
    fn ghost_wipes_history() {
        let cfg = make_cfg("ghost");
        let mut agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        agent.run("make a file");
        assert!(agent.history.is_empty());
    }

    #[test]
    fn provider_error_reported_and_user_msg_popped() {
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(ErrProvider));
        let out = agent.run("hi");
        assert!(out.starts_with("provider error:"), "got: {out}");
        assert!(agent.history.is_empty());
    }

    #[test]
    fn budget_exhaustion_reports_stop() {
        let mut cfg = make_cfg("session");
        cfg.max_turns = 2;
        let mut agent = build(&cfg, Box::new(LoopProvider));
        let out = agent.run("loop forever");
        assert_eq!(out, "(stopped: tool-loop budget reached)");
    }

    struct FlakyProvider;

    impl ChatBackend for FlakyProvider {
        fn chat(
            &mut self,
            cfg: &Config,
            _system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            if cfg.model == "backup-model" {
                Ok(Reply::text_only("saved by fallback"))
            } else {
                Err(ProviderError("HTTP 529: overloaded".into()))
            }
        }
    }

    #[test]
    fn fallback_model_rescues_the_session() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["backup-model".into()];
        let mut agent = build(&cfg, Box::new(FlakyProvider));
        assert_eq!(agent.run("hi"), "saved by fallback");
        assert_eq!(agent.cfg.model, "backup-model");

        let out = agent.run("again");
        assert_eq!(out, "saved by fallback");
    }

    #[test]
    fn no_fallback_reports_error() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["still-broken".into()];
        struct AlwaysErr;
        impl ChatBackend for AlwaysErr {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                Err(ProviderError("HTTP 500: down".into()))
            }
        }
        let mut agent = build(&cfg, Box::new(AlwaysErr));
        let out = agent.run("hi");
        assert!(out.starts_with("provider error:"), "got: {out}");
    }

    struct SubtaskProvider {
        calls: usize,
    }

    impl ChatBackend for SubtaskProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            _system: &str,
            history: &[Msg],
            tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            self.calls += 1;
            if self.calls == 1 {
                assert!(
                    tools.iter().any(|t| t["name"] == "subtask"),
                    "parent should see the subtask tool"
                );
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "s1".into(),
                        name: "subtask".into(),
                        args: json!({"prompt": "side quest"}),
                    }],
                    usage: Usage::default(),
                })
            } else {
                let child_said = history
                    .iter()
                    .find_map(|m| match m {
                        Msg::Tool { content, .. } => Some(content.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok(Reply::text_only(&format!("parent got: {child_said}")))
            }
        }
    }

    fn mock_openai_server(reply_text: &str) -> std::net::SocketAddr {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = serde_json::json!({
            "choices": [{"message": {"content": reply_text}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })
        .to_string();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(&stream);
                let mut content_len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let t = line.trim_end().to_ascii_lowercase();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("content-length:") {
                        content_len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut buf = vec![0u8; content_len];
                let _ = reader.read_exact(&mut buf);
                let mut s = &stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        addr
    }

    #[test]
    fn subtask_runs_bounded_child() {
        let addr = mock_openai_server("child says hi");
        let mut cfg = make_cfg("session");
        cfg.provider = "custom".into();
        cfg.base_url = format!("http://{addr}/v1");
        let mut agent = build(&cfg, Box::new(SubtaskProvider { calls: 0 }));
        let out = agent.run("do the thing");
        assert_eq!(out, "parent got: child says hi");
    }

    #[test]
    fn subtask_bad_args_and_depth_guard() {
        let cfg = make_cfg("session");
        let agent = build(&cfg, Box::new(SubtaskProvider { calls: 0 }));
        let msg = agent.run_subtask(&json!({}));
        assert!(msg.starts_with("error: bad arguments"), "got: {msg}");

        let mut child_schemas = agent.toolbox.schemas();
        child_schemas.push(subtask_schema());
        assert!(child_schemas.iter().any(|t| t["name"] == "subtask"));
    }

    fn agents_tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-agents-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn named_agent_spawn_send_list_history() {
        let dir = agents_tmpdir();
        let mut cfg = make_cfg("session");
        cfg.provider = "custom".into();
        cfg.base_url = format!("http://{}/v1", mock_openai_server("child says hi"));
        let mut agent = build(&cfg, Box::new(SubtaskProvider { calls: 0 }));
        agent.agents_dir = Some(dir.clone());

        let out = agent.run_agent_tool("agent_spawn", &json!({"name": "researcher", "task": "hi"}));
        assert_eq!(out, "[agent researcher] child says hi");

        let out = agent.run_agent_tool(
            "agent_spawn",
            &json!({"name": "researcher", "task": "again"}),
        );
        assert!(out.contains("already exists"), "got: {out}");

        agent.cfg.base_url = format!("http://{}/v1", mock_openai_server("child says more"));
        let out = agent.run_agent_tool(
            "agent_send",
            &json!({"name": "researcher", "message": "continue"}),
        );
        assert_eq!(out, "[agent researcher] child says more");

        let listed = agent.run_agent_tool("agent_list", &json!({}));
        assert_eq!(listed, "researcher: 4 messages");

        let hist = agent.run_agent_tool("agent_history", &json!({"name": "researcher"}));
        assert!(hist.contains("user: hi"), "got: {hist}");
        assert!(hist.contains("assistant: child says more"), "got: {hist}");
        let one = agent.run_agent_tool("agent_history", &json!({"name": "researcher", "limit": 1}));
        assert_eq!(one, "assistant: child says more");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn named_agent_errors_and_validation() {
        let dir = agents_tmpdir();
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(SubtaskProvider { calls: 0 }));
        agent.agents_dir = Some(dir.clone());

        assert_eq!(
            agent.run_agent_tool("agent_list", &json!({})),
            "(no named agents)"
        );
        let out = agent.run_agent_tool("agent_send", &json!({"name": "ghost9", "message": "x"}));
        assert!(out.contains("no agent named"), "got: {out}");
        let out = agent.run_agent_tool("agent_history", &json!({"name": "ghost9"}));
        assert!(out.contains("no agent named"), "got: {out}");
        let out = agent.run_agent_tool("agent_spawn", &json!({"name": "Bad Name!", "task": "x"}));
        assert!(out.starts_with("error: bad agent name"), "got: {out}");
        let out = agent.run_agent_tool("agent_spawn", &json!({"task": "x"}));
        assert!(out.starts_with("error: bad arguments"), "got: {out}");

        assert!(valid_agent_name("research-2_ok"));
        assert!(!valid_agent_name(""));
        assert!(!valid_agent_name("-lead"));
        assert!(!valid_agent_name("UPPER"));
        assert!(!valid_agent_name(&"a".repeat(AGENT_NAME_MAX + 1)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct CompactProvider {
        summaries: usize,
        fail_summary: bool,
    }

    impl ChatBackend for CompactProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            if system.starts_with("Summarize this conversation") {
                if self.fail_summary {
                    return Err(ProviderError("HTTP 500: summarizer down".into()));
                }
                self.summaries += 1;
                return Ok(Reply::text_only("facts and open tasks"));
            }
            Ok(Reply::text_only("answer"))
        }
    }

    #[test]
    fn compaction_triggers_over_threshold() {
        let mut cfg = make_cfg("session");
        cfg.compact_after = 4;
        let mut agent = build(
            &cfg,
            Box::new(CompactProvider {
                summaries: 0,
                fail_summary: false,
            }),
        );
        for i in 0..3 {
            agent.run(&format!("question {i}"));
        }

        assert_eq!(agent.history.len(), 4);
        match &agent.history[0] {
            Msg::User { content, .. } => {
                assert!(
                    content.starts_with("[conversation summary]\n"),
                    "got: {content}"
                );
                assert!(content.contains("facts and open tasks"));
            }
            other => panic!("expected summary user message, got {other:?}"),
        }

        match agent.history.last() {
            Some(Msg::Assistant { content, .. }) => assert_eq!(content, "answer"),
            other => panic!("expected assistant answer, got {other:?}"),
        }
    }

    #[test]
    fn compaction_noop_under_threshold_and_when_disabled() {
        let mut cfg = make_cfg("session");
        cfg.compact_after = 10;
        let mut agent = build(
            &cfg,
            Box::new(CompactProvider {
                summaries: 0,
                fail_summary: false,
            }),
        );
        agent.run("one");
        agent.run("two");
        assert_eq!(agent.history.len(), 4);
        let mut cfg = make_cfg("session");
        cfg.compact_after = 0;
        let mut agent = build(
            &cfg,
            Box::new(CompactProvider {
                summaries: 0,
                fail_summary: false,
            }),
        );
        for i in 0..5 {
            agent.run(&format!("q{i}"));
        }
        assert_eq!(agent.history.len(), 10);
    }

    #[test]
    fn compaction_never_runs_in_ghost() {
        let mut cfg = make_cfg("ghost");
        cfg.compact_after = 1;
        let mut agent = build(
            &cfg,
            Box::new(CompactProvider {
                summaries: 0,
                fail_summary: false,
            }),
        );
        agent.run("hello");
        assert!(agent.history.is_empty());
    }

    #[test]
    fn compaction_provider_error_keeps_history() {
        let mut cfg = make_cfg("session");
        cfg.compact_after = 2;
        let mut agent = build(
            &cfg,
            Box::new(CompactProvider {
                summaries: 0,
                fail_summary: true,
            }),
        );
        agent.run("one");
        agent.run("two");
        assert_eq!(agent.history.len(), 4);
        assert!(matches!(&agent.history[0], Msg::User { content, .. } if content == "one"));
    }

    #[test]
    fn skills_reach_system_prompt() {
        let cfg = make_cfg("session");
        struct CaptureSystem;
        impl ChatBackend for CaptureSystem {
            fn chat(
                &mut self,
                _c: &Config,
                system: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                Ok(Reply::text_only(&format!(
                    "sys:{}",
                    system.contains("rebase")
                )))
            }
        }
        let mut agent = build(&cfg, Box::new(CaptureSystem));
        agent.skills = vec![crate::skills::parse(
            "---\nname: git-flow\nkeywords: commit\n---\nAlways rebase first.",
        )
        .unwrap()];
        assert_eq!(agent.run("how do I commit"), "sys:true");
        assert_eq!(agent.run("what is rust"), "sys:false");
    }
}
