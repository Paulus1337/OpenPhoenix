use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

pub const VERSION: u64 = 1;
pub const DEFAULT_KEEP: usize = 50;
pub const TERMINAL_TTL_SECS: u64 = 7 * 24 * 3600;
pub const RESULT_TAIL: usize = 4000;

fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Lost,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Succeeded => "succeeded",
            Status::Failed => "failed",
            Status::TimedOut => "timed_out",
            Status::Cancelled => "cancelled",
            Status::Lost => "lost",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "running" => Status::Running,
            "succeeded" => Status::Succeeded,
            "failed" => Status::Failed,
            "timed_out" => Status::TimedOut,
            "cancelled" => Status::Cancelled,
            "lost" => Status::Lost,
            _ => Status::Queued,
        }
    }

    pub fn terminal(self) -> bool {
        !matches!(self, Status::Queued | Status::Running)
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: u64,
    pub kind: String,
    pub title: String,
    pub owner: String,
    pub status: Status,
    pub pid: u32,
    pub pid_start: u64,
    pub started: u64,
    pub ended: u64,
    pub timeout_secs: u64,
    pub exit_code: i32,
    pub error: String,
    pub log: PathBuf,
    pub delivered: bool,
}

#[derive(Debug, Default)]
pub struct Db {
    pub next_id: u64,
    pub tasks: Vec<Task>,
}

pub struct Spec {
    pub kind: String,
    pub title: String,
    pub owner: String,
    pub timeout_secs: u64,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

fn now() -> u64 {
    crate::scheduler::now_epoch()
}

pub fn default_path() -> PathBuf {
    crate::config::home().join("tasks.json")
}

pub fn log_dir(registry: &Path) -> PathBuf {
    let base = registry.parent().unwrap_or(Path::new("."));
    let stem = registry
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tasks");
    base.join(stem)
}

#[cfg(target_os = "linux")]
pub fn proc_start(pid: u32) -> Option<u64> {
    if zombie(pid) {
        return None;
    }
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = raw.rsplit_once(')')?.1;
    tail.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
pub fn zombie(pid: u32) -> bool {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    status
        .lines()
        .find_map(|l| l.strip_prefix("State:"))
        .is_some_and(|v| v.trim_start().starts_with('Z'))
}

#[cfg(target_os = "macos")]
pub fn zombie(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .trim_start()
                    .starts_with('Z')
        })
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn zombie(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn proc_start(pid: u32) -> Option<u64> {
    if zombie(pid) {
        return None;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let stamp = text.trim();
    if stamp.is_empty() {
        return None;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in stamp.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(h.max(1))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub fn proc_start(pid: u32) -> Option<u64> {
    let alive = std::process::Command::new("ps")
        .args(["-o", "pid=", "-p", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if alive {
        Some(0)
    } else {
        None
    }
}

#[cfg(windows)]
pub fn proc_start(pid: u32) -> Option<u64> {
    let script = format!("(Get-Process -Id {pid}).StartTime.ToFileTimeUtc()");
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().parse::<u64>().ok().map(|v| v.max(1))
}

pub fn alive(pid: u32, pid_start: u64) -> bool {
    if pid == 0 {
        return false;
    }
    match proc_start(pid) {
        Some(start) => pid_start == 0 || start == pid_start,
        None => false,
    }
}

pub fn apply_status(current: Status, next: Status) -> bool {
    if !current.terminal() {
        return true;
    }
    if current == Status::Lost && next != Status::Lost {
        return true;
    }
    false
}

fn to_json(t: &Task) -> Value {
    json!({
        "id": t.id,
        "kind": t.kind,
        "title": t.title,
        "owner": t.owner,
        "status": t.status.as_str(),
        "pid": t.pid,
        "pid_start": t.pid_start,
        "started": t.started,
        "ended": t.ended,
        "timeout_secs": t.timeout_secs,
        "exit_code": t.exit_code,
        "error": t.error,
        "log": t.log.to_string_lossy(),
        "delivered": t.delivered,
    })
}

fn from_json(v: &Value) -> Option<Task> {
    Some(Task {
        id: v["id"].as_u64()?,
        kind: v["kind"].as_str().unwrap_or("subagent").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        owner: v["owner"].as_str().unwrap_or("").to_string(),
        status: Status::parse(v["status"].as_str().unwrap_or("queued")),
        pid: v["pid"].as_u64().unwrap_or(0) as u32,
        pid_start: v["pid_start"].as_u64().unwrap_or(0),
        started: v["started"].as_u64().unwrap_or(0),
        ended: v["ended"].as_u64().unwrap_or(0),
        timeout_secs: v["timeout_secs"].as_u64().unwrap_or(0),
        exit_code: v["exit_code"].as_i64().unwrap_or(0) as i32,
        error: v["error"].as_str().unwrap_or("").to_string(),
        log: PathBuf::from(v["log"].as_str().unwrap_or("")),
        delivered: v["delivered"].as_bool().unwrap_or(false),
    })
}

pub fn load(path: &Path) -> Db {
    let empty = || Db {
        next_id: 1,
        tasks: Vec::new(),
    };
    let Ok(text) = fs::read_to_string(path) else {
        return empty();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return empty();
    };
    if v["v"].as_u64() != Some(VERSION) {
        return empty();
    }
    let tasks: Vec<Task> = v["tasks"]
        .as_array()
        .map(|a| a.iter().filter_map(from_json).collect())
        .unwrap_or_default();
    let max_id = tasks.iter().map(|t| t.id).max().unwrap_or(0);
    Db {
        next_id: v["next_id"].as_u64().unwrap_or(max_id + 1).max(max_id + 1),
        tasks,
    }
}

pub fn save(path: &Path, db: &Db) -> Result<(), String> {
    let v = json!({
        "v": VERSION,
        "next_id": db.next_id,
        "tasks": db.tasks.iter().map(to_json).collect::<Vec<_>>(),
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

fn set_terminal(path: &Path, id: u64, status: Status, exit_code: i32, error: &str) {
    let _g = guard();
    let mut db = load(path);
    let Some(t) = db.tasks.iter_mut().find(|t| t.id == id) else {
        return;
    };
    if !apply_status(t.status, status) {
        return;
    }
    t.status = status;
    t.exit_code = exit_code;
    t.ended = now();
    if !error.is_empty() {
        t.error = crate::security::one_line(error, 400);
    }
    let _ = save(path, &db);
}

pub fn active(path: &Path) -> usize {
    load(path)
        .tasks
        .iter()
        .filter(|t| !t.status.terminal())
        .count()
}

pub fn spawn(path: &Path, spec: Spec) -> Result<Task, String> {
    let title = crate::security::one_line(spec.title.trim(), 200);
    if title.is_empty() {
        return Err("empty title".into());
    }
    let (id, log) = {
        let _g = guard();
        let mut db = load(path);
        let id = db.next_id;
        db.next_id += 1;
        let log = log_dir(path).join(format!("{id}.log"));
        db.tasks.push(Task {
            id,
            kind: spec.kind.clone(),
            title: title.clone(),
            owner: spec.owner.clone(),
            status: Status::Queued,
            pid: 0,
            pid_start: 0,
            started: now(),
            ended: 0,
            timeout_secs: spec.timeout_secs,
            exit_code: 0,
            error: String::new(),
            log: log.clone(),
            delivered: false,
        });
        save(path, &db)?;
        (id, log)
    };

    let out = create_log(&log).inspect_err(|e| {
        set_terminal(path, id, Status::Failed, -1, e);
    })?;
    let errdup = out.try_clone().map_err(|e| {
        let msg = e.to_string();
        set_terminal(path, id, Status::Failed, -1, &msg);
        msg
    })?;

    let mut cmd = if cfg!(unix) && setsid_available() {
        let mut c = Command::new("setsid");
        c.arg("--").arg(&spec.program).args(&spec.args);
        c
    } else {
        let mut c = Command::new(&spec.program);
        c.args(&spec.args);
        c
    };
    let mut child = match cmd
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(errdup))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            set_terminal(path, id, Status::Failed, -1, &msg);
            return Err(msg);
        }
    };

    let pid = child.id();
    let pid_start = proc_start(pid).unwrap_or(0);
    let task = {
        let _g = guard();
        let mut db = load(path);
        let Some(t) = db.tasks.iter_mut().find(|t| t.id == id) else {
            return Err("task vanished".into());
        };
        t.pid = pid;
        t.pid_start = pid_start;
        t.status = Status::Running;
        t.started = now();
        let snapshot = t.clone();
        save(path, &db)?;
        snapshot
    };

    let watch = path.to_path_buf();
    let timeout = spec.timeout_secs;
    std::thread::spawn(move || {
        let deadline = if timeout == 0 {
            None
        } else {
            Some(std::time::Instant::now() + std::time::Duration::from_secs(timeout))
        };
        loop {
            match child.try_wait() {
                Ok(Some(st)) => {
                    let code = st.code().unwrap_or(-1);
                    let status = if st.success() {
                        Status::Succeeded
                    } else {
                        Status::Failed
                    };
                    let err = if st.success() {
                        String::new()
                    } else {
                        format!("exit {code}")
                    };
                    set_terminal(&watch, id, status, code, &err);
                    return;
                }
                Ok(None) => {
                    if cancel_requested(&watch, id) {
                        stop_pid(pid);
                        let _ = child.wait();
                        set_terminal(&watch, id, Status::Cancelled, -1, "cancelled");
                        return;
                    }
                    if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                        stop_pid(pid);
                        let _ = child.wait();
                        set_terminal(
                            &watch,
                            id,
                            Status::TimedOut,
                            -1,
                            &format!("timed out after {timeout}s"),
                        );
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                Err(e) => {
                    let msg = e.to_string();
                    set_terminal(&watch, id, Status::Failed, -1, &msg);
                    return;
                }
            }
        }
    });
    Ok(task)
}

fn create_log(path: &Path) -> Result<fs::File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path).map_err(|e| e.to_string())
}

fn cancel_requested(path: &Path, id: u64) -> bool {
    load(path)
        .tasks
        .iter()
        .any(|t| t.id == id && t.status == Status::Cancelled)
}

fn setsid_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("setsid")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

#[cfg(unix)]
fn signal_term(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-s", "TERM", "--", &format!("-{pid}")])
        .output();
    let _ = std::process::Command::new("kill")
        .args(["-s", "TERM", &pid.to_string()])
        .output();
}

#[cfg(unix)]
fn signal_kill(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pid}")])
        .output();
    let _ = std::process::Command::new("kill")
        .args(["-s", "KILL", &pid.to_string()])
        .output();
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

#[cfg(windows)]
fn signal_term(pid: u32) {
    terminate_process(pid);
}

#[cfg(windows)]
fn signal_kill(pid: u32) {
    terminate_process(pid);
}

fn stop_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    signal_term(pid);
    for _ in 0..20 {
        if proc_start(pid).is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    signal_kill(pid);
}

pub fn reap(path: &Path) -> usize {
    let _g = guard();
    let mut db = load(path);
    let mut changed = 0;
    let mut kill = Vec::new();
    for t in db.tasks.iter_mut() {
        if t.status.terminal() {
            continue;
        }
        if t.pid != 0 && !alive(t.pid, t.pid_start) {
            t.status = Status::Lost;
            t.ended = now();
            t.error = "process disappeared before reporting".into();
            changed += 1;
            continue;
        }
        if t.timeout_secs > 0 && t.started > 0 && now() >= t.started + t.timeout_secs {
            kill.push(t.pid);
            t.status = Status::TimedOut;
            t.ended = now();
            t.error = format!("timed out after {}s", t.timeout_secs);
            changed += 1;
        }
    }
    if changed > 0 {
        let _ = save(path, &db);
    }
    drop(_g);
    for pid in kill {
        stop_pid(pid);
    }
    changed
}

pub fn prune(path: &Path, keep: usize) -> usize {
    let _g = guard();
    let mut db = load(path);
    let cutoff = now().saturating_sub(TERMINAL_TTL_SECS);
    let mut terminal: Vec<u64> = db
        .tasks
        .iter()
        .filter(|t| t.status.terminal())
        .map(|t| t.id)
        .collect();
    terminal.sort_unstable();
    let overflow = terminal.len().saturating_sub(keep);
    let oldest: std::collections::HashSet<u64> = terminal.iter().take(overflow).copied().collect();
    let mut dropped = Vec::new();
    db.tasks.retain(|t| {
        let stale = t.status.terminal() && t.ended > 0 && t.ended < cutoff;
        if stale || oldest.contains(&t.id) {
            dropped.push(t.log.clone());
            return false;
        }
        true
    });
    if dropped.is_empty() {
        return 0;
    }
    let _ = save(path, &db);
    for log in &dropped {
        let _ = fs::remove_file(log);
    }
    dropped.len()
}

pub fn get(path: &Path, id: u64) -> Option<Task> {
    load(path).tasks.into_iter().find(|t| t.id == id)
}

pub fn list(path: &Path, owner: Option<&str>) -> Vec<Task> {
    let mut tasks: Vec<Task> = load(path)
        .tasks
        .into_iter()
        .filter(|t| owner.is_none_or(|o| t.owner == o))
        .collect();
    tasks.sort_by_key(|t| (t.status.terminal(), t.id));
    tasks
}

pub fn cancel(path: &Path, id: u64) -> Result<String, String> {
    let pid = {
        let _g = guard();
        let mut db = load(path);
        let Some(t) = db.tasks.iter_mut().find(|t| t.id == id) else {
            return Err(format!("no task #{id}"));
        };
        if t.status.terminal() {
            return Err(format!("task #{id} already {}", t.status.as_str()));
        }
        let pid = t.pid;
        t.status = Status::Cancelled;
        t.ended = now();
        t.error = "cancelled".into();
        save(path, &db)?;
        pid
    };
    stop_pid(pid);
    Ok(format!("cancelled task #{id}"))
}

pub fn tail(task: &Task, limit: usize) -> String {
    let Ok(mut fh) = fs::File::open(&task.log) else {
        return String::new();
    };
    let len = fh.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(limit as u64);
    if start > 0 && fh.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if fh.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let text = if start > 0 {
        match text.find('\n') {
            Some(i) => text[i + 1..].to_string(),
            None => text,
        }
    } else {
        text
    };
    text.trim().to_string()
}

pub fn mark_delivered(path: &Path, id: u64) {
    let _g = guard();
    let mut db = load(path);
    if let Some(t) = db.tasks.iter_mut().find(|t| t.id == id) {
        if t.delivered {
            return;
        }
        t.delivered = true;
        let _ = save(path, &db);
    }
}

pub fn undelivered(path: &Path, owner: &str) -> Vec<Task> {
    load(path)
        .tasks
        .into_iter()
        .filter(|t| t.status.terminal() && !t.delivered && t.owner == owner)
        .collect()
}

pub fn line(t: &Task) -> String {
    let age = if t.status.terminal() && t.ended > 0 {
        format!(
            "took {}",
            crate::scheduler::time_ago(t.ended - t.started.min(t.ended))
        )
    } else {
        format!(
            "up {}",
            crate::scheduler::time_ago(now().saturating_sub(t.started))
        )
    };
    let err = if t.error.is_empty() {
        String::new()
    } else {
        format!(" | {}", t.error)
    };
    format!(
        "#{} [{}] ({}) {} | {age}{err}",
        t.id,
        t.status.as_str(),
        t.kind,
        t.title
    )
}

pub fn render(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "(no background tasks)".into();
    }
    tasks.iter().map(line).collect::<Vec<_>>().join("\n")
}

pub fn report(path: &Path) -> Value {
    let db = load(path);
    let mut counts = std::collections::BTreeMap::new();
    for t in &db.tasks {
        *counts.entry(t.status.as_str()).or_insert(0u64) += 1;
    }
    json!({
        "total": db.tasks.len(),
        "active": db.tasks.iter().filter(|t| !t.status.terminal()).count(),
        "by_status": counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-tasks-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec(dir: &Path, args: &[&str], timeout: u64) -> Spec {
        Spec {
            kind: "shell".into(),
            title: format!("sh {}", args.join(" ")),
            owner: "chat:1".into(),
            timeout_secs: timeout,
            program: PathBuf::from("/bin/sh"),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: dir.to_path_buf(),
        }
    }

    fn wait_terminal(path: &Path, id: u64) -> Task {
        for _ in 0..200 {
            if let Some(t) = get(path, id) {
                if t.status.terminal() {
                    return t;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("task {id} never reached a terminal status");
    }

    #[test]
    fn spawn_captures_output_and_succeeds() {
        let d = tmpdir("ok");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "echo hello from task"], 30)).unwrap();
        assert_eq!(t.status, Status::Running);
        assert!(t.pid > 0);
        let done = wait_terminal(&p, t.id);
        assert_eq!(done.status, Status::Succeeded);
        assert_eq!(done.exit_code, 0);
        assert_eq!(tail(&done, RESULT_TAIL), "hello from task");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn failure_records_exit_code_and_stderr() {
        let d = tmpdir("fail");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "echo bad >&2; exit 3"], 30)).unwrap();
        let done = wait_terminal(&p, t.id);
        assert_eq!(done.status, Status::Failed);
        assert_eq!(done.exit_code, 3);
        assert_eq!(done.error, "exit 3");
        assert_eq!(tail(&done, RESULT_TAIL), "bad");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn timeout_kills_the_child() {
        let d = tmpdir("timeout");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "sleep 30"], 1)).unwrap();
        let pid = t.pid;
        let done = wait_terminal(&p, t.id);
        assert_eq!(done.status, Status::TimedOut);
        assert!(done.error.contains("timed out"));
        assert!(!alive(pid, done.pid_start), "child must be dead");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_stops_a_running_task() {
        let d = tmpdir("cancel");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "sleep 30"], 0)).unwrap();
        assert!(cancel(&p, t.id).unwrap().contains("cancelled"));
        for _ in 0..100 {
            if !alive(t.pid, t.pid_start) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!alive(t.pid, t.pid_start));
        assert_eq!(get(&p, t.id).unwrap().status, Status::Cancelled);
        assert!(cancel(&p, t.id).unwrap_err().contains("already cancelled"));
        assert!(cancel(&p, 999).unwrap_err().contains("no task #999"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reap_marks_a_vanished_process_lost() {
        let d = tmpdir("reap");
        let p = d.join("tasks.json");
        let mut db = load(&p);
        db.next_id = 2;
        db.tasks.push(Task {
            id: 1,
            kind: "subagent".into(),
            title: "orphan".into(),
            owner: String::new(),
            status: Status::Running,
            pid: 4_000_000,
            pid_start: 12345,
            started: now() - 10,
            ended: 0,
            timeout_secs: 0,
            exit_code: 0,
            error: String::new(),
            log: d.join("1.log"),
            delivered: false,
        });
        save(&p, &db).unwrap();
        assert_eq!(reap(&p), 1);
        let t = get(&p, 1).unwrap();
        assert_eq!(t.status, Status::Lost);
        assert!(t.error.contains("disappeared"));
        assert_eq!(reap(&p), 0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn canonical_outcome_overrides_a_lost_tombstone() {
        assert!(apply_status(Status::Running, Status::Succeeded));
        assert!(apply_status(Status::Lost, Status::Succeeded));
        assert!(apply_status(Status::Lost, Status::Failed));
        assert!(!apply_status(Status::Succeeded, Status::Lost));
        assert!(!apply_status(Status::Cancelled, Status::Succeeded));
        assert!(!apply_status(Status::Lost, Status::Lost));
    }

    #[test]
    fn prune_drops_oldest_and_stale_and_removes_logs() {
        let d = tmpdir("prune");
        let p = d.join("tasks.json");
        let mut db = load(&p);
        for i in 1..=6u64 {
            let log = d.join(format!("{i}.log"));
            fs::write(&log, "x").unwrap();
            db.tasks.push(Task {
                id: i,
                kind: "shell".into(),
                title: format!("t{i}"),
                owner: String::new(),
                status: Status::Succeeded,
                pid: 0,
                pid_start: 0,
                started: now() - 20,
                ended: now() - 10,
                timeout_secs: 0,
                exit_code: 0,
                error: String::new(),
                log,
                delivered: true,
            });
        }
        db.tasks[0].ended = now() - TERMINAL_TTL_SECS - 5;
        db.next_id = 7;
        save(&p, &db).unwrap();

        assert_eq!(prune(&p, 4), 2);
        let left: Vec<u64> = load(&p).tasks.iter().map(|t| t.id).collect();
        assert_eq!(left, vec![3, 4, 5, 6]);
        assert!(!d.join("1.log").exists());
        assert!(!d.join("2.log").exists());
        assert!(d.join("3.log").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn active_tasks_are_never_pruned() {
        let d = tmpdir("prune-active");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "sleep 5"], 0)).unwrap();
        assert_eq!(prune(&p, 0), 0);
        assert_eq!(active(&p), 1);
        cancel(&p, t.id).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn listing_puts_active_first_and_filters_by_owner() {
        let d = tmpdir("list");
        let p = d.join("tasks.json");
        let a = spawn(&p, spec(&d, &["-c", "exit 0"], 30)).unwrap();
        wait_terminal(&p, a.id);
        let b = spawn(&p, spec(&d, &["-c", "sleep 5"], 0)).unwrap();
        let ids: Vec<u64> = list(&p, None).iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![b.id, a.id]);
        assert_eq!(list(&p, Some("chat:1")).len(), 2);
        assert_eq!(list(&p, Some("chat:2")).len(), 0);
        cancel(&p, b.id).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn delivery_is_tracked_once_per_task() {
        let d = tmpdir("deliver");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "echo done"], 30)).unwrap();
        wait_terminal(&p, t.id);
        assert_eq!(undelivered(&p, "chat:1").len(), 1);
        assert_eq!(undelivered(&p, "chat:9").len(), 0);
        mark_delivered(&p, t.id);
        assert!(undelivered(&p, "chat:1").is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_result_stays_pending_until_it_is_acknowledged() {
        let d = tmpdir("deliver-ack");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "echo done"], 30)).unwrap();
        wait_terminal(&p, t.id);
        for _ in 0..3 {
            assert_eq!(
                undelivered(&p, "chat:1").len(),
                1,
                "an unacknowledged result must be redelivered, never dropped"
            );
        }
        mark_delivered(&p, t.id);
        assert!(undelivered(&p, "chat:1").is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn tail_returns_the_end_of_a_large_log() {
        let d = tmpdir("tail");
        let p = d.join("tasks.json");
        let t = spawn(
            &p,
            spec(
                &d,
                &[
                    "-c",
                    "i=0; while [ $i -lt 400 ]; do echo line$i; i=$((i+1)); done",
                ],
                30,
            ),
        )
        .unwrap();
        let done = wait_terminal(&p, t.id);
        let out = tail(&done, 200);
        assert!(out.len() <= 200, "tail must stay bounded: {}", out.len());
        assert!(out.ends_with("line399"), "got: {out}");
        assert!(!out.contains("line0\n"), "old lines must be dropped");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_or_foreign_version_reads_as_empty() {
        let d = tmpdir("corrupt");
        let p = d.join("tasks.json");
        fs::write(&p, "not json").unwrap();
        assert_eq!(load(&p).tasks.len(), 0);
        fs::write(&p, r#"{"v":99,"next_id":5,"tasks":[{"id":1}]}"#).unwrap();
        assert_eq!(load(&p).tasks.len(), 0);
        assert_eq!(load(&p).next_id, 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn ids_are_unique_under_parallel_spawns() {
        let d = tmpdir("par");
        let p = d.join("tasks.json");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let p2 = p.clone();
            let d2 = d.clone();
            handles.push(std::thread::spawn(move || {
                spawn(&p2, spec(&d2, &["-c", "exit 0"], 30)).map(|t| t.id)
            }));
        }
        let ids: Vec<u64> = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(Result::ok)
            .collect();
        assert_eq!(ids.len(), 8);
        let uniq: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(uniq.len(), 8, "ids must not collide: {ids:?}");
        for id in ids {
            wait_terminal(&p, id);
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn registry_and_logs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("perms");
        let p = d.join("tasks.json");
        let t = spawn(&p, spec(&d, &["-c", "echo hi"], 30)).unwrap();
        let done = wait_terminal(&p, t.id);
        let reg = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        let log = fs::metadata(&done.log).unwrap().permissions().mode() & 0o777;
        assert_eq!(reg, 0o600);
        assert_eq!(log, 0o600);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_title_is_refused() {
        let d = tmpdir("title");
        let p = d.join("tasks.json");
        let mut s = spec(&d, &["-c", "exit 0"], 30);
        s.title = "   ".into();
        assert!(spawn(&p, s).unwrap_err().contains("empty title"));
        assert_eq!(load(&p).tasks.len(), 0);
        let _ = fs::remove_dir_all(&d);
    }
}
