#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod agent;
mod allowlist;
mod attach;
mod audio;
mod audit;
mod autopilot;
mod board;
mod browser;
mod canvas;
mod catalog;
mod clawhub;
mod commands;
mod commitments;
mod config;
mod daemon;
mod discord;
mod doctor;
mod embeddings;
mod heartbeat;
mod hooks;
mod http;
mod imessage;
mod irc;
mod loop_detect;
mod matrix;
mod mattermost;
mod mcp;
mod media;
mod memory;
mod menu;
mod migrate;
mod net;
mod oauth;
mod onboard;
mod prompts;
mod providers;
mod proxy;
mod scheduler;
mod secrets;
mod security;
#[cfg(test)]
mod security_fuzz;
mod service;
mod sessions;
mod signal;
mod skills;
mod slack;
mod ssrf;
mod state;
mod tasks;
mod telegram;
mod text;
mod tools;
mod update;
mod web;
mod whatsapp;
mod ws;

use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static ACTIVITY: AtomicU64 = AtomicU64::new(0);

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mark_activity() {
    ACTIVITY.store(now_epoch(), Ordering::Relaxed);
}

const DREAM_PROMPT: &str = "You are idle; this is a private reflection run. \
Review your memory notes and workspace, consolidate anything worth keeping, \
and write at most five short observations or ideas for later. Do not run \
commands unless reading is required. Reply with the note text only.";

use agent::Agent;
use config::{Config, LEAN_LEVELS};
use memory::Memory;
use telegram::Telegram;
use tools::Toolbox;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DOCS_URL: &str = "docs: https://github.com/Paulus1337/OpenPhoenix/wiki";

const BANNER: &str = r"
   ____                   ____  _                      _
  / __ \____  ___  ____  / __ \/ /_  ____  ___  ____  (_)  __
 / / / / __ \/ _ \/ __ \/ /_/ / __ \/ __ \/ _ \/ __ \/ / |/_/
/ /_/ / /_/ /  __/ / / / ____/ / / / /_/ /  __/ / / / />  <
\____/ .___/\___/_/ /_/_/   /_/ /_/\____/\___/_/ /_/_/_/|_|
    /_/            R I S E   &   S H I N E      v{version}
";

fn banner() -> String {
    BANNER.replace("{version}", VERSION)
}

#[derive(Debug, PartialEq)]
enum Cmd {
    Init,
    Configure,
    Status,
    Dashboard,
    Run(String),
    Chat,
    Serve,
    Doctor,
    Jobs,
    Tasks(Vec<String>),
    Sessions(Vec<String>),
    Migrate,
    Update,
    Models,
    Schema,
    Secret(Vec<String>),
    Skill(Vec<String>),
    Service(Vec<String>),
    Commands,
    Docs,
    System,
    Channels,
    Directory,
    ExecPolicy,
    Gateway,
    Webhooks,
    Security,
    Health,
    Memory(Vec<String>),
    ConfigFile(Vec<String>),
    Audit(Vec<String>),
    Backup(Vec<String>),
    Transcripts(Vec<String>),
    Completion(Vec<String>),
    Reset,
    Uninstall,
    Board(Vec<String>),
    Canvas(Vec<String>),
    Mcp(Vec<String>),
    Hooks(Vec<String>),
    Agents(Vec<String>),
    Commitments(Vec<String>),
    Proxy(Vec<String>),
    Attach(Vec<String>),
    Capability,
    Media(Vec<String>),
    Oauth(Vec<String>),
    Transcribe(Vec<String>),
    Worktrees(Vec<String>),
}

#[derive(Debug)]
struct Args {
    cmd: Cmd,
    ghost: bool,
    recall: bool,
    lean: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    from: Option<String>,
    write: bool,
    force: bool,
    secrets: bool,
    check: bool,
    json: bool,
    install_daemon: bool,
    test_fallback: bool,
}

fn unknown_arg(word: &str) -> String {
    if let Some((name, why)) = commands::NOT_BUILT.iter().find(|(n, _)| *n == word) {
        return format!("`phoenix {name}` is not built here: {why}\n\n{}", usage());
    }
    match nearest_command(word) {
        Some(c) => format!(
            "unknown argument: {word}\n\ndid you mean `phoenix {c}`?\n\n{}",
            usage()
        ),
        None => format!("unknown argument: {word}\n\n{}", usage()),
    }
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut cmd: Option<Cmd> = None;
    let mut ghost = false;
    let mut recall = false;
    let mut lean = None;
    let mut model = None;
    let mut provider = None;
    let mut from = None;
    let mut write = false;
    let mut force = false;
    let mut secrets = false;
    let mut check = false;
    let mut json_out = false;
    let mut install_daemon = false;
    let mut test_fallback = false;
    let mut prompt_words: Vec<String> = Vec::new();
    let mut in_run = false;
    let mut in_skill = false;
    let mut skill_words: Vec<String> = Vec::new();
    let mut in_service = false;
    let mut service_words: Vec<String> = Vec::new();
    let mut in_tasks = false;
    let mut task_words: Vec<String> = Vec::new();
    let mut in_secret = false;
    let mut secret_words: Vec<String> = Vec::new();
    let mut in_sub = false;
    let mut sub_name = String::new();
    let mut sub_words: Vec<String> = Vec::new();
    let mut it = argv.iter();
    let mut literal = false;
    while let Some(arg) = it.next() {
        if literal {
            if in_run {
                prompt_words.push(arg.clone());
            } else if in_skill {
                skill_words.push(arg.clone());
            } else if in_secret {
                secret_words.push(arg.clone());
            } else if in_service {
                service_words.push(arg.clone());
            } else if in_sub {
                sub_words.push(arg.clone());
            } else {
                return Err(unknown_arg(arg));
            }
            continue;
        }
        match arg.as_str() {
            "--" => literal = true,
            "-V" | "--version" => return Err(format!("version:openphoenix {VERSION}")),
            "-h" | "--help" => return Err(format!("help:{}", usage())),
            "--ghost" => ghost = true,
            "--recall" => recall = true,
            "--lean" => {
                let v = it.next().ok_or("--lean needs a value")?;
                if !LEAN_LEVELS.contains(&v.as_str()) {
                    return Err(format!("--lean must be one of {LEAN_LEVELS:?}"));
                }
                lean = Some(v.clone());
            }
            "--model" => model = Some(it.next().ok_or("--model needs a value")?.clone()),
            "--provider" => provider = Some(it.next().ok_or("--provider needs a value")?.clone()),
            "--from" => from = Some(it.next().ok_or("--from needs a path")?.clone()),
            "--write" => write = true,
            "--force" => force = true,
            "--secrets" => secrets = true,
            "--check" => check = true,
            "--json" => json_out = true,
            "--install-daemon" => install_daemon = true,
            "--test-fallback" => test_fallback = true,
            "init" | "chat" | "serve" | "doctor" | "jobs" | "migrate" | "update" | "models"
            | "schema" | "configure" | "status" | "dashboard" | "onboard" | "setup"
            | "terminal" | "tui" | "cron" | "daemon" | "help" | "commands" | "docs" | "system"
            | "channels" | "directory" | "exec-policy" | "gateway" | "webhooks" | "security"
            | "health" | "reset" | "uninstall" | "agent" | "message" | "capability" | "infer"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                cmd = Some(match arg.as_str() {
                    "init" => Cmd::Init,
                    "configure" | "onboard" | "setup" => Cmd::Configure,
                    "status" => Cmd::Status,
                    "dashboard" => Cmd::Dashboard,
                    "serve" => Cmd::Serve,
                    "doctor" => Cmd::Doctor,
                    "jobs" | "cron" => Cmd::Jobs,
                    "migrate" => Cmd::Migrate,
                    "update" => Cmd::Update,
                    "models" => Cmd::Models,
                    "schema" => Cmd::Schema,
                    "commands" => Cmd::Commands,
                    "docs" => Cmd::Docs,
                    "system" => Cmd::System,
                    "channels" => Cmd::Channels,
                    "directory" => Cmd::Directory,
                    "exec-policy" => Cmd::ExecPolicy,
                    "gateway" => Cmd::Gateway,
                    "webhooks" => Cmd::Webhooks,
                    "security" => Cmd::Security,
                    "health" => Cmd::Health,
                    "capability" | "infer" => Cmd::Capability,
                    "reset" => Cmd::Reset,
                    "uninstall" => Cmd::Uninstall,
                    "help" => return Err(format!("help:{}", usage())),
                    "daemon" => Cmd::Service(Vec::new()),
                    "agent" | "message" => Cmd::Run(String::new()),
                    _ => Cmd::Chat,
                });
                if matches!(arg.as_str(), "agent" | "message") {
                    in_run = true;
                    cmd = None;
                }
            }
            "memory" | "audit" | "backup" | "transcripts" | "completion" | "approvals"
            | "exec-approvals" | "secrets" | "skills" | "logs" | "config" | "board" | "canvas"
            | "media" | "oauth" | "transcribe" | "worktrees" | "mcp" | "hooks" | "agents"
            | "commitments" | "proxy" | "attach" | "sessions"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_sub = true;
                sub_name = arg.clone();
            }

            "run"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_run = true;
            }
            "skill"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_skill = true;
            }
            "secret"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_secret = true;
            }
            "service"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_service = true;
            }
            "tasks"
                if cmd.is_none()
                    && !in_run
                    && !in_sub
                    && !in_skill
                    && !in_secret
                    && !in_service
                    && !in_tasks =>
            {
                in_tasks = true;
            }
            other => {
                if in_run {
                    prompt_words.push(other.to_string());
                } else if in_skill {
                    skill_words.push(other.to_string());
                } else if in_secret {
                    secret_words.push(other.to_string());
                } else if in_service {
                    service_words.push(other.to_string());
                } else if in_tasks {
                    task_words.push(other.to_string());
                } else if in_sub {
                    sub_words.push(other.to_string());
                } else {
                    return Err(unknown_arg(other));
                }
            }
        }
    }
    let cmd = if in_run {
        if prompt_words.is_empty() {
            return Err("run needs a prompt (or pass - to read it from stdin)".into());
        }
        Cmd::Run(prompt_words.join(" "))
    } else if in_skill {
        if skill_words.is_empty() {
            return Err("skill needs a subcommand: search QUERY | install OWNER/SLUG".into());
        }
        Cmd::Skill(skill_words)
    } else if in_secret {
        Cmd::Secret(secret_words)
    } else if in_service {
        Cmd::Service(service_words)
    } else if in_tasks {
        Cmd::Tasks(task_words)
    } else if in_sub {
        match sub_name.as_str() {
            "memory" => Cmd::Memory(sub_words),
            "sessions" => Cmd::Sessions(sub_words),
            "audit" => Cmd::Audit(sub_words),
            "backup" => Cmd::Backup(sub_words),
            "transcripts" => Cmd::Transcripts(sub_words),
            "completion" => Cmd::Completion(sub_words),
            "approvals" | "exec-approvals" => {
                let mut w = vec!["approvals".to_string()];
                w.extend(sub_words);
                Cmd::Service(w)
            }
            "secrets" => Cmd::Secret(sub_words),
            "skills" => {
                if sub_words.is_empty() {
                    return Err(
                        "skills needs a subcommand: search QUERY | install OWNER/SLUG".into(),
                    );
                }
                Cmd::Skill(sub_words)
            }
            "logs" => Cmd::Service(vec!["logs".to_string()]),
            "config" => Cmd::ConfigFile(sub_words),
            "board" => Cmd::Board(sub_words),
            "canvas" => Cmd::Canvas(sub_words),
            "mcp" => Cmd::Mcp(sub_words),
            "hooks" => Cmd::Hooks(sub_words),
            "agents" => Cmd::Agents(sub_words),
            "commitments" => Cmd::Commitments(sub_words),
            "proxy" => Cmd::Proxy(sub_words),
            "attach" => Cmd::Attach(sub_words),
            "media" => Cmd::Media(sub_words),
            "oauth" => Cmd::Oauth(sub_words),
            "transcribe" => Cmd::Transcribe(sub_words),
            "worktrees" => Cmd::Worktrees(sub_words),
            other => return Err(unknown_arg(other)),
        }
    } else {
        cmd.unwrap_or(Cmd::Chat)
    };
    Ok(Args {
        cmd,
        ghost,
        recall,
        lean,
        model,
        provider,
        from,
        write,
        force,
        secrets,
        check,
        json: json_out,
        install_daemon,
        test_fallback,
    })
}

fn cost_estimate(model: &str, u: &providers::Usage) -> Option<f64> {
    let m = model.to_ascii_lowercase();
    let (i, o) = if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku") {
        (1.0, 5.0)
    } else if m.contains("deepseek") {
        (0.3, 1.2)
    } else if m.contains("gpt") {
        (5.0, 15.0)
    } else {
        return None;
    };
    Some((u.input as f64 * i + u.output as f64 * o) / 1e6)
}

fn usage_line(model: &str, u: &providers::Usage) -> String {
    match cost_estimate(model, u) {
        Some(c) => format!("tokens: in={} out={} (~${c:.4} est)", u.input, u.output),
        None => format!("tokens: in={} out={}", u.input, u.output),
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn nearest_command(word: &str) -> Option<&'static str> {
    let w = word.to_ascii_lowercase();
    let limit = if w.len() <= 4 { 1 } else { 2 };
    commands::COMMANDS
        .iter()
        .map(|c| (c.name, edit_distance(&w, c.name)))
        .filter(|(_, d)| *d <= limit)
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| c)
}

fn usage() -> String {
    format!(
        "usage: phoenix [-V] [--ghost] [--recall] [--lean LEVEL] [--model NAME] \
[--provider KIND] [init|run PROMPT|chat|serve|doctor|jobs|tasks|sessions|skill|migrate]\n\
  init      write sample config\n\
  configure re-run the setup wizard on an existing nest\n\
  status    one screen: config, model, channels, service, http\n\
  dashboard open the web UI in a browser\n\
  run       one-shot task, ghost by default; run - reads the prompt from stdin\n\
  chat      interactive REPL (default)\n\
  serve     all configured channels + http api + cron jobs + dreaming\n\
  doctor    audit config, permissions, and risky settings\n\
  jobs      list cron jobs and validate their schedules\n\
  tasks     list background tasks: tasks | tasks ID | tasks cancel ID\n\
  sessions  list stored serve-mode sessions\n\
  skill     search or install ClawHub skills: skill search QUERY | skill install OWNER/SLUG\n\
  service   run serve as a background service: install|uninstall|start|stop|restart|status|logs\n\
  migrate   convert an AI gateway config [--from PATH] [--write] [--force] [--secrets]\n\
  update    fetch the latest release, verify checksum, swap this binary [--check]\n\
  models    list live models from the current provider (plus aliases)\n\
  schema    print the JSON Schema contract for config.toml\n\
  secret    encrypted secret store: secret set NAME | list | rm NAME | export [env]\n\
\n\
  --json    machine-readable output for doctor, jobs, tasks, sessions, status\n\
  --install-daemon  with init or configure: install the background service too\n\
\n\
first time here?\n\
  phoenix configure          answer a few questions and it sets itself up\n\
  phoenix doctor             check config, keys, and permissions\n\
  phoenix run \"say hello\"     one task, then it forgets\n\
  phoenix serve              go live on your chat apps\n\
\n\
no API key yet? ollama needs none; google and nvidia have free tiers.\n\
{DOCS_URL}\n\
openphoenix {VERSION}"
    )
}

fn cmd_secret(words: &[String]) -> u8 {
    let store = secrets::Store::at(&secrets::Store::default_path());
    let sub = words.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" => {
            let names = match store.names() {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            if names.is_empty() {
                println!("no stored secrets");
            } else {
                for n in names {
                    println!("{n}");
                }
            }
            0
        }
        "export" => {
            let fmt = words.get(1).map(String::as_str).unwrap_or("shell");
            let entries = match store.load() {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            if entries.is_empty() {
                eprintln!("no stored secrets; nothing to export");
                return 0;
            }
            match fmt {
                "shell" => {
                    for (k, v) in &entries {
                        println!("export {k}='{}'", v.replace('\'', "'\\''"));
                    }
                    eprintln!("load into this shell:  eval \"$(phoenix secret export)\"");
                }
                "env" | "docker" | "systemd" => {
                    for (k, v) in &entries {
                        println!("{k}={v}");
                    }
                    eprintln!(
                        "env-file format: docker run --env-file <(phoenix secret export env) \
or append to ~/.openphoenix/env for the service"
                    );
                }
                other => {
                    eprintln!("unknown format '{other}'; use: shell (default) | env");
                    return 2;
                }
            }
            0
        }
        "set" => {
            let Some(name) = words.get(1) else {
                eprintln!("usage: phoenix secret set NAME (value is read from stdin)");
                return 2;
            };
            let value = match read_prompt_stdin() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            match store.put(name, &value) {
                Ok(()) => {
                    println!(
                        "stored {name} in {}",
                        secrets::Store::default_path().display()
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        "rm" => {
            let Some(name) = words.get(1) else {
                eprintln!("usage: phoenix secret rm NAME");
                return 2;
            };
            match store.load() {
                Ok(mut all) => {
                    if all.remove(name).is_none() {
                        println!("no secret named {name}");
                        return 0;
                    }
                    match store.save(&all) {
                        Ok(()) => {
                            println!("removed {name}");
                            0
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            2
                        }
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        _ => {
            eprintln!(
                "usage: phoenix secret set NAME | secret list | secret rm NAME | secret export [env]"
            );
            eprintln!("the store is encrypted with {}", secrets::KEY_VAR);
            2
        }
    }
}

fn cmd_skill(words: &[String]) -> u8 {
    let cfg = match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let result = match words[0].as_str() {
        "search" if words.len() > 1 => clawhub::search(&cfg, &words[1..].join(" ")),
        "install" if words.len() == 2 => {
            clawhub::install(&cfg, &words[1], &config::home().join("skills"))
        }
        _ => Err("usage: phoenix skill search QUERY | phoenix skill install OWNER/SLUG".into()),
    };
    match result {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

static ALLOW_ALL_SHELL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, PartialEq)]
pub enum Approval {
    Yes,
    No,
    Always,
    Unclear,
}

pub fn read_approval(line: &str) -> Approval {
    let t = line.trim().to_ascii_lowercase();
    let t = t.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    match t {
        "" | "n" | "no" | "nope" | "deny" => Approval::No,
        "y" | "ye" | "yes" | "yeah" | "yep" | "ok" | "okay" | "sure" => Approval::Yes,
        "a" | "all" | "always" | "yolo" => Approval::Always,
        _ => Approval::Unclear,
    }
}

fn confirm_shell(command: &str) -> bool {
    if ALLOW_ALL_SHELL.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let lines: Vec<&str> = command.lines().collect();
    loop {
        println!();
        if lines.len() <= 1 {
            println!("  run this command?");
            println!("    {command}");
        } else {
            println!("  run these commands?");
            for l in &lines {
                println!("    {l}");
            }
        }
        print!("  [y] yes   [n] no   [a] yes to everything this session > ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                println!("\n  no answer, so that is a no.");
                return false;
            }
            Ok(_) => {}
        }
        match read_approval(&line) {
            Approval::Yes => return true,
            Approval::No => {
                println!("  skipped.");
                return false;
            }
            Approval::Always => {
                ALLOW_ALL_SHELL.store(true, std::sync::atomic::Ordering::SeqCst);
                println!("  fine, no more asking until you restart.");
                return true;
            }
            Approval::Unclear => {
                println!("  did not catch that. y for yes, n for no, a for yes to everything.");
            }
        }
    }
}

fn audit_sink(cfg: &Config) -> audit::Audit {
    if cfg.audit_log {
        audit::Audit::at(&config::home().join("audit.jsonl"))
    } else {
        audit::Audit::disabled()
    }
}

fn tasks_due(owner: &str) -> Vec<(u64, String)> {
    let path = tasks::default_path();
    tasks::reap(&path);
    let mut out = Vec::new();
    for t in tasks::undelivered(&path, owner) {
        let body = tasks::tail(&t, tasks::RESULT_TAIL);
        let body = if body.is_empty() {
            "(no output)".to_string()
        } else {
            body
        };
        out.push((
            t.id,
            format!("background task finished\n{}\n\n{body}", tasks::line(&t)),
        ));
    }
    out
}

fn tasks_delivered(id: u64) {
    tasks::mark_delivered(&tasks::default_path(), id);
}

fn build_agent(cfg: &Config, interactive: bool) -> Result<Agent, String> {
    let memory = Memory::in_workspace(&cfg.privacy, &cfg.workspace);
    let confirm: Option<tools::ConfirmFn> = if interactive && cfg.confirm_shell {
        Some(Box::new(confirm_shell))
    } else {
        None
    };
    let on_event: tools::EventFn = Box::new(|name: &str, args: &Value| {
        let a: String = args.to_string().chars().take(120).collect();
        eprintln!("  → {name} {a}");
    });
    let mut toolbox = Toolbox::new(cfg, memory, confirm, Some(on_event))?;
    if !cfg.mcp_servers.is_empty() {
        let (_, tools, problems) = mcp::connect_all(&cfg.mcp_servers);
        for p in &problems {
            eprintln!("mcp: {p}");
        }
        if !tools.is_empty() {
            toolbox.attach_mcp(tools);
            let names = toolbox.mcp_tool_names();
            eprintln!(
                "mcp: {} tools available ({})",
                names.len(),
                names.join(", ")
            );
        }
    }
    let provider = providers::make(cfg).map_err(|e| e.to_string())?;
    let mut agent = Agent::new(cfg.clone(), Box::new(provider), toolbox);
    agent.skills = skills::load_all(&[cfg.workspace.join("skills"), config::home().join("skills")]);
    Ok(agent)
}

fn slash(line: &str, agent: &mut Agent) -> bool {
    let cmd = line.split_whitespace().next().unwrap_or("");
    match cmd {
        "/quit" | "/exit" => return true,
        "/ghost" | "/session" | "/recall" => {
            let mode = &cmd[1..];
            agent.cfg.privacy = mode.to_string();
            agent.toolbox.memory.privacy = mode.to_string();
            agent.wipe();
            println!("privacy → {mode} (history wiped)");
            return false;
        }
        "/wipe" => {
            agent.wipe();
            println!("history wiped");
            return false;
        }
        _ => {}
    }
    let cfg = agent.cfg.clone();
    match channel_command(Some(agent), &cfg, line) {
        Some(reply) => println!("{}", reply.flatten()),
        None => println!("unknown command, try /help"),
    }
    false
}

fn cmd_chat(mut cfg: Config) -> u8 {
    cfg.approvals = false;
    let mut agent = match build_agent(&cfg, true) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    agent.stream_stdout = cfg.stream;
    let tty = io::stdout().is_terminal();
    let (c_you, c_phx, c_dim, c_off) = if tty {
        ("\x1b[36;1m", "\x1b[38;5;208;1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };
    println!("{}", banner());
    println!(
        "{c_dim}model={} privacy={} lean={}  (/help for commands){c_off}\n",
        cfg.model, cfg.privacy, cfg.lean
    );
    let stdin = io::stdin();
    loop {
        print!("{c_you}you ›{c_off} ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                println!();
                return 0;
            }
            Ok(_) => {}
        }
        let had_ctrl_c = line.contains('\u{3}');
        let cleaned: String = line.chars().filter(|c| *c != '\u{3}').collect();
        let line = cleaned.trim();
        if line.is_empty() {
            if had_ctrl_c {
                println!("  (cancelled; /exit or Ctrl-D to leave)");
            }
            continue;
        }
        if line.starts_with('/') {
            if slash(line, &mut agent) {
                return 0;
            }
            continue;
        }
        if matches!(
            line.to_ascii_lowercase().as_str(),
            "exit" | "quit" | "bye" | ":q"
        ) {
            println!("\nuntil next time.");
            return 0;
        }
        crate::agent::arm_interrupt();
        if agent.stream_stdout {
            print!("\n{c_phx}phoenix ›{c_off} ");
            let _ = io::stdout().flush();
            let out = crate::text::sanitize_terminal(&agent.run(line));
            if !agent.streamed_last {
                print!("{out}");
            }
            println!("\n");
        } else {
            println!(
                "\n{c_phx}phoenix ›{c_off} {}\n",
                crate::text::sanitize_terminal(&agent.run(line))
            );
        }
        crate::agent::disarm_interrupt();
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct CmdReply {
    pub text: String,
    pub buttons: Vec<Vec<(String, String)>>,
}

impl CmdReply {
    pub fn flatten(&self) -> String {
        if self.buttons.is_empty() {
            return self.text.clone();
        }
        let opts: Vec<String> = self
            .buttons
            .iter()
            .flatten()
            .map(|(label, _)| label.trim_start_matches("\u{2705} ").to_string())
            .collect();
        format!("{}\n{}", self.text, opts.join(" | "))
    }

    fn say(text: impl Into<String>) -> Option<CmdReply> {
        Some(CmdReply {
            text: text.into(),
            buttons: Vec::new(),
        })
    }
    fn pick(text: impl Into<String>, buttons: Vec<Vec<(String, String)>>) -> Option<CmdReply> {
        Some(CmdReply {
            text: text.into(),
            buttons,
        })
    }
}

pub const CHAT_COMMANDS: [&str; 33] = [
    "activation",
    "approve",
    "commands",
    "compact",
    "context",
    "forget",
    "deny",
    "exit",
    "fast",
    "ghost",
    "help",
    "lean",
    "model",
    "models",
    "new",
    "pending",
    "privacy",
    "quit",
    "recall",
    "reset",
    "session",
    "skill",
    "status",
    "stop",
    "tasks",
    "think",
    "thinking",
    "tools",
    "trace",
    "usage",
    "verbose",
    "whoami",
    "wipe",
];

fn command_token(text: &str) -> Option<(String, bool)> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let name = first.strip_prefix('/')?;
    let name = name.split('@').next().unwrap_or(name);
    if name.is_empty() || name.len() > 20 {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((name.to_ascii_lowercase(), words.next().is_none()))
}

pub fn command_name(text: &str) -> Option<&'static str> {
    let (name, _) = command_token(text)?;
    CHAT_COMMANDS.iter().find(|c| **c == name).copied()
}

pub fn nearest_chat_command(text: &str) -> Option<&'static str> {
    let (name, _) = command_token(text)?;
    let limit = if name.len() <= 4 { 1 } else { 2 };
    CHAT_COMMANDS
        .iter()
        .map(|c| (*c, edit_distance(&name, c)))
        .filter(|(_, d)| *d > 0 && *d <= limit)
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| c)
}

pub fn looks_like_command(text: &str) -> bool {
    let Some((_, bare)) = command_token(text) else {
        return false;
    };
    command_name(text).is_some() || bare || nearest_chat_command(text).is_some()
}

pub const ABORT_TRIGGERS: [&str; 23] = [
    "stop",
    "esc",
    "abort",
    "interrupt",
    "halt",
    "cancel",
    "stop please",
    "please stop",
    "stop it",
    "stop now",
    "stop phoenix",
    "phoenix stop",
    "stop action",
    "stop current action",
    "stop run",
    "stop current run",
    "stop agent",
    "stop the agent",
    "stop doing anything",
    "detente",
    "arrete",
    "stopp",
    "pare",
];

pub fn is_abort_request(text: &str) -> bool {
    let mut norm: String = text
        .to_lowercase()
        .replace(['\u{2019}', '`'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    while norm
        .chars()
        .last()
        .map(|c| ".!?\u{ff01}\u{ff1f}\u{2026},\u{ff0c}\u{3002};\u{ff1b}:\u{ff1a}'\")]}".contains(c))
        .unwrap_or(false)
    {
        norm.pop();
    }
    let bare = norm.strip_prefix('/').unwrap_or(&norm);
    ABORT_TRIGGERS.contains(&bare)
}

fn level_buttons(current: &str, levels: &[&str], cmd: &str) -> Vec<Vec<(String, String)>> {
    let mut rows: Vec<Vec<(String, String)>> = Vec::new();
    for chunk in levels.chunks(3) {
        rows.push(
            chunk
                .iter()
                .map(|l| {
                    let mark = if *l == current { "✅ " } else { "" };
                    (format!("{mark}{l}"), format!("{cmd} {l}"))
                })
                .collect(),
        );
    }
    rows
}

fn model_buttons(cfg: &Config, current: &str) -> Vec<Vec<(String, String)>> {
    let mut names: Vec<String> = providers::list_models(cfg).unwrap_or_default();
    if names.is_empty() {
        names = ["opus", "sonnet", "gpt", "gemini"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    names.retain(|n| format!("/model {n}").len() <= 64);
    let mut rows: Vec<Vec<(String, String)>> = Vec::new();
    for chunk in names.iter().take(12).collect::<Vec<_>>().chunks(2) {
        rows.push(
            chunk
                .iter()
                .map(|n| {
                    let mark = if n.as_str() == current { "✅ " } else { "" };
                    (format!("{mark}{n}"), format!("/model {n}"))
                })
                .collect(),
        );
    }
    rows
}

fn apply_model_choice(a: &mut Agent, arg: &str) -> Result<(String, String), String> {
    let before = a.cfg.provider.clone();
    match providers::resolve_alias(arg) {
        Some((kind, m)) if kind != a.cfg.provider => {
            a.retarget(&format!("{kind}/{m}"))?;
        }
        Some((_, m)) => {
            a.cfg.model = m.to_string();
        }
        None => {
            a.retarget(arg)?;
        }
    }
    let note = if a.cfg.provider != before {
        let key_note = if a.cfg.api_key.is_empty() && a.cfg.provider != "ollama" {
            let hint = config::provider_key_vars(&a.cfg.provider)
                .first()
                .copied()
                .unwrap_or("PHOENIX_API_KEY");
            format!(
                "\nwarning: no API key found for {}; export {hint}=…",
                a.cfg.provider
            )
        } else {
            String::new()
        };
        format!("\nprovider → {}{key_note}", a.cfg.provider)
    } else {
        String::new()
    };
    Ok((a.cfg.model.clone(), note))
}

fn channel_command(agent: Option<&mut Agent>, cfg: &Config, text: &str) -> Option<CmdReply> {
    let t = text.trim();
    if !looks_like_command(t) {
        if is_abort_request(t) {
            let Some(a) = agent else {
                return CmdReply::say("nothing running");
            };
            let ids: Vec<u64> = a
                .toolbox
                .pending_entries()
                .iter()
                .map(|(i, _)| *i)
                .collect();
            if ids.is_empty() {
                return CmdReply::say("nothing to stop right now");
            }
            let n = ids.len();
            for id in ids {
                let _ = a.toolbox.deny(id);
            }
            return CmdReply::say(format!("stopped: denied {n} pending command(s)"));
        }
        return None;
    }
    let (head, arg) = match t.split_once(char::is_whitespace) {
        Some((h, a)) => (h, a.trim()),
        None => (t, ""),
    };
    let head_owned = head.split('@').next().unwrap_or(head).to_lowercase();
    let head: &str = &head_owned;
    let cur_privacy = agent
        .as_ref()
        .map(|a| a.cfg.privacy.clone())
        .unwrap_or_else(|| cfg.privacy.clone());
    let cur_lean = agent
        .as_ref()
        .map(|a| a.cfg.lean.clone())
        .unwrap_or_else(|| cfg.lean.clone());
    let cur_thinking = agent
        .as_ref()
        .map(|a| a.cfg.thinking.clone())
        .unwrap_or_else(|| cfg.thinking.clone());
    let cur_model = agent
        .as_ref()
        .map(|a| a.cfg.model.clone())
        .unwrap_or_else(|| cfg.model.clone());

    match head {
        "/help" | "/commands" => {
            return CmdReply::say(
                "ℹ️ Help\n\
\n\
Session\n\
/new | /reset | /compact [instructions] | /stop\n\
\n\
Options\n\
/think <level> | /model <id> | /models | /fast on|off\n\
/verbose on|off|full | /trace on|off|raw | /lean off|lean|grunt\n\
/privacy ghost|session|recall\n\
\n\
Status\n\
/status | /tasks | /whoami | /context | /usage\n\
\n\
Skills\n\
/skill <name> [input]\n\
\n\
Approvals\n\
/pending | /approve <id> | /deny <id>\n\
\n\
More: /tools for available capabilities. Send any option command \
without an argument to get buttons.",
            );
        }
        "/privacy" => {
            if config::PRIVACY_MODES.contains(&arg) {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                a.cfg.privacy = arg.to_string();
                a.toolbox.memory.privacy = arg.to_string();
                a.wipe();
                return CmdReply::say(format!("privacy → {arg} (this chat, history wiped)"));
            }
            return CmdReply::pick(
                format!("privacy: {cur_privacy}\npick one:"),
                level_buttons(&cur_privacy, &config::PRIVACY_MODES, "/privacy"),
            );
        }
        "/lean" => {
            if config::LEAN_LEVELS.contains(&arg) {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                a.cfg.lean = arg.to_string();
                return CmdReply::say(format!("lean → {arg} (this chat)"));
            }
            return CmdReply::pick(
                format!("lean: {cur_lean}\npick one:"),
                level_buttons(&cur_lean, &config::LEAN_LEVELS, "/lean"),
            );
        }
        "/think" | "/thinking" => {
            if config::THINKING_LEVELS.contains(&arg) {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                a.cfg.thinking = arg.to_string();
                return CmdReply::say(format!("thinking → {arg} (this chat)"));
            }
            return CmdReply::pick(
                format!("thinking: {cur_thinking}\npick a level:"),
                level_buttons(&cur_thinking, &config::THINKING_LEVELS, "/think"),
            );
        }
        "/model" | "/models" => {
            if !arg.is_empty() {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                return match apply_model_choice(a, arg) {
                    Ok((model, note)) => {
                        CmdReply::say(format!("model → {model} (this chat){note}"))
                    }
                    Err(e) => CmdReply::say(format!("model unchanged: {e}")),
                };
            }
            return CmdReply::pick(
                format!("model: {cur_model}\npick one:"),
                model_buttons(cfg, &cur_model),
            );
        }
        "/forget" => {
            let st = crate::state::State::load();
            return CmdReply::say(match st.clear() {
                Ok(()) => "state cleared: activation, last seen, and cooldowns".to_string(),
                Err(e) => format!("could not clear state: {e}"),
            });
        }
        "/new" | "/reset" => {
            let Some(a) = agent else {
                return CmdReply::say("session reset");
            };
            a.wipe();
            return CmdReply::say("session reset");
        }
        "/compact" => {
            let Some(a) = agent else {
                return CmdReply::say("nothing to compact");
            };
            return CmdReply::say(a.compact_now(arg));
        }
        "/stop" => {
            let Some(a) = agent else {
                return CmdReply::say("nothing running");
            };
            let n = a.toolbox.pending_count();
            if n == 0 {
                return CmdReply::say(
                    "nothing to stop: phoenix runs one turn at a time and this one is done",
                );
            }
            let ids: Vec<u64> = a
                .toolbox
                .pending_entries()
                .iter()
                .map(|(i, _)| *i)
                .collect();
            for id in ids {
                let _ = a.toolbox.deny(id);
            }
            return CmdReply::say(format!("stopped: denied {n} pending command(s)"));
        }
        "/fast" => {
            let Some(a) = agent else {
                return CmdReply::say("no active chat session yet");
            };
            match arg {
                "on" | "auto" => {
                    if a.cfg.fast_model.is_empty() {
                        return CmdReply::say(
                            "no fast model set: add fast_model = \"provider/model\" under [agent]",
                        );
                    }
                    if a.cfg.prev_model.is_empty() {
                        a.cfg.prev_model = a.cfg.model.clone();
                    }
                    let spec = a.cfg.fast_model.clone();
                    if let Err(e) = a.retarget(&spec) {
                        a.cfg.prev_model = String::new();
                        return CmdReply::say(format!("fast unchanged: {e}"));
                    }
                    return CmdReply::say(format!("fast → on ({})", a.cfg.model));
                }
                "off" | "default" => {
                    if a.cfg.prev_model.is_empty() {
                        return CmdReply::say(format!("fast already off ({})", a.cfg.model));
                    }
                    let prev = std::mem::take(&mut a.cfg.prev_model);
                    if let Err(e) = a.retarget(&prev) {
                        a.cfg.prev_model = prev;
                        return CmdReply::say(format!("fast unchanged: {e}"));
                    }
                    return CmdReply::say(format!("fast → off ({})", a.cfg.model));
                }
                _ => {
                    let state = if a.cfg.prev_model.is_empty() {
                        "off"
                    } else {
                        "on"
                    };
                    let fast = if a.cfg.fast_model.is_empty() {
                        "(unset)".to_string()
                    } else {
                        a.cfg.fast_model.clone()
                    };
                    return CmdReply::pick(
                        format!(
                            "fast: {state}\nfast model: {fast}\ncurrent: {}",
                            a.cfg.model
                        ),
                        level_buttons(state, &config::FAST_LEVELS, "/fast"),
                    );
                }
            }
        }
        "/verbose" => {
            if config::VERBOSE_LEVELS.contains(&arg) {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                a.cfg.verbose = arg.to_string();
                return CmdReply::say(format!("verbose → {arg} (this chat)"));
            }
            let cur = agent
                .map(|a| a.cfg.verbose.clone())
                .unwrap_or_else(|| cfg.verbose.clone());
            return CmdReply::pick(
                format!("verbose: {cur}\nshow tool calls after each reply:"),
                level_buttons(&cur, &config::VERBOSE_LEVELS, "/verbose"),
            );
        }
        "/trace" => {
            if config::TRACE_LEVELS.contains(&arg) {
                let Some(a) = agent else {
                    return CmdReply::say("no active chat session yet");
                };
                a.cfg.trace = arg.to_string();
                if a.cfg.verbose == "off" && arg != "off" {
                    a.cfg.verbose = "on".to_string();
                    return CmdReply::say(format!("trace → {arg} (verbose turned on too)"));
                }
                return CmdReply::say(format!("trace → {arg} (this chat)"));
            }
            let cur = agent
                .map(|a| a.cfg.trace.clone())
                .unwrap_or_else(|| cfg.trace.clone());
            return CmdReply::pick(
                format!("trace: {cur}\nhow much tool detail to show:"),
                level_buttons(&cur, &config::TRACE_LEVELS, "/trace"),
            );
        }
        "/tools" => {
            let Some(a) = agent else {
                return CmdReply::say("no active chat session yet");
            };
            let names = a.toolbox.available();
            return CmdReply::say(format!(
                "{} tools available:\n{}",
                names.len(),
                names.join(", ")
            ));
        }
        "/context" => {
            let Some(a) = agent else {
                return CmdReply::say("no active chat session yet");
            };
            return CmdReply::say(a.context_report());
        }
        "/whoami" => {
            let persona = config::home().join("persona");
            let carried: Vec<String> = std::fs::read_dir(&persona)
                .map(|d| {
                    d.flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            let identity = if carried.is_empty() {
                "phoenix (no persona files carried)".to_string()
            } else {
                format!("persona: {}", carried.join(", "))
            };
            return CmdReply::say(format!(
                "openphoenix {VERSION}\n{identity}\nmodel: {}/{cur_model}\nworkspace: {}\n\
privacy: {cur_privacy}",
                cfg.provider,
                cfg.workspace.display()
            ));
        }
        "/tasks" => {
            let tpath = tasks::default_path();
            tasks::reap(&tpath);
            if arg == "bg" {
                return CmdReply::say(tasks::render(&tasks::list(&tpath, None)));
            }
            if let Some(rest) = arg.strip_prefix("cancel") {
                let Ok(id) = rest.trim().parse::<u64>() else {
                    return CmdReply::say("usage: /tasks cancel ID");
                };
                return CmdReply::say(match tasks::cancel(&tpath, id) {
                    Ok(m) => m,
                    Err(e) => e,
                });
            }
            let path = config::home().join("board.json");
            let status = if arg.is_empty() { None } else { Some(arg) };
            let cards = match crate::board::list(&path, status) {
                Ok(list) => list,
                Err(e) => format!("task board error: {e}"),
            };
            if !arg.is_empty() {
                return CmdReply::say(cards);
            }
            let bg = tasks::list(&tpath, None);
            if bg.is_empty() {
                return CmdReply::say(cards);
            }
            return CmdReply::say(format!("{cards}\n\nbackground:\n{}", tasks::render(&bg)));
        }
        "/skill" => {
            let Some(a) = agent else {
                return CmdReply::say("no active chat session yet");
            };
            if a.skills.is_empty() {
                return CmdReply::say(
                    "no skills installed: add them with: phoenix skill install OWNER/SLUG",
                );
            }
            if arg.is_empty() {
                let names: Vec<String> = a
                    .skills
                    .iter()
                    .map(|s| format!("• {}: {}", s.name, s.description))
                    .collect();
                return CmdReply::say(format!(
                    "{} skill(s):\n{}\nrun one: /skill NAME [input]",
                    a.skills.len(),
                    names.join("\n")
                ));
            }
            let (name, input) = match arg.split_once(char::is_whitespace) {
                Some((n, i)) => (n, i.trim()),
                None => (arg, ""),
            };
            let Some(skill) = a
                .skills
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(name))
                .cloned()
            else {
                let names: Vec<&str> = a.skills.iter().map(|s| s.name.as_str()).collect();
                return CmdReply::say(format!(
                    "no skill '{name}'\navailable: {}",
                    names.join(", ")
                ));
            };
            let prompt = if input.is_empty() {
                format!("Follow this skill now.\n\n{}", skill.body)
            } else {
                format!("Follow this skill now.\n\n{}\n\nInput: {input}", skill.body)
            };
            return CmdReply::say(a.run(&prompt));
        }
        "/usage" => {
            return CmdReply::say(match agent {
                Some(a) => usage_line(&a.cfg.model, &a.usage),
                None => "no usage yet in this chat".to_string(),
            });
        }
        "/pending" => {
            return CmdReply::say(match agent {
                Some(a) => a.toolbox.pending_list(),
                None => "nothing pending".to_string(),
            });
        }
        _ => {}
    }
    if t == "/status" {
        let (model, lean, thinking, privacy, pending, input, output) = match &agent {
            Some(a) => (
                a.cfg.model.clone(),
                a.cfg.lean.clone(),
                a.cfg.thinking.clone(),
                a.cfg.privacy.clone(),
                a.toolbox.pending_count(),
                a.usage.input,
                a.usage.output,
            ),
            None => (
                cfg.model.clone(),
                cfg.lean.clone(),
                cfg.thinking.clone(),
                cfg.privacy.clone(),
                0,
                0,
                0,
            ),
        };
        return CmdReply::say(format!(
            "openphoenix {VERSION}\nmodel: {}/{model}\nprivacy: {privacy} | lean: {lean} | \
thinking: {thinking}\nsessions: {} | approvals: {}\npending approvals: {pending}\n{}\n\
/help for all commands",
            cfg.provider,
            if cfg.sessions { "on" } else { "off" },
            if cfg.approvals { "on" } else { "off" },
            usage_line(&model, &providers::Usage { input, output }),
        ));
    }
    if matches!(head, "/approve" | "/deny") {
        let approve = head == "/approve";
        let Some(a) = agent else {
            return CmdReply::say("nothing pending");
        };
        return CmdReply::say(match arg.parse::<u64>() {
            Ok(id) if approve => a.toolbox.approve(id),
            Ok(id) => a.toolbox.deny(id),
            Err(_) if a.toolbox.pending_count() == 0 => "nothing pending".to_string(),
            Err(_) => format!("usage: {head} ID\n{}", a.toolbox.pending_list()),
        });
    }
    CmdReply::say(format!("unknown command: {head}\nsend /help for the list"))
}

const MAX_STDIN_PROMPT: u64 = 4 * 1024 * 1024;

fn read_prompt_stdin() -> Result<String, String> {
    let mut buf = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_STDIN_PROMPT + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_STDIN_PROMPT {
        return Err(format!(
            "stdin prompt exceeds {} MB",
            MAX_STDIN_PROMPT / 1024 / 1024
        ));
    }
    let text =
        String::from_utf8(buf).map_err(|_| "stdin prompt must be valid UTF-8".to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).trim();
    if text.is_empty() {
        return Err("stdin prompt is empty".into());
    }
    Ok(text.to_string())
}

fn cmd_run(mut cfg: Config, prompt: &str) -> u8 {
    let interactive = io::stdin().is_terminal();
    let prompt = if prompt.trim() == "-" {
        match read_prompt_stdin() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    } else {
        prompt.to_string()
    };
    let prompt = prompt.as_str();

    cfg.approvals = false;
    let mut agent = match build_agent(&cfg, interactive) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let reply = agent.run(prompt);
    println!("{}", crate::text::sanitize_terminal(&reply));
    if reply.starts_with("provider error:") {
        return 1;
    }
    0
}

fn cmd_serve(cfg: Config) -> u8 {
    println!("\u{1f525} phoenix rising: openphoenix {VERSION} taking flight");

    let lock_path = daemon::default_path();
    let _lock = match daemon::Lock::acquire(&lock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    daemon::install_stop_handler();
    let tasks_path = tasks::default_path();
    let recovered = tasks::reap(&tasks_path);
    tasks::prune(&tasks_path, tasks::DEFAULT_KEEP);
    if recovered > 0 {
        println!("phoenix: recovered {recovered} task(s) left behind by the last run");
    }

    let tg = match Telegram::new(&cfg) {
        Ok(t) => Some(t),
        Err(e) => {
            let others = cfg.http_enabled
                || whatsapp::WhatsApp::wanted(&cfg)
                || discord::Discord::wanted(&cfg)
                || slack::Slack::wanted(&cfg)
                || signal::Signal::wanted(&cfg)
                || imessage::IMessage::wanted(&cfg);
            if !others {
                eprintln!("error: {e}");
                return 2;
            }
            println!("phoenix: telegram off ({e}); serving remaining channels");
            None
        }
    };
    if let Err(e) = providers::make(&cfg) {
        eprintln!("error: {e}");
        return 2;
    }

    if cfg.http_enabled {
        if cfg.http_token.is_empty() {
            eprintln!("error: http.enabled requires http.token (or PHOENIX_HTTP_TOKEN)");
            return 2;
        }
        let bind_ip = cfg.http_bind.clone();
        let public = !http::is_loopback_ip(&bind_ip);
        if public && (cfg.http_web && (cfg.http_user.is_empty() || cfg.http_pass.is_empty())) {
            eprintln!(
                "error: http.bind is {bind_ip} (reachable from the network) with the web UI on \
but no username/password; refusing to expose an unauthenticated UI"
            );
            return 2;
        }
        let listener = match std::net::TcpListener::bind((bind_ip.as_str(), cfg.http_port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: http bind {bind_ip}:{}: {e}", cfg.http_port);
                return 2;
            }
        };
        if public {
            println!(
                "phoenix: http listening on {bind_ip}:{} (reachable from your network; put it \
behind HTTPS)",
                cfg.http_port
            );
        }
        let http_cfg = cfg.clone();
        let token = cfg.http_token.clone();
        let web_opts = http::WebOpts {
            web: cfg.http_web,
            audit: audit_sink(&cfg),
            canvas: cfg.canvas_enabled,
            canvas_file: canvas::state_path(),
            strong_headers: cfg.http_headers != "minimal",
            user: cfg.http_user.clone(),
            pass: cfg.http_pass.clone(),
            crawlers: cfg.http_allow_crawlers.clone(),
            model: format!("{}/{}", cfg.provider, cfg.model),
            sessions_dir: if cfg.sessions {
                config::home().join("sessions")
            } else {
                std::path::PathBuf::new()
            },
        };
        std::thread::spawn(move || {
            let handler = move |prompt: &str| {
                let mut c = http_cfg.clone();
                c.privacy = "ghost".to_string();
                c.approvals = false;
                match build_agent(&c, false) {
                    Ok(mut a) => a.run(prompt),
                    Err(e) => format!("error: {e}"),
                }
            };
            http::serve(listener, &token, handler, &web_opts);
        });
        println!("phoenix: http api on {bind_ip}:{}", cfg.http_port);
    }

    if whatsapp::WhatsApp::wanted(&cfg) {
        let wa = match whatsapp::WhatsApp::new(&cfg) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let listener = match std::net::TcpListener::bind(("127.0.0.1", cfg.wa_webhook_port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "error: whatsapp bind 127.0.0.1:{}: {e}",
                    cfg.wa_webhook_port
                );
                return 2;
            }
        };
        let wa_cfg = cfg.clone();
        let wa_sessions = cfg.sessions && cfg.privacy != "ghost";
        let wa_sess_dir = config::home().join("sessions");
        std::thread::spawn(move || {
            let mut agents: HashMap<String, Agent> = HashMap::new();
            let mut handler = move |from: &str, text: &str| -> String {
                mark_activity();
                let key = format!("wa-{from}");
                if matches!(text.trim(), "/new" | "/reset") {
                    sessions::reset(&wa_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if !agents.contains_key(&key) {
                    match build_agent(&wa_cfg, false) {
                        Ok(mut a) => {
                            if wa_sessions {
                                a.history = sessions::load(&wa_sess_dir, &key);
                            }
                            agents.insert(key.clone(), a);
                        }
                        Err(e) => return format!("error: {e}"),
                    }
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &wa_cfg, text) {
                    return reply.flatten();
                }
                let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                    return String::new();
                };
                if wa_sessions {
                    if let Some(a) = agents.get(&key) {
                        if let Err(e) = sessions::save(&wa_sess_dir, &key, &a.history) {
                            eprintln!("session save failed: {e}");
                        }
                    }
                }
                reply
            };
            wa.serve(listener, &mut handler);
        });
        println!(
            "phoenix: whatsapp webhook on 127.0.0.1:{}",
            cfg.wa_webhook_port
        );
    }

    if discord::Discord::wanted(&cfg) {
        let dc = match discord::Discord::new(&cfg) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let dc_cfg = cfg.clone();
        let dc_sessions = cfg.sessions && cfg.privacy != "ghost";
        let dc_sess_dir = config::home().join("sessions");
        std::thread::spawn(move || {
            let mut agents: HashMap<String, Agent> = HashMap::new();
            let mut handler = move |channel: &str, text: &str| -> String {
                mark_activity();
                let key = format!("dc-{channel}");
                if matches!(text.trim(), "/new" | "/reset") {
                    sessions::reset(&dc_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if !agents.contains_key(&key) {
                    match build_agent(&dc_cfg, false) {
                        Ok(mut a) => {
                            if dc_sessions {
                                a.history = sessions::load(&dc_sess_dir, &key);
                            }
                            agents.insert(key.clone(), a);
                        }
                        Err(e) => return format!("error: {e}"),
                    }
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &dc_cfg, text) {
                    return reply.flatten();
                }
                let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                    return String::new();
                };
                if dc_sessions {
                    if let Some(a) = agents.get(&key) {
                        if let Err(e) = sessions::save(&dc_sess_dir, &key, &a.history) {
                            eprintln!("session save failed: {e}");
                        }
                    }
                }
                reply
            };
            dc.serve(&mut handler);
        });
    }

    if slack::Slack::wanted(&cfg) {
        let sl = match slack::Slack::new(&cfg) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let sl_cfg = cfg.clone();
        let sl_sessions = cfg.sessions && cfg.privacy != "ghost";
        let sl_sess_dir = config::home().join("sessions");
        std::thread::spawn(move || {
            let mut agents: HashMap<String, Agent> = HashMap::new();
            let mut handler = move |channel: &str, thread: Option<&str>, text: &str| -> String {
                mark_activity();
                let key = match thread {
                    Some(ts) => format!("sl-{channel}#t{ts}"),
                    None => format!("sl-{channel}"),
                };
                if matches!(text.trim(), "/new" | "/reset") {
                    sessions::reset(&sl_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if !agents.contains_key(&key) {
                    match build_agent(&sl_cfg, false) {
                        Ok(mut a) => {
                            if sl_sessions {
                                a.history = sessions::load(&sl_sess_dir, &key);
                            }
                            agents.insert(key.clone(), a);
                        }
                        Err(e) => return format!("error: {e}"),
                    }
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &sl_cfg, text) {
                    return reply.flatten();
                }
                let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                    return String::new();
                };
                if sl_sessions {
                    if let Some(a) = agents.get(&key) {
                        if let Err(e) = sessions::save(&sl_sess_dir, &key, &a.history) {
                            eprintln!("session save failed: {e}");
                        }
                    }
                }
                reply
            };
            sl.serve(&mut handler);
        });
    }

    if matrix::Matrix::wanted(&cfg) {
        match matrix::Matrix::new(&cfg) {
            Ok(mx) => {
                let mx_cfg = cfg.clone();
                let mx_sessions = cfg.sessions && cfg.privacy != "ghost";
                let mx_sess_dir = config::home().join("sessions");
                std::thread::spawn(move || {
                    let mut agents: HashMap<String, Agent> = HashMap::new();
                    let mut handler = move |sender: &str, text: &str| -> String {
                        mark_activity();
                        let key = format!("mx-{}", sessions::sanitize(sender));
                        if matches!(text.trim(), "/new" | "/reset") {
                            sessions::reset(&mx_sess_dir, &key);
                            agents.remove(&key);
                            return "session reset".to_string();
                        }
                        if !agents.contains_key(&key) {
                            match build_agent(&mx_cfg, false) {
                                Ok(mut a) => {
                                    if mx_sessions {
                                        a.history = sessions::load(&mx_sess_dir, &key);
                                    }
                                    agents.insert(key.clone(), a);
                                }
                                Err(e) => return format!("error: {e}"),
                            }
                        }
                        if let Some(reply) = channel_command(agents.get_mut(&key), &mx_cfg, text) {
                            return reply.flatten();
                        }
                        let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                            return String::new();
                        };
                        if mx_sessions {
                            if let Some(a) = agents.get(&key) {
                                if let Err(e) = sessions::save(&mx_sess_dir, &key, &a.history) {
                                    eprintln!("session save failed: {e}");
                                }
                            }
                        }
                        reply
                    };
                    mx.serve(&mut handler);
                });
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }

    if mattermost::Mattermost::wanted(&cfg) {
        match mattermost::Mattermost::new(&cfg) {
            Ok(mm) => {
                let mm_cfg = cfg.clone();
                let mm_sessions = cfg.sessions && cfg.privacy != "ghost";
                let mm_sess_dir = config::home().join("sessions");
                std::thread::spawn(move || {
                    let mut agents: HashMap<String, Agent> = HashMap::new();
                    let mut handler = move |sender: &str, text: &str| -> String {
                        mark_activity();
                        let key = format!("mm-{}", sessions::sanitize(sender));
                        if matches!(text.trim(), "/new" | "/reset") {
                            sessions::reset(&mm_sess_dir, &key);
                            agents.remove(&key);
                            return "session reset".to_string();
                        }
                        if !agents.contains_key(&key) {
                            match build_agent(&mm_cfg, false) {
                                Ok(mut a) => {
                                    if mm_sessions {
                                        a.history = sessions::load(&mm_sess_dir, &key);
                                    }
                                    agents.insert(key.clone(), a);
                                }
                                Err(e) => return format!("error: {e}"),
                            }
                        }
                        if let Some(reply) = channel_command(agents.get_mut(&key), &mm_cfg, text) {
                            return reply.flatten();
                        }
                        let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                            return String::new();
                        };
                        if mm_sessions {
                            if let Some(a) = agents.get(&key) {
                                if let Err(e) = sessions::save(&mm_sess_dir, &key, &a.history) {
                                    eprintln!("session save failed: {e}");
                                }
                            }
                        }
                        reply
                    };
                    mm.serve(&mut handler);
                });
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }

    if irc::Irc::wanted(&cfg) {
        match irc::Irc::new(&cfg) {
            Ok(ic) => {
                let ic_cfg = cfg.clone();
                let ic_sessions = cfg.sessions && cfg.privacy != "ghost";
                let ic_sess_dir = config::home().join("sessions");
                std::thread::spawn(move || {
                    let mut agents: HashMap<String, Agent> = HashMap::new();
                    let mut handler = move |sender: &str, text: &str| -> String {
                        mark_activity();
                        let key = format!("irc-{sender}");
                        if matches!(text.trim(), "/new" | "/reset") {
                            sessions::reset(&ic_sess_dir, &key);
                            agents.remove(&key);
                            return "session reset".to_string();
                        }
                        if !agents.contains_key(&key) {
                            match build_agent(&ic_cfg, false) {
                                Ok(mut a) => {
                                    if ic_sessions {
                                        a.history = sessions::load(&ic_sess_dir, &key);
                                    }
                                    agents.insert(key.clone(), a);
                                }
                                Err(e) => return format!("error: {e}"),
                            }
                        }
                        if let Some(reply) = channel_command(agents.get_mut(&key), &ic_cfg, text) {
                            return reply.flatten();
                        }
                        let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                            return String::new();
                        };
                        if ic_sessions {
                            if let Some(a) = agents.get(&key) {
                                if let Err(e) = sessions::save(&ic_sess_dir, &key, &a.history) {
                                    eprintln!("session save failed: {e}");
                                }
                            }
                        }
                        reply
                    };
                    ic.serve(&mut handler);
                });
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }

    if signal::Signal::wanted(&cfg) {
        let sg = match signal::Signal::new(&cfg) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let sg_cfg = cfg.clone();
        let sg_sessions = cfg.sessions && cfg.privacy != "ghost";
        let sg_sess_dir = config::home().join("sessions");
        std::thread::spawn(move || {
            let mut agents: HashMap<String, Agent> = HashMap::new();
            let mut handler = move |sender: &str, text: &str| -> String {
                mark_activity();
                let key = format!("sg-{sender}");
                if matches!(text.trim(), "/new" | "/reset") {
                    sessions::reset(&sg_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if !agents.contains_key(&key) {
                    match build_agent(&sg_cfg, false) {
                        Ok(mut a) => {
                            if sg_sessions {
                                a.history = sessions::load(&sg_sess_dir, &key);
                            }
                            agents.insert(key.clone(), a);
                        }
                        Err(e) => return format!("error: {e}"),
                    }
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &sg_cfg, text) {
                    return reply.flatten();
                }
                let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                    return String::new();
                };
                if sg_sessions {
                    if let Some(a) = agents.get(&key) {
                        if let Err(e) = sessions::save(&sg_sess_dir, &key, &a.history) {
                            eprintln!("session save failed: {e}");
                        }
                    }
                }
                reply
            };
            sg.serve(&mut handler);
        });
    }

    if imessage::IMessage::wanted(&cfg) {
        let im = match imessage::IMessage::new(&cfg) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let im_cfg = cfg.clone();
        let im_sessions = cfg.sessions && cfg.privacy != "ghost";
        let im_sess_dir = config::home().join("sessions");
        std::thread::spawn(move || {
            let mut agents: HashMap<String, Agent> = HashMap::new();
            let mut handler = move |chat: &str, text: &str| -> String {
                mark_activity();
                let key = format!("im-{chat}");
                if matches!(text.trim(), "/new" | "/reset") {
                    sessions::reset(&im_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if !agents.contains_key(&key) {
                    match build_agent(&im_cfg, false) {
                        Ok(mut a) => {
                            if im_sessions {
                                a.history = sessions::load(&im_sess_dir, &key);
                            }
                            agents.insert(key.clone(), a);
                        }
                        Err(e) => return format!("error: {e}"),
                    }
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &im_cfg, text) {
                    return reply.flatten();
                }
                let Some(reply) = agents.get_mut(&key).map(|a| a.run(text)) else {
                    return String::new();
                };
                if im_sessions {
                    if let Some(a) = agents.get(&key) {
                        if let Err(e) = sessions::save(&im_sess_dir, &key, &a.history) {
                            eprintln!("session save failed: {e}");
                        }
                    }
                }
                reply
            };
            im.serve(&mut handler);
        });
    }

    let job_cfg = cfg.clone();
    let tg_deliver = tg.clone();
    let _sched = scheduler::Scheduler::start(
        &cfg.jobs,
        move |job| {
            let mut c = job_cfg.clone();
            c.privacy = "ghost".to_string();

            c.approvals = false;
            if !scheduler::precheck_passes(job, &c.workspace) {
                return None;
            }
            if let Some(out) = scheduler::script_result(job, &c.workspace) {
                return Some(out);
            }
            if !job.can_act {
                c.deny_tools = heartbeat::observe_only_denies(&c.deny_tools);
            }
            if !job.model.is_empty() {
                config::retarget(&mut c, &job.model);
                if let Err(e) = c.validate() {
                    return Some(format!("job failed: model override: {e}"));
                }
            }
            Some(match build_agent(&c, false) {
                Ok(mut a) => a.run(&job.prompt),
                Err(e) => format!("job failed: {e}"),
            })
        },
        move |job, result| {
            if !job.webhook.is_empty() {
                if let Err(e) = scheduler::post_webhook(&job.webhook, &job.name, result) {
                    eprintln!("job {} webhook failed: {e}", job.name);
                }
                return;
            }
            let Some(tg) = &tg_deliver else {
                println!("[{}]\n{result}", job.name);
                return;
            };
            let targets = if job.chat_ids.is_empty() {
                tg.allowed.entries()
            } else {
                job.chat_ids.clone()
            };
            for chat_id in targets {
                let _ = tg.send(&chat_id, &format!("[{}]\n{result}", job.name));
            }
        },
    );

    let hb_cfg = cfg.clone();
    let hb_targets = if !cfg.heartbeat_chat_ids.is_empty() {
        cfg.heartbeat_chat_ids.clone()
    } else if let Some(t) = &tg {
        t.allowed.entries()
    } else {
        Vec::new()
    };
    let tg_hb = tg.clone();
    let hb_window = heartbeat::busy_window_secs(cfg.heartbeat_minutes);
    let _heartbeat = heartbeat::Heartbeat::start(
        cfg.heartbeat_minutes,
        move || {
            let last = ACTIVITY.load(Ordering::Relaxed);
            last != 0 && now_epoch().saturating_sub(last) < hb_window
        },
        move || {
            let mut c = hb_cfg.clone();
            c.privacy = "ghost".to_string();

            c.approvals = false;
            if c.heartbeat_prompt == config::HEARTBEAT_PROMPT
                && !heartbeat::file_warrants_a_beat(&c.workspace.join("HEARTBEAT.md"))
            {
                return "HEARTBEAT_OK".to_string();
            }
            if !c.heartbeat_can_act {
                c.deny_tools = heartbeat::observe_only_denies(&c.deny_tools);
            }
            let prompt = format!(
                "Current time: {}.\n{}",
                scheduler::now_local().iso(),
                c.heartbeat_prompt
            );
            match build_agent(&c, false) {
                Ok(mut a) => a.run(&prompt),
                Err(e) => format!("heartbeat failed: {e}"),
            }
        },
        move |result| {
            let Some(tg) = &tg_hb else {
                println!("[heartbeat]\n{result}");
                return;
            };
            for chat_id in &hb_targets {
                let _ = tg.send(chat_id, &format!("[heartbeat]\n{result}"));
            }
        },
    );
    if cfg.heartbeat_minutes > 0 {
        println!("phoenix: heartbeat every {} min", cfg.heartbeat_minutes);
    }

    if cfg.update_check_hours > 0 {
        let every = u64::from(cfg.update_check_hours) * 3600;
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(every));
            match update::run(true) {
                Ok(_) => {}
                Err(e) => eprintln!("update check failed: {e}"),
            }
        });
        println!(
            "phoenix: update check every {}h (check only, never applies)",
            cfg.update_check_hours
        );
    }

    mark_activity();
    if cfg.dream_minutes > 0 {
        let dream_cfg = cfg.clone();
        std::thread::spawn(move || {
            let mut dreamed_for: u64 = 0;
            loop {
                std::thread::sleep(Duration::from_secs(60));
                let last = ACTIVITY.load(Ordering::Relaxed);
                if last == 0 || last == dreamed_for {
                    continue;
                }
                let idle = now_epoch().saturating_sub(last);
                if idle < dream_cfg.dream_minutes as u64 * 60 {
                    continue;
                }
                dreamed_for = last;
                let mut c = dream_cfg.clone();
                c.privacy = "ghost".to_string();
                c.approvals = false;
                let prompt = if c.dream_prompt.is_empty() {
                    DREAM_PROMPT.to_string()
                } else {
                    c.dream_prompt.clone()
                };
                let note = match build_agent(&c, false) {
                    Ok(mut a) => a.run(&prompt),
                    Err(e) => format!("dream failed: {e}"),
                };
                let t = scheduler::now_local();
                let entry = format!(
                    "\n## {:04}-{:02}-{:02} {:02}:{:02}\n\n{}\n",
                    t.year,
                    t.mon,
                    t.mday,
                    t.hour,
                    t.min,
                    note.trim()
                );
                let path = config::home().join("dreams.md");
                let appended = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| std::io::Write::write_all(&mut f, entry.as_bytes()));
                if let Err(e) = appended {
                    eprintln!("dream journal write failed: {e}");
                } else {
                    println!("phoenix: dreamed while idle (dreams.md)");
                }
            }
        });
        println!("phoenix: dreaming after {} idle min", cfg.dream_minutes);
    }

    let use_sessions = cfg.sessions && cfg.privacy != "ghost";
    let sess_dir = config::home().join("sessions");
    let mut agents: HashMap<String, Agent> = HashMap::new();
    let serve_cfg = cfg.clone();
    let audio_cfg = cfg.clone();
    let transcriber = |bytes: &[u8]| audio::transcribe(&audio_cfg, bytes, "voice.ogg");

    let Some(tg) = tg else {
        while !daemon::stopping() {
            std::thread::sleep(Duration::from_millis(250));
        }
        println!("phoenix: stop requested, shutting down cleanly");
        return 0;
    };
    let tg_out = tg.clone();
    let progress: std::rc::Rc<std::cell::RefCell<Option<(String, i64)>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let progress_at = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
    let tg_prog = tg.clone();
    let result = tg.serve(
        &mut |chat_id, thread_id, text, media| {
            mark_activity();
            let skey = match thread_id {
                Some(t) => format!("{chat_id}#t{t}"),
                None => chat_id.to_string(),
            };
            if matches!(text.trim(), "/new" | "/reset") {
                sessions::reset(&sess_dir, &skey);
                agents.remove(skey.as_str());
                return "session reset".to_string();
            }
            if !agents.contains_key(skey.as_str()) {
                match build_agent(&serve_cfg, false) {
                    Ok(mut a) => {
                        if use_sessions {
                            a.history = sessions::load(&sess_dir, &skey);
                        }
                        a.toolbox.set_owner(chat_id);
                        let chat = chat_id.to_string();
                        let pthread = thread_id;
                        let slot = progress.clone();
                        let at = progress_at.clone();
                        let tgp = tg_prog.clone();
                        a.toolbox.set_event_hook(Box::new(move |name, args| {
                            println!("→ {name} {args}");
                            let line = telegram::progress_line(name, args);
                            let mut slot = slot.borrow_mut();
                            match slot.as_ref() {
                                None => {
                                    if let Some(id) = tgp.progress_start(&chat, pthread, &line) {
                                        *slot = Some((chat.clone(), id));
                                        at.set(std::time::Instant::now());
                                    }
                                }
                                Some((c, id)) => {
                                    if at.get().elapsed().as_millis() >= 1500 {
                                        tgp.progress_edit(c, *id, &line);
                                        at.set(std::time::Instant::now());
                                    }
                                }
                            }
                        }));
                        agents.insert(skey.clone(), a);
                    }
                    Err(e) => return format!("error: {e}"),
                }
            }
            if let Some(reply) = channel_command(agents.get_mut(skey.as_str()), &serve_cfg, text) {
                if reply.buttons.is_empty() {
                    return reply.text;
                }
                let _ = tg_out.send_with_buttons(chat_id, thread_id, &reply.text, &reply.buttons);
                return String::new();
            }
            let Some(agent) = agents.get_mut(skey.as_str()) else {
                return String::new();
            };
            let before_max = agent
                .toolbox
                .pending_entries()
                .last()
                .map(|(id, _)| *id)
                .unwrap_or(0);
            let reply = agent.run_with_media(text, media);
            if let Some((c, id)) = progress.borrow_mut().take() {
                tg_prog.progress_clear(&c, id);
            }

            let _ = tg_out.send_in(chat_id, thread_id, &reply);
            for (task_id, note) in tasks_due(chat_id) {
                if tg_out.send_in(chat_id, thread_id, &note).is_ok() {
                    tasks_delivered(task_id);
                }
            }
            for (id, command) in agent.toolbox.pending_entries() {
                if id <= before_max {
                    continue;
                }
                let preview: String = command.chars().take(1000).collect();
                let _ = tg_out.send_with_buttons(
                    chat_id,
                    thread_id,
                    &format!("run command #{id}?\n{preview}"),
                    &[vec![
                        (format!("\u{2705} approve #{id}"), format!("/approve {id}")),
                        (format!("\u{274c} deny #{id}"), format!("/deny {id}")),
                    ]],
                );
            }
            if use_sessions {
                if let Some(a) = agents.get(skey.as_str()) {
                    if let Err(e) = sessions::save(&sess_dir, &skey, &a.history) {
                        eprintln!("session save failed: {e}");
                    }
                }
            }
            String::new()
        },
        if cfg.audio_transcribe {
            Some(&transcriber)
        } else {
            None
        },
    );
    if daemon::stopping() {
        println!("phoenix: stop requested, shutting down cleanly");
        return 0;
    }
    if let Err(e) = result {
        eprintln!("error: {e}");
        return 2;
    }
    0
}

fn cmd_security(cfg: &Config, json_out: bool) -> u8 {
    let cfg_path = config::config_path();
    let raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let memory_path = config::home().join("memory.md");
    let all = doctor::check(cfg, &cfg_path, &raw, &memory_path);
    let keep = |m: &str| {
        let m = m.to_ascii_lowercase();
        [
            "permission",
            "api key",
            "token",
            "secret",
            "approval",
            "allowlist",
            "allowed",
            "deny",
            "outside",
            "shell",
            "bind",
            "password",
            "crawler",
            "refuses",
            "world",
        ]
        .iter()
        .any(|k| m.contains(k))
    };
    let found: Vec<&doctor::Finding> = all.iter().filter(|f| keep(&f.msg)).collect();
    if json_out {
        let items: Vec<Value> = found
            .iter()
            .map(|f| json!({"level": f.level, "msg": f.msg}))
            .collect();
        let doc = json!({"v": 1, "ok": !found.iter().any(|f| f.level == "fail"),
            "findings": items});
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        println!("{} security findings", found.len());
        for f in &found {
            println!("{:<6}{}", f.level, f.msg);
        }
        if !cfg.audit_log {
            println!("warn  audit log is off; set security.audit_log = true to record tool use");
        }
    }
    u8::from(found.iter().any(|f| f.level == "fail"))
}

fn cmd_health(cfg: &Config, json_out: bool) -> u8 {
    let cfg_path = config::config_path();
    let raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let memory_path = config::home().join("memory.md");
    let findings = doctor::check(cfg, &cfg_path, &raw, &memory_path);
    let fails = findings.iter().filter(|f| f.level == "fail").count();
    let warns = findings.iter().filter(|f| f.level == "warn").count();
    let has_key = !cfg.api_key.is_empty() || cfg.provider == "ollama";
    let service = if service::systemd_available() {
        service::state()
    } else {
        "n/a".to_string()
    };
    let ok = fails == 0 && has_key;
    if json_out {
        let doc = json!({"v": 1, "ok": ok, "checks": findings.len(),
            "failures": fails, "warnings": warns,
            "provider": cfg.provider, "model": cfg.model,
            "key": has_key, "service": service});
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        println!(
            "{} {}/{} {} checks, {fails} failing, {warns} warning, key {}, service {service}",
            if ok { "ok" } else { "unhealthy" },
            cfg.provider,
            cfg.model,
            findings.len(),
            if has_key { "present" } else { "MISSING" },
        );
    }
    u8::from(!ok)
}

fn cmd_board(words: &[String]) -> u8 {
    let path = config::home().join("board.json");
    let sub = words.first().map(String::as_str).unwrap_or("list");
    match sub {
        "list" => match board::list(&path, words.get(1).map(String::as_str)) {
            Ok(t) => {
                print!("{t}");
                0
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        "add" => {
            let title = words[1..].join(" ");
            if title.is_empty() {
                eprintln!("usage: phoenix board add TITLE");
                return 2;
            }
            match board::add(&path, &title, "", "normal") {
                Ok(id) => {
                    println!("card {id} added");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        "update" => {
            let (Some(id), Some(status)) = (words.get(1), words.get(2)) else {
                eprintln!("usage: phoenix board update ID STATUS");
                return 2;
            };
            let Ok(id) = id.parse::<u64>() else {
                eprintln!("card id must be a number");
                return 2;
            };
            match board::update(&path, id, Some(status), None, None, None) {
                Ok(t) => {
                    println!("{t}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        other => {
            eprintln!(
                "unknown board subcommand '{other}': list [STATUS] | add TITLE | update ID STATUS"
            );
            2
        }
    }
}

fn cmd_proxy(cfg: &Config, words: &[String]) -> u8 {
    let capture = proxy::capture_path();
    match words.first().map(String::as_str).unwrap_or("log") {
        "run" => {
            let port: u16 = words.get(1).and_then(|v| v.parse().ok()).unwrap_or(8899);
            let upstream = words
                .get(2)
                .cloned()
                .unwrap_or_else(|| providers::base_url_of(cfg));
            match proxy::serve(port, &upstream, &capture) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        "log" | "list" => {
            let limit: usize = words
                .get(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(20)
                .clamp(1, 1000);
            print!("{}", proxy::log_text(&capture, limit));
            0
        }
        "show" => {
            let Some(index) = words.get(1).and_then(|v| v.parse::<usize>().ok()) else {
                eprintln!("usage: phoenix proxy show N");
                return 2;
            };
            print!("{}", proxy::show_text(&capture, index));
            0
        }
        "clear" => {
            let _ = std::fs::remove_file(&capture);
            println!("captures cleared");
            0
        }
        other => {
            eprintln!(
                "unknown proxy subcommand '{other}': run [PORT] [UPSTREAM] | log [N] | show N | clear"
            );
            2
        }
    }
}

fn cmd_commitments(words: &[String]) -> u8 {
    let path = commitments::store_path();
    let now = commitments::now_ms();
    match words.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            let items = commitments::load(&path);
            let filter = words.get(1).map(String::as_str);
            if let Some(s) = filter {
                if !commitments::known_status(s) {
                    eprintln!(
                        "unknown status '{s}': expected one of {:?}",
                        commitments::STATUSES
                    );
                    return 2;
                }
            }
            print!("{}", commitments::list_text(&items, filter, now));
            0
        }
        "due" => {
            let items = commitments::load(&path);
            let due = commitments::due_now(&items, now);
            if due.is_empty() {
                println!("nothing due");
                return 0;
            }
            println!("{} due", due.len());
            for c in due {
                println!("  #{:<4}{}", c.id, crate::security::one_line(&c.text, 70));
            }
            0
        }
        "add" => {
            let Some(when) = words.get(1) else {
                eprintln!("usage: phoenix commitments add WHEN TEXT   (WHEN: 30m, 2h, 3d)");
                return 2;
            };
            let due = match commitments::parse_due(when) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let text = words[2..].join(" ");
            match commitments::add(&path, &text, due, "cli") {
                Ok(id) => {
                    println!("commitment #{id} recorded");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        "done" | "dismiss" => {
            let sub = words.first().map(String::as_str).unwrap_or("done");
            let status = if sub == "done" { "done" } else { "dismissed" };
            let Some(id) = words.get(1).and_then(|v| v.parse::<u64>().ok()) else {
                eprintln!("usage: phoenix commitments {sub} ID");
                return 2;
            };
            match commitments::set_status(&path, id, status) {
                Ok(()) => {
                    println!("#{id} marked {status}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        other => {
            eprintln!(
                "unknown commitments subcommand '{other}': list [STATUS] | due | add WHEN TEXT | done ID | dismiss ID"
            );
            2
        }
    }
}

fn cmd_agents(words: &[String]) -> u8 {
    let dir = config::home().join("agents");
    match words.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            let entries = sessions::list(&dir);
            if entries.is_empty() {
                println!("no named agents yet; the agent tool creates them with agent_spawn");
                return 0;
            }
            println!("{} agents", entries.len());
            for (name, count) in entries {
                println!("  {name:<20}{count} message(s)");
            }
            0
        }
        "show" | "history" => {
            let Some(name) = words.get(1) else {
                eprintln!("usage: phoenix agents show NAME");
                return 2;
            };
            if !agent::valid_agent_name(name) {
                eprintln!("bad agent name '{name}'");
                return 2;
            }
            let history = sessions::load(&dir, name);
            if history.is_empty() {
                eprintln!("no agent named '{name}'");
                return 2;
            }
            for m in &history {
                match m {
                    providers::Msg::User { content, .. } => println!("user: {content}"),
                    providers::Msg::Assistant { content, .. } if !content.is_empty() => {
                        println!("{name}: {content}")
                    }
                    providers::Msg::Assistant { .. } => {}
                    providers::Msg::Tool { content, .. } => {
                        let clip: String = content.chars().take(200).collect();
                        println!("  tool: {clip}");
                    }
                }
            }
            0
        }
        "remove" | "rm" => {
            let Some(name) = words.get(1) else {
                eprintln!("usage: phoenix agents remove NAME");
                return 2;
            };
            if !agent::valid_agent_name(name) {
                eprintln!("bad agent name '{name}'");
                return 2;
            }
            if sessions::load(&dir, name).is_empty() {
                eprintln!("no agent named '{name}'");
                return 2;
            }
            sessions::reset(&dir, name);
            println!("agent '{name}' removed");
            0
        }
        other => {
            eprintln!("unknown agents subcommand '{other}': list | show NAME | remove NAME");
            2
        }
    }
}

fn cmd_hooks(cfg: &Config, words: &[String]) -> u8 {
    match words.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            print!("{}", hooks::summary(&cfg.hooks));
            u8::from(!hooks::problems(&cfg.hooks).is_empty())
        }
        "test" => {
            let event = words
                .get(1)
                .cloned()
                .unwrap_or_else(|| "turn_end".to_string());
            if !hooks::known_event(&event) {
                eprintln!(
                    "unknown event '{event}': expected one of {:?}",
                    hooks::EVENTS
                );
                return 2;
            }
            let listening = cfg
                .hooks
                .iter()
                .filter(|h| h.enabled && h.event == event)
                .count();
            if listening == 0 {
                println!("no enabled hooks listen for '{event}'");
                return 0;
            }
            let problems = hooks::fire(&cfg.hooks, &event, &json!({"test": true}));
            println!("fired '{event}' at {listening} hook(s)");
            for p in &problems {
                eprintln!("  {p}");
            }
            u8::from(!problems.is_empty())
        }
        other => {
            eprintln!("unknown hooks subcommand '{other}': list | test [EVENT]");
            2
        }
    }
}

fn cmd_mcp(cfg: &Config, words: &[String]) -> u8 {
    match words.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            print!("{}", mcp::summary(&cfg.mcp_servers));
            0
        }
        "probe" => {
            if cfg.mcp_servers.is_empty() {
                print!("{}", mcp::summary(&cfg.mcp_servers));
                return 0;
            }
            let (live, tools, problems) = mcp::connect_all(&cfg.mcp_servers);
            println!("{} servers answered, {} tools", live.len(), tools.len());
            for (name, server) in &live {
                let info = if server.server_info.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", server.server_info)
                };
                println!("  {name}{info}");
                for t in tools.iter().filter(|t| t.server == *name) {
                    println!("      {}", t.exposed_name());
                }
            }
            for p in &problems {
                eprintln!("  {p}");
            }
            u8::from(!problems.is_empty())
        }
        "call" => {
            let Some(tool_name) = words.get(1) else {
                eprintln!("usage: phoenix mcp call TOOL [JSON_ARGS]");
                return 2;
            };
            let args_raw = words[2..].join(" ");
            let args: serde_json::Value = if args_raw.trim().is_empty() {
                json!({})
            } else {
                match serde_json::from_str(&args_raw) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("arguments must be a JSON object: {e}");
                        return 2;
                    }
                }
            };
            if cfg.mcp_servers.is_empty() {
                eprintln!("no mcp servers configured");
                return 2;
            }
            let (live, tools, problems) = mcp::connect_all(&cfg.mcp_servers);
            for p in &problems {
                eprintln!("{p}");
            }
            let Some(tool) = tools
                .iter()
                .find(|t| t.exposed_name() == *tool_name || t.name == *tool_name)
            else {
                eprintln!("no tool named {tool_name}; run `phoenix mcp probe` to list them");
                return 2;
            };
            let Some((_, mut server)) = live.into_iter().find(|(n, _)| *n == tool.server) else {
                eprintln!(
                    "the server {} for {tool_name} is not connected",
                    tool.server
                );
                return 2;
            };
            match server.call_tool(&tool.name, &args) {
                Ok((text, is_err)) => {
                    println!("{text}");
                    u8::from(is_err)
                }
                Err(e) => {
                    eprintln!("call failed: {e}");
                    2
                }
            }
        }
        other => {
            eprintln!("unknown mcp subcommand '{other}': list | probe | call TOOL [JSON]");
            2
        }
    }
}

fn cmd_canvas(words: &[String]) -> u8 {
    let path = canvas::state_path();
    match words.first().map(String::as_str).unwrap_or("show") {
        "show" => {
            print!("{}", canvas::render(&path));
            0
        }
        "hide" | "clear" => {
            canvas::hide(&path);
            println!("canvas cleared");
            0
        }
        "present" => {
            let html = words[1..].join(" ");
            match canvas::present(&path, &html) {
                Ok(()) => {
                    println!("canvas updated ({})", path.display());
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        other => {
            eprintln!("unknown canvas subcommand '{other}': show | present HTML | hide");
            2
        }
    }
}

fn cmd_media(cfg: &Config, words: &[String]) -> u8 {
    let prompt = words.join(" ");
    if prompt.is_empty() {
        eprintln!("usage: phoenix media PROMPT   (writes a png in the workspace)");
        return 2;
    }
    match media::generate_image(cfg, &prompt) {
        Ok(bytes) => {
            let out = cfg.workspace.join(format!(
                "phoenix-image-{}.png",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ));
            if let Some(p) = out.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            match std::fs::write(&out, &bytes) {
                Ok(()) => {
                    println!(
                        "{}  {}",
                        out.display(),
                        doctor::format_bytes(bytes.len() as u64)
                    );
                    0
                }
                Err(e) => {
                    eprintln!("cannot write {}: {e}", out.display());
                    2
                }
            }
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn cmd_oauth(words: &[String]) -> u8 {
    match words.first().map(String::as_str).unwrap_or("show") {
        "show" | "status" => match oauth::load() {
            Some(t) => {
                let now = oauth::now_ms();
                let left = t.expires_at_ms.saturating_sub(now) / 1000;
                println!("stored at {}", oauth::store_path().display());
                println!(
                    "  access token   {}",
                    if t.access.is_empty() {
                        "none"
                    } else {
                        "present"
                    }
                );
                println!(
                    "  refresh token  {}",
                    if t.refresh.is_empty() {
                        "none"
                    } else {
                        "present"
                    }
                );
                println!(
                    "  expires        {}",
                    if t.expires_at_ms == 0 {
                        "unknown".to_string()
                    } else if left == 0 {
                        "expired".to_string()
                    } else {
                        format!("in {left}s")
                    }
                );
                0
            }
            None => {
                println!("no OAuth tokens at {}", oauth::store_path().display());
                0
            }
        },
        "refresh" => match oauth::fresh_access() {
            Some(_) => {
                println!("access token is fresh");
                0
            }
            None => {
                eprintln!("could not refresh; no usable refresh token");
                2
            }
        },
        other => {
            eprintln!("unknown oauth subcommand '{other}': show | refresh");
            2
        }
    }
}

fn cmd_transcribe(cfg: &Config, words: &[String]) -> u8 {
    let Some(file) = words.first() else {
        eprintln!("usage: phoenix transcribe FILE");
        return 2;
    };
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {file}: {e}");
            return 2;
        }
    };
    let name = std::path::Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio.wav".to_string());
    match audio::transcribe(cfg, &bytes, &name) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn cmd_worktrees(words: &[String]) -> u8 {
    let run = |args: &[&str]| -> u8 {
        match std::process::Command::new("git").args(args).status() {
            Ok(s) if s.success() => 0,
            Ok(s) => s.code().unwrap_or(2).clamp(0, 255) as u8,
            Err(e) => {
                eprintln!("git is required for worktrees: {e}");
                2
            }
        }
    };
    match words.first().map(String::as_str).unwrap_or("list") {
        "list" => run(&["worktree", "list"]),
        "add" => {
            let (Some(path), Some(branch)) = (words.get(1), words.get(2)) else {
                eprintln!("usage: phoenix worktrees add PATH BRANCH");
                return 2;
            };
            run(&["worktree", "add", path, branch])
        }
        "remove" => {
            let Some(path) = words.get(1) else {
                eprintln!("usage: phoenix worktrees remove PATH");
                return 2;
            };
            run(&["worktree", "remove", path])
        }
        "prune" => run(&["worktree", "prune"]),
        other => {
            eprintln!(
                "unknown worktrees subcommand '{other}': list | add PATH BRANCH | remove PATH | prune"
            );
            2
        }
    }
}

fn cmd_config(cfg: &Config, words: &[String]) -> u8 {
    let path = config::config_path();
    let sub = words.first().map(String::as_str).unwrap_or("show");
    match sub {
        "path" => {
            println!("{}", path.display());
            0
        }
        "show" | "get" => {
            let raw = match std::fs::read_to_string(&path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("cannot read {}: {e}", path.display());
                    return 2;
                }
            };
            match words.get(1) {
                None => {
                    print!("{}", crate::security::redact_config(&raw));
                    0
                }
                Some(key) => {
                    let hit = raw
                        .lines()
                        .find(|l| l.trim_start().starts_with(key.as_str()));
                    match hit {
                        Some(l) => {
                            println!("{}", crate::security::redact_config(l.trim()).trim_end());
                            0
                        }
                        None => {
                            eprintln!("no key '{key}' in {}", path.display());
                            2
                        }
                    }
                }
            }
        }
        "validate" | "check" => match cfg.validate() {
            Ok(()) => {
                let raw = std::fs::read_to_string(&path).unwrap_or_default();
                let unknown = config::unknown_keys(&raw);
                if unknown.is_empty() {
                    println!("{} is valid", path.display());
                    0
                } else {
                    println!(
                        "{} parses, but {} key(s) sit outside the schema:",
                        path.display(),
                        unknown.len()
                    );
                    for k in &unknown {
                        match config::misplaced_hint(k) {
                            Some(h) => println!("  {k}  ({h})"),
                            None => println!("  {k}"),
                        }
                    }
                    1
                }
            }
            Err(e) => {
                eprintln!("invalid: {e}");
                2
            }
        },
        other => {
            eprintln!("unknown config subcommand '{other}': show [KEY] | path | validate");
            2
        }
    }
}

fn cmd_memory(cfg: &Config, words: &[String]) -> u8 {
    let _ = cfg;
    let mem = memory::Memory::new("recall");
    let sub = words.first().map(String::as_str).unwrap_or("show");
    match sub {
        "show" | "list" => {
            let text = mem.recall("");
            println!("{text}");
            0
        }
        "search" => {
            let q = words[1..].join(" ");
            if q.is_empty() {
                eprintln!("usage: phoenix memory search QUERY");
                return 2;
            }
            println!("{}", mem.recall(&q));
            0
        }
        "add" => {
            let note = words[1..].join(" ");
            if note.is_empty() {
                eprintln!("usage: phoenix memory add NOTE");
                return 2;
            }
            println!("{}", mem.remember(&note));
            0
        }
        "wipe" => {
            println!("{}", mem.wipe());
            0
        }
        other => {
            eprintln!("unknown memory subcommand '{other}': show | search QUERY | add NOTE | wipe");
            2
        }
    }
}

fn cmd_audit(words: &[String], json_out: bool) -> u8 {
    let path = config::home().join("audit.jsonl");
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => {
            println!(
                "no audit log at {}; set security.audit_log = true to start one",
                path.display()
            );
            return 0;
        }
    };
    let limit: usize = words
        .first()
        .and_then(|w| w.parse().ok())
        .unwrap_or(20)
        .clamp(1, 1000);
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(limit);
    if json_out {
        let items: Vec<Value> = lines[start..]
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let doc = json!({"v": 1, "ok": true, "total": lines.len(), "events": items});
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        println!("{} events, showing {}", lines.len(), lines.len() - start);
        for l in &lines[start..] {
            println!("{l}");
        }
    }
    0
}

fn cmd_backup(words: &[String]) -> u8 {
    let nest = config::home();
    if !nest.exists() {
        eprintln!("no nest at {}", nest.display());
        return 2;
    }
    let default = format!(
        "phoenix-backup-{}.tar.gz",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let out = words.first().cloned().unwrap_or(default);
    let parent = nest.parent().unwrap_or(std::path::Path::new("."));
    let name = match nest.file_name() {
        Some(n) => n,
        None => {
            eprintln!("cannot back up {}", nest.display());
            return 2;
        }
    };
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(parent)
        .arg(name)
        .status();
    match status {
        Ok(s) if s.success() => {
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            println!("{out}  {}", doctor::format_bytes(size));
            println!("restore with: tar -xzf {out} -C {}", parent.display());
            0
        }
        Ok(s) => {
            eprintln!("tar failed with status {s}");
            2
        }
        Err(e) => {
            eprintln!("tar is required for backups: {e}");
            2
        }
    }
}

fn cmd_transcripts(words: &[String]) -> u8 {
    let dir = config::home().join("sessions");
    let Some(id) = words.first() else {
        let all = sessions::list(&dir);
        if all.is_empty() {
            println!("no stored sessions");
        } else {
            println!("pass a session id to print it:");
            for (id, n) in all {
                println!("  {id}  {n} message(s)");
            }
        }
        return 0;
    };
    let history = sessions::load(&dir, id);
    if history.is_empty() {
        eprintln!("no transcript for '{id}'");
        return 2;
    }
    for m in &history {
        match m {
            providers::Msg::User { content, .. } => println!("user: {content}"),
            providers::Msg::Assistant { content, .. } if !content.is_empty() => {
                println!("phoenix: {content}")
            }
            providers::Msg::Assistant { .. } => {}
            providers::Msg::Tool { content, .. } => {
                let clip: String = content.chars().take(200).collect();
                println!("  tool: {clip}");
            }
        }
    }
    0
}

fn cmd_reset(force: bool) -> u8 {
    let path = config::config_path();
    if !force {
        println!(
            "this would delete {} and the sessions beside it.\nRe-run with --force if that is what you want.",
            path.display()
        );
        return 2;
    }
    let nest = config::home();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(nest.join("sessions"));
    println!("config and sessions removed; run `phoenix configure` to start again");
    0
}

fn cmd_uninstall(force: bool) -> u8 {
    if !force {
        println!(
            "this would stop and remove the service, leaving {} in place.\nRe-run with --force to do it.",
            config::home().display()
        );
        return 2;
    }
    let code = service::cmd_service(&["uninstall".to_string()]);
    println!("the nest at {} was left alone", config::home().display());
    code
}

fn cmd_doctor(cfg: &Config, json: bool) -> u8 {
    let cfg_path = config::config_path();
    let raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let memory_path = config::home().join("memory.md");
    let mut findings = doctor::check(cfg, &cfg_path, &raw, &memory_path);
    if service::systemd_available() {
        let s = service::state();
        findings.push(match s.as_str() {
            "active" => doctor::Finding {
                level: "ok",
                msg: "service: active - the beacon is lit".into(),
            },
            "not installed" => doctor::Finding {
                level: "ok",
                msg: "service: not installed - `phoenix service install` runs serve at boot"
                    .into(),
            },
            _ => doctor::Finding {
                level: "warn",
                msg: format!(
                    "service: {s} - check `phoenix service status` or reinstall with `phoenix service install`"
                ),
            },
        });
    }
    if json {
        let items: Vec<Value> = findings
            .iter()
            .map(|x| json!({"level": x.level, "message": x.msg}))
            .collect();
        let doc = json!({
            "v": 1,
            "ok": !doctor::has_failures(&findings),
            "config_path": cfg_path.display().to_string(),
            "findings": items,
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        for x in &findings {
            let tag = match x.level {
                "fail" => "FAIL",
                "warn" => "warn",
                _ => "ok  ",
            };
            println!("{tag}  {}", x.msg);
        }
    }
    if doctor::has_failures(&findings) {
        1
    } else {
        0
    }
}

fn cmd_tasks(words: &[String], json_out: bool) -> u8 {
    let path = tasks::default_path();
    tasks::reap(&path);
    tasks::prune(&path, tasks::DEFAULT_KEEP);
    let sub = words.first().map(String::as_str).unwrap_or("");
    match sub {
        "" => {
            let all = tasks::list(&path, None);
            if json_out {
                let items: Vec<Value> = all
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "kind": t.kind,
                            "title": t.title,
                            "status": t.status.as_str(),
                            "pid": t.pid,
                            "started": t.started,
                            "ended": t.ended,
                            "exit_code": t.exit_code,
                            "error": t.error,
                            "log": t.log.to_string_lossy(),
                        })
                    })
                    .collect();
                let doc = json!({"v": 1, "tasks": items});
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
            } else {
                println!("{}", tasks::render(&all));
            }
            0
        }
        "cancel" => {
            let Some(id) = words.get(1).and_then(|w| w.parse::<u64>().ok()) else {
                eprintln!("usage: phoenix tasks cancel ID");
                return 2;
            };
            match tasks::cancel(&path, id) {
                Ok(m) => {
                    println!("{m}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        other => {
            let Ok(id) = other.parse::<u64>() else {
                eprintln!("usage: phoenix tasks | tasks ID | tasks cancel ID");
                return 2;
            };
            let Some(t) = tasks::get(&path, id) else {
                eprintln!("error: no task #{id}");
                return 2;
            };
            let body = tasks::tail(&t, tasks::RESULT_TAIL);
            if json_out {
                let doc = json!({
                    "v": 1,
                    "id": t.id,
                    "kind": t.kind,
                    "title": t.title,
                    "status": t.status.as_str(),
                    "exit_code": t.exit_code,
                    "error": t.error,
                    "output": body,
                });
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
            } else {
                println!("{}", tasks::line(&t));
                if !body.is_empty() {
                    println!("\n{body}");
                }
            }
            0
        }
    }
}

fn cmd_jobs(cfg: &Config, json_out: bool) -> u8 {
    let mut bad = false;
    let mut items: Vec<Value> = Vec::new();
    for job in &cfg.jobs {
        let err = scheduler::cron_valid(&job.cron).err();
        if err.is_some() {
            bad = true;
        }
        let targets = if job.chat_ids.is_empty() {
            "all allowed chats".to_string()
        } else {
            job.chat_ids.join(", ")
        };
        let next = if err.is_none() {
            scheduler::next_fire(&job.cron, scheduler::now_epoch())
                .ok()
                .flatten()
        } else {
            None
        };
        if json_out {
            items.push(json!({
                "name": job.name,
                "cron": job.cron,
                "valid": err.is_none(),
                "error": err,
                "next_at": next.as_ref().map(scheduler::Tm::stamp),
                "chat_ids": job.chat_ids,
                "prompt": job.prompt,
            }));
        } else {
            let sched = match &err {
                None => job.cron.clone(),
                Some(e) => format!("INVALID ({e})"),
            };
            let when = next
                .as_ref()
                .map(|t| format!("  next {}", t.stamp()))
                .unwrap_or_default();
            let prompt: String = job.prompt.chars().take(60).collect();
            println!("{}  [{sched}]{when}  -> {targets}\n    {prompt}", job.name);
        }
    }
    if json_out {
        let doc = json!({"v": 1, "ok": !bad, "jobs": items});
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else if cfg.jobs.is_empty() {
        println!("no jobs configured");
    }
    if bad {
        1
    } else {
        0
    }
}

fn cmd_sessions(json_out: bool, words: &[String]) -> u8 {
    let dir = config::home().join("sessions");
    match words.first().map(String::as_str) {
        None => {
            let all = sessions::list(&dir);
            if json_out {
                let items: Vec<Value> = all
                    .iter()
                    .map(|(id, n)| json!({"id": id, "messages": n}))
                    .collect();
                let doc = json!({"v": 1, "ok": true, "sessions": items});
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
            } else if all.is_empty() {
                println!("no stored sessions");
            } else {
                for (id, n) in all {
                    println!("{id}  {n} message(s)");
                }
            }
            0
        }
        Some("snapshots") => {
            let all = sessions::snapshots(&dir);
            if json_out {
                let doc = json!({"v": 1, "ok": true, "snapshots": all});
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
            } else if all.is_empty() {
                println!("no snapshots stored");
            } else {
                for s in all {
                    println!("{s}");
                }
            }
            0
        }
        Some("snapshot") => {
            let Some(chat) = words.get(1) else {
                eprintln!("usage: phoenix sessions snapshot CHAT_ID [NAME]");
                return 2;
            };
            let fallback = format!("snap-{}", scheduler::now_epoch());
            let name = words.get(2).unwrap_or(&fallback);
            match sessions::snapshot(&dir, chat, name) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        Some("restore") => {
            let (Some(chat), Some(name)) = (words.get(1), words.get(2)) else {
                eprintln!("usage: phoenix sessions restore CHAT_ID NAME");
                return 2;
            };
            match sessions::restore(&dir, chat, name) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        Some("diff") => {
            let (Some(chat), Some(name)) = (words.get(1), words.get(2)) else {
                eprintln!("usage: phoenix sessions diff CHAT_ID NAME");
                return 2;
            };
            match sessions::diff(&dir, chat, name) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            }
        }
        Some(other) => {
            eprintln!(
                "unknown sessions subcommand: {other} (snapshot | restore | diff | snapshots)"
            );
            2
        }
    }
}

fn real_main() -> u8 {
    crate::agent::install_interrupt();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            if let Some(v) = e.strip_prefix("version:") {
                println!("{v}");
                return 0;
            }
            if let Some(h) = e.strip_prefix("help:") {
                println!("{h}");
                return 0;
            }
            eprintln!("{e}");
            return 2;
        }
    };

    if args.cmd == Cmd::Migrate {
        return cmd_migrate(args.from.as_deref(), args.write, args.force, args.secrets);
    }

    if args.cmd == Cmd::Schema {
        match serde_json::to_string_pretty(&config::json_schema()) {
            Ok(doc) => {
                println!("{doc}");
                return 0;
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }

    if args.cmd == Cmd::Update {
        match update::run(args.check) {
            Ok(msg) => {
                println!("{msg}");
                return 0;
            }
            Err(e) => {
                eprintln!("update failed: {e}");
                return 2;
            }
        }
    }

    let first_flight = !config::config_path().exists()
        && io::stdin().is_terminal()
        && matches!(args.cmd, Cmd::Init | Cmd::Chat | Cmd::Serve);
    if first_flight {
        let gw = config::home_dir().join(".openclaw/openclaw.json");
        let mut src = gw.exists().then_some(gw);
        let mut declined_migration = false;
        if let Some(found) = src.as_deref().and_then(autopilot::inspect_nest) {
            let path = src.clone().unwrap_or_default();
            if !autopilot::ask_migrate(&path, &found, &mut io::stdin().lock(), &mut io::stdout()) {
                src = None;
                declined_migration = true;
            }
        }
        let src = src;
        let force_wizard = declined_migration
            || env::var("PHOENIX_WIZARD")
                .map(|v| v == "1")
                .unwrap_or(false);
        let mut auto_done = false;
        if !force_wizard {
            println!(
                "\n🔥 openphoenix {} - first flight\n",
                env!("CARGO_PKG_VERSION")
            );
            match autopilot::auto_first_run(
                src.as_deref(),
                &config::config_path(),
                &|k| env::var(k).ok(),
                autopilot::ollama_alive(),
                &mut autopilot::probe_live,
                &mut io::stdout(),
            ) {
                Ok(_) => auto_done = true,
                Err(e) => {
                    println!("autopilot grounded ({e}); asking the old way.");
                }
            }
        }
        if auto_done {
            if args.install_daemon {
                match service::install() {
                    Ok(out) => println!("{out}"),
                    Err(e) => eprintln!("service install failed: {e}"),
                }
            }
            if args.cmd == Cmd::Init {
                return 0;
            }
        } else {
            match onboard::first_run(
                src.as_deref(),
                &config::config_path(),
                true,
                &mut io::stdin().lock(),
                &mut io::stdout(),
            ) {
                Ok(_) => {
                    if args.install_daemon {
                        match service::install() {
                            Ok(out) => println!("{out}"),
                            Err(e) => eprintln!("service install failed: {e}"),
                        }
                    }
                    if args.cmd == Cmd::Init {
                        return 0;
                    }
                }
                Err(e) => {
                    eprintln!("setup error: {e}");
                    return 2;
                }
            }
        }
    }

    if args.cmd == Cmd::Init {
        match config::init_config() {
            Ok(p) => {
                println!("config: {}", p.display());
                return 0;
            }
            Err(e) => {
                eprintln!("config error: {e}");
                return 2;
            }
        }
    }

    match &args.cmd {
        Cmd::Configure => return cmd_configure(args.install_daemon),
        Cmd::Status => return cmd_status(args.json),
        Cmd::Dashboard => return cmd_dashboard(),
        Cmd::Commands => {
            print!("{}", commands::list_text(args.json));
            return 0;
        }
        Cmd::Docs => {
            print!("{}", commands::docs_text());
            return 0;
        }
        Cmd::System => {
            print!("{}", commands::system_text());
            return 0;
        }
        Cmd::Completion(words) => {
            let shell = words.first().map(String::as_str).unwrap_or("bash");
            return match commands::completion_script(shell) {
                Ok(s) => {
                    print!("{s}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            };
        }
        _ => {}
    }

    let mut cfg = match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 2;
        }
    };

    if matches!(args.cmd, Cmd::Run(_)) && !args.recall && !args.ghost {
        cfg.privacy = "ghost".to_string();
    }
    if args.ghost {
        cfg.privacy = "ghost".to_string();
    }
    if args.recall {
        cfg.privacy = "recall".to_string();
    }
    if let Some(l) = &args.lean {
        cfg.lean = l.clone();
    }
    if let Some(p) = &args.provider {
        config::switch_provider(&mut cfg, p);
    }
    if let Some(m) = &args.model {
        config::retarget(&mut cfg, m);
    }
    if let Err(e) = cfg.validate() {
        eprintln!("config error: {e}");
        return 2;
    }

    match args.cmd {
        Cmd::Doctor => return cmd_doctor(&cfg, args.json),
        Cmd::Jobs => return cmd_jobs(&cfg, args.json),
        Cmd::Tasks(words) => return cmd_tasks(&words, args.json),
        Cmd::Sessions(words) => return cmd_sessions(args.json, &words),
        Cmd::Secret(words) => return cmd_secret(&words),
        Cmd::Skill(words) => return cmd_skill(&words),
        Cmd::Service(words) => return service::cmd_service(&words),
        Cmd::Channels => {
            print!("{}", commands::channels_text(&cfg));
            return 0;
        }
        Cmd::Directory => {
            print!("{}", commands::directory_text(&cfg));
            return 0;
        }
        Cmd::ExecPolicy => {
            print!("{}", commands::exec_policy_text(&cfg));
            return 0;
        }
        Cmd::Gateway => {
            print!("{}", commands::gateway_text(&cfg));
            return 0;
        }
        Cmd::Webhooks => {
            print!("{}", commands::webhooks_text(&cfg));
            return 0;
        }
        Cmd::Security => return cmd_security(&cfg, args.json),
        Cmd::Health => return cmd_health(&cfg, args.json),
        Cmd::Memory(words) => return cmd_memory(&cfg, &words),
        Cmd::ConfigFile(words) => return cmd_config(&cfg, &words),
        Cmd::Capability => {
            print!("{}", commands::capability_text(&cfg));
            return 0;
        }
        Cmd::Board(words) => return cmd_board(&words),
        Cmd::Canvas(words) => return cmd_canvas(&words),
        Cmd::Mcp(words) => return cmd_mcp(&cfg, &words),
        Cmd::Hooks(words) => return cmd_hooks(&cfg, &words),
        Cmd::Agents(words) => return cmd_agents(&words),
        Cmd::Commitments(words) => return cmd_commitments(&words),
        Cmd::Proxy(words) => return cmd_proxy(&cfg, &words),
        Cmd::Attach(words) => {
            let prompt = words.join(" ");
            let prompt = (!prompt.trim().is_empty()).then_some(prompt);
            return attach::run(&cfg, prompt.as_deref(), attach::DEFAULT_TIMEOUT_SECS);
        }
        Cmd::Media(words) => return cmd_media(&cfg, &words),
        Cmd::Oauth(words) => return cmd_oauth(&words),
        Cmd::Transcribe(words) => return cmd_transcribe(&cfg, &words),
        Cmd::Worktrees(words) => return cmd_worktrees(&words),
        Cmd::Audit(words) => return cmd_audit(&words, args.json),
        Cmd::Backup(words) => return cmd_backup(&words),
        Cmd::Transcripts(words) => return cmd_transcripts(&words),
        Cmd::Reset => return cmd_reset(args.force),
        Cmd::Uninstall => return cmd_uninstall(args.force),
        _ => {}
    }

    if cfg.api_key.is_empty() && cfg.provider != "ollama" {
        let vars = config::provider_key_vars(&cfg.provider);
        let mut checked = vec!["PHOENIX_API_KEY"];
        checked.extend_from_slice(vars);
        eprintln!(
            "no API key for provider \"{}\": add api_key under [provider] in {} \
or set one of {} in the environment",
            cfg.provider,
            config::config_path().display(),
            checked.join(", ")
        );
        if let Some(url) = onboard::key_signup_url(&cfg.provider) {
            eprintln!("Get a key for {}: {url}", cfg.provider);
        }
        if !config::any_provider_key_in_env() {
            eprintln!("\n{}", onboard::no_key_anywhere_help());
        }
        return 2;
    }

    match args.cmd {
        Cmd::Run(prompt) => cmd_run(cfg, &prompt),
        Cmd::Serve => cmd_serve(cfg),
        Cmd::Chat => cmd_chat(cfg),
        Cmd::Models => cmd_models(&cfg, args.test_fallback),
        Cmd::Schema
        | Cmd::Init
        | Cmd::Configure
        | Cmd::Status
        | Cmd::Dashboard
        | Cmd::Doctor
        | Cmd::Jobs
        | Cmd::Sessions(_)
        | Cmd::Migrate
        | Cmd::Update
        | Cmd::Tasks(_)
        | Cmd::Secret(_)
        | Cmd::Skill(_)
        | Cmd::Service(_)
        | Cmd::Commands
        | Cmd::Docs
        | Cmd::System
        | Cmd::Channels
        | Cmd::Directory
        | Cmd::ExecPolicy
        | Cmd::Gateway
        | Cmd::Webhooks
        | Cmd::Security
        | Cmd::Health
        | Cmd::Memory(_)
        | Cmd::ConfigFile(_)
        | Cmd::Audit(_)
        | Cmd::Backup(_)
        | Cmd::Transcripts(_)
        | Cmd::Completion(_)
        | Cmd::Reset
        | Cmd::Uninstall
        | Cmd::Board(_)
        | Cmd::Canvas(_)
        | Cmd::Mcp(_)
        | Cmd::Hooks(_)
        | Cmd::Agents(_)
        | Cmd::Commitments(_)
        | Cmd::Proxy(_)
        | Cmd::Attach(_)
        | Cmd::Capability
        | Cmd::Media(_)
        | Cmd::Oauth(_)
        | Cmd::Transcribe(_)
        | Cmd::Worktrees(_) => {
            unreachable!()
        }
    }
}

fn cmd_configure(install_daemon: bool) -> u8 {
    let path = config::config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("cannot create {}: {e}", parent.display());
                return 2;
            }
        }
        if let Err(e) = config::init_config() {
            eprintln!("cannot write {}: {e}", path.display());
            return 2;
        }
        println!("new nest at {}\n", path.display());
    } else {
        println!(
            "reconfiguring {} (the current file is copied to {}.bak first)\n",
            path.display(),
            path.display()
        );
    }
    let gw = config::home_dir().join(".openclaw/openclaw.json");
    let src = gw.exists().then_some(gw);
    match onboard::first_run(
        src.as_deref(),
        &path,
        true,
        &mut io::stdin().lock(),
        &mut io::stdout(),
    ) {
        Ok(true) => {
            if install_daemon {
                match service::install() {
                    Ok(out) => println!("{out}"),
                    Err(e) => eprintln!("service install failed: {e}"),
                }
            }
            0
        }
        Ok(false) => {
            println!("nothing changed.");
            0
        }
        Err(e) => {
            eprintln!("configure error: {e}");
            2
        }
    }
}

fn cmd_status(json: bool) -> u8 {
    let path = config::config_path();
    if !path.exists() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "v": 1, "ok": false, "config_path": path.display().to_string(),
                    "error": "no config"
                }))
                .unwrap_or_default()
            );
        } else {
            println!("no nest yet at {}\n  run: phoenix init", path.display());
        }
        return 2;
    }
    let cfg = match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 2;
        }
    };

    let mut channels: Vec<(&str, usize)> = Vec::new();
    let mut add = |name: &'static str, on: bool, allowed: usize| {
        if on {
            channels.push((name, allowed));
        }
    };
    add(
        "telegram",
        !cfg.telegram_token.is_empty(),
        cfg.telegram_allowed.len(),
    );
    add("whatsapp", !cfg.wa_token.is_empty(), cfg.wa_allowed.len());
    add(
        "discord",
        !cfg.discord_token.is_empty(),
        cfg.discord_allowed.len(),
    );
    add(
        "slack",
        !cfg.slack_bot_token.is_empty(),
        cfg.slack_allowed.len(),
    );
    add(
        "signal",
        !cfg.signal_account.is_empty(),
        cfg.signal_allowed.len(),
    );
    add("irc", !cfg.irc_server.is_empty(), cfg.irc_allowed.len());
    add(
        "matrix",
        !cfg.matrix_token.is_empty(),
        cfg.matrix_allowed.len(),
    );
    add(
        "mattermost",
        !cfg.mattermost_token.is_empty(),
        cfg.mattermost_allowed.len(),
    );
    add("imessage", cfg.imessage_enabled, cfg.imessage_allowed.len());

    let key_source = if !cfg.api_key.is_empty() {
        "config"
    } else if cfg.provider == "ollama" {
        "not needed"
    } else if config::provider_key_vars(&cfg.provider)
        .iter()
        .any(|v| env::var(v).map(|s| !s.is_empty()).unwrap_or(false))
        || env::var("PHOENIX_API_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    {
        "environment"
    } else {
        "MISSING"
    };
    let svc = if service::systemd_available() {
        service::state()
    } else {
        "no systemd".to_string()
    };

    let serve = daemon::report(&daemon::default_path());
    let serve_running = serve["running"] == json!(true);
    let mut http_ms: Option<u64> = None;
    if serve_running && cfg.http_enabled {
        let bind = match cfg.http_bind.as_str() {
            "" | "0.0.0.0" => "127.0.0.1",
            b => b,
        };
        let url = format!("http://{}:{}/health", bind, cfg.http_port);
        let started = std::time::Instant::now();
        if ureq::get(&url)
            .timeout(std::time::Duration::from_millis(800))
            .call()
            .is_ok()
        {
            http_ms = Some(started.elapsed().as_millis() as u64);
        }
    }
    let sess = sessions::list(&config::home().join("sessions"));
    let sess_msgs: usize = sess.iter().map(|(_, n)| n).sum();
    let upd = update::last_check(&update::check_cache_path());
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if json {
        let chans: Vec<Value> = channels
            .iter()
            .map(|(n, a)| json!({"name": n, "allowed": a}))
            .collect();
        let doc = json!({
            "v": 1,
            "ok": key_source != "MISSING",
            "config_path": path.display().to_string(),
            "provider": cfg.provider,
            "model": cfg.model,
            "fallbacks": cfg.fallbacks,
            "api_key": key_source,
            "workspace": cfg.workspace.display().to_string(),
            "approvals": cfg.approvals,
            "channels": chans,
            "service": svc,
            "serve": {
                "running": serve_running,
                "pid": serve["pid"].as_u64(),
                "uptime_secs": serve["uptime_secs"].as_u64(),
                "http_ms": http_ms,
            },
            "sessions": {"count": sess.len(), "messages": sess_msgs},
            "update": upd.as_ref().map(|(at, tag, ok)| json!({
                "checked_at": at, "latest_tag": tag, "up_to_date": ok
            })),
            "http": {
                "enabled": cfg.http_enabled,
                "bind": cfg.http_bind,
                "port": cfg.http_port,
                "web": cfg.http_web,
                "url": dashboard_url(&cfg),
            },
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return u8::from(key_source == "MISSING") * 2;
    }

    let tty = io::stdout().is_terminal();
    let (dim, bold, off) = if tty {
        ("\x1b[2m", "\x1b[1m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    println!("{bold}openphoenix {}{off}", VERSION);
    println!("  {dim}config    {off}{}", path.display());
    println!("  {dim}model     {off}{}/{}", cfg.provider, cfg.model);
    if !cfg.fallbacks.is_empty() {
        println!("  {dim}fallbacks {off}{}", cfg.fallbacks.join(", "));
    }
    println!("  {dim}api key   {off}{key_source}");
    println!("  {dim}workspace {off}{}", cfg.workspace.display());
    println!(
        "  {dim}approvals {off}{}",
        if cfg.approvals {
            "on - shell waits for you"
        } else {
            "off - shell runs unattended"
        }
    );
    if channels.is_empty() {
        println!("  {dim}channels  {off}none configured");
    } else {
        let list: Vec<String> = channels
            .iter()
            .map(|(n, a)| {
                if *a == 0 {
                    format!("{n} (closed: no one allowed)")
                } else {
                    format!("{n} ({a} allowed)")
                }
            })
            .collect();
        println!("  {dim}channels  {off}{}", list.join(", "));
    }
    println!("  {dim}service   {off}{svc}");
    if serve_running {
        let pid = serve["pid"].as_u64().unwrap_or(0);
        let up = scheduler::time_ago(serve["uptime_secs"].as_u64().unwrap_or(0));
        match http_ms {
            Some(ms) => println!(
                "  {dim}serve     {off}running as pid {pid} for {up}, /health answers in {ms} ms"
            ),
            None if cfg.http_enabled => println!(
                "  {dim}serve     {off}running as pid {pid} for {up}, but /health is not answering"
            ),
            None => println!("  {dim}serve     {off}running as pid {pid} for {up}"),
        }
    } else {
        println!("  {dim}serve     {off}not running");
    }
    if sess.is_empty() {
        println!("  {dim}sessions  {off}none stored");
    } else {
        println!(
            "  {dim}sessions  {off}{} stored holding {} message(s)",
            sess.len(),
            sess_msgs
        );
    }
    if cfg.http_enabled {
        let url = dashboard_url(&cfg);
        println!(
            "  {dim}http      {off}{}:{}{}",
            cfg.http_bind,
            cfg.http_port,
            if cfg.http_web { "  web UI on" } else { "" }
        );
        if cfg.http_web {
            println!("  {dim}dashboard {off}{url}");
        }
    } else {
        println!("  {dim}http      {off}off");
    }
    match &upd {
        Some((at, tag, true)) => println!(
            "  {dim}update    {off}up to date with {tag} (checked {} ago)",
            scheduler::time_ago(now_secs.saturating_sub(*at))
        ),
        Some((at, tag, false)) => println!(
            "  {dim}update    {off}{tag} is out: run `phoenix update` (checked {} ago)",
            scheduler::time_ago(now_secs.saturating_sub(*at))
        ),
        None => println!("  {dim}update    {off}never checked: run `phoenix update --check`"),
    }
    if key_source == "MISSING" {
        let hint = config::provider_key_vars(&cfg.provider)
            .first()
            .copied()
            .unwrap_or("PHOENIX_API_KEY");
        println!("\nno API key yet: export {hint}=... or run `phoenix configure`");
        return 2;
    }
    println!("\n{dim}next: phoenix chat | phoenix doctor | phoenix serve{off}");
    0
}

fn dashboard_url(cfg: &Config) -> String {
    let host = match cfg.http_bind.as_str() {
        "0.0.0.0" | "::" | "" => "127.0.0.1",
        h => h,
    };
    if host.contains(':') {
        format!("http://[{host}]:{}/", cfg.http_port)
    } else {
        format!("http://{host}:{}/", cfg.http_port)
    }
}

fn cmd_dashboard() -> u8 {
    let cfg = match config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 2;
        }
    };
    if !cfg.http_enabled {
        println!(
            "the http server is off. Turn it on with `phoenix configure`, \
or set [http] enabled = true in {}.",
            config::config_path().display()
        );
        return 2;
    }
    let url = dashboard_url(&cfg);
    if !cfg.http_web {
        println!("the API is on at {url} but the browser UI is off ([http] web = true).");
        return 2;
    }
    println!("dashboard: {url}");
    let probe: std::net::SocketAddr = match format!("127.0.0.1:{}", cfg.http_port).parse() {
        Ok(a) => a,
        Err(_) => {
            println!("(start it with `phoenix serve`)");
            return 0;
        }
    };
    if std::net::TcpStream::connect_timeout(&probe, std::time::Duration::from_millis(300)).is_err()
    {
        println!("(nothing is listening yet - start it with `phoenix serve`)");
        return 0;
    }
    for opener in ["xdg-open", "open", "wslview"] {
        if std::process::Command::new(opener)
            .arg(&url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
        {
            return 0;
        }
    }
    println!("(no browser opener found - copy the URL above)");
    0
}

fn models_text(cfg: &Config, cap: usize) -> String {
    let mut out = format!(
        "aliases: opus sonnet gpt gpt-mini gpt-nano gemini gemini-flash gemini-flash-lite\n\
current: {}/{}\n",
        cfg.provider, cfg.model
    );
    if let Some(info) = catalog::lookup(&cfg.provider, &cfg.model) {
        if info.provider != cfg.provider {
            out.push_str(&format!("catalog match: {}/{}\n", info.provider, info.id));
        }
        out.push_str(&format!(
            "context: {} tokens, max output {}\n",
            info.context_window, info.max_tokens
        ));
        if let Some(c) = catalog::estimate_cost(&cfg.provider, &cfg.model, 1_000_000, 0) {
            out.push_str(&format!("cost: ${c:.2} per million input tokens\n"));
        }
    }
    match providers::list_models(cfg) {
        Ok(models) if models.is_empty() => out.push_str("provider returned no model list\n"),
        Ok(models) => {
            out.push_str(&format!("{} live from {}:\n", models.len(), cfg.provider));
            for m in models.iter().take(cap) {
                let mark = if *m == cfg.model { "  * " } else { "    " };
                let win = catalog::context_window(&cfg.provider, m)
                    .map(|w| format!("  ({}k ctx)", w / 1000))
                    .unwrap_or_default();
                out.push_str(&format!("{mark}{m}{win}\n"));
            }
            if models.len() > cap {
                out.push_str(&format!("    … {} more\n", models.len() - cap));
            }
        }
        Err(e) => {
            out.push_str(&format!("model list failed: {e}\n"));
            let known = catalog::known_models(&cfg.provider);
            if !known.is_empty() {
                out.push_str(&format!(
                    "{} known offline for {} (no network needed):\n",
                    known.len(),
                    cfg.provider
                ));
                for m in known.iter().take(cap) {
                    let mark = if *m == cfg.model { "  * " } else { "    " };
                    out.push_str(&format!("{mark}{m}\n"));
                }
            }
        }
    }
    out
}

fn cmd_models(cfg: &Config, test_fallback: bool) -> u8 {
    if test_fallback {
        return cmd_models_test_fallback(cfg);
    }
    print!("{}", models_text(cfg, usize::MAX));
    0
}

fn cmd_models_test_fallback(cfg: &Config) -> u8 {
    use crate::providers::ChatBackend;
    let mut chain: Vec<(&str, Config)> = vec![("primary", cfg.clone())];
    for spec in &cfg.fallbacks {
        let mut c = cfg.clone();
        crate::config::retarget(&mut c, spec);
        chain.push(("fallback", c));
    }
    if chain.len() == 1 {
        println!("no fallbacks configured; testing the primary only");
    }
    let mut failures = 0u32;
    for (label, c) in chain {
        let target = format!("{}/{}", c.provider, c.model);
        let started = std::time::Instant::now();
        let outcome = providers::make(&c).and_then(|mut p| {
            let history = vec![providers::Msg::User {
                content: "Reply with the single word: pong".to_string(),
                images: Vec::new(),
            }];
            p.chat(&c, "You are a connectivity probe.", &history, &[])
        });
        let ms = started.elapsed().as_millis();
        match outcome {
            Ok(r) => {
                let word = crate::security::one_line(r.text.trim(), 40);
                println!("ok   {label} {target} answered in {ms} ms: {word}");
            }
            Err(e) => {
                failures += 1;
                let err = crate::security::one_line(&crate::security::redact(&e.to_string()), 120);
                println!("FAIL {label} {target} after {ms} ms: {err}");
            }
        }
    }
    if failures == 0 {
        println!("fallback chain healthy");
        0
    } else {
        println!("{failures} link(s) failed; this chain would not save you");
        1
    }
}

fn cmd_migrate(from: Option<&str>, write: bool, force: bool, secrets: bool) -> u8 {
    let src = from
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::config::home_dir().join(".openclaw/openclaw.json"));
    let raw = match std::fs::read_to_string(&src) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", src.display());
            return 2;
        }
    };
    let mut v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {} is not valid JSON: {e}", src.display());
            return 2;
        }
    };

    let mut keys = Vec::new();
    if secrets {
        if let Some(dir) = src.parent() {
            if let Some(tok) = migrate::resolve_secret_token(&v, dir) {
                v["channels"]["telegram"]["botToken"] = Value::String(tok);
            }
            keys = migrate::collect_keys(dir);
        }
    }
    let mut m = migrate::from_gateway(&v);
    if !keys.is_empty() {
        let summary: Vec<String> = keys
            .iter()
            .map(|(p, k)| format!("{p} ({})", k.len()))
            .collect();
        m.notes.retain(|n| !n.starts_with("export PHOENIX_API_KEY"));
        if write {
            match crate::secrets::stash_provider_keys(&keys) {
                Ok(_) => m.notes.push(format!(
                    "API keys encrypted into the secret store (AES-256-GCM): {}; the unlock \
key PHOENIX_SECRET_KEY lives in the env file (mode 600)",
                    summary.join(", ")
                )),
                Err(e) => {
                    eprintln!("error: cannot encrypt the keys: {e}");
                    return 2;
                }
            }
        } else {
            m.notes.push(format!(
                "--write will encrypt these keys into the secret store: {}",
                summary.join(", ")
            ));
        }
    }
    if write {
        let path = config::config_path();
        if path.exists() && !force {
            eprintln!("error: {} exists; use --force to overwrite", path.display());
            return 2;
        }
        if let Err(e) = onboard::write_config(&path, &m.toml) {
            eprintln!("error: write {}: {e}", path.display());
            return 2;
        }
        println!("wrote {}", path.display());
        if let Some(dir) = src.parent() {
            let ws = migrate::gateway_workspace(&v, dir);
            let workspace = config::parse(&m.toml)
                .map(|c| c.workspace)
                .unwrap_or_else(|_| config::home_dir().join("phoenix"));
            for n in migrate::carry_persona(&ws, &workspace) {
                println!("{n}");
            }
        }
    } else {
        println!("{}", m.toml);
    }
    if !m.notes.is_empty() {
        eprintln!("\nnext steps:");
        for n in &m.notes {
            eprintln!("  - {n}");
        }
    }
    0
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        eprintln!(
            "phoenix hit an internal error and is shutting down cleanly: {} (at {at})",
            crate::security::one_line(&security::redact(&msg), 300)
        );
    }));
}

fn main() -> ExitCode {
    install_panic_hook();
    ExitCode::from(real_main())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_answers_are_read_forgivingly_and_never_default_to_yes() {
        for yes in ["y", "Y", "yes", "YES", " yeah ", "ok", "sure", "yep"] {
            assert_eq!(read_approval(yes), Approval::Yes, "{yes:?}");
        }
        for no in ["n", "no", "", "   ", "nope", "deny"] {
            assert_eq!(read_approval(no), Approval::No, "{no:?}");
        }
        for all in ["a", "always", "all", "yolo"] {
            assert_eq!(read_approval(all), Approval::Always, "{all:?}");
        }
        for junk in ["zy", "maybe", "wat", "yy"] {
            assert_eq!(
                read_approval(junk),
                Approval::Unclear,
                "{junk:?} must re-ask, never be read as yes or silently no"
            );
        }
    }

    fn cfg_with_http(bind: &str, port: u16) -> Config {
        Config {
            http_bind: bind.to_string(),
            http_port: port,
            ..Config::default()
        }
    }

    #[test]
    fn the_dashboard_url_never_tells_you_to_browse_to_a_bind_address() {
        assert_eq!(
            dashboard_url(&cfg_with_http("0.0.0.0", 8787)),
            "http://127.0.0.1:8787/"
        );
        assert_eq!(
            dashboard_url(&cfg_with_http("::", 9000)),
            "http://127.0.0.1:9000/"
        );
        assert_eq!(
            dashboard_url(&cfg_with_http("", 8787)),
            "http://127.0.0.1:8787/"
        );
    }

    #[test]
    fn the_dashboard_url_keeps_a_real_host_and_brackets_ipv6() {
        assert_eq!(
            dashboard_url(&cfg_with_http("127.0.0.1", 8787)),
            "http://127.0.0.1:8787/"
        );
        assert_eq!(
            dashboard_url(&cfg_with_http("192.168.1.10", 8080)),
            "http://192.168.1.10:8080/"
        );
        assert_eq!(
            dashboard_url(&cfg_with_http("fd00::1", 8787)),
            "http://[fd00::1]:8787/",
            "a bare IPv6 host in a URL must be bracketed"
        );
    }

    #[test]
    fn every_documented_command_actually_parses() {
        for (words, want) in [
            (vec!["init"], Cmd::Init),
            (vec!["configure"], Cmd::Configure),
            (vec!["status"], Cmd::Status),
            (vec!["dashboard"], Cmd::Dashboard),
            (vec!["chat"], Cmd::Chat),
            (vec!["serve"], Cmd::Serve),
            (vec!["doctor"], Cmd::Doctor),
            (vec!["jobs"], Cmd::Jobs),
            (vec!["sessions"], Cmd::Sessions(Vec::new())),
            (vec!["migrate"], Cmd::Migrate),
            (vec!["models"], Cmd::Models),
            (vec!["schema"], Cmd::Schema),
        ] {
            let argv: Vec<String> = words.iter().map(|s| s.to_string()).collect();
            let got = parse_args(&argv).map(|a| a.cmd);
            assert_eq!(got.as_ref(), Ok(&want), "`phoenix {}` must parse", words[0]);
        }
    }

    #[test]
    fn the_help_text_lists_every_command_it_can_actually_run() {
        let text = usage();
        for c in [
            "init",
            "configure",
            "status",
            "dashboard",
            "chat",
            "serve",
            "doctor",
            "jobs",
            "sessions",
            "migrate",
            "update",
            "models",
            "schema",
            "secret",
            "skill",
            "service",
        ] {
            assert!(text.contains(c), "help must document `{c}`");
        }
    }

    #[test]
    fn service_subcommands_shadowed_by_top_level_names_still_parse() {
        for sub in [
            "status",
            "install",
            "uninstall",
            "logs",
            "start",
            "stop",
            "restart",
        ] {
            let argv: Vec<String> = vec!["service".into(), sub.to_string()];
            let got = parse_args(&argv).map(|a| a.cmd);
            assert_eq!(
                got,
                Ok(Cmd::Service(vec![sub.to_string()])),
                "`phoenix service {sub}` must reach the service handler"
            );
        }
        let argv: Vec<String> = vec!["secret".into(), "set".into(), "service".into()];
        let got = parse_args(&argv).map(|a| a.cmd);
        assert_eq!(
            got,
            Ok(Cmd::Secret(vec!["set".into(), "service".into()])),
            "words after `secret` must never re-enter command parsing"
        );
    }

    #[test]
    fn install_daemon_is_accepted_alongside_setup_commands() {
        let argv: Vec<String> = vec!["init".into(), "--install-daemon".into()];
        let a = parse_args(&argv).unwrap_or_else(|e| panic!("must parse: {e}"));
        assert_eq!(a.cmd, Cmd::Init);
        assert!(a.install_daemon);

        let argv: Vec<String> = vec!["configure".into(), "--install-daemon".into()];
        let a = parse_args(&argv).unwrap_or_else(|e| panic!("must parse: {e}"));
        assert_eq!(a.cmd, Cmd::Configure);
        assert!(a.install_daemon);
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::*;

    #[test]
    fn json_flag_is_parsed_and_defaults_off() {
        let a = parse_args(&["doctor".into()]).expect("parsed");
        assert!(!a.json, "human output must stay the default");
        for cmd in ["doctor", "jobs", "sessions"] {
            let a = parse_args(&[cmd.into(), "--json".into()]).expect("parsed");
            assert!(a.json, "{cmd} --json must set the flag");
        }
        let a = parse_args(&["--json".into(), "doctor".into()]).expect("parsed");
        assert!(a.json, "the flag must work before the subcommand too");
    }

    #[test]
    fn a_typo_suggests_the_nearest_command() {
        let e = parse_args(&["doctr".into()]).expect_err("must fail");
        assert!(e.contains("phoenix doctor"), "{e}");
        let e = parse_args(&["serv".into()]).expect_err("must fail");
        assert!(e.contains("phoenix serve"), "{e}");
        let e = parse_args(&["scheme".into()]).expect_err("must fail");
        assert!(e.contains("phoenix schema"), "{e}");
    }

    #[test]
    fn nonsense_gets_usage_without_a_misleading_suggestion() {
        let e = parse_args(&["xyzzyplugh".into()]).expect_err("must fail");
        assert!(!e.contains("did you mean"), "{e}");
        assert!(e.contains("usage:"), "{e}");
    }

    #[test]
    fn edit_distance_is_symmetric_and_correct() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("doctor", "doctor"), 0);
        assert_eq!(edit_distance("doctr", "doctor"), 1);
        assert_eq!(edit_distance("doctor", "doctr"), 1);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("\u{6f22}\u{5b57}", "\u{6f22}"), 1);
    }

    #[test]
    fn a_double_dash_ends_flag_parsing_for_prompts() {
        let a = parse_args(&["run".into(), "--".into(), "--model".into(), "why".into()])
            .expect("parsed");
        assert_eq!(a.cmd, Cmd::Run("--model why".into()));
        assert!(a.model.is_none(), "flags after -- must not be consumed");
    }

    #[test]
    fn flags_before_the_terminator_still_apply() {
        let a = parse_args(&["--ghost".into(), "run".into(), "--".into(), "-h".into()])
            .expect("parsed");
        assert!(a.ghost);
        assert_eq!(a.cmd, Cmd::Run("-h".into()));
    }

    #[test]
    fn a_stray_terminator_without_a_subcommand_is_an_error() {
        assert!(parse_args(&["--".into(), "junk".into()]).is_err());
    }

    #[test]
    fn run_dash_is_accepted_as_a_stdin_prompt_marker() {
        let a = parse_args(&["run".into(), "-".into()]).expect("parsed");
        assert_eq!(a.cmd, Cmd::Run("-".into()));
    }

    #[test]
    fn run_without_a_prompt_points_at_stdin() {
        let e = parse_args(&["run".into()]).expect_err("must fail");
        assert!(e.contains("stdin"), "{e}");
    }

    #[test]
    fn command_detection_only_matches_command_shapes() {
        assert!(looks_like_command("/help"));
        assert!(looks_like_command("/think adaptive"));
        assert!(looks_like_command("  /model claude-opus-5"));
        assert!(!looks_like_command("hello"));
        assert!(!looks_like_command("and/or"));
        assert!(!looks_like_command("/"));
        assert!(looks_like_command("/Status"));
        assert!(looks_like_command("/status@phoenix_bot"));
        assert!(!looks_like_command("3/4 done"));
    }

    #[test]
    fn unknown_command_never_reaches_the_model() {
        let cfg = Config::default();
        let r = channel_command(None, &cfg, "/nonsense").expect("handled");
        assert!(r.text.contains("unknown command"), "{}", r.text);
        assert!(r.text.contains("/help"));
        assert!(channel_command(None, &cfg, "just a message").is_none());
    }

    #[test]
    fn model_alias_switch_retargets_the_provider_too() {
        struct Silent;
        impl providers::ChatBackend for Silent {
            fn chat(
                &mut self,
                _: &Config,
                _: &str,
                _: &[providers::Msg],
                _: &[Value],
            ) -> Result<providers::Reply, providers::ProviderError> {
                Ok(providers::Reply::text_only("unused"))
            }
        }
        let cfg = Config {
            provider: "openai".into(),
            model: "gpt-5.4".into(),
            workspace: std::env::temp_dir().join(format!("phx-modelsw-{}", std::process::id())),
            ..Config::default()
        };
        let _ = std::fs::create_dir_all(&cfg.workspace);
        let toolbox = Toolbox::new(&cfg, Memory::new("ghost"), None, None).unwrap();
        let mut a = Agent::new(cfg.clone(), Box::new(Silent), toolbox);
        let r = channel_command(Some(&mut a), &cfg, "/model opus").expect("handled");
        assert!(r.text.contains("model → claude-opus-5"), "{}", r.text);
        assert!(r.text.contains("provider → anthropic"), "{}", r.text);
        assert_eq!(a.cfg.provider, "anthropic");
        assert_eq!(a.cfg.model, "claude-opus-5");
        let r = channel_command(Some(&mut a), &cfg, "/models gpt").expect("handled");
        assert!(r.text.contains("provider → openai"), "{}", r.text);
        assert_eq!(a.cfg.provider, "openai");
        assert_eq!(a.cfg.model, "gpt-5.4");
    }

    #[test]
    fn bad_argument_offers_buttons_instead_of_falling_through() {
        let cfg = Config::default();
        let r = channel_command(None, &cfg, "/lean on").expect("handled");
        assert!(!r.buttons.is_empty(), "expected picker buttons");
        let labels: Vec<String> = r
            .buttons
            .iter()
            .flatten()
            .map(|(l, _)| l.replace("\u{2705} ", ""))
            .collect();
        assert_eq!(labels, vec!["off", "lean", "grunt"]);
        let data: Vec<&str> = r
            .buttons
            .iter()
            .flatten()
            .map(|(_, d)| d.as_str())
            .collect();
        assert!(data.contains(&"/lean grunt"));
    }

    #[test]
    fn think_alias_and_openclaw_levels() {
        let cfg = Config::default();
        for cmd in ["/think", "/thinking"] {
            let r = channel_command(None, &cfg, cmd).expect("handled");
            let labels: Vec<String> = r
                .buttons
                .iter()
                .flatten()
                .map(|(l, _)| l.replace("\u{2705} ", ""))
                .collect();
            assert_eq!(
                labels,
                vec![
                    "default", "off", "minimal", "low", "medium", "adaptive", "high", "xhigh",
                    "max"
                ],
                "{cmd}"
            );
        }
    }

    #[test]
    fn current_value_is_marked_in_pickers() {
        let cfg = Config {
            privacy: "recall".into(),
            ..Config::default()
        };
        let r = channel_command(None, &cfg, "/privacy").expect("handled");
        let marked: Vec<&String> = r
            .buttons
            .iter()
            .flatten()
            .map(|(l, _)| l)
            .filter(|l| l.starts_with('\u{2705}'))
            .collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].ends_with("recall"));
    }

    #[test]
    fn flatten_renders_options_for_buttonless_channels() {
        let cfg = Config::default();
        let r = channel_command(None, &cfg, "/lean").expect("handled");
        let flat = r.flatten();
        assert!(flat.contains("off | lean | grunt"), "{flat}");
        assert!(!flat.contains('\u{2705}'), "{flat}");
    }

    #[test]
    fn parse_defaults_to_chat() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.cmd, Cmd::Chat);
        assert!(!a.ghost && !a.recall);
    }

    #[test]
    fn parse_run_collects_prompt() {
        let argv: Vec<String> = ["--ghost", "run", "do", "the", "thing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_args(&argv).unwrap();
        assert!(a.ghost);
        assert_eq!(a.cmd, Cmd::Run("do the thing".into()));
    }

    #[test]
    fn parse_flags() {
        let argv: Vec<String> = ["--lean", "grunt", "--model", "m1", "--provider", "openai"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_args(&argv).unwrap();
        assert_eq!(a.lean.as_deref(), Some("grunt"));
        assert_eq!(a.model.as_deref(), Some("m1"));
        assert_eq!(a.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn no_advertised_command_is_rejected_as_unknown() {
        let mut unknown: Vec<&str> = Vec::new();
        for spec in commands::COMMANDS {
            if let Err(e) = parse_args(&[spec.name.to_string()]) {
                if e.contains("unknown argument") {
                    unknown.push(spec.name);
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "advertised but unrecognised: {unknown:?}"
        );
    }

    #[test]
    fn commands_we_do_not_have_are_rejected_not_quietly_accepted() {
        for (name, why) in commands::NOT_BUILT {
            let e = parse_args(&[name.to_string()])
                .err()
                .unwrap_or_else(|| panic!("{name} is not built but the parser accepted it"));
            assert!(
                e.contains("not built here"),
                "{name} must be refused with a reason, got: {e}"
            );
            assert!(e.contains(why), "{name} must name its reason, got: {e}");
        }
    }

    #[test]
    fn worktrees_is_real_and_parses_to_its_own_arm() {
        let a = parse_args(&["worktrees".to_string()]).expect("worktrees must parse");
        assert!(matches!(a.cmd, Cmd::Worktrees(_)));
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse_args(&["--lean".into(), "max".into()]).is_err());
        assert!(parse_args(&["run".into()]).is_err());
        assert!(parse_args(&["--bogus".into()]).is_err());
    }

    #[test]
    fn version_flag() {
        let err = parse_args(&["-V".into()]).unwrap_err();
        assert_eq!(err, format!("version:openphoenix {VERSION}"));
    }

    #[test]
    fn banner_contains_version() {
        assert!(banner().contains(VERSION));
    }
}
