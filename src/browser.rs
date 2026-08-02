use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::Config;
use crate::media::b64_decode;
use crate::ws::{WsClient, WsMsg};

const CMD_TIMEOUT_SECS: u64 = 20;

#[cfg(unix)]
fn running_as_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            return meta.uid() == 0;
        }
    }
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

const NAV_TIMEOUT_SECS: u64 = 30;

const LAUNCH_TIMEOUT_SECS: u64 = 15;

const POLL_MS: u64 = 200;

const MAX_SNAPSHOT_DEPTH: usize = 60;
const MAX_TREE_RECURSION: usize = 500;
const MAX_SNAPSHOT_LINES: usize = 1500;
const MAX_SNAPSHOT_CHARS: usize = 60_000;

const INTERACTIVE_ROLES: [&str; 17] = [
    "button",
    "checkbox",
    "combobox",
    "link",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "searchbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "textbox",
    "treeitem",
];

const CONTENT_ROLES: [&str; 10] = [
    "article",
    "cell",
    "columnheader",
    "gridcell",
    "heading",
    "listitem",
    "main",
    "navigation",
    "region",
    "rowheader",
];

const STRUCTURAL_ROLES: [&str; 20] = [
    "application",
    "directory",
    "document",
    "generic",
    "grid",
    "group",
    "ignored",
    "list",
    "menu",
    "menubar",
    "none",
    "presentation",
    "row",
    "rowgroup",
    "table",
    "tablist",
    "toolbar",
    "tree",
    "treegrid",
    "webarea",
];

pub fn check_url(url: &str) -> Result<(), String> {
    check_url_with(url, false)
}

pub fn check_url_with(url: &str, allow_private: bool) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("only http(s) URLs allowed, got: {url}"));
    }
    crate::ssrf::check_url_with(url, allow_private)
}

pub fn check_cdp_host(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("ws://"))
        .ok_or_else(|| format!("cdp url must be http:// or ws://, got: {url}"))?;
    let hostport = rest.split('/').next().unwrap_or("");
    let host = if let Some(h) = hostport.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        hostport
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(hostport)
    };
    match host {
        "127.0.0.1" | "localhost" | "::1" => Ok(()),
        other => Err(format!("cdp endpoint must be localhost, got host: {other}")),
    }
}

pub fn parse_devtools_port(text: &str) -> Result<u16, String> {
    text.lines()
        .next()
        .unwrap_or("")
        .trim()
        .parse::<u16>()
        .map_err(|_| "DevToolsActivePort file has no port".to_string())
}

#[derive(Debug, PartialEq)]
pub enum CdpIncoming {
    Response {
        id: u64,
        result: Value,
        error: Option<String>,
    },

    Event {
        method: String,
        params: Value,
    },
}

pub fn encode_cmd(id: u64, method: &str, params: &Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

pub fn classify(text: &str) -> Result<CdpIncoming, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("bad cdp json: {e}"))?;
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        let error = v.get("error").map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown cdp error")
                .to_string()
        });
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        return Ok(CdpIncoming::Response { id, result, error });
    }
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        return Ok(CdpIncoming::Event {
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        });
    }
    Err("cdp message has neither id nor method".into())
}

#[derive(Debug, Clone)]
pub struct RefEntry {
    pub backend_node_id: i64,
    pub role: String,
    pub name: String,
}

#[derive(Debug)]
pub struct Snapshot {
    pub outline: String,
    pub refs: HashMap<String, RefEntry>,
}

fn ax_str(node: &Value, field: &str) -> String {
    node.get(field)
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn escape_name(name: &str) -> String {
    name.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

pub fn build_snapshot(ax: &Value) -> Snapshot {
    let nodes = ax.get("nodes").and_then(Value::as_array);
    let Some(nodes) = nodes else {
        return Snapshot {
            outline: "(no accessibility tree)".into(),
            refs: HashMap::new(),
        };
    };
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| n.get("nodeId").and_then(Value::as_str).map(|id| (id, n)))
        .collect();
    let root = nodes.iter().find(|n| n.get("parentId").is_none());
    let mut lines = Vec::new();
    let mut refs = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut next_ref = 0usize;
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(root) = root {
        walk(
            root,
            &by_id,
            0,
            0,
            &mut lines,
            &mut refs,
            &mut counts,
            &mut next_ref,
            &mut visited,
        );
    }
    if lines.is_empty() {
        lines.push("(empty page)".into());
    }
    let truncated = lines.len() > MAX_SNAPSHOT_LINES;
    if truncated {
        lines.truncate(MAX_SNAPSHOT_LINES);
    }
    let mut outline = lines.join("\n");
    if truncated {
        outline.push_str(&format!(
            "\n… outline truncated at {MAX_SNAPSHOT_LINES} lines; \
narrow the page or act on the refs above"
        ));
    }
    if outline.chars().count() > MAX_SNAPSHOT_CHARS {
        outline = outline.chars().take(MAX_SNAPSHOT_CHARS).collect();
        outline.push_str("\n… outline truncated");
    }
    refs.retain(|r, _| outline.contains(&format!("[ref={r}]")));
    Snapshot { outline, refs }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: &Value,
    by_id: &HashMap<&str, &Value>,
    depth: usize,
    recursion: usize,
    lines: &mut Vec<String>,
    refs: &mut HashMap<String, RefEntry>,
    counts: &mut HashMap<String, usize>,
    next_ref: &mut usize,
    visited: &mut std::collections::HashSet<String>,
) {
    if recursion > MAX_TREE_RECURSION || depth > MAX_SNAPSHOT_DEPTH {
        return;
    }
    if lines.len() > MAX_SNAPSHOT_LINES {
        return;
    }
    if let Some(id) = node.get("nodeId").and_then(Value::as_str) {
        if !visited.insert(id.to_string()) {
            return;
        }
    }
    let ignored = node
        .get("ignored")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let role = ax_str(node, "role").to_lowercase();
    let name = ax_str(node, "name");
    let interactive = INTERACTIVE_ROLES.contains(&role.as_str());
    let content = CONTENT_ROLES.contains(&role.as_str());
    let structural = STRUCTURAL_ROLES.contains(&role.as_str()) || role.is_empty();
    let skip_line = ignored || (structural && name.is_empty() && !interactive);
    let mut child_depth = depth;
    if !skip_line {
        let mut line = format!("{}- {role}", "  ".repeat(depth));
        if !name.is_empty() {
            line.push_str(&format!(" \"{}\"", escape_name(&name)));
        }
        let wants_ref = interactive || (content && !name.is_empty());
        let backend = node.get("backendDOMNodeId").and_then(Value::as_i64);
        if wants_ref {
            if let Some(backend_node_id) = backend {
                *next_ref += 1;
                let r = format!("e{next_ref}");
                let key = format!("{role}:{name}");
                let nth = counts.get(&key).copied().unwrap_or(0);
                *counts.entry(key).or_insert(0) += 1;
                line.push_str(&format!(" [ref={r}]"));
                if nth > 0 {
                    line.push_str(&format!(" [nth={nth}]"));
                }
                refs.insert(
                    r,
                    RefEntry {
                        backend_node_id,
                        role: role.clone(),
                        name: name.clone(),
                    },
                );
            }
        }
        let value = ax_str(node, "value");
        if !value.is_empty() {
            line.push_str(&format!(" value=\"{}\"", escape_name(&value)));
        }
        lines.push(line);
        child_depth = depth + 1;
    }
    if let Some(children) = node.get("childIds").and_then(Value::as_array) {
        for cid in children {
            if let Some(child) = cid.as_str().and_then(|id| by_id.get(id)) {
                walk(
                    child,
                    by_id,
                    child_depth,
                    recursion + 1,
                    lines,
                    refs,
                    counts,
                    next_ref,
                    visited,
                );
            }
        }
    }
}

pub struct Browser {
    child: Option<Child>,
    ws: WsClient,
    next_id: u64,
    page_enabled: bool,
    pub refs: HashMap<String, RefEntry>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.ws.send_close(1000);
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for_port_file(dir: &Path, deadline: Instant) -> Result<u16, String> {
    let path = dir.join("DevToolsActivePort");
    loop {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(port) = parse_devtools_port(&text) {
                return Ok(port);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "browser did not publish {} within {LAUNCH_TIMEOUT_SECS}s",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn page_ws_url(http_base: &str) -> Result<String, String> {
    check_cdp_host(http_base)?;
    let list_url = format!("{}/json/list", http_base.trim_end_matches('/'));
    let resp = ureq::get(&list_url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("cdp endpoint unreachable: {e}"))?;
    let mut body = String::new();
    resp.into_reader()
        .take(1_000_000)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    let targets: Value =
        serde_json::from_str(&body).map_err(|e| format!("bad target list: {e}"))?;
    let ws = targets
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|t| t.get("type").and_then(Value::as_str) == Some("page"))
        })
        .and_then(|t| t.get("webSocketDebuggerUrl"))
        .and_then(Value::as_str)
        .ok_or("no page target at the cdp endpoint")?;
    check_cdp_host(ws)?;
    Ok(ws.to_string())
}

impl Browser {
    pub fn start(cfg: &Config) -> Result<Browser, String> {
        if !cfg.browser_cdp_url.is_empty() {
            let ws_url = page_ws_url(&cfg.browser_cdp_url)?;
            return Self::attach(ws_url, None);
        }
        let binary = if cfg.browser_binary.is_empty() {
            "/usr/bin/chromium".to_string()
        } else {
            cfg.browser_binary.clone()
        };
        let profile: PathBuf = cfg.workspace.join(".phoenix-browser");
        std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;

        let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));
        let mut cmd = Command::new(&binary);
        if cfg.browser_headless {
            cmd.arg("--headless=new");
        }

        #[cfg(unix)]
        if running_as_root() {
            cmd.arg("--no-sandbox");
        }
        cmd.arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd
            .spawn()
            .map_err(|e| format!("cannot launch {binary}: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(LAUNCH_TIMEOUT_SECS);
        let port = match wait_for_port_file(&profile, deadline) {
            Ok(p) => p,
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let ws_url = match page_ws_url(&format!("http://127.0.0.1:{port}")) {
            Ok(u) => u,
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Self::attach(ws_url, Some(child))
    }

    fn attach(ws_url: String, child: Option<Child>) -> Result<Browser, String> {
        let mut ws = WsClient::connect(&ws_url)?;
        ws.set_read_timeout(Some(Duration::from_millis(POLL_MS)))?;
        Ok(Browser {
            child,
            ws,
            next_id: 1,
            page_enabled: false,
            refs: HashMap::new(),
        })
    }

    fn cmd_watch(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
        on_event: &mut dyn FnMut(&str, &Value),
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.ws.send_text(&encode_cmd(id, method, &params))?;
        loop {
            if Instant::now() >= deadline {
                return Err(format!("cdp command {method} timed out"));
            }
            let msg = match self.ws.next()? {
                Some(m) => m,
                None => continue,
            };
            let text = match msg {
                WsMsg::Text(t) => t,
                WsMsg::Binary(_) => continue,
                WsMsg::Close(code) => {
                    return Err(format!("browser closed the connection ({code})"))
                }
            };
            match classify(&text)? {
                CdpIncoming::Response {
                    id: rid,
                    result,
                    error,
                } if rid == id => {
                    return match error {
                        Some(e) => Err(format!("{method}: {e}")),
                        None => Ok(result),
                    };
                }
                CdpIncoming::Response { .. } => continue,
                CdpIncoming::Event { method, params } => on_event(&method, &params),
            }
        }
    }

    fn cmd(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(CMD_TIMEOUT_SECS);
        self.cmd_watch(method, params, deadline, &mut |_, _| {})
    }

    pub fn navigate(&mut self, url: &str) -> Result<String, String> {
        check_url(url)?;
        if !self.page_enabled {
            self.cmd("Page.enable", json!({}))?;
            self.page_enabled = true;
        }
        let deadline = Instant::now() + Duration::from_secs(NAV_TIMEOUT_SECS);
        let mut loaded = false;
        let result = self.cmd_watch(
            "Page.navigate",
            json!({"url": url}),
            deadline,
            &mut |m, _| {
                if m == "Page.loadEventFired" {
                    loaded = true;
                }
            },
        )?;
        if let Some(err) = result.get("errorText").and_then(Value::as_str) {
            if !err.is_empty() {
                return Err(format!("navigation failed: {err}"));
            }
        }

        while !loaded && Instant::now() < deadline {
            match self.ws.next()? {
                Some(WsMsg::Text(t)) => {
                    if let Ok(CdpIncoming::Event { method, .. }) = classify(&t) {
                        if method == "Page.loadEventFired" {
                            loaded = true;
                        }
                    }
                }
                Some(WsMsg::Close(code)) => {
                    return Err(format!("browser closed the connection ({code})"))
                }
                _ => {}
            }
        }
        self.refs.clear();
        if loaded {
            Ok(format!("loaded {url}"))
        } else {
            Ok(format!(
                "navigated to {url} (load event not seen within {NAV_TIMEOUT_SECS}s; \
the page may still be rendering)"
            ))
        }
    }

    pub fn snapshot(&mut self) -> Result<String, String> {
        self.cmd("Accessibility.enable", json!({}))?;
        let ax = self.cmd("Accessibility.getFullAXTree", json!({}))?;
        let snap = build_snapshot(&ax);
        self.refs = snap.refs;
        let count = self.refs.len();
        Ok(format!(
            "{}\n({count} interactive refs; act with browser_click / browser_type)",
            snap.outline
        ))
    }

    fn resolve(&self, r: &str) -> Result<RefEntry, String> {
        self.refs.get(r).cloned().ok_or_else(|| {
            format!("unknown ref '{r}'; take a fresh browser_snapshot and use its refs")
        })
    }

    fn center_of(&mut self, backend_node_id: i64) -> Result<(f64, f64), String> {
        self.cmd("DOM.getDocument", json!({"depth": 0}))?;
        let _ = self.cmd(
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend_node_id}),
        );
        let quads = self.cmd(
            "DOM.getContentQuads",
            json!({"backendNodeId": backend_node_id}),
        )?;
        let quad = quads
            .get("quads")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_array)
            .ok_or("element has no visible box; it may be hidden")?;
        let xs: Vec<f64> = quad.iter().step_by(2).filter_map(Value::as_f64).collect();
        let ys: Vec<f64> = quad
            .iter()
            .skip(1)
            .step_by(2)
            .filter_map(Value::as_f64)
            .collect();
        if xs.len() < 4 || ys.len() < 4 {
            return Err("element quad is malformed".into());
        }
        Ok((
            xs.iter().sum::<f64>() / xs.len() as f64,
            ys.iter().sum::<f64>() / ys.len() as f64,
        ))
    }

    pub fn click(&mut self, r: &str) -> Result<String, String> {
        let entry = self.resolve(r)?;
        let (x, y) = self.center_of(entry.backend_node_id)?;
        for (kind, clicks) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 1)] {
            self.cmd(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": clicks}),
            )?;
        }
        Ok(format!(
            "clicked {} \"{}\" ({r}); snapshot again to see the result",
            entry.role, entry.name
        ))
    }

    pub fn type_text(&mut self, r: &str, text: &str, submit: bool) -> Result<String, String> {
        let entry = self.resolve(r)?;
        self.cmd("DOM.getDocument", json!({"depth": 0}))?;
        self.cmd("DOM.focus", json!({"backendNodeId": entry.backend_node_id}))?;
        self.cmd("Input.insertText", json!({"text": text}))?;
        if submit {
            for kind in ["rawKeyDown", "char", "keyUp"] {
                let mut params = json!({
                    "type": kind,
                    "key": "Enter",
                    "code": "Enter",
                    "windowsVirtualKeyCode": 13,
                    "nativeVirtualKeyCode": 13,
                });
                if kind == "char" {
                    params["text"] = json!("\r");
                }
                self.cmd("Input.dispatchKeyEvent", params)?;
            }
        }
        Ok(format!(
            "typed {} chars into {} \"{}\" ({r}){}",
            text.chars().count(),
            entry.role,
            entry.name,
            if submit { ", then pressed Enter" } else { "" }
        ))
    }

    pub fn screenshot_png(&mut self) -> Result<Vec<u8>, String> {
        let result = self.cmd("Page.captureScreenshot", json!({"format": "png"}))?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or("screenshot returned no data")?;
        b64_decode(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scheme_gate() {
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("data:text/html,<h1>x</h1>").is_err());
        assert!(check_url("chrome://settings").is_err());
        assert!(check_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn navigation_cannot_reach_private_or_metadata_addresses() {
        for u in [
            "http://127.0.0.1:8080/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost:3000/",
            "http://10.0.0.5/",
            "http://[::1]:9222/",
        ] {
            assert!(check_url(u).is_err(), "{u} must be refused");
        }
    }

    #[test]
    fn the_cdp_control_channel_is_still_allowed_on_loopback() {
        assert!(check_cdp_host("http://127.0.0.1:9222").is_ok());
        assert!(check_cdp_host("ws://[::1]:9222/devtools/page/AB12").is_ok());
    }

    #[test]
    fn cdp_host_gate() {
        assert!(check_cdp_host("http://127.0.0.1:9222").is_ok());
        assert!(check_cdp_host("http://localhost:9222/json").is_ok());
        assert!(check_cdp_host("ws://127.0.0.1:9222/devtools/page/AB12").is_ok());
        assert!(check_cdp_host("ws://[::1]:9222/devtools/page/AB12").is_ok());
        assert!(check_cdp_host("http://10.0.0.5:9222").is_err());
        assert!(check_cdp_host("http://evil.example:9222").is_err());
        assert!(check_cdp_host("https://127.0.0.1:9222").is_err());
    }

    #[test]
    fn devtools_port_file() {
        assert_eq!(
            parse_devtools_port("9333\n/devtools/browser/x").unwrap(),
            9333
        );
        assert_eq!(parse_devtools_port("80").unwrap(), 80);
        assert!(parse_devtools_port("").is_err());
        assert!(parse_devtools_port("not-a-port\n").is_err());
    }

    #[test]
    fn encode_and_classify_roundtrip() {
        let frame = encode_cmd(7, "Page.navigate", &json!({"url": "https://x"}));
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "Page.navigate");
        assert_eq!(v["params"]["url"], "https://x");
        let ok = classify(r#"{"id":7,"result":{"frameId":"F"}}"#).unwrap();
        assert_eq!(
            ok,
            CdpIncoming::Response {
                id: 7,
                result: json!({"frameId": "F"}),
                error: None
            }
        );
    }

    #[test]
    fn classify_error_and_event() {
        let err = classify(r#"{"id":3,"error":{"code":-32000,"message":"no node"}}"#).unwrap();
        match err {
            CdpIncoming::Response { id, error, .. } => {
                assert_eq!(id, 3);
                assert_eq!(error.as_deref(), Some("no node"));
            }
            other => panic!("expected response, got {other:?}"),
        }
        let ev =
            classify(r#"{"method":"Page.loadEventFired","params":{"timestamp":1.5}}"#).unwrap();
        assert_eq!(
            ev,
            CdpIncoming::Event {
                method: "Page.loadEventFired".into(),
                params: json!({"timestamp": 1.5})
            }
        );
        assert!(classify("{}").is_err());
        assert!(classify("not json").is_err());
    }

    fn ax_fixture() -> Value {
        json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Demo"},
             "childIds": ["2", "3", "6"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "heading"}, "name": {"value": "Welcome"},
             "parentId": "1", "childIds": [], "backendDOMNodeId": 10},
            {"nodeId": "3", "role": {"value": "generic"}, "name": {"value": ""},
             "parentId": "1", "childIds": ["4", "5"], "backendDOMNodeId": 11},
            {"nodeId": "4", "role": {"value": "button"}, "name": {"value": "Go"},
             "parentId": "3", "childIds": [], "backendDOMNodeId": 12},
            {"nodeId": "5", "role": {"value": "button"}, "name": {"value": "Go"},
             "parentId": "3", "childIds": [], "backendDOMNodeId": 13},
            {"nodeId": "6", "role": {"value": "textbox"}, "name": {"value": "Search"},
             "parentId": "1", "childIds": [], "backendDOMNodeId": 14,
             "value": {"value": "old text"}},
        ]})
    }

    #[test]
    fn snapshot_outline_and_refs() {
        let snap = build_snapshot(&ax_fixture());

        assert!(!snap.outline.contains("generic"));
        assert!(snap.outline.contains("- heading \"Welcome\" [ref=e1]"));
        assert!(snap.outline.contains("- button \"Go\" [ref=e2]"));
        assert!(snap.outline.contains("- button \"Go\" [ref=e3] [nth=1]"));
        assert!(snap
            .outline
            .contains("- textbox \"Search\" [ref=e4] value=\"old text\""));
        assert_eq!(snap.refs.len(), 4);
        assert_eq!(snap.refs["e2"].backend_node_id, 12);
        assert_eq!(snap.refs["e3"].backend_node_id, 13);
        assert_eq!(snap.refs["e4"].role, "textbox");
    }

    #[test]
    fn snapshot_unique_elements_have_no_nth() {
        let snap = build_snapshot(&ax_fixture());

        for line in snap.outline.lines() {
            if line.contains("[ref=e1]") || line.contains("[ref=e4]") {
                assert!(!line.contains("[nth="), "unexpected nth: {line}");
            }
        }
    }

    #[test]
    fn cyclic_accessibility_tree_does_not_hang() {
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Loop"},
             "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "A"},
             "parentId": "1", "childIds": ["3"], "backendDOMNodeId": 2},
            {"nodeId": "3", "role": {"value": "button"}, "name": {"value": "B"},
             "parentId": "2", "childIds": ["2"], "backendDOMNodeId": 3},
        ]});
        let snap = build_snapshot(&ax);
        assert!(snap.outline.contains("\"A\""));
        assert!(snap.outline.contains("\"B\""));
        assert_eq!(
            snap.outline.matches("\"A\"").count(),
            1,
            "a cycle must be visited once: {}",
            snap.outline
        );
    }

    #[test]
    fn self_referencing_node_terminates() {
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Self"},
             "childIds": ["1"], "backendDOMNodeId": 1},
        ]});
        let snap = build_snapshot(&ax);
        assert!(!snap.outline.is_empty());
    }

    #[test]
    fn huge_page_outline_is_bounded_and_refs_stay_resolvable() {
        let mut nodes = vec![json!({
            "nodeId": "root", "role": {"value": "RootWebArea"}, "name": {"value": "Big"},
            "childIds": (0..4000).map(|i| i.to_string()).collect::<Vec<_>>(),
            "backendDOMNodeId": 1
        })];
        for i in 0..4000 {
            nodes.push(json!({
                "nodeId": i.to_string(),
                "role": {"value": "button"},
                "name": {"value": format!("Button {i}")},
                "parentId": "root",
                "childIds": [],
                "backendDOMNodeId": 100 + i
            }));
        }
        let snap = build_snapshot(&json!({"nodes": nodes}));
        assert!(
            snap.outline.chars().count() <= MAX_SNAPSHOT_CHARS + 40,
            "outline not bounded: {} chars",
            snap.outline.chars().count()
        );
        assert!(snap.outline.contains("truncated"), "must say it truncated");
        for r in snap.refs.keys() {
            assert!(
                snap.outline.contains(&format!("[ref={r}]")),
                "ref {r} is not present in the outline the model can see"
            );
        }
        assert!(!snap.refs.is_empty(), "some refs must survive");
    }

    #[test]
    fn deeply_nested_tree_does_not_blow_the_stack() {
        let deep = 5000usize;
        let mut nodes = Vec::with_capacity(deep);
        for i in 0..deep {
            let mut node = json!({
                "nodeId": i.to_string(),
                "role": {"value": "generic"},
                "name": {"value": ""},
                "backendDOMNodeId": i as i64 + 1,
                "childIds": if i + 1 < deep { vec![(i + 1).to_string()] } else { vec![] },
            });
            if i > 0 {
                node["parentId"] = json!((i - 1).to_string());
            }
            nodes.push(node);
        }
        let snap = build_snapshot(&json!({"nodes": nodes}));
        assert!(!snap.outline.is_empty());
    }

    #[test]
    fn snapshot_skips_ignored_nodes() {
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": ""},
             "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Ghost"},
             "parentId": "1", "childIds": [], "ignored": true, "backendDOMNodeId": 2},
        ]});
        let snap = build_snapshot(&ax);
        assert!(!snap.outline.contains("Ghost"));
        assert!(snap.refs.is_empty());
    }

    #[test]
    fn snapshot_handles_empty_tree() {
        assert_eq!(
            build_snapshot(&json!({})).outline,
            "(no accessibility tree)"
        );
        let snap = build_snapshot(&json!({"nodes": []}));
        assert_eq!(snap.outline, "(empty page)");
    }

    #[test]
    fn snapshot_escapes_hostile_names() {
        let ax = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": ""},
             "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"},
             "name": {"value": "a\"b\nc"},
             "parentId": "1", "childIds": [], "backendDOMNodeId": 2},
        ]});
        let snap = build_snapshot(&ax);
        assert!(snap.outline.contains("\"a\\\"b\\nc\""));
    }

    #[test]
    #[ignore]
    fn live_browser_smoke() {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let body = "<!doctype html><title>Smoke</title><h1>Phoenix rises</h1>\
<form><input aria-label=\"query\" name=\"q\"><button>Search</button></form>";
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        let ws = std::env::temp_dir().join(format!("px-browser-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let cfg = Config {
            workspace: ws,
            browser_enabled: true,
            ..Config::default()
        };
        let mut b = Browser::start(&cfg).expect("launch chromium");
        let nav = b.navigate(&format!("http://{addr}/")).expect("navigate");
        println!("nav: {nav}");
        let snap = b.snapshot().expect("snapshot");
        println!("snapshot:\n{snap}");
        assert!(snap.contains("Phoenix rises"));
        assert!(snap.contains("button"));
        let typed = b.type_text("e2", "firebird", false).expect("type");
        println!("type: {typed}");
        let clicked = b.click("e3").expect("click");
        println!("click: {clicked}");
        let png = b.screenshot_png().expect("screenshot");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "not a png");
        println!("screenshot: {} bytes", png.len());
    }
}
