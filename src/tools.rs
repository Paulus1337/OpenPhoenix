use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};

use crate::config::Config;
use crate::memory::Memory;
use crate::security::{redact, CommandGate, PathJail, SecurityError};

pub const MAX_OUT: usize = 16_000;
pub const MAX_FETCH: u64 = 200_000;
pub const SHELL_TIMEOUT_SECS: u64 = 120;

pub type ConfirmFn = Box<dyn Fn(&str) -> bool>;
pub type EventFn = Box<dyn Fn(&str, &Value)>;

pub enum ToolError {
    Blocked(String),
    BadArgs(String),
    Other(String),
}

impl From<SecurityError> for ToolError {
    fn from(e: SecurityError) -> Self {
        ToolError::Blocked(e.0)
    }
}

pub fn clip(text: String) -> String {
    let n = text.chars().count();
    if n <= MAX_OUT {
        return text;
    }
    let cut: String = text.chars().take(MAX_OUT).collect();
    format!("{cut}\n…[truncated {} chars]", n - MAX_OUT)
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub id: u64,
    pub command: String,
}

pub struct Toolbox {
    cfg: Config,
    pub memory: Memory,
    confirm: Option<ConfirmFn>,
    on_event: Option<EventFn>,
    jail: PathJail,
    gate: CommandGate,
    memory_tools: bool,
    pending: std::cell::RefCell<Vec<Pending>>,
    next_id: std::cell::Cell<u64>,
    browser: std::cell::RefCell<Option<crate::browser::Browser>>,
}

const TOOL_NAMES: [&str; 15] = [
    "shell",
    "read_file",
    "write_file",
    "list_dir",
    "http_get",
    "web_search",
    "image_generate",
    "speak",
    "video_generate",
    "music_generate",
    "canvas_present",
    "canvas_hide",
    "task_add",
    "task_list",
    "task_update",
];

const BROWSER_TOOLS: [&str; 7] = [
    "browser_open",
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_screenshot",
    "browser_close",
];

impl Toolbox {
    pub fn new(
        cfg: &Config,
        memory: Memory,
        confirm: Option<ConfirmFn>,
        on_event: Option<EventFn>,
    ) -> Result<Self, String> {
        let jail = PathJail::new(&cfg.workspace, cfg.allow_outside_workspace)
            .map_err(|e| e.to_string())?;
        let gate = CommandGate::new(&cfg.deny_commands).map_err(|e| e.to_string())?;

        let mut memory = memory;
        let memory_tools = memory.enabled();
        if memory_tools && cfg.mem_embeddings {
            memory.embed = Some(crate::embeddings::EmbedConfig::from_config(cfg));
        }
        Ok(Toolbox {
            cfg: cfg.clone(),
            memory,
            confirm,
            on_event,
            jail,
            gate,
            memory_tools,
            pending: std::cell::RefCell::new(Vec::new()),
            next_id: std::cell::Cell::new(1),
            browser: std::cell::RefCell::new(None),
        })
    }

    pub fn pending_list(&self) -> String {
        let q = self.pending.borrow();
        if q.is_empty() {
            return "nothing pending".to_string();
        }
        q.iter()
            .map(|p| format!("#{} `{}`", p.id, p.command))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn approve(&self, id: u64) -> String {
        let cmd = {
            let mut q = self.pending.borrow_mut();
            match q.iter().position(|p| p.id == id) {
                Some(i) => q.remove(i).command,
                None => return format!("no pending command #{id}"),
            }
        };
        match run_shell(&cmd, self.jail.workspace()) {
            Ok((out, code)) => {
                let trimmed = out.trim();
                if trimmed.is_empty() {
                    format!("#{id} `{cmd}` → (exit {code}, no output)")
                } else {
                    clip(format!("#{id} `{cmd}` →\n{}", redact(trimmed)))
                }
            }
            Err(ToolError::Blocked(e)) => format!("#{id} `{cmd}` → blocked: {e}"),
            Err(ToolError::BadArgs(e)) | Err(ToolError::Other(e)) => {
                format!("#{id} `{cmd}` → error: {e}")
            }
        }
    }

    pub fn deny(&self, id: u64) -> String {
        let mut q = self.pending.borrow_mut();
        match q.iter().position(|p| p.id == id) {
            Some(i) => {
                let p = q.remove(i);
                format!("denied #{} `{}`", p.id, p.command)
            }
            None => format!("no pending command #{id}"),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.borrow().len()
    }

    pub fn pending_entries(&self) -> Vec<(u64, String)> {
        self.pending
            .borrow()
            .iter()
            .map(|p| (p.id, p.command.clone()))
            .collect()
    }

    pub fn schemas(&self) -> Vec<Value> {
        let mut out = vec![
            json!({"name": "shell",
                   "description": "Run a shell command in the workspace. Returns stdout+stderr.",
                   "parameters": {"type": "object",
                       "properties": {"command": {"type": "string",
                                                  "description": "command to run"}},
                       "required": ["command"]}}),
            json!({"name": "read_file",
                   "description": "Read a text file from the workspace.",
                   "parameters": {"type": "object",
                       "properties": {"path": {"type": "string"}},
                       "required": ["path"]}}),
            json!({"name": "write_file",
                   "description": "Write/overwrite a text file in the workspace.",
                   "parameters": {"type": "object",
                       "properties": {"path": {"type": "string"},
                                      "content": {"type": "string"}},
                       "required": ["path", "content"]}}),
            json!({"name": "list_dir",
                   "description": "List a workspace directory.",
                   "parameters": {"type": "object",
                       "properties": {"path": {"type": "string",
                                               "description": "default: workspace root"}},
                       "required": []}}),
            json!({"name": "http_get",
                   "description": "Fetch a URL, return readable text (tags stripped).",
                   "parameters": {"type": "object",
                       "properties": {"url": {"type": "string"}},
                       "required": ["url"]}}),
            json!({"name": "web_search",
                   "description": "Search the web (DuckDuckGo). Returns top results.",
                   "parameters": {"type": "object",
                       "properties": {"query": {"type": "string"}},
                       "required": ["query"]}}),
        ];
        if self.cfg.media_images {
            out.push(json!({"name": "image_generate",
                "description": "Generate an image from a text prompt. Returns a MEDIA:<path> line; \
                 include that line verbatim on its own line in your final reply to deliver the image.",
                "parameters": {"type": "object",
                    "properties": {"prompt": {"type": "string",
                                              "description": "what to draw"}},
                    "required": ["prompt"]}}));
        }
        if self.cfg.media_tts {
            out.push(json!({"name": "speak",
                "description": "Turn text into speech audio. Returns a MEDIA:<path> line; \
                 include that line verbatim on its own line in your final reply to deliver the voice note.",
                "parameters": {"type": "object",
                    "properties": {"text": {"type": "string",
                                            "description": "exact words to speak"}},
                    "required": ["text"]}}));
        }
        if self.cfg.media_video {
            out.push(json!({"name": "video_generate",
                "description": "Generate a short video clip from a text prompt. Returns a \
                 MEDIA:<path> line; include that line verbatim on its own line in your \
                 final reply to deliver the video.",
                "parameters": {"type": "object",
                    "properties": {"prompt": {"type": "string",
                                              "description": "what to film"}},
                    "required": ["prompt"]}}));
        }
        if self.cfg.media_music {
            out.push(json!({"name": "music_generate",
                "description": "Generate a music clip from a text prompt (style, genre, mood, \
                 tempo). Returns a MEDIA:<path> line; include that line verbatim on its \
                 own line in your final reply to deliver the audio.",
                "parameters": {"type": "object",
                    "properties": {"prompt": {"type": "string",
                                              "description": "style, genre, mood"}},
                    "required": ["prompt"]}}));
        }
        if self.cfg.canvas_enabled {
            out.push(json!({"name": "canvas_present",
                "description": "Show an HTML document on the canvas surface (GET /canvas on \
                 the HTTP server). The page live-reloads when you present again. Inline \
                 style and script are allowed; external loads are blocked.",
                "parameters": {"type": "object",
                    "properties": {"html": {"type": "string",
                                            "description": "full or partial HTML document"}},
                    "required": ["html"]}}));
            out.push(json!({"name": "canvas_hide",
                "description": "Clear the canvas surface back to its empty placeholder.",
                "parameters": {"type": "object", "properties": {}, "required": []}}));
        }
        if self.cfg.board_enabled {
            out.push(json!({"name": "task_add",
                "description": "Add a durable card to the task board. Cards survive restarts.",
                "parameters": {"type": "object",
                    "properties": {"title": {"type": "string"},
                                   "notes": {"type": "string"},
                                   "priority": {"type": "string",
                                                "description": "low, normal (default), or high"}},
                    "required": ["title"]}}));
            out.push(json!({"name": "task_list",
                "description": "List task board cards, optionally filtered by status \
                 (todo, doing, blocked, done).",
                "parameters": {"type": "object",
                    "properties": {"status": {"type": "string"}},
                    "required": []}}));
            out.push(json!({"name": "task_update",
                "description": "Update a card by id: status (todo, doing, blocked, done), \
                 title, notes, or priority. Only the fields you pass change.",
                "parameters": {"type": "object",
                    "properties": {"id": {"type": "integer"},
                                   "status": {"type": "string"},
                                   "title": {"type": "string"},
                                   "notes": {"type": "string"},
                                   "priority": {"type": "string"}},
                    "required": ["id"]}}));
        }
        if self.cfg.browser_enabled {
            out.push(json!({"name": "browser_open",
                "description": "Open a web page in the managed browser (starts it if needed). \
                 http(s) URLs only.",
                "parameters": {"type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url"]}}));
            out.push(json!({"name": "browser_navigate",
                "description": "Navigate the current browser tab to a URL. http(s) only.",
                "parameters": {"type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url"]}}));
            out.push(json!({"name": "browser_snapshot",
                "description": "Accessibility outline of the current page with element refs \
                 like [ref=e12]. Use those refs with browser_click / browser_type.",
                "parameters": {"type": "object", "properties": {}, "required": []}}));
            out.push(json!({"name": "browser_click",
                "description": "Click an element by snapshot ref (like e12).",
                "parameters": {"type": "object",
                    "properties": {"ref": {"type": "string",
                                            "description": "ref from browser_snapshot"}},
                    "required": ["ref"]}}));
            out.push(json!({"name": "browser_type",
                "description": "Focus an element by snapshot ref and type text into it. \
                 Set submit=true to press Enter afterwards.",
                "parameters": {"type": "object",
                    "properties": {"ref": {"type": "string"},
                                   "text": {"type": "string"},
                                   "submit": {"type": "boolean"}},
                    "required": ["ref", "text"]}}));
            out.push(json!({"name": "browser_screenshot",
                "description": "PNG screenshot of the current page. Returns a MEDIA:<path> line; \
                 include that line verbatim on its own line in your final reply to deliver it.",
                "parameters": {"type": "object", "properties": {}, "required": []}}));
            out.push(json!({"name": "browser_close",
                "description": "Close the managed browser session.",
                "parameters": {"type": "object", "properties": {}, "required": []}}));
        }
        if self.memory_tools {
            out.push(json!({"name": "remember",
                "description": "Store a durable note in the user-auditable memory file.",
                "parameters": {"type": "object",
                    "properties": {"note": {"type": "string"}},
                    "required": ["note"]}}));
            out.push(json!({"name": "recall",
                "description": "Search durable memory notes.",
                "parameters": {"type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]}}));
        }
        out
    }

    pub fn run(&self, name: &str, args: &Value) -> String {
        let known = TOOL_NAMES.contains(&name)
            || (self.memory_tools && (name == "remember" || name == "recall"))
            || (self.cfg.browser_enabled && BROWSER_TOOLS.contains(&name));
        if !known {
            return format!("error: unknown tool '{name}'");
        }
        if let Some(ev) = &self.on_event {
            ev(name, args);
        }
        let result = match name {
            "shell" => self.t_shell(args),
            "read_file" => self.t_read_file(args),
            "write_file" => self.t_write_file(args),
            "list_dir" => self.t_list_dir(args),
            "http_get" => self.t_http_get(args),
            "web_search" => self.t_web_search(args),
            "image_generate" => self.t_image_generate(args),
            "speak" => self.t_speak(args),
            "video_generate" => self.t_video_generate(args),
            "music_generate" => self.t_music_generate(args),
            "canvas_present" | "canvas_hide" => self.t_canvas(name, args),
            "task_add" | "task_list" | "task_update" => self.t_task(name, args),
            "browser_open" | "browser_navigate" | "browser_snapshot" | "browser_click"
            | "browser_type" | "browser_screenshot" | "browser_close" => self.t_browser(name, args),
            "remember" => req_str(args, "note").map(|n| self.memory.remember(n)),
            "recall" => req_str(args, "query").map(|q| self.memory.recall(q)),
            _ => unreachable!(),
        };
        match result {
            Ok(s) => clip(s),
            Err(ToolError::Blocked(e)) => format!("blocked: {e}"),
            Err(ToolError::BadArgs(e)) => format!("error: bad arguments for {name}: {e}"),
            Err(ToolError::Other(e)) => format!("error: {e}"),
        }
    }

    fn t_shell(&self, args: &Value) -> Result<String, ToolError> {
        let command = req_str(args, "command")?;
        self.gate.check(command)?;
        if self.cfg.confirm_shell {
            if let Some(confirm) = &self.confirm {
                if !confirm(command) {
                    return Ok("user declined command".into());
                }
            }
        }

        if self.cfg.approvals && self.confirm.is_none() {
            let id = self.next_id.get();
            self.next_id.set(id + 1);
            self.pending.borrow_mut().push(Pending {
                id,
                command: command.to_string(),
            });
            return Ok(format!(
                "approval required: command #{id} is queued, not executed. \
Tell the user to review and send /approve {id} to run it or /deny {id} to drop it. \
Queued command: `{command}`"
            ));
        }
        let (out, code) = run_shell(command, self.jail.workspace())?;
        let trimmed = out.trim();
        if trimmed.is_empty() {
            Ok(format!("(exit {code}, no output)"))
        } else {
            Ok(redact(trimmed))
        }
    }

    fn t_read_file(&self, args: &Value) -> Result<String, ToolError> {
        let path = req_str(args, "path")?;
        let p = self.jail.resolve(path)?;
        let bytes = fs::read(&p).map_err(|e| ToolError::Other(e.to_string()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn t_write_file(&self, args: &Value) -> Result<String, ToolError> {
        let path = req_str(args, "path")?;
        let content = req_str(args, "content")?;
        let p = self.jail.resolve(path)?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).map_err(|e| ToolError::Other(e.to_string()))?;
        }
        fs::write(&p, content).map_err(|e| ToolError::Other(e.to_string()))?;
        Ok(format!(
            "wrote {} chars to {}",
            content.chars().count(),
            p.display()
        ))
    }

    fn t_image_generate(&self, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.media_images {
            return Err(ToolError::Blocked(
                "media.images is disabled in config".into(),
            ));
        }
        let prompt = req_str(args, "prompt")?;
        let bytes = crate::media::generate_image(&self.cfg, prompt).map_err(ToolError::Other)?;
        self.save_media("img", "png", &bytes)
    }

    fn t_speak(&self, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.media_tts {
            return Err(ToolError::Blocked("media.tts is disabled in config".into()));
        }
        let text = req_str(args, "text")?;
        let bytes = crate::media::speak(&self.cfg, text).map_err(ToolError::Other)?;
        self.save_media("tts", "mp3", &bytes)
    }

    fn t_video_generate(&self, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.media_video {
            return Err(ToolError::Blocked(
                "media.video is disabled in config".into(),
            ));
        }
        let prompt = req_str(args, "prompt")?;
        let bytes = crate::media::generate_video(&self.cfg, prompt).map_err(ToolError::Other)?;
        self.save_media("vid", "mp4", &bytes)
    }

    fn t_music_generate(&self, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.media_music {
            return Err(ToolError::Blocked(
                "media.music is disabled in config".into(),
            ));
        }
        let prompt = req_str(args, "prompt")?;
        let bytes = crate::media::generate_music(&self.cfg, prompt).map_err(ToolError::Other)?;
        self.save_media("music", "mp3", &bytes)
    }

    fn t_canvas(&self, name: &str, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.canvas_enabled {
            return Err(ToolError::Blocked("canvas.enabled is off in config".into()));
        }
        let path = crate::canvas::state_path();
        match name {
            "canvas_present" => {
                let html = req_str(args, "html")?;
                crate::canvas::present(&path, html).map_err(ToolError::Other)?;
                Ok("canvas updated; serving at /canvas on the HTTP port".into())
            }
            _ => {
                crate::canvas::hide(&path);
                Ok("canvas cleared".into())
            }
        }
    }

    fn t_task(&self, name: &str, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.board_enabled {
            return Err(ToolError::Blocked("board.enabled is off in config".into()));
        }
        let path = crate::config::home().join("board.json");
        let opt = |key: &str| args.get(key).and_then(Value::as_str);
        match name {
            "task_add" => {
                let title = req_str(args, "title")?;
                let id = crate::board::add(
                    &path,
                    title,
                    opt("notes").unwrap_or(""),
                    opt("priority").unwrap_or("normal"),
                )
                .map_err(ToolError::Other)?;
                Ok(format!("added card #{id}"))
            }
            "task_list" => crate::board::list(&path, opt("status")).map_err(ToolError::Other),
            _ => {
                let Some(id) = args.get("id").and_then(Value::as_u64) else {
                    return Err(ToolError::BadArgs("missing required 'id'".into()));
                };
                crate::board::update(
                    &path,
                    id,
                    opt("status"),
                    opt("title"),
                    opt("notes"),
                    opt("priority"),
                )
                .map_err(ToolError::Other)
            }
        }
    }

    fn save_media(&self, kind: &str, ext: &str, bytes: &[u8]) -> Result<String, ToolError> {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let rel = format!("media/{kind}-{ms}.{ext}");
        let p = self.jail.resolve(&rel)?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).map_err(|e| ToolError::Other(e.to_string()))?;
        }
        fs::write(&p, bytes).map_err(|e| ToolError::Other(e.to_string()))?;
        Ok(format!(
            "saved {} bytes. To deliver it, put this line alone in your final reply:\nMEDIA:{}",
            bytes.len(),
            p.display()
        ))
    }

    fn t_browser(&self, name: &str, args: &Value) -> Result<String, ToolError> {
        if !self.cfg.browser_enabled {
            return Err(ToolError::Blocked("browser is disabled in config".into()));
        }
        let mut slot = self.browser.borrow_mut();
        match name {
            "browser_open" | "browser_navigate" => {
                let url = req_str(args, "url")?;
                crate::browser::check_url(url).map_err(ToolError::Blocked)?;
                if slot.is_none() {
                    *slot =
                        Some(crate::browser::Browser::start(&self.cfg).map_err(ToolError::Other)?);
                }
                let b = slot.as_mut().expect("session just ensured");
                b.navigate(url).map_err(ToolError::Other)
            }
            "browser_close" => {
                if slot.take().is_some() {
                    Ok("browser closed".into())
                } else {
                    Ok("no browser session to close".into())
                }
            }
            _ => {
                let b = slot.as_mut().ok_or_else(|| {
                    ToolError::Other("no browser session; use browser_open first".into())
                })?;
                match name {
                    "browser_snapshot" => {
                        b.snapshot().map(|s| redact(&s)).map_err(ToolError::Other)
                    }
                    "browser_click" => {
                        let r = req_str(args, "ref")?;
                        b.click(r).map_err(ToolError::Other)
                    }
                    "browser_type" => {
                        let r = req_str(args, "ref")?;
                        let text = req_str(args, "text")?;
                        let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
                        b.type_text(r, text, submit).map_err(ToolError::Other)
                    }
                    "browser_screenshot" => {
                        let png = b.screenshot_png().map_err(ToolError::Other)?;
                        self.save_media("shot", "png", &png)
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    fn t_list_dir(&self, args: &Value) -> Result<String, ToolError> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let p = self.jail.resolve(path)?;
        let mut entries: Vec<_> = fs::read_dir(&p)
            .map_err(|e| ToolError::Other(e.to_string()))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        let mut rows = Vec::new();
        for child in entries {
            let is_dir = child.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let kind = if is_dir { "dir " } else { "file" };
            let size = if is_dir {
                "-".to_string()
            } else {
                child
                    .metadata()
                    .map(|m| m.len().to_string())
                    .unwrap_or_else(|_| "-".into())
            };
            rows.push(format!(
                "{kind} {size:>10} {}",
                child.file_name().to_string_lossy()
            ));
        }
        if rows.is_empty() {
            Ok("(empty)".into())
        } else {
            Ok(rows.join("\n"))
        }
    }

    fn t_http_get(&self, args: &Value) -> Result<String, ToolError> {
        let url = req_str(args, "url")?;
        let raw = fetch(url)?;
        let text = strip_html(&raw);
        if text.is_empty() {
            Ok("(empty page)".into())
        } else {
            Ok(text)
        }
    }

    fn t_web_search(&self, args: &Value) -> Result<String, ToolError> {
        let query = req_str(args, "query")?;
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            percent_encode(query)
        );
        let raw = fetch(&url)?;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"<a[^>]+class="result__a"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap()
        });
        static RE_TAG: OnceLock<Regex> = OnceLock::new();
        let re_tag = RE_TAG.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
        let mut results = Vec::new();
        for m in re.captures_iter(&raw) {
            let mut href = m.get(1).map(|x| x.as_str()).unwrap_or("").to_string();
            let title = re_tag.replace_all(m.get(2).map(|x| x.as_str()).unwrap_or(""), "");
            if href.contains("uddg=") {
                let tail = href.split("uddg=").last().unwrap_or("");
                href = percent_decode(tail.split('&').next().unwrap_or(""));
            }
            results.push(format!("- {}\n  {href}", unescape_html(&title).trim()));
            if results.len() >= 8 {
                break;
            }
        }
        if results.is_empty() {
            Ok("no results".into())
        } else {
            Ok(results.join("\n"))
        }
    }
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::BadArgs(format!("missing required '{key}'")))
}

fn run_shell(command: &str, cwd: &Path) -> Result<(String, i32), ToolError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ToolError::Other(e.to_string()))?;
    let mut so = child.stdout.take().expect("piped stdout");
    let mut se = child.stderr.take().expect("piped stderr");
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(SHELL_TIMEOUT_SECS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ToolError::Other(format!(
                        "command timed out after {SHELL_TIMEOUT_SECS}s"
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(ToolError::Other(e.to_string())),
        }
    };
    let mut out = String::from_utf8_lossy(&ho.join().unwrap_or_default()).into_owned();
    out.push_str(&String::from_utf8_lossy(&he.join().unwrap_or_default()));
    Ok((out, status.code().unwrap_or(-1)))
}

fn fetch(url: &str) -> Result<String, ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::Blocked("only http(s) URLs allowed".into()));
    }
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(30))
        .set("User-Agent", "OpenPhoenix/0.1")
        .call()
        .map_err(|e| ToolError::Other(e.to_string()))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_FETCH)
        .read_to_end(&mut buf)
        .map_err(|e| ToolError::Other(e.to_string()))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn strip_html(raw: &str) -> String {
    static RE_BLOCK: OnceLock<Regex> = OnceLock::new();
    static RE_TAG: OnceLock<Regex> = OnceLock::new();
    static RE_WS: OnceLock<Regex> = OnceLock::new();
    let re_block = RE_BLOCK.get_or_init(|| {
        Regex::new(r"(?is)<script.*?</script>|<style.*?</style>|<noscript.*?</noscript>").unwrap()
    });
    let re_tag = RE_TAG.get_or_init(|| Regex::new(r"(?s)<[^>]+>").unwrap());
    let re_ws = RE_WS.get_or_init(|| Regex::new(r"\s+").unwrap());
    let text = re_block.replace_all(raw, " ");
    let text = re_tag.replace_all(&text, " ");
    let text = re_ws.replace_all(&text, " ");
    unescape_html(&text).trim().to_string()
}

fn unescape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let end = rest.as_bytes().iter().take(12).position(|&b| b == b';');
        match end {
            Some(e) => {
                let entity = &rest[1..e];
                let decoded: Option<String> = match entity {
                    "amp" => Some("&".into()),
                    "lt" => Some("<".into()),
                    "gt" => Some(">".into()),
                    "quot" => Some("\"".into()),
                    "apos" => Some("'".into()),
                    "nbsp" => Some("\u{a0}".into()),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                            .map(String::from)
                    }
                    _ if entity.starts_with('#') => entity[1..]
                        .parse::<u32>()
                        .ok()
                        .and_then(char::from_u32)
                        .map(String::from),
                    _ => None,
                };
                match decoded {
                    Some(d) => {
                        out.push_str(&d);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'~' | b'-' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-tools-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_box(privacy: &str) -> Toolbox {
        let ws = tmpdir();
        let cfg = Config {
            workspace: ws.clone(),
            confirm_shell: false,
            approvals: false,
            privacy: privacy.into(),
            ..Config::default()
        };
        Toolbox::new(&cfg, Memory::with_home(privacy, &ws), None, None).unwrap()
    }

    fn approval_box() -> Toolbox {
        let ws = tmpdir();
        let cfg = Config {
            workspace: ws.clone(),
            confirm_shell: false,
            approvals: true,
            privacy: "ghost".into(),
            ..Config::default()
        };
        Toolbox::new(&cfg, Memory::with_home("ghost", &ws), None, None).unwrap()
    }

    #[test]
    fn approvals_queue_instead_of_running() {
        let tb = approval_box();
        let out = tb.run("shell", &serde_json::json!({"command": "echo hi"}));
        assert!(out.contains("approval required"));
        assert!(out.contains("#1"));
        assert!(out.contains("echo hi"));
        assert_eq!(tb.pending_count(), 1);
        assert!(tb.pending_list().contains("#1 `echo hi`"));
    }

    #[test]
    fn approve_runs_and_removes() {
        let tb = approval_box();
        tb.run("shell", &serde_json::json!({"command": "echo hi"}));
        let out = tb.approve(1);
        assert!(out.contains("hi"), "{out}");
        assert_eq!(tb.pending_count(), 0);
        assert!(tb.approve(1).contains("no pending command"));
    }

    #[test]
    fn deny_drops_without_running() {
        let tb = approval_box();
        tb.run(
            "shell",
            &serde_json::json!({"command": "echo hi > proof.txt"}),
        );
        let out = tb.deny(1);
        assert!(out.contains("denied"));
        assert_eq!(tb.pending_count(), 0);
        assert!(!tb.jail.workspace().join("proof.txt").exists());
    }

    #[test]
    fn gate_blocks_before_queue() {
        let tb = approval_box();
        let out = tb.run("shell", &serde_json::json!({"command": "rm -rf /"}));
        assert!(out.contains("blocked"));
        assert_eq!(tb.pending_count(), 0);
    }

    #[test]
    fn approvals_off_runs_directly() {
        let tb = make_box("ghost");
        let out = tb.run("shell", &serde_json::json!({"command": "echo direct"}));
        assert!(out.contains("direct"));
        assert_eq!(tb.pending_count(), 0);
    }

    #[test]
    fn ids_increment_across_queue() {
        let tb = approval_box();
        tb.run("shell", &serde_json::json!({"command": "echo a"}));
        tb.run("shell", &serde_json::json!({"command": "echo b"}));
        tb.deny(1);
        tb.run("shell", &serde_json::json!({"command": "echo c"}));
        let list = tb.pending_list();
        assert!(list.contains("#2") && list.contains("#3"));
        assert!(!list.contains("#1 "));
    }

    #[test]
    fn write_read_list_roundtrip() {
        let b = make_box("session");
        let out = b.run("write_file", &json!({"path": "x.txt", "content": "abc"}));
        assert!(out.starts_with("wrote 3 chars"));
        assert_eq!(b.run("read_file", &json!({"path": "x.txt"})), "abc");
        assert!(b.run("list_dir", &json!({})).contains("x.txt"));
    }

    #[test]
    fn shell_runs_and_redacts() {
        let b = make_box("session");
        assert_eq!(b.run("shell", &json!({"command": "echo ok"})), "ok");
        let out = b.run(
            "shell",
            &json!({"command": format!("echo sk-{}", "b".repeat(30))}),
        );
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn shell_no_output_reports_exit() {
        let b = make_box("session");
        assert_eq!(
            b.run("shell", &json!({"command": "true"})),
            "(exit 0, no output)"
        );
        assert_eq!(
            b.run("shell", &json!({"command": "exit 3"})),
            "(exit 3, no output)"
        );
    }

    #[test]
    fn blocked_command() {
        let b = make_box("session");
        let out = b.run("shell", &json!({"command": "rm -rf /"}));
        assert!(out.starts_with("blocked:"), "got: {out}");
    }

    #[test]
    fn path_escape_blocked() {
        let b = make_box("session");
        let out = b.run("read_file", &json!({"path": "../../etc/passwd"}));
        assert!(out.starts_with("blocked:"), "got: {out}");
    }

    #[test]
    fn unknown_tool_and_bad_args() {
        let b = make_box("session");
        assert!(b.run("nope", &json!({})).contains("unknown tool"));
        let out = b.run("shell", &json!({}));
        assert!(out.contains("bad arguments"), "got: {out}");
    }

    #[test]
    fn memory_tools_only_in_recall() {
        let names = |b: &Toolbox| -> Vec<String> {
            b.schemas()
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_string())
                .collect()
        };
        let session = make_box("session");
        assert!(!names(&session).contains(&"remember".to_string()));
        assert!(session
            .run("remember", &json!({"note": "x"}))
            .contains("unknown tool"));
        let recall = make_box("recall");
        assert!(names(&recall).contains(&"remember".to_string()));
        assert!(names(&recall).contains(&"recall".to_string()));
        assert_eq!(recall.run("remember", &json!({"note": "hi"})), "noted");
    }

    fn browser_box() -> Toolbox {
        let ws = tmpdir();
        let cfg = Config {
            workspace: ws.clone(),
            confirm_shell: false,
            browser_enabled: true,
            ..Config::default()
        };
        Toolbox::new(&cfg, Memory::with_home("session", &ws), None, None).unwrap()
    }

    #[test]
    fn browser_tools_hidden_when_disabled() {
        let b = make_box("session");
        let names: Vec<String> = b
            .schemas()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with("browser_")));
        let out = b.run("browser_snapshot", &json!({}));
        assert!(out.contains("unknown tool"), "got: {out}");
    }

    #[test]
    fn browser_tools_registered_when_enabled() {
        let b = browser_box();
        let names: Vec<String> = b
            .schemas()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        for tool in BROWSER_TOOLS {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    #[test]
    fn browser_actions_need_a_session() {
        let b = browser_box();
        for (tool, args) in [
            ("browser_snapshot", json!({})),
            ("browser_click", json!({"ref": "e1"})),
            ("browser_type", json!({"ref": "e1", "text": "hi"})),
            ("browser_screenshot", json!({})),
        ] {
            let out = b.run(tool, &args);
            assert!(out.contains("no browser session"), "{tool}: {out}");
        }
        assert_eq!(
            b.run("browser_close", &json!({})),
            "no browser session to close"
        );
    }

    #[test]
    fn browser_open_refuses_bad_schemes() {
        let b = browser_box();

        for url in ["file:///etc/passwd", "data:text/html,x", "chrome://flags"] {
            let out = b.run("browser_open", &json!({"url": url}));
            assert!(out.starts_with("blocked:"), "{url}: {out}");
            let out = b.run("browser_navigate", &json!({"url": url}));
            assert!(out.starts_with("blocked:"), "{url}: {out}");
        }
    }

    #[test]
    fn browser_bad_args_reported() {
        let b = browser_box();
        let out = b.run("browser_open", &json!({}));
        assert!(out.contains("bad arguments"), "got: {out}");
        let out = b.run("browser_click", &json!({}));
        assert!(out.contains("no browser session"), "got: {out}");
    }

    #[test]
    fn non_http_url_blocked() {
        let b = make_box("session");
        let out = b.run("http_get", &json!({"url": "file:///etc/passwd"}));
        assert!(out.starts_with("blocked:"), "got: {out}");
    }

    #[test]
    fn clip_truncates_by_chars() {
        let s = "x".repeat(MAX_OUT + 100);
        let out = clip(s);
        assert!(out.contains("…[truncated 100 chars]"));
        assert_eq!(clip("short".into()), "short");
    }

    #[test]
    fn html_helpers() {
        assert_eq!(
            strip_html("<p>Hello <b>world</b></p><script>bad()</script>"),
            "Hello world"
        );
        assert_eq!(unescape_html("a &amp; b &lt;c&gt; &#65;"), "a & b <c> A");
        assert_eq!(percent_encode("a b/c"), "a%20b/c");
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
    }
}
