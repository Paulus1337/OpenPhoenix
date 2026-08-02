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
    pub fallback_notice: Option<String>,
    audit: crate::audit::Audit,
    fallbacks: Vec<String>,
    depth: u8,
    deadline: Option<std::time::Instant>,
    deadline_warned: bool,
    loops: crate::loop_detect::LoopDetector,

    agents_dir: Option<std::path::PathBuf>,
}

const AGENT_TOOLS: [&str; 4] = ["agent_spawn", "agent_send", "agent_list", "agent_history"];

const COMPACTION_PROMPT: &str = "Summarize this conversation compactly.

MUST PRESERVE:
- Active tasks and their current status (in-progress, blocked, pending)
- Batch progress (e.g. '5/17 items completed')
- The last thing the user requested and what was being done about it
- Decisions made and their rationale
- TODOs, open questions, and constraints
- Commitments or follow-ups promised
- Exact file paths, commands, identifiers, and error strings

Drop pleasantries and superseded detail. Write plain prose, no preamble.";

const DEFAULT_CONTEXT_TOKENS: usize = 200_000;
const COMPACT_TRIGGER_RATIO: f64 = 0.75;

const MAX_SPAWN_DEPTH: u8 = 1;
const BUSY_RETRIES: u32 = 3;
const EMPTY_REPLY_RETRIES: u32 = 3;
const CHILD_DEADLINE_SECS: u64 = 600;
const CHILD_WRAP_UP_SECS: u64 = 60;

pub fn install_interrupt() {
    crate::daemon::install_interrupt_handler();
}

thread_local! {
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn interrupted() -> bool {
    if crate::daemon::stopping() {
        return true;
    }
    if !ARMED.with(std::cell::Cell::get) {
        return false;
    }
    crate::daemon::interrupt_pending_swap()
}

pub fn arm_interrupt() {
    crate::daemon::set_interrupt_pending(false);
    ARMED.with(|a| a.set(true));
    crate::daemon::set_interrupt_armed(true);
}

pub fn disarm_interrupt() {
    ARMED.with(|a| a.set(false));
    crate::daemon::set_interrupt_armed(false);
    crate::daemon::set_interrupt_pending(false);
}

pub fn model_context_tokens(model: &str) -> usize {
    model_context_tokens_for("", model)
}

pub fn model_context_tokens_for(provider: &str, model: &str) -> usize {
    if let Some(w) = crate::catalog::context_window(provider, model) {
        return w as usize;
    }
    let m = model.to_ascii_lowercase();
    if m.contains("gpt-5") || m.contains("o3") || m.contains("o4") {
        400_000
    } else if m.contains("gemini") {
        1_000_000
    } else if m.contains("claude") || m.contains("opus") || m.contains("sonnet") {
        200_000
    } else if m.contains("llama") || m.contains("mistral") || m.contains("deepseek") {
        128_000
    } else {
        DEFAULT_CONTEXT_TOKENS
    }
}

fn context_budget_tokens_for(provider: &str, model: &str) -> usize {
    ((model_context_tokens_for(provider, model) as f64) * COMPACT_TRIGGER_RATIO) as usize
}

#[cfg(test)]
fn context_budget_tokens(model: &str) -> usize {
    context_budget_tokens_for("", model)
}

pub fn estimate_msg_tokens(m: &Msg) -> usize {
    let chars = match m {
        Msg::User { content, images } => content.len() + images.len() * 4000,
        Msg::Assistant {
            content,
            tool_calls,
        } => {
            content.len()
                + tool_calls
                    .iter()
                    .map(|tc| tc.name.len() + tc.args.to_string().len())
                    .sum::<usize>()
        }
        Msg::Tool { content, .. } => content.len(),
    };
    chars / 4
}

pub fn estimate_tokens(history: &[Msg]) -> usize {
    history.iter().map(estimate_msg_tokens).sum()
}

fn context_line(used: usize, window: usize) -> String {
    let w = window.max(1);
    let pct = (used.saturating_mul(100) / w).min(999);
    format!("\nContext use: about {pct}% of a {w}-token window.")
}

pub fn shed_history_for_overflow(history: &mut Vec<Msg>, target_tokens: usize) -> usize {
    let len = history.len();
    if len < 2 {
        return 0;
    }
    let mut cut = 0usize;
    while cut < len - 1 && estimate_tokens(&history[cut..]) > target_tokens {
        cut += 1;
    }
    while cut < len && matches!(history[cut], Msg::Tool { .. }) {
        cut += 1;
    }
    if cut == 0 || cut >= len {
        return 0;
    }
    let stub = Msg::User {
        content: format!("[context overflow: dropped the {cut} oldest messages]"),
        images: Vec::new(),
    };
    history.splice(..cut, [stub]);
    cut
}

const SUMMARY_SAFETY_MARGIN: f64 = 1.2;
const SUMMARY_OVERHEAD_TOKENS: usize = 4096;
const OVERSIZED_MSG_RATIO: f64 = 0.5;

fn chunk_by_max_tokens(msgs: &[Msg], max_tokens: usize) -> Vec<Vec<Msg>> {
    if msgs.is_empty() {
        return Vec::new();
    }
    let effective = ((max_tokens as f64 / SUMMARY_SAFETY_MARGIN) as usize).max(1);
    let mut chunks: Vec<Vec<Msg>> = Vec::new();
    let mut cur: Vec<Msg> = Vec::new();
    let mut cur_tokens = 0usize;
    for m in msgs {
        let t = estimate_msg_tokens(m);
        if !cur.is_empty() && cur_tokens + t > effective {
            chunks.push(std::mem::take(&mut cur));
            cur_tokens = 0;
        }
        cur.push(m.clone());
        cur_tokens += t;
        if t > effective {
            chunks.push(std::mem::take(&mut cur));
            cur_tokens = 0;
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

fn split_oversized(msgs: &[Msg], context_window: usize) -> (Vec<Msg>, Vec<String>) {
    let threshold = (context_window as f64 * OVERSIZED_MSG_RATIO) as usize;
    let mut small = Vec::new();
    let mut notes = Vec::new();
    for m in msgs {
        let t = estimate_msg_tokens(m);
        if (t as f64 * SUMMARY_SAFETY_MARGIN) as usize > threshold {
            let role = match m {
                Msg::User { .. } => "user",
                Msg::Assistant { .. } => "assistant",
                Msg::Tool { .. } => "tool",
            };
            notes.push(format!(
                "[large {role} message (~{}K tokens) omitted from summary]",
                t.div_ceil(1000)
            ));
        } else {
            small.push(m.clone());
        }
    }
    (small, notes)
}

pub const AGENT_NAME_MAX: usize = 32;

pub fn valid_agent_name(name: &str) -> bool {
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
                                       "description": "task for the child agent"},
                           "deny_tools": {"type": "array", "items": {"type": "string"},
                                       "description": "extra tool names the child may not use"},
                           "workspace": {"type": "string",
                                       "description": "relative subdirectory of the parent \
workspace to use as the child's workspace"}},
            "required": ["prompt"]}})
}

fn child_workspace(parent: &std::path::Path, sub: &str) -> Result<std::path::PathBuf, String> {
    let sub = sub.trim();
    if sub.is_empty() {
        return Err("workspace must be a non-empty relative subdirectory".into());
    }
    let p = std::path::Path::new(sub);
    if p.is_absolute() {
        return Err("workspace must be relative to the parent workspace".into());
    }
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(_) => {}
            _ => return Err("workspace may not contain '..', '.', or root components".into()),
        }
    }
    Ok(parent.join(p))
}

fn extra_denies(args: &Value) -> Vec<String> {
    args.get("deny_tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl Agent {
    pub fn new(cfg: Config, provider: Box<dyn ChatBackend>, toolbox: Toolbox) -> Self {
        let fallbacks = cfg.fallbacks.clone();
        let cfg_audit = cfg.audit_log;
        Agent {
            cfg,
            provider,
            toolbox,
            history: Vec::new(),
            usage: Usage::default(),
            skills: Vec::new(),
            stream_stdout: false,
            streamed_last: false,
            fallback_notice: None,
            audit: if cfg_audit {
                crate::audit::Audit::at(&crate::config::home().join("audit.jsonl"))
            } else {
                crate::audit::Audit::disabled()
            },
            fallbacks,
            depth: 0,
            deadline: None,
            deadline_warned: false,
            loops: crate::loop_detect::LoopDetector::new(),
            agents_dir: None,
        }
    }

    pub fn retarget(&mut self, spec: &str) -> Result<(), String> {
        let prev = self.cfg.clone();
        crate::config::retarget(&mut self.cfg, spec);
        if self.cfg.provider == prev.provider
            && self.cfg.base_url == prev.base_url
            && self.cfg.api == prev.api
        {
            return Ok(());
        }
        match providers::make(&self.cfg) {
            Ok(p) => {
                self.provider = Box::new(p);
                Ok(())
            }
            Err(e) => {
                self.cfg = prev;
                Err(e.to_string())
            }
        }
    }

    fn agents_dir(&self) -> std::path::PathBuf {
        self.agents_dir
            .clone()
            .unwrap_or_else(|| crate::config::home().join("agents"))
    }

    fn make_child(&self, privacy: &str) -> Result<Agent, String> {
        self.make_child_denying(privacy, &[], None)
    }

    fn make_child_denying(
        &self,
        privacy: &str,
        extra: &[String],
        workspace: Option<&str>,
    ) -> Result<Agent, String> {
        if self.depth >= MAX_SPAWN_DEPTH {
            return Err(format!(
                "child agents are not allowed at this depth (current depth {}, max {MAX_SPAWN_DEPTH})",
                self.depth
            ));
        }
        let mut cfg = self.cfg.clone();
        cfg.privacy = privacy.to_string();

        cfg.approvals = false;
        for name in extra {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if !cfg.deny_tools.iter().any(|d| d.eq_ignore_ascii_case(name)) {
                cfg.deny_tools.push(name.to_string());
            }
        }
        if let Some(sub) = workspace {
            let dir = child_workspace(&self.cfg.workspace, sub)?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create child workspace: {e}"))?;
            cfg.workspace = dir;
        }
        let toolbox =
            Toolbox::new(&cfg, Memory::new("ghost"), None, None).map_err(|e| e.to_string())?;
        let provider = providers::make(&cfg).map_err(|e| e.to_string())?;
        let mut child = Agent::new(cfg, Box::new(provider), toolbox);
        child.depth = self.depth + 1;
        child.deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(CHILD_DEADLINE_SECS));
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
        let ws = args.get("workspace").and_then(Value::as_str);
        let mut child = match self.make_child_denying("ghost", &extra_denies(args), ws) {
            Ok(c) => c,
            Err(e) => return format!("error: {e}"),
        };
        child.run(prompt)
    }

    pub fn wipe(&mut self) {
        self.history.clear();
    }

    pub fn colab_toolbox(&self) -> Result<Toolbox, String> {
        let memory = Memory::in_workspace(&self.cfg.privacy, &self.cfg.workspace);
        Toolbox::new(&self.cfg, memory, None, None).map_err(|e| e.to_string())
    }

    pub fn run_colab(
        &mut self,
        b_spec: &str,
        task: &str,
        max_rounds: u32,
        on_round: impl FnMut(&crate::colab::Round),
    ) -> Result<crate::colab::ColabResult, String> {
        let b_toolbox = self.colab_toolbox()?;
        crate::colab::run(self, b_spec, b_toolbox, task, max_rounds, on_round)
    }

    fn summarize_span(&mut self, system: &str, msgs: &[Msg]) -> Result<String, String> {
        let window = model_context_tokens(&self.cfg.model);
        let (small, oversized) = split_oversized(msgs, window);
        let budget = window
            .saturating_sub(SUMMARY_OVERHEAD_TOKENS)
            .max(SUMMARY_OVERHEAD_TOKENS);
        let chunks = chunk_by_max_tokens(&small, budget);
        let mut parts: Vec<String> = Vec::new();
        let total = chunks.len();
        for (i, chunk) in chunks.iter().enumerate() {
            let sys = if total > 1 {
                format!("{system}\nThis is part {} of {total} of a longer conversation. Summarize only this part.", i + 1)
            } else {
                system.to_string()
            };
            match self.provider.chat(&self.cfg, &sys, chunk, &[]) {
                Ok(r) if !r.text.is_empty() => parts.push(r.text),
                Ok(_) => {}
                Err(e) => return Err(redact(&e.to_string())),
            }
        }
        parts.extend(oversized);
        if parts.is_empty() {
            return Ok(String::new());
        }
        if parts.len() == 1 {
            return Ok(parts.remove(0));
        }
        Ok(parts.join("\n\n"))
    }

    pub fn compact_now(&mut self, instructions: &str) -> String {
        let len = self.history.len();
        if len < 2 {
            return "nothing to compact".into();
        }
        let mut cut = len.saturating_sub(2);
        while cut < len && matches!(self.history[cut], Msg::Tool { .. }) {
            cut += 1;
        }
        if cut == 0 || cut >= len {
            return "nothing to compact".into();
        }
        let oldest: Vec<Msg> = self.history[..cut].to_vec();
        let mut system = String::from(
            "Summarize this conversation compactly, preserving facts, decisions, \
file paths, and open tasks.",
        );
        if !instructions.is_empty() {
            system.push_str("\nExtra instructions: ");
            system.push_str(instructions);
        }
        match self.summarize_span(&system, &oldest) {
            Ok(text) if !text.is_empty() => {
                let summary = Msg::User {
                    content: format!("[conversation summary]\n{text}"),
                    images: Vec::new(),
                };
                self.history.splice(..cut, [summary]);
                format!(
                    "compacted {cut} messages → 1 summary ({} left)",
                    self.history.len()
                )
            }
            Ok(_) => "compaction returned nothing; history unchanged".into(),
            Err(e) => format!("compaction failed: {e}"),
        }
    }

    fn shed_for_overflow(&mut self) -> usize {
        let window = model_context_tokens_for(&self.cfg.provider, &self.cfg.model);
        shed_history_for_overflow(&mut self.history, window / 2)
    }

    pub fn run(&mut self, user_text: &str) -> String {
        self.run_with_media(user_text, Vec::new())
    }

    fn fire_hooks(&self, event: &str, detail: &serde_json::Value) {
        if self.cfg.hooks.is_empty() {
            return;
        }
        for p in crate::hooks::fire(&self.cfg.hooks, event, detail) {
            eprintln!("hook: {p}");
        }
    }

    fn call_model(&mut self, system: &str, schemas: &[Value]) -> Result<Reply, ProviderError> {
        if self.stream_stdout {
            let mut out = std::io::stdout();
            self.provider
                .chat_stream(&self.cfg, system, &self.history, schemas, &mut |t: &str| {
                    let _ = write!(out, "{t}");
                    let _ = out.flush();
                })
        } else {
            self.provider
                .chat(&self.cfg, system, &self.history, schemas)
        }
    }

    pub fn run_with_media(&mut self, user_text: &str, images: Vec<(String, String)>) -> String {
        self.fire_hooks(
            "turn_start",
            &json!({"chars": user_text.len(), "images": images.len(),
                    "model": self.cfg.model, "provider": self.cfg.provider}),
        );
        let live_tools: Vec<String> = {
            let mut v: Vec<String> = self
                .toolbox
                .schemas()
                .iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect();
            if self.depth == 0 {
                v.push("subagent".to_string());
                v.extend(
                    agent_tool_schemas()
                        .iter()
                        .filter_map(|s| s["name"].as_str().map(str::to_string)),
                );
            }
            v
        };
        let mut system = prompts::build_full(
            &self.cfg,
            &crate::config::home().join("persona"),
            &live_tools,
        );
        let extra = skills::inject(&self.skills, user_text);
        if !extra.is_empty() {
            system.push_str(&extra);
        }
        self.history.push(Msg::User {
            content: user_text.to_string(),
            images,
        });
        system.push_str(&context_line(
            estimate_tokens(&self.history),
            model_context_tokens_for(&self.cfg.provider, &self.cfg.model),
        ));
        self.streamed_last = false;
        self.fallback_notice = None;
        let mut reply = Reply::text_only("(no reply)");
        let mut provider_err: Option<ProviderError> = None;
        let finished;
        let mut overflow_sheds = 0u32;
        loop {
            if let Some(deadline) = self.deadline {
                let now = std::time::Instant::now();
                if now >= deadline {
                    reply = Reply::text_only(&format!(
                        "stopped: child agent exceeded its {CHILD_DEADLINE_SECS}s wall-clock budget"
                    ));
                    finished = true;
                    break;
                }
                if !self.deadline_warned && (deadline - now).as_secs() < CHILD_WRAP_UP_SECS {
                    self.deadline_warned = true;
                    self.history.push(Msg::User {
                        content: format!(
                            "[time budget] under {CHILD_WRAP_UP_SECS}s remain: stop new work, \
answer now with what you have"
                        ),
                        images: Vec::new(),
                    });
                }
            }
            if interrupted() {
                reply = Reply::text_only("stopped: interrupted before the next model call");
                finished = true;
                break;
            }
            let mut schemas = self.toolbox.schemas();
            if self.depth == 0 {
                schemas.push(subtask_schema());
                schemas.extend(agent_tool_schemas());
            }
            let mut result = self.call_model(&system, &schemas);
            for attempt in 1..=BUSY_RETRIES {
                let Err(e) = &result else { break };
                if !providers::rotatable(e) {
                    break;
                }
                let wait = providers::retry_after_hint(e)
                    .map(std::time::Duration::from_secs)
                    .unwrap_or_else(|| providers::busy_backoff(attempt));
                eprintln!(
                    "model busy ({}); retry {attempt} of {BUSY_RETRIES} in {}s",
                    crate::security::one_line(&redact(&e.to_string()), 90),
                    wait.as_secs()
                );
                let deadline = std::time::Instant::now() + wait;
                while std::time::Instant::now() < deadline {
                    if interrupted() {
                        break;
                    }
                    std::thread::sleep(
                        std::time::Duration::from_millis(200)
                            .min(deadline - std::time::Instant::now()),
                    );
                }
                if interrupted() {
                    break;
                }
                result = self.call_model(&system, &schemas);
            }
            let mut empty_retries = 0u32;
            while let Err(e) = &result {
                if !providers::is_empty_reply(e) || empty_retries >= EMPTY_REPLY_RETRIES {
                    break;
                }
                empty_retries += 1;
                eprintln!(
                    "the model sent back nothing; retry {empty_retries} of {EMPTY_REPLY_RETRIES}"
                );
                if interrupted() {
                    break;
                }
                result = self.call_model(&system, &schemas);
            }
            if let Err(e) = &result {
                if providers::is_empty_reply(e) && empty_retries >= EMPTY_REPLY_RETRIES {
                    result = Err(ProviderError(format!(
                        "the model sent back nothing {EMPTY_REPLY_RETRIES} times in a row; \
try again, or switch models with /model"
                    )));
                }
            }
            match result {
                Ok(r) => reply = r,
                Err(e) => {
                    if overflow_sheds == 0 && providers::context_overflow(&e) {
                        overflow_sheds = 1;
                        let dropped = self.shed_for_overflow();
                        if dropped > 0 {
                            eprintln!(
                                "context overflow: dropped the {dropped} oldest messages; retrying"
                            );
                            continue;
                        }
                    }
                    if !self.fallbacks.is_empty() {
                        let next = self.fallbacks.remove(0);
                        let from = format!("{}/{}", self.cfg.provider, self.cfg.model);
                        eprintln!(
                            "provider error: {}; retrying with fallback {next}",
                            redact(&e.to_string())
                        );
                        match self.retarget(&next) {
                            Ok(()) => {
                                self.fallback_notice = Some(format!(
                                    "switched from {from} to {next} after: {}",
                                    crate::security::one_line(&redact(&e.to_string()), 80)
                                ));
                            }
                            Err(e2) => {
                                eprintln!("fallback provider unavailable: {}", redact(&e2));
                            }
                        }
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
            let mut loop_break: Option<String> = None;
            for tc in reply.tool_calls.clone() {
                if interrupted() {
                    let msg = "stopped: interrupted during tool execution".to_string();
                    self.history.push(Msg::Tool {
                        id: tc.id.clone(),
                        content: msg.clone(),
                    });
                    loop_break = Some(msg);
                    continue;
                }
                let mut warning: Option<String> = None;
                match self.loops.detect(&tc.name, &tc.args) {
                    crate::loop_detect::Detection::Block(msg) => {
                        self.loops.record(&tc.name, &tc.args, &msg);
                        self.history.push(Msg::Tool {
                            id: tc.id.clone(),
                            content: msg.clone(),
                        });
                        loop_break = Some(msg);
                        continue;
                    }
                    crate::loop_detect::Detection::Warn(msg) => warning = Some(msg),
                    crate::loop_detect::Detection::Ok => {}
                }
                let result = if tc.name == "subtask" && self.depth == 0 {
                    self.run_subtask(&tc.args)
                } else if AGENT_TOOLS.contains(&tc.name.as_str()) && self.depth == 0 {
                    self.run_agent_tool(&tc.name, &tc.args)
                } else {
                    self.toolbox.run(&tc.name, &tc.args)
                };
                self.loops.record(&tc.name, &tc.args, &result);
                let result = match warning {
                    Some(w) => format!("{result}\n\n[{w}]"),
                    None => result,
                };
                let cap =
                    crate::tools::max_tool_result_chars(model_context_tokens(&self.cfg.model));
                self.history.push(Msg::Tool {
                    id: tc.id,
                    content: crate::tools::clip_to(result, cap),
                });
            }
            if let Some(msg) = loop_break {
                reply = Reply::text_only(&msg);
                finished = true;
                break;
            }
        }
        self.streamed_last =
            self.stream_stdout && finished && provider_err.is_none() && !reply.text.is_empty();
        let final_text = if let Some(e) = provider_err {
            if matches!(self.history.last(), Some(Msg::User { .. })) {
                self.history.pop();
            }
            format!("provider error: {}", redact(&e.to_string()))
        } else {
            reply.text.clone()
        };
        let calls = self.toolbox.take_calls();
        if !self.cfg.hooks.is_empty() {
            for (name, args) in &calls {
                let short: String = args.chars().take(400).collect();
                self.fire_hooks("tool_call", &json!({"tool": name, "args": short}));
            }
            if final_text.starts_with("provider error:") {
                self.fire_hooks(
                    "error",
                    &json!({"message": final_text, "model": self.cfg.model}),
                );
            }
            self.fire_hooks(
                "turn_end",
                &json!({"chars": final_text.len(), "tool_calls": calls.len(),
                        "finished": finished, "model": self.cfg.model,
                        "input_tokens": self.usage.input, "output_tokens": self.usage.output}),
            );
        }
        let final_text = match self.cfg.verbose.as_str() {
            "off" => final_text,
            level if calls.is_empty() => {
                let _ = level;
                final_text
            }
            level => {
                let mut lines = vec![format!("── {} tool call(s)", calls.len())];
                for (name, args) in &calls {
                    if self.cfg.trace == "raw" {
                        lines.push(format!("• {name} {args}"));
                    } else if self.cfg.trace == "on" || level == "full" {
                        let short: String = args.chars().take(120).collect();
                        lines.push(format!("• {name} {short}"));
                    } else {
                        lines.push(format!("• {name}"));
                    }
                }
                format!("{final_text}\n\n{}", lines.join("\n"))
            }
        };
        let final_text = match (&self.fallback_notice, self.cfg.verbose.as_str()) {
            (Some(note), v) if v != "off" => format!("{final_text}\n\n(fallback: {note})"),
            _ => final_text,
        };
        self.audit.turn(
            &self.cfg.provider,
            &self.cfg.model,
            reply.usage.input,
            reply.usage.output,
        );
        if self.cfg.privacy == "ghost" {
            self.wipe();
        }
        self.compact_if_needed();
        let final_text = crate::security::strip_internal_markers(&final_text);
        let final_text = crate::security::mask_values(&final_text, &self.cfg.secret_values());
        if final_text.is_empty() {
            "(empty reply)".into()
        } else {
            final_text
        }
    }

    pub fn context_report(&self) -> String {
        let mut chars = 0usize;
        let (mut users, mut assistants, mut tools) = (0, 0, 0);
        for m in &self.history {
            match m {
                Msg::User { content, .. } => {
                    chars += content.len();
                    users += 1;
                }
                Msg::Assistant { content, .. } => {
                    chars += content.len();
                    assistants += 1;
                }
                Msg::Tool { content, .. } => {
                    chars += content.len();
                    tools += 1;
                }
            }
        }
        let compact = if self.cfg.compact_after == 0 {
            "token-based only".to_string()
        } else {
            format!("at {} messages", self.cfg.compact_after)
        };
        let used = chars / 4;
        let window = model_context_tokens_for(&self.cfg.provider, &self.cfg.model);
        let pct = if window > 0 {
            (used as f64 / window as f64 * 100.0).round() as u64
        } else {
            0
        };
        format!(
            "messages: {} (user {users} / assistant {assistants} / tool {tools})\n\
context: ~{used} / {window} tokens ({pct}%)\n\
auto-compaction: {compact}, plus at {}% of the window",
            self.history.len(),
            (COMPACT_TRIGGER_RATIO * 100.0) as u64
        )
    }

    fn compact_if_needed(&mut self) {
        if self.cfg.privacy == "ghost" {
            return;
        }
        let len = self.history.len();
        let by_count = self.cfg.compact_after > 0 && len > self.cfg.compact_after as usize;
        let tokens = estimate_tokens(&self.history);
        let budget = context_budget_tokens_for(&self.cfg.provider, &self.cfg.model);
        let by_tokens = tokens > budget;
        if !by_count && !by_tokens {
            return;
        }
        if len < 2 {
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
        let summary_text = match self.summarize_span(COMPACTION_PROMPT, &oldest) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("compaction failed: {e}");
                String::new()
            }
        };
        let summary = if summary_text.is_empty() {
            if !by_tokens {
                return;
            }
            format!(
                "[conversation summary unavailable; {cut} older messages were dropped to stay \
within the context window]"
            )
        } else {
            format!("[conversation summary]\n{summary_text}")
        };
        self.history.splice(
            ..cut,
            [Msg::User {
                content: summary,
                images: Vec::new(),
            }],
        );
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

    #[test]
    fn overflow_shedding_drops_oldest_and_keeps_tool_pairing() {
        let mut h = vec![
            Msg::User {
                content: "a".repeat(4000),
                images: Vec::new(),
            },
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "t1".into(),
                    name: "x".into(),
                    args: json!({}),
                }],
            },
            Msg::Tool {
                id: "t1".into(),
                content: "b".repeat(4000),
            },
            Msg::User {
                content: "recent question".into(),
                images: Vec::new(),
            },
        ];
        let dropped = shed_history_for_overflow(&mut h, 10);
        assert!(dropped >= 3, "dropped {dropped}");
        let stub_ok =
            matches!(&h[0], Msg::User { content, .. } if content.contains("context overflow"));
        assert!(stub_ok, "first message must be the overflow stub");
    }

    #[test]
    fn overflow_shedding_leaves_tiny_histories_alone() {
        let mut h = vec![Msg::User {
            content: "hi".into(),
            images: Vec::new(),
        }];
        assert_eq!(shed_history_for_overflow(&mut h, 1), 0);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn overflow_shedding_reaches_the_target_budget() {
        let mut h: Vec<Msg> = (0..10)
            .map(|i| Msg::User {
                content: format!("{i}").repeat(2000),
                images: Vec::new(),
            })
            .collect();
        let before = estimate_tokens(&h);
        let dropped = shed_history_for_overflow(&mut h, before / 4);
        assert!(dropped > 0);
        assert!(estimate_tokens(&h) < before / 2);
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
    fn the_gateway_never_caps_a_productive_run() {
        struct LongButProductive {
            calls: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl ChatBackend for LongButProductive {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                let n = self.calls.get() + 1;
                self.calls.set(n);
                if n > 200 {
                    return Ok(Reply::text_only("done"));
                }
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("c{n}"),
                        name: "shell".into(),
                        args: json!({"command": format!("echo step {n}")}),
                    }],
                    usage: Usage::default(),
                })
            }
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let mut cfg = make_cfg("session");
        cfg.confirm_shell = false;
        let mut agent = build(
            &cfg,
            Box::new(LongButProductive {
                calls: calls.clone(),
            }),
        );
        let out = agent.run("do a long job");
        assert_eq!(
            out, "done",
            "the gateway must not cut a run short: only the model or a degenerate \
loop may end it, never a turn budget"
        );
        assert!(
            calls.get() > 24,
            "ran only {} turns; a gateway-side cap is back",
            calls.get()
        );
    }

    struct EmptyProvider {
        calls: std::rc::Rc<std::cell::Cell<usize>>,
        recover_after: usize,
    }

    impl ChatBackend for EmptyProvider {
        fn chat(
            &mut self,
            _cfg: &Config,
            _system: &str,
            _history: &[Msg],
            _tools: &[Value],
        ) -> Result<Reply, ProviderError> {
            self.calls.set(self.calls.get() + 1);
            if self.calls.get() > self.recover_after {
                Ok(Reply::text_only("awake now"))
            } else {
                Err(ProviderError(providers::EMPTY_REPLY_ERROR.into()))
            }
        }
    }

    #[test]
    fn an_empty_reply_is_retried_and_can_recover() {
        let cfg = make_cfg("session");
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut agent = build(
            &cfg,
            Box::new(EmptyProvider {
                calls: calls.clone(),
                recover_after: 2,
            }),
        );
        assert_eq!(agent.run("hi"), "awake now");
        assert_eq!(calls.get(), 3, "one call plus two empty-reply retries");
    }

    #[test]
    fn three_empty_replies_in_a_row_tell_the_user_what_to_do() {
        let cfg = make_cfg("session");
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut agent = build(
            &cfg,
            Box::new(EmptyProvider {
                calls: calls.clone(),
                recover_after: usize::MAX,
            }),
        );
        let out = agent.run("hi");
        assert_eq!(calls.get(), 4, "one call plus EMPTY_REPLY_RETRIES retries");
        assert!(out.starts_with("provider error:"), "{out}");
        assert!(
            out.contains("the model sent back nothing 3 times in a row"),
            "{out}"
        );
        assert!(out.contains("switch models with /model"), "{out}");
        assert!(agent.history.is_empty(), "the user turn must be replayable");
    }

    #[test]
    fn an_empty_reply_falls_back_before_it_gives_up() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["backup-model".into()];
        cfg.verbose = "on".into();
        struct EmptyThenFallback;
        impl ChatBackend for EmptyThenFallback {
            fn chat(
                &mut self,
                cfg: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                if cfg.model == "backup-model" {
                    Ok(Reply::text_only("saved by fallback"))
                } else {
                    Err(ProviderError(providers::EMPTY_REPLY_ERROR.into()))
                }
            }
        }
        let mut agent = build(&cfg, Box::new(EmptyThenFallback));
        let out = agent.run("hi");
        assert!(out.contains("saved by fallback"), "{out}");
        assert!(out.contains("backup-model"), "{out}");
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
    fn compaction_drops_history_when_over_budget_and_summary_fails() {
        struct SummaryFails;
        impl ChatBackend for SummaryFails {
            fn chat(
                &mut self,
                _c: &Config,
                system: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                if system.starts_with("Summarize") {
                    return Err(ProviderError("HTTP 500: summarizer down".into()));
                }
                Ok(Reply::text_only("ok"))
            }
        }
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(SummaryFails));
        let budget = context_budget_tokens(&cfg.model);
        let huge = "x".repeat(budget * 4);
        for _ in 0..2 {
            agent.history.push(Msg::User {
                content: huge.clone(),
                images: Vec::new(),
            });
            agent.history.push(Msg::Assistant {
                content: "ack".into(),
                tool_calls: Vec::new(),
            });
        }
        let before = estimate_tokens(&agent.history);
        assert!(before > budget, "test setup must exceed the budget");
        agent.compact_if_needed();
        let after = estimate_tokens(&agent.history);
        assert!(after < before, "over-budget history must shrink: {after}");
        assert!(
            agent
                .history
                .iter()
                .any(|m| matches!(m, Msg::User { content, .. }
                    if content.contains("summary unavailable"))),
            "a marker must record the dropped turns"
        );
    }

    #[test]
    fn count_only_compaction_keeps_history_when_the_summary_call_fails() {
        struct SummaryFails;
        impl ChatBackend for SummaryFails {
            fn chat(
                &mut self,
                _c: &Config,
                system: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                if system.starts_with("Summarize") {
                    return Err(ProviderError("HTTP 500: summarizer down".into()));
                }
                Ok(Reply::text_only("ok"))
            }
        }
        let mut cfg = make_cfg("session");
        cfg.compact_after = 2;
        let mut agent = build(&cfg, Box::new(SummaryFails));
        agent.run("one");
        agent.run("two");
        let kept = agent.history.len();
        assert!(kept >= 4, "history is retried, not discarded: {kept}");
        assert!(
            !agent
                .history
                .iter()
                .any(|m| matches!(m, Msg::User { content, .. }
                    if content.contains("summary unavailable"))),
            "no data loss marker when only the message count tripped"
        );
    }

    #[test]
    fn oversized_tool_results_are_clipped_before_entering_history() {
        struct OneToolCall(bool);
        impl ChatBackend for OneToolCall {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                if self.0 {
                    return Ok(Reply::text_only("done"));
                }
                self.0 = true;
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "t1".into(),
                        name: "shell".into(),
                        args: json!({"command": "printf 'y%.0s' $(seq 1 300000)"}),
                    }],
                    usage: Usage::default(),
                })
            }
        }
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(OneToolCall(false)));
        agent.run("make a huge tool result");
        let cap = crate::tools::max_tool_result_chars(model_context_tokens(&cfg.model));
        let tool_len = agent
            .history
            .iter()
            .find_map(|m| match m {
                Msg::Tool { content, .. } => Some(content.chars().count()),
                _ => None,
            })
            .expect("a tool result must be recorded");
        assert!(
            tool_len <= cap + 100,
            "tool result {tool_len} exceeded cap {cap}"
        );
    }

    fn interrupt_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::daemon::set_interrupt_pending(false);
        g
    }

    #[test]
    fn an_interrupt_stops_the_turn_before_the_next_model_call() {
        struct CountingProvider(std::rc::Rc<std::cell::Cell<usize>>);
        impl ChatBackend for CountingProvider {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                self.0.set(self.0.get() + 1);
                Ok(Reply::text_only("never reached"))
            }
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let cfg = make_cfg("session");
        let _g = interrupt_guard();
        let mut agent = build(&cfg, Box::new(CountingProvider(calls.clone())));
        arm_interrupt();
        crate::daemon::set_interrupt_pending(true);
        let out = agent.run("do a long thing");
        assert!(out.contains("interrupted"), "{out}");
        assert_eq!(
            calls.get(),
            0,
            "no model call may happen after an interrupt"
        );
        assert!(!interrupted(), "the flag must be consumed, not sticky");
    }

    #[test]
    fn arm_interrupt_clears_a_stale_flag() {
        let _g = interrupt_guard();
        crate::daemon::set_interrupt_pending(true);
        arm_interrupt();
        assert!(!interrupted());
    }

    #[test]
    fn an_unarmed_thread_ignores_the_process_wide_flag() {
        let _g = interrupt_guard();
        crate::daemon::set_interrupt_pending(true);
        let seen = std::thread::spawn(interrupted).join().unwrap();
        assert!(!seen, "only the thread running a turn may consume the flag");
        crate::daemon::set_interrupt_pending(false);
    }

    #[test]
    fn a_fallback_switch_is_disclosed_to_the_user() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["backup-model".into()];
        cfg.verbose = "on".into();
        let mut agent = build(&cfg, Box::new(FlakyProvider));
        let out = agent.run("hi");
        assert!(out.contains("fallback:"), "{out}");
        assert!(out.contains("backup-model"), "{out}");
        assert!(
            agent.fallback_notice.is_some(),
            "the switch must be recorded"
        );
    }

    #[test]
    fn a_quiet_session_keeps_the_fallback_notice_out_of_the_reply() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["backup-model".into()];
        cfg.verbose = "off".into();
        let mut agent = build(&cfg, Box::new(FlakyProvider));
        let out = agent.run("hi");
        assert!(!out.contains("fallback:"), "{out}");
        assert!(
            agent.fallback_notice.is_some(),
            "still recorded for callers that want it"
        );
    }

    #[test]
    fn a_clean_turn_clears_any_earlier_fallback_notice() {
        let mut cfg = make_cfg("session");
        cfg.fallbacks = vec!["backup-model".into()];
        cfg.verbose = "on".into();
        let mut agent = build(&cfg, Box::new(FlakyProvider));
        agent.run("first");
        assert!(agent.fallback_notice.is_some());
        let out = agent.run("second");
        assert!(
            agent.fallback_notice.is_none(),
            "a healthy turn must not repeat a stale notice"
        );
        assert!(!out.contains("fallback:"), "{out}");
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
    fn the_system_prompt_reports_context_use() {
        assert_eq!(
            context_line(50_000, 100_000),
            "\nContext use: about 50% of a 100000-token window."
        );
        assert_eq!(
            context_line(0, 100_000),
            "\nContext use: about 0% of a 100000-token window."
        );
        assert!(
            context_line(1, 0).contains("of a 1-token window"),
            "a zero window must not divide by zero"
        );

        struct EchoSystem;
        impl ChatBackend for EchoSystem {
            fn chat(
                &mut self,
                _cfg: &Config,
                system: &str,
                _history: &[Msg],
                _tools: &[Value],
            ) -> Result<Reply, ProviderError> {
                Ok(Reply::text_only(if system.contains("Context use: about") {
                    "seen"
                } else {
                    "missing"
                }))
            }
        }
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(EchoSystem));
        assert_eq!(
            agent.run("hello"),
            "seen",
            "the model must see its context budget each turn"
        );
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

    #[test]
    fn a_spawn_can_deny_extra_tools_for_that_child_only() {
        let cfg = make_cfg("session");
        let agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        let child = agent
            .make_child_denying(
                "ghost",
                &["shell".to_string(), " ".to_string(), "SHELL".to_string()],
                None,
            )
            .unwrap();
        assert!(
            child.cfg.deny_tools.iter().any(|d| d == "shell"),
            "the extra deny must reach the child config"
        );
        assert_eq!(
            child
                .cfg
                .deny_tools
                .iter()
                .filter(|d| d.eq_ignore_ascii_case("shell"))
                .count(),
            1,
            "case-insensitive dedup, blanks skipped"
        );
        assert!(
            !agent.cfg.deny_tools.iter().any(|d| d == "shell"),
            "the parent config must stay untouched"
        );
        let schemas = child.toolbox.schemas();
        assert!(
            !schemas.iter().any(|s| s["name"] == "shell"),
            "a denied tool must vanish from the child schema list"
        );
        assert_eq!(
            extra_denies(&json!({"deny_tools": ["a", "b"]})),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(extra_denies(&json!({})).is_empty());
    }

    #[test]
    fn a_child_workspace_stays_inside_the_parent_workspace() {
        let parent = std::path::Path::new("/work");
        assert_eq!(
            child_workspace(parent, "sub/dir").unwrap(),
            std::path::PathBuf::from("/work/sub/dir")
        );
        assert!(child_workspace(parent, "/etc").is_err(), "absolute refused");
        assert!(
            child_workspace(parent, "../outside").is_err(),
            "escape refused"
        );
        assert!(child_workspace(parent, "a/../../b").is_err());
        assert!(child_workspace(parent, "").is_err());
        assert!(child_workspace(parent, "   ").is_err());

        let cfg = make_cfg("session");
        let agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        let child = agent
            .make_child_denying("ghost", &[], Some("nest"))
            .unwrap();
        assert_eq!(child.cfg.workspace, cfg.workspace.join("nest"));
        assert!(
            child.cfg.workspace.is_dir(),
            "the child workspace is created"
        );
        let err = match agent.make_child_denying("ghost", &[], Some("../up")) {
            Ok(_) => panic!("an escaping workspace must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("may not contain"), "{err}");
    }

    #[test]
    fn children_cannot_spawn_grandchildren() {
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        agent.depth = MAX_SPAWN_DEPTH;
        let err = match agent.make_child("ghost") {
            Ok(_) => panic!("a child at max depth must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("not allowed at this depth"), "{err}");
        let out = agent.run_subtask(&json!({"prompt": "go"}));
        assert!(out.starts_with("error:"), "{out}");
        assert!(out.contains("depth"), "{out}");

        let dir = std::env::temp_dir().join(format!("phx-send-depth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        agent.agents_dir = Some(dir.clone());
        crate::sessions::save(
            &dir,
            "peer",
            &[Msg::User {
                content: "hello".into(),
                images: Vec::new(),
            }],
        )
        .unwrap();
        let out = agent.agent_send(&json!({"name": "peer", "message": "ping"}));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.starts_with("error:") && out.contains("depth"),
            "a send hop past the depth cap must be refused, closing ping-pong loops: {out}"
        );
    }

    #[test]
    fn a_child_near_its_budget_is_told_to_wrap_up() {
        struct OneLiner;
        impl ChatBackend for OneLiner {
            fn chat(
                &mut self,
                _cfg: &Config,
                _system: &str,
                history: &[Msg],
                _tools: &[Value],
            ) -> Result<Reply, ProviderError> {
                let warned = history.iter().any(
                    |m| matches!(m, Msg::User { content, .. } if content.contains("[time budget]")),
                );
                Ok(Reply::text_only(if warned {
                    "wrapped up"
                } else {
                    "kept going"
                }))
            }
        }
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(OneLiner));
        agent.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
        let out = agent.run("long task");
        assert_eq!(out, "wrapped up", "the wrap-up note must reach the model");
        assert!(agent.deadline_warned);

        let mut fresh = build(&cfg, Box::new(OneLiner));
        fresh.deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(CHILD_DEADLINE_SECS));
        let out = fresh.run("long task");
        assert_eq!(out, "kept going", "far from the budget no note is injected");
        assert!(!fresh.deadline_warned);
    }

    #[test]
    fn child_wall_clock_deadline_stops_the_run() {
        let cfg = make_cfg("session");
        let mut agent = build(&cfg, Box::new(LoopProvider));
        agent.deadline = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let out = agent.run("go");
        assert!(out.contains("wall-clock budget"), "{out}");
    }

    #[test]
    fn parent_has_no_deadline_and_children_do() {
        let cfg = make_cfg("session");
        let agent = build(&cfg, Box::new(FakeProvider { calls: 0 }));
        assert!(agent.deadline.is_none(), "parent must not be time-boxed");
        let child = match agent.make_child("ghost") {
            Ok(c) => c,
            Err(e) => panic!("child creation failed: {e}"),
        };
        assert!(child.deadline.is_some(), "children must be time-boxed");
        assert_eq!(child.depth, 1);
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
    fn oversized_messages_are_omitted_not_sent_to_summarizer() {
        let huge = "x".repeat(600_000);
        let msgs = vec![
            Msg::User {
                content: "small".into(),
                images: Vec::new(),
            },
            Msg::Tool {
                id: "t1".into(),
                content: huge,
            },
        ];
        let (small, notes) = split_oversized(&msgs, 200_000);
        assert_eq!(small.len(), 1, "huge message must be pulled out");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("large tool message"), "{:?}", notes);
        assert!(notes[0].contains("K tokens"), "{:?}", notes);
    }

    #[test]
    fn chunking_splits_by_budget_and_never_drops_a_message() {
        let msgs: Vec<Msg> = (0..40)
            .map(|i| Msg::User {
                content: format!("{i}").repeat(4000),
                images: Vec::new(),
            })
            .collect();
        let chunks = chunk_by_max_tokens(&msgs, 8000);
        assert!(chunks.len() > 1, "must split into several chunks");
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, msgs.len(), "chunking must not lose messages");
    }

    #[test]
    fn oversized_history_still_compacts_via_multiple_summary_calls() {
        struct CountingSummarizer {
            calls: std::sync::Arc<AtomicUsize>,
        }
        impl ChatBackend for CountingSummarizer {
            fn chat(
                &mut self,
                _c: &Config,
                system: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                if system.starts_with("Summarize this conversation") {
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    return Ok(Reply::text_only("part summary"));
                }
                Ok(Reply::text_only("answer"))
            }
        }
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let mut cfg = make_cfg("session");
        cfg.model = "who-knows".into();
        let mut agent = build(
            &cfg,
            Box::new(CountingSummarizer {
                calls: calls.clone(),
            }),
        );
        for i in 0..24 {
            agent.history.push(Msg::User {
                content: format!("m{i} ").repeat(30_000),
                images: Vec::new(),
            });
        }
        let before = agent.history.len();
        let out = agent.compact_now("");
        assert!(out.starts_with("compacted"), "{out}");
        assert!(
            calls.load(Ordering::SeqCst) > 1,
            "history far over the window must summarize in parts, got {} call(s)",
            calls.load(Ordering::SeqCst)
        );
        assert!(agent.history.len() < before);
    }

    #[test]
    fn context_windows_and_token_estimates() {
        assert_eq!(model_context_tokens("claude-sonnet-5"), 1_000_000);
        assert_eq!(model_context_tokens("gpt-5.4"), 272_000);
        assert_eq!(model_context_tokens_for("openai", "gpt-5.6-sol"), 1_050_000);
        assert_eq!(model_context_tokens("llama3.3"), 128_000);
        assert_eq!(model_context_tokens("who-knows"), DEFAULT_CONTEXT_TOKENS);

        let history = vec![
            Msg::User {
                content: "a".repeat(400),
                images: Vec::new(),
            },
            Msg::Tool {
                id: "1".into(),
                content: "b".repeat(400),
            },
        ];
        assert_eq!(estimate_tokens(&history), 200);
        let with_image = vec![Msg::User {
            content: String::new(),
            images: vec![("image/png".into(), "x".into())],
        }];
        assert_eq!(estimate_tokens(&with_image), 1000);
    }

    #[test]
    fn compaction_triggers_on_token_pressure_without_message_threshold() {
        struct Summarizer;
        impl ChatBackend for Summarizer {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                Ok(Reply::text_only("summary of earlier work"))
            }
        }
        let cfg = Config {
            compact_after: 0,
            model: "llama3.3".into(),
            privacy: "session".into(),
            workspace: std::env::temp_dir(),
            ..Config::default()
        };
        let toolbox = Toolbox::new(&cfg, Memory::new("ghost"), None, None).unwrap();
        let mut agent = Agent::new(cfg, Box::new(Summarizer), toolbox);
        let big = "x".repeat(400_000);
        agent.history = vec![
            Msg::User {
                content: big.clone(),
                images: Vec::new(),
            },
            Msg::Assistant {
                content: big,
                tool_calls: Vec::new(),
            },
        ];
        agent.compact_if_needed();
        assert_eq!(agent.history.len(), 2);
        match &agent.history[0] {
            Msg::User { content, .. } => {
                assert!(content.starts_with("[conversation summary]"), "{content}")
            }
            other => panic!("expected summary first: {other:?}"),
        }
    }

    #[test]
    fn tool_loop_breaks_only_when_outcomes_stop_changing() {
        struct Repeater {
            calls: std::cell::Cell<u32>,
        }
        impl ChatBackend for Repeater {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                self.calls.set(self.calls.get() + 1);
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("t{}", self.calls.get()),
                        name: "list_dir".into(),
                        args: serde_json::json!({"path": "."}),
                    }],
                    usage: Usage::default(),
                })
            }
        }
        let cfg = Config {
            workspace: tmpdir(),
            ..Config::default()
        };
        let toolbox = Toolbox::new(&cfg, Memory::new("ghost"), None, None).unwrap();
        let backend = Repeater {
            calls: std::cell::Cell::new(0),
        };
        let mut agent = Agent::new(cfg, Box::new(backend), toolbox);
        let out = agent.run("go");
        assert!(
            out.starts_with(crate::loop_detect::LOOP_BLOCK_PREFIX),
            "{out}"
        );
        assert!(out.contains("list_dir"), "{out}");
        let warned = agent.history.iter().any(|m| match m {
            Msg::Tool { content, .. } => content.contains("loop warning"),
            _ => false,
        });
        assert!(warned, "a warning must be delivered before the hard block");
    }

    #[test]
    fn repeated_calls_with_changing_results_are_never_blocked() {
        struct Progressing {
            calls: std::cell::Cell<u32>,
        }
        impl ChatBackend for Progressing {
            fn chat(
                &mut self,
                _c: &Config,
                _s: &str,
                _h: &[Msg],
                _t: &[Value],
            ) -> Result<Reply, ProviderError> {
                let n = self.calls.get() + 1;
                self.calls.set(n);
                if n > 25 {
                    return Ok(Reply::text_only("finished"));
                }
                Ok(Reply {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("t{n}"),
                        name: "write_file".into(),
                        args: serde_json::json!({"path": format!("f{n}.txt"), "content": "x"}),
                    }],
                    usage: Usage::default(),
                })
            }
        }
        let cfg = Config {
            workspace: tmpdir(),
            confirm_shell: false,
            ..Config::default()
        };
        let toolbox = Toolbox::new(&cfg, Memory::new("ghost"), None, None).unwrap();
        let mut agent = Agent::new(
            cfg,
            Box::new(Progressing {
                calls: std::cell::Cell::new(0),
            }),
            toolbox,
        );
        let out = agent.run("go");
        assert_eq!(out, "finished", "25 productive calls must not be blocked");
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
        agent.skills = vec![crate::skills::read(
            "---\nname: git-flow\nkeywords: commit\n---\nAlways rebase first.",
        )
        .unwrap()];
        assert_eq!(agent.run("how do I commit"), "sys:true");
        assert_eq!(agent.run("what is rust"), "sys:false");
    }
}
