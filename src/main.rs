mod agent;
mod audio;
mod board;
mod browser;
mod canvas;
mod clawhub;
mod config;
mod discord;
mod doctor;
mod embeddings;
mod heartbeat;
mod http;
mod imessage;
mod media;
mod memory;
mod migrate;
mod onboard;
mod prompts;
mod providers;
mod scheduler;
mod security;
#[cfg(test)]
mod security_fuzz;
mod service;
mod sessions;
mod signal;
mod skills;
mod slack;
mod telegram;
mod tools;
mod whatsapp;
mod ws;

use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

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
    Run(String),
    Chat,
    Serve,
    Doctor,
    Jobs,
    Sessions,
    Migrate,
    Skill(Vec<String>),
    Service(Vec<String>),
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
    let mut prompt_words: Vec<String> = Vec::new();
    let mut in_run = false;
    let mut in_skill = false;
    let mut skill_words: Vec<String> = Vec::new();
    let mut in_service = false;
    let mut service_words: Vec<String> = Vec::new();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
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
            "init" | "chat" | "serve" | "doctor" | "jobs" | "sessions" | "migrate"
                if cmd.is_none() && !in_run =>
            {
                cmd = Some(match arg.as_str() {
                    "init" => Cmd::Init,
                    "serve" => Cmd::Serve,
                    "doctor" => Cmd::Doctor,
                    "jobs" => Cmd::Jobs,
                    "sessions" => Cmd::Sessions,
                    "migrate" => Cmd::Migrate,
                    _ => Cmd::Chat,
                });
            }
            "run" if cmd.is_none() && !in_run => {
                in_run = true;
            }
            "skill" if cmd.is_none() && !in_run && !in_skill => {
                in_skill = true;
            }
            "service" if cmd.is_none() && !in_run && !in_skill && !in_service => {
                in_service = true;
            }
            other => {
                if in_run {
                    prompt_words.push(other.to_string());
                } else if in_skill {
                    skill_words.push(other.to_string());
                } else if in_service {
                    service_words.push(other.to_string());
                } else {
                    return Err(format!("unknown argument: {other}\n{}", usage()));
                }
            }
        }
    }
    let cmd = if in_run {
        if prompt_words.is_empty() {
            return Err("run needs a prompt".into());
        }
        Cmd::Run(prompt_words.join(" "))
    } else if in_skill {
        if skill_words.is_empty() {
            return Err("skill needs a subcommand: search QUERY | install OWNER/SLUG".into());
        }
        Cmd::Skill(skill_words)
    } else if in_service {
        Cmd::Service(service_words)
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
    })
}

fn usage() -> String {
    format!(
        "usage: phoenix [-V] [--ghost] [--recall] [--lean LEVEL] [--model NAME] \
[--provider KIND] [init|run PROMPT|chat|serve|doctor|jobs|sessions|skill|migrate]\n\
  init      write sample config\n\
  run       one-shot task (ghost by default)\n\
  chat      interactive REPL (default)\n\
  serve     all configured channels + http api + cron jobs + dreaming\n\
  doctor    audit config, permissions, and risky settings\n\
  jobs      list cron jobs and validate their schedules\n\
  sessions  list stored serve-mode sessions\n\
  skill     search or install ClawHub skills: skill search QUERY | skill install OWNER/SLUG\n\
  service   run serve as a background service: install|uninstall|start|stop|restart|status|logs\n\
  migrate   convert an AI gateway config [--from PATH] [--write] [--force] [--secrets]\n\
openphoenix {VERSION}"
    )
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

fn confirm_shell(command: &str) -> bool {
    print!("\n  run? `{command}` [y/N] ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

fn build_agent(cfg: &Config, interactive: bool) -> Result<Agent, String> {
    let memory = Memory::new(&cfg.privacy);
    let confirm: Option<tools::ConfirmFn> = if interactive && cfg.confirm_shell {
        Some(Box::new(confirm_shell))
    } else {
        None
    };
    let on_event: tools::EventFn = Box::new(|name: &str, args: &Value| {
        let a: String = args.to_string().chars().take(120).collect();
        eprintln!("  → {name} {a}");
    });
    let toolbox = Toolbox::new(cfg, memory, confirm, Some(on_event))?;
    let provider = providers::make(cfg).map_err(|e| e.to_string())?;
    let mut agent = Agent::new(cfg.clone(), Box::new(provider), toolbox);
    agent.skills = skills::load_dir(&config::home().join("skills"));
    Ok(agent)
}

fn slash(line: &str, agent: &mut Agent) -> bool {
    let (cmd, arg) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd {
        "/quit" | "/exit" => return true,
        "/help" => println!(
            "/ghost /session /recall - privacy | /lean off|lean|grunt | \
/model NAME | /wipe | /usage | /quit"
        ),
        "/ghost" | "/session" | "/recall" => {
            let mode = &cmd[1..];
            agent.cfg.privacy = mode.to_string();
            agent.toolbox.memory.privacy = mode.to_string();
            agent.wipe();
            println!("privacy → {mode} (history wiped)");
        }
        "/lean" if LEAN_LEVELS.contains(&arg) => {
            agent.cfg.lean = arg.to_string();
            println!("lean → {arg}");
        }
        "/model" if !arg.is_empty() => {
            let m = providers::resolve_alias(arg)
                .map(|(_, m)| m.to_string())
                .unwrap_or_else(|| arg.to_string());
            agent.cfg.model = m.clone();
            println!("model → {m}");
        }
        "/wipe" => {
            agent.wipe();
            println!("history wiped");
        }
        "/usage" => println!("tokens in={} out={}", agent.usage.input, agent.usage.output),
        _ => println!("unknown command, try /help"),
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
    println!("{}", banner());
    println!(
        "model={} privacy={} lean={}  (/help for commands)\n",
        cfg.model, cfg.privacy, cfg.lean
    );
    let stdin = io::stdin();
    loop {
        print!("you › ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                println!();
                return 0;
            }
            Ok(_) => {}
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('/') {
            if slash(line, &mut agent) {
                return 0;
            }
            continue;
        }
        if agent.stream_stdout {
            print!("\nphoenix › ");
            let _ = io::stdout().flush();
            let out = agent.run(line);
            if !agent.streamed_last {
                print!("{out}");
            }
            println!("\n");
        } else {
            println!("\nphoenix › {}\n", agent.run(line));
        }
    }
}

fn channel_command(agent: Option<&mut Agent>, cfg: &Config, text: &str) -> Option<String> {
    let t = text.trim();
    if t == "/pending" {
        return Some(match agent {
            Some(a) => a.toolbox.pending_list(),
            None => "nothing pending".to_string(),
        });
    }
    if t == "/status" {
        let (model, lean, pending, input, output) = match &agent {
            Some(a) => (
                a.cfg.model.clone(),
                a.cfg.lean.clone(),
                a.toolbox.pending_count(),
                a.usage.input,
                a.usage.output,
            ),
            None => (cfg.model.clone(), cfg.lean.clone(), 0, 0, 0),
        };
        return Some(format!(
            "openphoenix {VERSION}\nmodel: {}/{model}\nprivacy: {} | lean: {lean}\n\
sessions: {} | approvals: {}\npending approvals: {pending}\ntokens: in={input} out={output}",
            cfg.provider,
            cfg.privacy,
            if cfg.sessions { "on" } else { "off" },
            if cfg.approvals { "on" } else { "off" },
        ));
    }
    if let Some(arg) = t.strip_prefix("/model") {
        if !arg.is_empty() && !arg.starts_with(' ') {
            return None;
        }
        let arg = arg.trim();
        if !arg.is_empty() {
            let Some(a) = agent else {
                return Some("send a message first, then switch models".to_string());
            };
            let (model, note) = match providers::resolve_alias(arg) {
                Some((kind, m)) if kind != a.cfg.provider => (
                    m.to_string(),
                    format!(
                        ": note: alias targets provider '{kind}', current is '{}'",
                        a.cfg.provider
                    ),
                ),
                Some((_, m)) => (m.to_string(), String::new()),
                None => (arg.to_string(), String::new()),
            };
            a.cfg.model = model.clone();
            return Some(format!("model → {model} (this chat){note}"));
        }
        return Some("usage: /model NAME".to_string());
    }
    if let Some(arg) = t.strip_prefix("/lean") {
        let arg = arg.trim();
        if config::LEAN_LEVELS.contains(&arg) {
            let Some(a) = agent else {
                return Some("send a message first, then switch lean level".to_string());
            };
            a.cfg.lean = arg.to_string();
            return Some(format!("lean → {arg} (this chat)"));
        }
        if arg.is_empty() || t == "/lean" {
            return Some("usage: /lean off|lean|grunt".to_string());
        }
        return None;
    }
    for (word, approve) in [("/approve", true), ("/deny", false)] {
        if let Some(rest) = t.strip_prefix(word) {
            if !rest.is_empty() && !rest.starts_with(' ') {
                continue;
            }
            let Some(a) = agent else {
                return Some("nothing pending".to_string());
            };
            return Some(match rest.trim().parse::<u64>() {
                Ok(id) if approve => a.toolbox.approve(id),
                Ok(id) => a.toolbox.deny(id),
                Err(_) if a.toolbox.pending_count() == 0 => "nothing pending".to_string(),
                Err(_) => format!("usage: {word} ID\n{}", a.toolbox.pending_list()),
            });
        }
    }
    None
}

fn cmd_run(mut cfg: Config, prompt: &str) -> u8 {
    let interactive = io::stdin().is_terminal();

    cfg.approvals = false;
    let mut agent = match build_agent(&cfg, interactive) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    println!("{}", agent.run(prompt));
    0
}

fn cmd_serve(cfg: Config) -> u8 {
    println!("\u{1f525} phoenix rising \u{2014} openphoenix {VERSION} taking flight");

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
        let listener = match std::net::TcpListener::bind(("127.0.0.1", cfg.http_port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: http bind 127.0.0.1:{}: {e}", cfg.http_port);
                return 2;
            }
        };
        let http_cfg = cfg.clone();
        let token = cfg.http_token.clone();
        let web_opts = http::WebOpts {
            web: cfg.http_web,
            canvas: cfg.canvas_enabled,
            canvas_file: canvas::state_path(),
            strong_headers: cfg.http_headers != "minimal",
            user: cfg.http_user.clone(),
            pass: cfg.http_pass.clone(),
            crawlers: cfg.http_allow_crawlers.clone(),
        };
        std::thread::spawn(move || {
            let mut handler = move |prompt: &str| {
                let mut c = http_cfg.clone();
                c.privacy = "ghost".to_string();

                c.approvals = false;
                match build_agent(&c, false) {
                    Ok(mut a) => a.run(prompt),
                    Err(e) => format!("error: {e}"),
                }
            };
            http::serve(listener, &token, &mut handler, &web_opts);
        });
        println!("phoenix: http api on 127.0.0.1:{}", cfg.http_port);
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
                if text.trim() == "/new" {
                    sessions::reset(&wa_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &wa_cfg, text) {
                    return reply;
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
                let reply = agents.get_mut(&key).expect("agent inserted").run(text);
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
                if text.trim() == "/new" {
                    sessions::reset(&dc_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &dc_cfg, text) {
                    return reply;
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
                let reply = agents.get_mut(&key).expect("agent inserted").run(text);
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
            let mut handler = move |channel: &str, text: &str| -> String {
                mark_activity();
                let key = format!("sl-{channel}");
                if text.trim() == "/new" {
                    sessions::reset(&sl_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &sl_cfg, text) {
                    return reply;
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
                let reply = agents.get_mut(&key).expect("agent inserted").run(text);
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
                if text.trim() == "/new" {
                    sessions::reset(&sg_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &sg_cfg, text) {
                    return reply;
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
                let reply = agents.get_mut(&key).expect("agent inserted").run(text);
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
                if text.trim() == "/new" {
                    sessions::reset(&im_sess_dir, &key);
                    agents.remove(&key);
                    return "session reset".to_string();
                }
                if let Some(reply) = channel_command(agents.get_mut(&key), &im_cfg, text) {
                    return reply;
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
                let reply = agents.get_mut(&key).expect("agent inserted").run(text);
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
        move |prompt| {
            let mut c = job_cfg.clone();
            c.privacy = "ghost".to_string();

            c.approvals = false;
            match build_agent(&c, false) {
                Ok(mut a) => a.run(prompt),
                Err(e) => format!("job failed: {e}"),
            }
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
                tg.allowed.clone()
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
        t.allowed.clone()
    } else {
        Vec::new()
    };
    let tg_hb = tg.clone();
    let _heartbeat = heartbeat::Heartbeat::start(
        cfg.heartbeat_minutes,
        move || {
            let mut c = hb_cfg.clone();
            c.privacy = "ghost".to_string();

            c.approvals = false;
            let prompt = c.heartbeat_prompt.clone();
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
                let prev = std::fs::read_to_string(&path).unwrap_or_default();
                if let Err(e) = std::fs::write(&path, prev + &entry) {
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
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    };
    let tg_out = tg.clone();
    let result = tg.serve(
        &mut |chat_id, text, media| {
            mark_activity();
            if text.trim() == "/new" {
                sessions::reset(&sess_dir, chat_id);
                agents.remove(chat_id);
                return "session reset".to_string();
            }
            if let Some(reply) = channel_command(agents.get_mut(chat_id), &serve_cfg, text) {
                return reply;
            }
            if !agents.contains_key(chat_id) {
                match build_agent(&serve_cfg, false) {
                    Ok(mut a) => {
                        if use_sessions {
                            a.history = sessions::load(&sess_dir, chat_id);
                        }
                        agents.insert(chat_id.to_string(), a);
                    }
                    Err(e) => return format!("error: {e}"),
                }
            }
            let agent = agents.get_mut(chat_id).expect("agent inserted");
            let before_max = agent
                .toolbox
                .pending_entries()
                .last()
                .map(|(id, _)| *id)
                .unwrap_or(0);
            let reply = agent.run_with_media(text, media);

            let _ = tg_out.send(chat_id, &reply);
            for (id, command) in agent.toolbox.pending_entries() {
                if id <= before_max {
                    continue;
                }
                let preview: String = command.chars().take(1000).collect();
                let _ = tg_out.send_with_buttons(
                    chat_id,
                    &format!("run command #{id}?\n{preview}"),
                    &[vec![
                        (format!("\u{2705} approve #{id}"), format!("/approve {id}")),
                        (format!("\u{274c} deny #{id}"), format!("/deny {id}")),
                    ]],
                );
            }
            if use_sessions {
                if let Some(a) = agents.get(chat_id) {
                    if let Err(e) = sessions::save(&sess_dir, chat_id, &a.history) {
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
    if let Err(e) = result {
        eprintln!("error: {e}");
        return 2;
    }
    0
}

fn cmd_doctor(cfg: &Config) -> u8 {
    let cfg_path = config::config_path();
    let raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let memory_path = config::home().join("memory.md");
    let findings = doctor::check(cfg, &cfg_path, &raw, &memory_path);
    for x in &findings {
        let tag = match x.level {
            "fail" => "FAIL",
            "warn" => "warn",
            _ => "ok  ",
        };
        println!("{tag}  {}", x.msg);
    }
    if service::systemd_available() {
        let s = service::state();
        if s == "active" {
            println!("ok    service: active - the beacon is lit");
        } else {
            println!("warn  service: {s} - `phoenix service install` runs serve in the background");
        }
    }
    if doctor::has_failures(&findings) {
        1
    } else {
        0
    }
}

fn cmd_jobs(cfg: &Config) -> u8 {
    if cfg.jobs.is_empty() {
        println!("no jobs configured");
        return 0;
    }
    let mut bad = false;
    for job in &cfg.jobs {
        let sched = match scheduler::cron_valid(&job.cron) {
            Ok(()) => job.cron.clone(),
            Err(e) => {
                bad = true;
                format!("INVALID ({e})")
            }
        };
        let targets = if job.chat_ids.is_empty() {
            "all allowed chats".to_string()
        } else {
            job.chat_ids.join(", ")
        };
        let prompt: String = job.prompt.chars().take(60).collect();
        println!("{}  [{sched}]  -> {targets}\n    {prompt}", job.name);
    }
    if bad {
        1
    } else {
        0
    }
}

fn cmd_sessions() -> u8 {
    let dir = config::home().join("sessions");
    let all = sessions::list(&dir);
    if all.is_empty() {
        println!("no stored sessions");
    } else {
        for (id, n) in all {
            println!("{id}  {n} message(s)");
        }
    }
    0
}

fn real_main() -> u8 {
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

    let first_flight = !config::config_path().exists()
        && io::stdin().is_terminal()
        && matches!(args.cmd, Cmd::Init | Cmd::Chat | Cmd::Serve);
    if first_flight {
        let gw = config::home_dir().join(".openclaw/openclaw.json");
        let src = gw.exists().then_some(gw);
        match onboard::first_run(
            src.as_deref(),
            &config::config_path(),
            true,
            &mut io::stdin().lock(),
            &mut io::stdout(),
        ) {
            Ok(_) => {
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
    if let Some(m) = &args.model {
        cfg.model = m.clone();
    }
    if let Some(p) = &args.provider {
        cfg.provider = p.clone();
    }
    if let Err(e) = cfg.validate() {
        eprintln!("config error: {e}");
        return 2;
    }

    match args.cmd {
        Cmd::Doctor => return cmd_doctor(&cfg),
        Cmd::Jobs => return cmd_jobs(&cfg),
        Cmd::Sessions => return cmd_sessions(),
        Cmd::Skill(words) => return cmd_skill(&words),
        Cmd::Service(words) => return service::cmd_service(&words),
        _ => {}
    }

    if cfg.api_key.is_empty() && cfg.provider != "ollama" {
        let vars = config::provider_key_vars(&cfg.provider);
        let mut checked = vec!["PHOENIX_API_KEY"];
        checked.extend_from_slice(vars);
        eprintln!(
            "no API key for provider \"{}\": set provider.api_key or one of \
{} in the environment (run `phoenix init` for a sample config)",
            cfg.provider,
            checked.join(", ")
        );
        return 2;
    }

    match args.cmd {
        Cmd::Run(prompt) => cmd_run(cfg, &prompt),
        Cmd::Serve => cmd_serve(cfg),
        Cmd::Chat => cmd_chat(cfg),
        Cmd::Init
        | Cmd::Doctor
        | Cmd::Jobs
        | Cmd::Sessions
        | Cmd::Migrate
        | Cmd::Skill(_)
        | Cmd::Service(_) => {
            unreachable!()
        }
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

    if secrets {
        if let Some(dir) = src.parent() {
            if let Some(tok) = migrate::resolve_secret_token(&v, dir) {
                v["channels"]["telegram"]["botToken"] = Value::String(tok);
            }
        }
    }
    let m = migrate::from_gateway(&v);
    if write {
        let path = config::config_path();
        if path.exists() && !force {
            eprintln!("error: {} exists; use --force to overwrite", path.display());
            return 2;
        }
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("error: {e}");
                return 2;
            }
        }
        if let Err(e) = std::fs::write(&path, &m.toml) {
            eprintln!("error: write {}: {e}", path.display());
            return 2;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        println!("wrote {}", path.display());
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

fn main() -> ExitCode {
    ExitCode::from(real_main())
}

#[cfg(test)]
mod tests {
    use super::*;

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
