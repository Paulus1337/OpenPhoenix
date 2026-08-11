use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

pub fn t11_secret_scrub(cx: &Cx) -> T {
    let mut t = T::new("11-secret-scrub");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::write_config(
        &home,
        "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-verysecretkey12345678\"\nbase_url = \"https://user:supersecretpw@example.invalid/v1\"\n\n[telegram]\ntoken = \"123456:ABCDEFtelegramtoken\"\nallowed_chat_ids = [1]\n",
    );
    let show = util::phx(cx, &home, &["config", "show"]).all();
    t.check("config show redacts something", show.contains("redacted"));
    t.check("api key never printed", !show.contains("sk-verysecretkey"));
    t.check(
        "telegram token never printed",
        !show.contains("ABCDEFtelegramtoken"),
    );
    t.check(
        "password inside URL never printed",
        !show.contains("supersecretpw"),
    );
    let _ = util::phx(
        cx,
        &home,
        &["memory", "add", "my key is sk-verysecretkey12345678"],
    );
    let mem = home.join(".openphoenix").join("memory.md");
    let memdir = home.join(".openphoenix").join("memory");
    t.check(
        "memory never stores the key",
        !util::file_has(&mem, "sk-verysecretkey") && !util::dir_has(&memdir, "sk-verysecretkey"),
    );
    let r = util::run_in(
        &home,
        &cx.phx,
        &["secret", "set", "E2ETOK"],
        &[("PHOENIX_SECRET_KEY", "e2e-master-key")],
        Some(b"hunter2value\n"),
        30_000,
    );
    let _ = r;
    let sec = home.join(".openphoenix").join("secrets.enc");
    t.check("secret store created", sec.is_file());
    t.check("secret store is mode 600", util::mode_of(&sec) == 0o600);
    t.check(
        "secret value not stored in plaintext",
        !util::file_has(&sec, "hunter2value"),
    );
    t
}

pub fn t12_fail_closed(cx: &Cx) -> T {
    let mut t = T::new("12-fail-closed");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::phx(cx, &home, &["init"]);
    let cfg = util::read_s(&util::cfg_path(&home));
    t.check(
        "default config keeps http off",
        !util::kv_line(&cfg, "enabled", "true"),
    );
    t.check(
        "default config has no mcp servers",
        !cfg.contains("mcp.servers"),
    );
    t.check(
        "default config binds nothing to 0.0.0.0",
        !util::kv_line(&cfg, "bind", "\"0.0.0.0\""),
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::write_config(
        &home,
        "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-e2e\"\n\n[telegram]\ntoken = \"123456:FAKEE2E\"\nallowed_chat_ids = []\n",
    );
    let r = util::run_in(&home, &cx.phx, &["serve"], &[], None, 30_000);
    t.check(
        "empty telegram allowlist refuses everyone",
        r.all().contains("refusing to serve everyone"),
    );
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
    t.check("http without token cannot serve", r.rc == 2);
    t
}

pub fn t13_migration(cx: &Cx) -> T {
    let mut t = T::new("13-migration");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let claw = home.join(".previous-gateway");
    let _ = fs::create_dir_all(claw.join("workspace").join("memory"));
    let cfgj = claw.join("gateway.json");
    let secj = claw.join("secrets.json");
    let _ = util::write_s(
        &cfgj,
        "{\n  \"agents\": {\"defaults\": {\n    \"model\": {\"primary\": \"anthropic/claude-sonnet-5\", \"fallbacks\": [\"openai/gpt-5.2\"]},\n    \"workspace\": \"~/.previous-gateway/workspace\"\n  }},\n  \"channels\": {\"telegram\": {\"botToken\": \"123456:MIGRATEME\", \"allowFrom\": [1868769425]}}\n}\n",
    );
    let _ = util::write_s(
        &secj,
        "{\"telegram\": {\"botToken\": \"123456:MIGRATEME\"}}\n",
    );
    let _ = util::write_s(&claw.join("workspace").join("SOUL.md"), "# SOUL\n");
    let _ = util::write_s(&claw.join("workspace").join("MEMORY.md"), "- fact\n");
    let mut both = fs::read(&cfgj).unwrap_or_default();
    both.extend(fs::read(&secj).unwrap_or_default());
    let sum_before = util::sha256_hex(&both);
    let mtime_before = fs::metadata(&cfgj).and_then(|m| m.modified()).ok();
    let r = util::phx(cx, &home, &["migrate"]);
    if r.rc == 0 {
        t.ok("migrate dry-run exits 0");
    } else {
        let head: String = r.all().lines().take(2).collect::<Vec<_>>().join(" | ");
        t.bad(&format!("migrate dry-run rc={}: {head}", r.rc));
    }
    t.check(
        "dry-run does not write a config",
        !util::cfg_path(&home).is_file(),
    );
    let r = util::phx(cx, &home, &["migrate", "--write"]);
    if r.rc == 0 {
        t.ok("migrate --write exits 0");
    } else {
        let head: String = r.all().lines().take(2).collect::<Vec<_>>().join(" | ");
        t.bad(&format!("migrate --write rc={}: {head}", r.rc));
    }
    let cfg = util::cfg_path(&home);
    t.check("migrated config exists", cfg.is_file());
    let r = util::phx(cx, &home, &["config", "check"]);
    t.check("migrated config is valid", r.out.contains("is valid"));
    t.check(
        "model choice carried over",
        util::file_has(&cfg, "claude-sonnet-5"),
    );
    let mut both = fs::read(&cfgj).unwrap_or_default();
    both.extend(fs::read(&secj).unwrap_or_default());
    t.check(
        "old nest bytes untouched",
        util::sha256_hex(&both) == sum_before,
    );
    let mtime_after = fs::metadata(&cfgj).and_then(|m| m.modified()).ok();
    t.check(
        "old nest mtime untouched",
        mtime_before.is_some() && mtime_before == mtime_after,
    );
    t
}

pub fn t14_session_quarantine(cx: &Cx) -> T {
    let mut t = T::new("14-session-quarantine");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("provider mock did not start");
        return t;
    };
    let Ok(tel) = mocks::telegram(1, "quarantine run", None) else {
        t.bad("telegram mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("provider mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(
        &util::cfg_path(&home),
        "\n[log]\nlevel = \"warn\"\n\n[agent]\nsessions = true\n\n[telegram]\ntoken = \"123456:FAKEE2E\"\nallowed_chat_ids = [1]\n",
    );
    let sess = home.join(".openphoenix").join("sessions");
    let _ = fs::create_dir_all(&sess);
    let _ = util::write_s(&sess.join("1.json"), "this is not json {{{\n");
    let api = format!("http://127.0.0.1:{}", tel.port());
    let sv = util::serve(cx, &home, &[("PHOENIX_TELEGRAM_API", api.as_str())]);
    let Ok(mut sv) = sv else {
        t.bad("serve spawn failed");
        return t;
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut got = false;
    while Instant::now() < deadline {
        if tel.count(|e| e["method"] == "sendMessage") > 0 {
            got = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if got {
        t.ok("serve processed the message");
    } else {
        t.bad("no reply reached telegram mock");
    }
    let names: Vec<String> = fs::read_dir(&sess)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    t.check(
        "corrupt transcript was quarantined",
        names.iter().any(|n| n.contains("corrupt")),
    );
    t.check(
        "quarantine was reported",
        util::file_has(&sv.log, "quarantined and started fresh"),
    );
    t.check(
        "valid fresh session written",
        names.iter().any(|n| n == "1.json"),
    );
    let _ = sv.term();
    t
}

pub fn t15_file_permissions(cx: &Cx) -> T {
    let mut t = T::new("15-file-permissions");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let _ = util::phx(cx, &home, &["init"]);
    t.check(
        "config is 600 after init",
        util::mode_of(&util::cfg_path(&home)) == 0o600,
    );
    let _ = util::phx(cx, &home, &["memory", "add", "perm probe"]);
    t.check(
        "memory file is 600",
        util::mode_of(&home.join(".openphoenix").join("memory.md")) == 0o600,
    );
    let _ = util::run_in(
        &home,
        &cx.phx,
        &["secret", "set", "PERMTOK"],
        &[("PHOENIX_SECRET_KEY", "e2e-master-key")],
        Some(b"v\n"),
        30_000,
    );
    t.check(
        "secret store is 600",
        util::mode_of(&home.join(".openphoenix").join("secrets.enc")) == 0o600,
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let opts = mocks::ProviderOpts {
        tool_call: "write_file:{\"path\":\"/etc/phx-evil-e2e\",\"content\":\"nope\"}".to_string(),
        ..Default::default()
    };
    let Ok(mock) = mocks::provider(opts) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::phx(cx, &home, &["run", "write the file"]);
    t.check(
        "absolute path outside workspace refused",
        !std::path::Path::new("/etc/phx-evil-e2e").exists(),
    );
    t.check(
        "jail error reported to the model",
        mock.log_text().contains("workspace"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let opts = mocks::ProviderOpts {
        tool_call: "write_file:{\"path\":\"in-jail-e2e.txt\",\"content\":\"hello jail\"}"
            .to_string(),
        ..Default::default()
    };
    let Ok(mock) = mocks::provider(opts) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::phx(cx, &home, &["run", "write the file"]);
    t.check(
        "workspace-relative write lands in the workspace",
        util::dir_has(&home.join("phoenix"), "hello jail"),
    );
    t
}

pub fn t16_update(cx: &Cx) -> T {
    let mut t = T::new("16-update");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(work) = util::tmpdir("phx-upd-work") else {
        t.bad("no work dir");
        return t;
    };
    let ut = work.join("phoenix-under-test");
    let _ = fs::copy(&cx.phx, &ut);
    util::chmod(&ut, 0o755);
    let Ok(rdir) = util::tmpdir("phx-upd-rel") else {
        t.bad("no release dir");
        return t;
    };
    let Ok(rel) = mocks::release(rdir.clone()) else {
        t.bad("release mock did not start");
        return t;
    };
    if !util::wait_http(rel.port(), "/repos/x/releases/latest") {
        t.bad("release mock did not start");
    }
    let base = format!("http://127.0.0.1:{}", rel.port());
    let envs: [(&str, &str); 1] = [("PHOENIX_UPDATE_BASE", base.as_str())];
    let asset = rdir.join("phoenix-linux-x86_64");
    let sums = rdir.join("SHA256SUMS");
    let orig = fs::read(&cx.phx).unwrap_or_default();
    let mut new_bin = orig.clone();
    new_bin.extend_from_slice(b"\ne2e-marker");
    let good_sum = util::sha256_hex(&new_bin);
    let _ = fs::write(&asset, &new_bin);
    let _ = util::write_s(
        &sums,
        &format!("{}  phoenix-linux-x86_64\n", "0".repeat(64)),
    );
    let before = util::sha256_hex(&fs::read(&ut).unwrap_or_default());
    let r = util::run_in(&home, &ut, &["update"], &envs, None, 120_000);
    t.check(
        &format!("checksum mismatch rejected (rc={})", r.rc),
        r.rc != 0,
    );
    let after = util::sha256_hex(&fs::read(&ut).unwrap_or_default());
    t.check("binary untouched after bad checksum", before == after);
    let trunc: &[u8] = if new_bin.len() >= 1000 {
        &new_bin[..1000]
    } else {
        &new_bin[..]
    };
    let _ = fs::write(&asset, trunc);
    let _ = util::write_s(&sums, &format!("{good_sum}  phoenix-linux-x86_64\n"));
    let r = util::run_in(&home, &ut, &["update"], &envs, None, 120_000);
    t.check(
        &format!("truncated download rejected (rc={})", r.rc),
        r.rc != 0,
    );
    let after = util::sha256_hex(&fs::read(&ut).unwrap_or_default());
    t.check("binary untouched after truncation", before == after);
    let _ = fs::write(&asset, &new_bin);
    let _ = util::write_s(&sums, &format!("{good_sum}  phoenix-linux-x86_64\n"));
    let r = util::run_in(&home, &ut, &["update"], &envs, None, 120_000);
    if r.rc == 0 {
        t.ok("valid checksummed update succeeds");
    } else {
        let head: String = r.all().lines().take(2).collect::<Vec<_>>().join(" | ");
        t.bad(&format!("valid update failed: {head}"));
    }
    t.check("update announces the rebirth", r.all().contains("reborn"));
    let cache = home.join(".openphoenix").join("update-check.json");
    t.check("update wrote the check cache", cache.is_file());
    t.check(
        "check cache records up to date",
        fs::read_to_string(&cache)
            .unwrap_or_default()
            .contains("\"up_to_date\":true"),
    );
    let after = util::sha256_hex(&fs::read(&ut).unwrap_or_default());
    t.check("binary swapped to the served build", after == good_sum);
    let r = util::run_in(&home, &ut, &["--version"], &[], None, 30_000);
    t.check(
        "swapped binary still runs",
        r.out.contains(env!("CARGO_PKG_VERSION")),
    );
    let cur = fs::read(&ut).unwrap_or_default();
    let cur_sum = util::sha256_hex(&cur);
    let _ = fs::write(&asset, &cur);
    let _ = util::write_s(&sums, &format!("{cur_sum}  phoenix-linux-x86_64\n"));
    let r = util::run_in(&home, &ut, &["update"], &envs, None, 120_000);
    t.check(
        &format!("same-version update exits 0 (rc={})", r.rc),
        r.rc == 0,
    );
    t.check(
        "same version is recognized",
        r.all().contains("already flying"),
    );
    t
}

pub fn t17_mock_provider(cx: &Cx) -> T {
    let mut t = T::new("17-mock-provider");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let r = util::phx(cx, &home, &["run", "say hello"]);
    t.check(
        "run returns the canned reply",
        r.out.contains("mock reply model=mock-model"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts {
        fail_status: 500,
        ..Default::default()
    }) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let r = util::run_in(&home, &cx.phx, &["run", "hi"], &[], None, 180_000);
    t.check(
        &format!("provider 500 surfaces as failure (rc={})", r.rc),
        r.rc != 0 && r.rc != 124,
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts {
        fail_model: "primary-model".to_string(),
        ..Default::default()
    }) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "primary-model",
    );
    let _ = util::append_s(&util::cfg_path(&home), "fallbacks = [\"fallback-model\"]\n");
    let r = util::run_in(&home, &cx.phx, &["run", "hi"], &[], None, 180_000);
    t.check(
        "rate-limited primary falls back",
        r.out.contains("model=fallback-model"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::write_config(
        &home,
        &format!(
            "[provider]\nkind = \"nvidia\"\nmodel = \"nvidia/test-model\"\napi_key = \"sk-e2e\"\nbase_url = \"http://127.0.0.1:{}/v1\"\n",
            mock.port()
        ),
    );
    let _ = util::phx(cx, &home, &["run", "hi"]);
    t.check(
        "namespaced model id reaches the wire intact",
        mock.log_text().contains("nvidia/test-model"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(&util::cfg_path(&home), "api = \"openai-responses\"\n");
    let _ = util::phx(cx, &home, &["run", "hi"]);
    t.check(
        "responses dialect posts to /responses",
        mock.count(|e| e["path"] == "/v1/responses") > 0,
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(&util::cfg_path(&home), "api = \"anthropic-messages\"\n");
    let _ = util::phx(cx, &home, &["run", "hi"]);
    t.check(
        "anthropic dialect posts to /messages",
        mock.count(|e| e["path"] == "/v1/messages") > 0,
    );
    t.check(
        "anthropic-version header sent",
        mock.count(|e| e["headers"]["anthropic-version"].is_string()) > 0,
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::write_config(
        &home,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\nbase_url = \"http://127.0.0.1:{}/v1\"\n",
            mock.port()
        ),
    );
    let _ = util::phx_env(
        cx,
        &home,
        &["run", "hi"],
        &[("PHOENIX_API_KEY", "sekret-e2e-env-key")],
    );
    t.check(
        "env key rides the auth header",
        mock.count(|e| e["headers"]["authorization"] == "Bearer sekret-e2e-env-key") > 0,
    );
    t.check(
        "env key never leaks into the body",
        !mock.bodies_text().contains("sekret-e2e-env-key"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts {
        tool_call: "mcp_echoer_echo:{\"text\":\"fenced hello\"}".to_string(),
        ..Default::default()
    }) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let me = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/usr/local/bin/phoenix-e2e".to_string());
    let _ = util::append_s(
        &util::cfg_path(&home),
        &format!("\n[mcp.servers.echoer]\ncommand = \"{me}\"\nargs = [\"mcp-echo\"]\n"),
    );
    let _ = util::phx(cx, &home, &["run", "use the mcp tool"]);
    t.check(
        "mcp tool result reaches the model",
        mock.log_text().contains("mcp echo: fenced hello"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts {
        fail_model: "primary-model".to_string(),
        ..Default::default()
    }) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "primary-model",
    );
    let _ = util::append_s(&util::cfg_path(&home), "fallbacks = [\"fallback-model\"]\n");
    let r = util::run_in(
        &home,
        &cx.phx,
        &["models", "--test-fallback"],
        &[],
        None,
        180_000,
    );
    t.check(
        "test-fallback fails the broken primary and exits 1",
        r.rc == 1 && r.out.contains("FAIL primary"),
    );
    t.check(
        "test-fallback proves the fallback answers",
        r.out.contains("ok   fallback"),
    );
    drop(mock);
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let r = util::run_in(
        &home,
        &cx.phx,
        &["models", "--test-fallback"],
        &[],
        None,
        180_000,
    );
    t.check(
        "test-fallback with no fallbacks tests the primary and exits 0",
        r.rc == 0
            && r.out.contains("no fallbacks configured")
            && r.out.contains("fallback chain healthy"),
    );
    t
}

pub fn t18_doctor(cx: &Cx) -> T {
    let mut t = T::new("18-doctor");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let workspace = home.join("workspace");
    let _ = fs::create_dir_all(&workspace);
    let _ = util::write_config(
        &home,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"gpt-5.3-chat-latest\"\n\n[agent]\nworkspace = \"{}\"\n",
            workspace.display()
        ),
    );
    let r = util::phx(cx, &home, &["doctor", "--json"]);
    let doc = serde_json::from_str::<serde_json::Value>(&r.out).ok();
    let shaped = doc
        .as_ref()
        .and_then(|d| d.get("findings"))
        .and_then(serde_json::Value::as_array);
    let finding_count = shaped.map_or(usize::MAX, Vec::len);
    let non_ok = shaped.map_or(usize::MAX, |items| {
        items
            .iter()
            .filter(|item| item["level"].as_str() != Some("ok"))
            .count()
    });
    t.check("hermetic doctor exits 0", r.rc == 0);
    t.check(
        "doctor --json is valid and shaped",
        doc.is_some() && shaped.is_some(),
    );
    t.check(
        "hermetic doctor reports zero findings",
        finding_count > 0 && non_ok == 0,
    );
    t.check(
        "doctor reports the hermetic config path",
        doc.as_ref().and_then(|d| d["config_path"].as_str())
            == Some(util::cfg_path(&home).to_string_lossy().as_ref()),
    );
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let port = util::free_port();
    let _ = util::write_config(
        &home,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"mock-model\"\napi_key = \"sk-e2e\"\n\n[http]\nenabled = true\nbind = \"0.0.0.0\"\nport = {port}\ntoken = \"e2e-token\"\n"
        ),
    );
    let r = util::phx(cx, &home, &["doctor"]);
    t.check(
        "doctor flags the public bind",
        r.out.contains("reachable from the network"),
    );
    let _ = util::append_s(&util::cfg_path(&home), "web = true\n");
    let r = util::phx(cx, &home, &["doctor"]);
    t.check(
        "doctor fails the web UI without credentials",
        util::line_starts(&r.out, "FAIL"),
    );
    util::chmod(&util::cfg_path(&home), 0o644);
    let r = util::phx(cx, &home, &["doctor"]);
    let lower = r.out.to_lowercase();
    t.check(
        "doctor warns on loose config permissions",
        lower.contains("mode 644") || lower.contains("chmod 600"),
    );
    util::chmod(&util::cfg_path(&home), 0o600);
    t
}

pub fn t19_telegram_poll(cx: &Cx) -> T {
    let mut t = T::new("19-telegram-poll");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("provider mock did not start");
        return t;
    };
    let Ok(tel) = mocks::telegram(1, "hello pip", None) else {
        t.bad("telegram mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("provider mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "mock-model",
    );
    let _ = util::append_s(
        &util::cfg_path(&home),
        "\n[log]\nlevel = \"info\"\n\n[telegram]\ntoken = \"123456:FAKEE2E\"\nallowed_chat_ids = [1]\n",
    );
    let api = format!("http://127.0.0.1:{}", tel.port());
    let sv = util::serve(cx, &home, &[("PHOENIX_TELEGRAM_API", api.as_str())]);
    let Ok(mut sv) = sv else {
        t.bad("serve spawn failed");
        return t;
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut got = false;
    while Instant::now() < deadline {
        if tel.count(|e| e["method"] == "sendMessage") > 0 {
            got = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if got {
        t.ok("polling delivered a reply end to end");
    } else {
        t.bad("no sendMessage reached the mock");
    }
    t.check(
        "startup probes getMe",
        tel.count(|e| e["method"] == "getMe") > 0,
    );
    t.check(
        "startup clears any stale webhook",
        tel.count(|e| e["method"] == "deleteWebhook") > 0,
    );
    t.check(
        "processed update confirmed with advanced offset",
        tel.count(|e| e["method"] == "getUpdates" && e["params"]["offset"] == "8") > 0,
    );
    t.check(
        "reply carries the model output",
        tel.count(|e| {
            e["method"] == "sendMessage" && e["params"].to_string().contains("mock reply")
        }) > 0,
    );
    t.check(
        "typing action sent",
        tel.count(|e| e["method"] == "sendChatAction") > 0,
    );
    t.check(
        "structured startup announces telegram",
        util::file_has(&sv.log, "serving as @e2ebot"),
    );
    let _ = sv.term();
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock2) = mocks::provider(mocks::ProviderOpts::default()) else {
        t.bad("provider mock did not start");
        return t;
    };
    let Ok(tel2) = mocks::telegram(1, "hello pip", Some(5)) else {
        t.bad("telegram mock did not start");
        return t;
    };
    if !util::wait_http(mock2.port(), "/health") {
        t.bad("provider mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock2.port()),
        "mock-model",
    );
    let _ = util::append_s(
        &util::cfg_path(&home),
        "\n[telegram]\ntoken = \"123456:FAKEE2E\"\nallowed_chat_ids = [1]\ngroup_mention_only = false\n",
    );
    let api2 = format!("http://127.0.0.1:{}", tel2.port());
    let sv2 = util::serve(cx, &home, &[("PHOENIX_TELEGRAM_API", api2.as_str())]);
    let Ok(mut sv2) = sv2 else {
        t.bad("serve spawn failed");
        return t;
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut got = false;
    while Instant::now() < deadline {
        if tel2.count(|e| e["method"] == "sendMessage") > 0 {
            got = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    t.check("topic message answered end to end", got);
    t.check(
        "reply lands in the forum topic",
        tel2.count(|e| e["method"] == "sendMessage" && e["params"]["message_thread_id"] == "5") > 0,
    );
    t.check(
        "typing lands in the forum topic",
        tel2.count(|e| e["method"] == "sendChatAction" && e["params"]["message_thread_id"] == "5")
            > 0,
    );
    let _ = sv2.term();
    t
}

pub fn t20_colab(cx: &Cx) -> T {
    let mut t = T::new("18-colab");
    let Some(home) = home_or(&mut t) else {
        return t;
    };
    let Ok(mock) = mocks::provider(mocks::ProviderOpts {
        converge: true,
        ..Default::default()
    }) else {
        t.bad("provider mock did not start");
        return t;
    };
    if !util::wait_http(mock.port(), "/health") {
        t.bad("provider mock did not start");
    }
    let _ = util::mock_config(
        &home,
        &format!("http://127.0.0.1:{}", mock.port()),
        "model-a",
    );
    let r = util::run_in(
        &home,
        &cx.phx,
        &["chat"],
        &[],
        Some(
            b"/colab on openai/model-b
do the team task
/colab off
do the solo task
/exit
",
        ),
        120_000,
    );
    t.check("colab lifecycle exits cleanly", r.rc == 0);
    t.check("colab was enabled", r.out.contains("Colab is ON"));
    t.check("colab was disabled", r.out.contains("colab is now off"));
    let models: Vec<String> = mock
        .log
        .lock()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["model"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let first_partner = models.iter().position(|model| model == "model-b");
    let last_main = models.iter().rposition(|model| model == "model-a");
    t.check(
        "main model ran",
        models.iter().any(|model| model == "model-a"),
    );
    t.check(
        "partner model ran while colab was on",
        first_partner.is_some(),
    );
    t.check(
        "a later solo turn used only the main model",
        matches!((first_partner, last_main), (Some(partner), Some(main)) if main > partner)
            && models.last().map(String::as_str) == Some("model-a"),
    );
    t
}
