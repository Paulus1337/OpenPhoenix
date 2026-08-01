use std::fs;
use std::path::PathBuf;

use crate::mocks;
use crate::util::{self, Cx, T};

fn home_or(t: &mut T) -> Option<PathBuf> {
    match util::fresh_home() {
        Ok(h) => Some(h),
        Err(e) => {
            t.bad(&format!("fresh home: {e}"));
            None
        }
    }
}

pub fn t01_binary_smoke(cx: &Cx) -> T {
    let mut t = T::new("01-binary-smoke");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::phx(cx, &home, &["--version"]);
    t.check("--version exits 0", r.rc == 0);
    t.check(
        "version names the crate version",
        r.out.contains(env!("CARGO_PKG_VERSION")),
    );
    let r = util::phx(cx, &home, &["commands"]);
    t.check("commands exits 0", r.rc == 0);
    t.check("commands lists 60", r.out.contains("60 commands"));
    t.check("commands mentions serve", r.out.contains("serve"));
    t.check("commands mentions doctor", r.out.contains("doctor"));
    let r = util::phx(cx, &home, &["commands", "--json"]);
    t.check(
        "commands --json is valid JSON",
        serde_json::from_str::<serde_json::Value>(&r.out).is_ok(),
    );
    let bin = fs::read(&cx.phx).unwrap_or_default();
    t.check("no em dash in the binary", !util::has_em_dash(&bin));
    t.check(
        "no credential shapes in the binary",
        !util::has_cred_shapes(&bin),
    );
    let r = util::phx(cx, &home, &["notacommand"]);
    t.check("unknown command exits 2", r.rc == 2);
    t.check(
        "unknown command names itself",
        r.all().contains("unknown argument: notacommand"),
    );
    t
}

pub fn t02_init_config(cx: &Cx) -> T {
    let mut t = T::new("02-init-config");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::phx(cx, &home, &["status"]);
    t.check(
        "status before init says no nest",
        r.out.contains("no nest yet"),
    );
    let r = util::phx(cx, &home, &["init"]);
    t.check("init exits 0", r.rc == 0);
    let cfg = util::cfg_path(&home);
    t.check("config file exists", cfg.is_file());
    t.check("config file is mode 600", util::mode_of(&cfg) == 0o600);
    let r = util::phx(cx, &home, &["config", "check"]);
    t.check("config check says valid", r.out.contains("is valid"));
    let r = util::phx(cx, &home, &["config", "path"]);
    t.check(
        "config path prints the path",
        r.out.contains(".openphoenix/config.toml"),
    );
    let _ = util::append_s(&cfg, "api_key = \"sk-e2etestkey1234567890abcd\"\n");
    let r = util::phx(cx, &home, &["config", "show"]);
    t.check(
        "config show redacts the api key",
        r.out.contains("redacted"),
    );
    t.check(
        "config show never prints the key",
        !r.out.contains("sk-e2etestkey"),
    );
    let r = util::phx_env(
        cx,
        &home,
        &["status"],
        &[("PHOENIX_API_KEY", "sk-e2estatuskey1234")],
    );
    t.check("status with a key exits 0", r.rc == 0);
    t.check(
        "status shows serve as not running",
        r.out.contains("serve") && r.out.contains("not running"),
    );
    t.check(
        "status counts stored sessions",
        r.out.contains("sessions") && r.out.contains("none stored"),
    );
    t.check(
        "status admits updates were never checked",
        r.out.contains("never checked"),
    );
    let r = util::phx(cx, &home, &["config", "check"]);
    t.check(
        "config check flags the misplaced key",
        r.rc == 1 && r.all().contains("telegram.api_key"),
    );
    t.check(
        "config check points at the right table",
        r.all().contains("did you mean [provider] api_key?"),
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::write_config(&home, "not = valid = toml [[[\n");
    let r = util::phx(cx, &home, &["config", "check"]);
    t.check("invalid toml fails config check", r.rc == 2);
    t.check(
        "invalid toml names the error",
        r.all().contains("config error"),
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::phx(cx, &home, &["doctor"]);
    t.check("doctor runs without a config", r.rc == 0);
    t
}

pub fn t03_onboard_piped(cx: &Cx) -> T {
    let mut t = T::new("03-onboard-piped");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::run_in(&home, &cx.phx, &["configure"], &[], Some(b"\n"), 30_000);
    if r.rc == 124 {
        t.bad("piped onboarding must not hang (timed out)");
    } else {
        t.ok(&format!("piped onboarding exits (rc={})", r.rc));
    }
    let all = r.all();
    t.check(
        "onboarding offers ollama as the free route",
        all.contains("ollama"),
    );
    t.check(
        "onboarding names a signup URL or probes a provider",
        all.contains("https://") || all.contains("asking"),
    );
    t.check(
        "onboarding wrote or left a nest cleanly",
        util::cfg_path(&home).is_file() || !all.contains("panicked"),
    );
    t.check("no panic in onboarding output", !all.contains("panicked"));
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::run_in(&home, &cx.phx, &["configure"], &[], Some(b""), 30_000);
    if r.rc == 124 {
        t.bad("empty stdin onboarding must not hang");
    } else {
        t.ok(&format!("empty stdin onboarding exits (rc={})", r.rc));
    }
    t
}

pub fn t04_cli_commands(cx: &Cx) -> T {
    let mut t = T::new("04-cli-commands");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::phx(cx, &home, &["init"]);
    let cfg = util::cfg_path(&home);
    let _ = util::replace_in_file(&cfg, "# api_key = \"\"", "api_key = \"sk-e2e\"");
    for c in [
        "commands",
        "docs",
        "system",
        "channels",
        "directory",
        "exec-policy",
        "gateway",
        "webhooks",
        "security",
        "sessions",
        "tasks",
        "cron",
        "status",
    ] {
        let r = util::phx(cx, &home, &[c]);
        t.check(&format!("phoenix {c} exits 0"), r.rc == 0);
    }
    let r = util::phx(cx, &home, &["health"]);
    if r.rc <= 1 && r.rc >= 0 {
        t.ok(&format!(
            "health summarizes with rc {} (0 healthy, 1 unhealthy)",
            r.rc
        ));
    } else {
        t.bad(&format!("health unexpected rc={}", r.rc));
    }
    let r = util::phx(cx, &home, &["channels"]);
    t.check(
        "channels reports the roster",
        r.out.contains("channels configured"),
    );
    let r = util::phx(cx, &home, &["gateway"]);
    t.check(
        "gateway says the http door is closed",
        r.out.to_lowercase().contains("off"),
    );
    for sh in ["bash", "zsh", "fish"] {
        let r = util::phx(cx, &home, &["completion", sh]);
        t.check(&format!("completion {sh} exits 0"), r.rc == 0);
        t.check(
            &format!("completion {sh} is non-empty"),
            !r.out.trim().is_empty(),
        );
    }
    let r = util::phx(cx, &home, &["completion", "powershell"]);
    t.check("completion powershell is refused", r.rc != 0);
    let r = util::phx(cx, &home, &["board", "list"]);
    if r.rc == 0 || r.rc == 2 {
        t.ok(&format!("board list exits 0 or 2 (rc={})", r.rc));
    } else {
        t.bad(&format!("board list unexpected rc={}", r.rc));
    }
    t
}

pub fn t05_commands_reject_unknown(cx: &Cx) -> T {
    let mut t = T::new("05-commands-reject-unknown");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::phx(cx, &home, &["definitelynotacommand"]);
    t.check("unknown command exits 2", r.rc == 2);
    t.check(
        "unknown command prints an error",
        r.all().contains("unknown argument"),
    );
    let r = util::phx(cx, &home, &["statu"]);
    t.check(
        "near-miss suggests the real command",
        r.all().contains("did you mean"),
    );
    let r = util::phx(cx, &home, &["acp"]);
    t.check("not-built acp exits 2", r.rc == 2);
    t.check(
        "not-built acp names its reason",
        r.all().contains("not built here"),
    );
    let r = util::phx(cx, &home, &["nodes"]);
    t.check(
        "not-built nodes names its reason",
        r.all().contains("not built here"),
    );
    let r = util::phx(cx, &home, &["commands", "--json"]);
    let shaped = serde_json::from_str::<serde_json::Value>(&r.out)
        .ok()
        .and_then(|d| {
            let nb = d.get("not_built")?.as_array()?.clone();
            if nb.is_empty() {
                return None;
            }
            let all_reasons = nb.iter().all(|x| {
                x.get("reason")
                    .and_then(serde_json::Value::as_str)
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
            });
            all_reasons.then_some(())
        })
        .is_some();
    t.check("commands --json lists not_built reasons", shaped);
    t
}

pub fn t06_memory_ops(cx: &Cx) -> T {
    let mut t = T::new("06-memory-ops");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::phx(cx, &home, &["init"]);
    let r = util::phx(cx, &home, &["memory", "add", "e2e probe note"]);
    t.check("memory add notes it", r.out.contains("noted"));
    let r = util::phx(cx, &home, &["memory", "show"]);
    t.check(
        "memory show returns the note",
        r.out.contains("e2e probe note"),
    );
    let r = util::phx(cx, &home, &["memory", "search", "probe"]);
    t.check(
        "memory search finds the note",
        r.out.contains("e2e probe note"),
    );
    let mem = home.join(".openphoenix").join("memory.md");
    t.check("memory file exists", mem.is_file());
    t.check("memory file is mode 600", util::mode_of(&mem) == 0o600);
    let _ = util::phx(
        cx,
        &home,
        &[
            "memory",
            "add",
            "the token is ghp_AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIII",
        ],
    );
    t.check(
        "secrets never reach the memory file",
        !util::dir_has(&home.join(".openphoenix"), "ghp_AAAA"),
    );
    let memdir = home.join(".openphoenix").join("memory");
    t.check(
        "redaction marker written instead",
        util::file_has(&mem, "redacted") || util::dir_has(&memdir, "redacted"),
    );
    let r = util::phx(cx, &home, &["memory", "wipe"]);
    t.check("memory wipe clears it", r.out.contains("memory wiped"));
    let r = util::phx(cx, &home, &["memory", "show"]);
    t.check(
        "memory show is empty after wipe",
        r.out.contains("no memories"),
    );
    for mode in ["ghost", "session", "recall"] {
        let Some(home) = home_or(&mut t) else {
            return t;
        };
        let _ = util::phx(cx, &home, &["init"]);
        let cfg = util::cfg_path(&home);
        let _ = util::replace_line_prefix(&cfg, "privacy = ", &format!("privacy = \"{mode}\""));
        let r = util::phx(cx, &home, &["memory", "add", &format!("note in {mode}")]);
        t.check(
            &format!("operator memory add works in {mode} mode"),
            r.out.contains("noted"),
        );
    }
    t
}

pub fn t07_serve_startup(cx: &Cx) -> T {
    let mut t = T::new("07-serve-startup");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let r = util::phx(cx, &home, &["serve"]);
    t.check("serve without a config fails", r.rc == 2);
    t.check(
        "serve without a key names a signup page",
        r.all().contains("https://"),
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::write_config(
        &home,
        "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-e2e\"\n",
    );
    let r = util::phx(cx, &home, &["serve"]);
    t.check("serve with no channel and no http fails", r.rc == 2);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let port = util::free_port();
    let _ = util::write_config(
        &home,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-e2e\"\n\n[http]\nenabled = true\nport = {port}\n"
        ),
    );
    let r = util::phx(cx, &home, &["serve"]);
    t.check("http without token refuses to serve", r.rc == 2);
    t.check(
        "http without token names the fix",
        r.all().contains("http.token"),
    );
    let _ = util::append_s(&util::cfg_path(&home), "token = \"e2e-token\"\n");
    let sv = util::serve(cx, &home, &[]);
    match sv {
        Ok(mut sv) => {
            if util::wait_http(port, "/health") {
                t.ok("serve starts with http only");
            } else {
                t.bad(&format!("serve did not come up on port {port}"));
            }
            let log = util::read_s(&sv.log);
            t.check(
                "startup banner burns bright",
                log.contains("phoenix rising"),
            );
            t.check("http api announced", log.contains("http api on"));
            let r = util::phx(cx, &home, &["serve"]);
            t.check("second serve is locked out", r.rc == 2);
            t.check(
                "lock error names the pid",
                r.all().contains("already running"),
            );
            match sv.term() {
                Some(0) => t.ok("SIGTERM exits 0"),
                Some(rc) => t.bad(&format!("SIGTERM exit code {rc}")),
                None => t.bad("serve did not stop on SIGTERM"),
            }
            let log = util::read_s(&sv.log);
            t.check(
                "clean shutdown message",
                log.contains("shutting down cleanly"),
            );
        }
        Err(e) => t.bad(&format!("serve spawn: {e}")),
    }
    t
}

pub fn t08_http_api(cx: &Cx) -> T {
    let mut t = T::new("08-http-api");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock provider did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock provider did not start");
    }
    let port = util::free_port();
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(
        &util::cfg_path(&home),
        &format!("\n[http]\nenabled = true\nport = {port}\ntoken = \"e2e-token\"\n"),
    );
    let sv = util::serve(cx, &home, &[]);
    let Ok(mut sv) = sv else {
        t.bad("serve did not come up");
        return t;
    };
    if !util::wait_http(port, "/health") {
        t.bad("serve did not come up");
    }
    let auth = ("Authorization", "Bearer e2e-token");
    let (code, body) = util::http(port, "GET", "/health", &[], None).unwrap_or((0, String::new()));
    t.check(
        "health is open and true",
        code == 200 && body.contains("\"ok\":true"),
    );
    let code = util::http_code(port, "POST", "/run", &[], Some(br#"{"prompt":"hi"}"#));
    t.check(
        &format!("run without auth is 401 (got {code})"),
        code == 401,
    );
    let code = util::http_code(port, "POST", "/run", &[auth], None);
    t.check(
        &format!("run without body is 400 (got {code})"),
        code == 400,
    );
    let code = util::http_code(port, "POST", "/run", &[auth], Some(b"{}"));
    t.check(
        &format!("run without prompt is 400 (got {code})"),
        code == 400,
    );
    let (code, body) = util::http(
        port,
        "POST",
        "/run",
        &[auth],
        Some(br#"{"prompt":"say hello"}"#),
    )
    .unwrap_or((0, String::new()));
    t.check(
        "run with auth reaches the mock model",
        code == 200 && body.contains("mock reply"),
    );
    let (code, body) =
        util::http(port, "GET", "/robots.txt", &[], None).unwrap_or((0, String::new()));
    t.check(
        "robots.txt closes the door",
        code == 200 && body.contains("Disallow: /"),
    );
    let code = util::http_code(port, "GET", "/nope", &[auth], None);
    t.check(&format!("unknown path is 404 (got {code})"), code == 404);
    let code = util::http_code(
        port,
        "GET",
        "/ws",
        &[
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ("Sec-WebSocket-Version", "13"),
        ],
        None,
    );
    t.check(
        &format!("websocket upgrade refused without auth (got {code})"),
        code == 401,
    );
    let code = util::http_code(port, "POST", "/hook/e2e", &[], Some(b"{}"));
    if code != 500 && code != 0 {
        t.ok(&format!("webhook path routes (got {code})"));
    } else {
        t.bad(&format!("webhook path gave {code}"));
    }
    let _ = sv.term();
    t
}

pub fn t09_web_ui(cx: &Cx) -> T {
    let mut t = T::new("09-web-ui");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock provider did not start");
        return t;
    };
    let port = util::free_port();
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(
        &util::cfg_path(&home),
        &format!(
            "\n[http]\nenabled = true\nport = {port}\ntoken = \"e2e-token\"\nweb = true\nusername = \"admin\"\npassword = \"pw\"\n"
        ),
    );
    let sv = util::serve(cx, &home, &[]);
    let Ok(mut sv) = sv else {
        t.bad("serve did not come up");
        return t;
    };
    if !util::wait_http(port, "/health") {
        t.bad("serve did not come up");
    }
    let basic = format!("Basic {}", util::b64(b"admin:pw"));
    let auth = ("Authorization", basic.as_str());
    let (code, body) = util::http(port, "GET", "/", &[auth], None).unwrap_or((0, String::new()));
    let lower = body.to_lowercase();
    t.check(
        "web root serves HTML with credentials",
        code == 200 && (lower.contains("<html") || lower.contains("<!doctype")),
    );
    let (code, body) =
        util::http(port, "GET", "/style.css", &[auth], None).unwrap_or((0, String::new()));
    t.check("stylesheet served", code == 200 && body.contains("{"));
    let (code, body) =
        util::http(port, "GET", "/app.js", &[auth], None).unwrap_or((0, String::new()));
    let lower = body.to_lowercase();
    t.check(
        "app script served",
        code == 200 && (lower.contains("function") || body.contains("=>")),
    );
    let code = util::http_code(port, "GET", "/", &[], None);
    t.check(
        &format!("web root without auth is 401 (got {code})"),
        code == 401,
    );
    let wrong = format!("Basic {}", util::b64(b"admin:wrong"));
    let code = util::http_code(port, "GET", "/", &[("Authorization", wrong.as_str())], None);
    t.check(&format!("wrong password is 401 (got {code})"), code == 401);
    let _ = sv.term();
    let Some(home2) = home_or(&mut t) else {
        return t;
    };
    let port2 = util::free_port();
    let _ = util::write_config(
        &home2,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-e2e\"\n\n[http]\nenabled = true\nbind = \"0.0.0.0\"\nport = {port2}\ntoken = \"e2e-token\"\nweb = true\n"
        ),
    );
    let r = util::phx(cx, &home2, &["serve"]);
    t.check(
        "public bind without credentials refuses to serve",
        r.rc == 2,
    );
    t.check(
        "refusal names the unauthenticated UI",
        r.all().to_lowercase().contains("refusing"),
    );
    t
}

pub fn t10_ssrf_block(cx: &Cx) -> T {
    let mut t = T::new("10-ssrf-block");
    for target in [
        "http://127.0.0.1:9999/",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
    ] {
        let Some(home) = home_or(&mut t) else {
            return t;
        };
        let opts = mocks::ProviderOpts {
            tool_call: format!("http_get:{{\"url\":\"{target}\"}}"),
            ..Default::default()
        };
        let Ok(mock) = mocks::provider(opts) else {
            t.bad("mock did not start");
            continue;
        };
        if !util::wait_http(mock.port(), "/health") {
            t.bad("mock did not start");
        }
        let _ = util::mock_config(
            &home,
            &format!("http://127.0.0.1:{}", mock.port()),
            "mock-model",
        );
        let _ = util::phx(cx, &home, &["run", "fetch the url"]);
        let bodies = mock.bodies_text();
        if bodies.contains("private, loopback") || bodies.contains("special-use") {
            t.ok(&format!("SSRF guard blocked {target}"));
        } else {
            t.bad(&format!("SSRF guard let {target} through"));
        }
    }
    t
}
