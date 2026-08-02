use serde_json::Value;

pub fn resolve_secret_token(v: &Value, gateway_dir: &std::path::Path) -> Option<String> {
    let tok = &v["channels"]["telegram"]["botToken"];
    if tok.get("source")?.as_str()? != "file" {
        return None;
    }
    let id = tok.get("id")?.as_str()?;
    let raw = std::fs::read_to_string(gateway_dir.join("secrets.json")).ok()?;
    let secrets: Value = serde_json::from_str(&raw).ok()?;
    let s = secrets.pointer(id)?.as_str()?;
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

pub struct Migration {
    pub toml: String,
    pub notes: Vec<String>,
    pub secrets: Vec<(String, String)>,
}

fn known_provider(kind: &str) -> bool {
    matches!(kind, "anthropic" | "openai" | "openrouter" | "ollama")
}

fn compat_base_url(kind: &str) -> Option<&'static str> {
    match kind {
        "nvidia" => Some("https://integrate.api.nvidia.com/v1"),
        "groq" => Some("https://api.groq.com/openai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        _ => None,
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| toml_str(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn map_model(id: &str, notes: &mut Vec<String>) -> (String, String, String) {
    let Some((prefix, rest)) = id.split_once('/') else {
        notes.push(format!(
            "model '{id}' has no provider prefix; kept as-is under provider \"anthropic\": adjust if wrong"
        ));
        return ("anthropic".into(), id.to_string(), String::new());
    };
    if known_provider(prefix) {
        return (prefix.to_string(), rest.to_string(), String::new());
    }
    if let Some(url) = compat_base_url(prefix) {
        notes.push(format!(
            "provider '{prefix}' mapped to an OpenAI-compatible endpoint ({url})"
        ));
        return (prefix.to_string(), rest.to_string(), url.to_string());
    }
    notes.push(format!(
        "provider '{prefix}' is not built in; set provider.base_url to its OpenAI-compatible endpoint"
    ));
    (prefix.to_string(), rest.to_string(), String::new())
}

pub fn primary_provider(v: &Value) -> String {
    let primary = v["agents"]["defaults"]["model"]["primary"]
        .as_str()
        .unwrap_or("");
    if primary.is_empty() {
        return String::new();
    }
    let mut notes = Vec::new();
    map_model(primary, &mut notes).0
}

pub fn resolve_api_key(v: &Value, gateway_dir: &std::path::Path, provider: &str) -> Option<String> {
    let vars = crate::config::provider_key_vars(provider);
    if let Some(env_obj) = v.get("env").and_then(|e| e.as_object()) {
        for var in vars {
            if let Some(s) = env_obj.get(*var).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    let raw = std::fs::read_to_string(gateway_dir.join("gateway.systemd.env")).ok()?;
    for line in raw.lines() {
        if let Some((k, val)) = line.trim().split_once('=') {
            let val = val.trim();
            if !val.is_empty() && vars.iter().any(|x| *x == k.trim()) {
                return Some(val.to_string());
            }
        }
    }
    None
}

pub fn collect_keys(gateway_dir: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let secrets: Value = std::fs::read_to_string(gateway_dir.join("secrets.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    let mut entries: Vec<(String, String, String)> = Vec::new();
    if let Some(agents) = secrets.get("authProfiles").and_then(|a| a.as_object()) {
        for profiles in agents.values() {
            if let Some(profiles) = profiles.as_object() {
                for (name, body) in profiles {
                    let Some(key) = body.get("key").and_then(|k| k.as_str()) else {
                        continue;
                    };
                    if key.is_empty() {
                        continue;
                    }
                    let provider = name.split_once(':').map(|(p, _)| p).unwrap_or(name);
                    entries.push((provider.to_string(), name.clone(), key.to_string()));
                }
            }
        }
    }
    entries.sort_by(|a, b| (&a.0, natural(&a.1)).cmp(&(&b.0, natural(&b.1))));
    if let Ok(envf) = std::fs::read_to_string(gateway_dir.join("gateway.systemd.env")) {
        for line in envf.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, val)) = line.split_once('=') {
                let k = k.trim();
                let val = val.trim().trim_matches(['"', '\''].as_slice());
                if val.is_empty() {
                    continue;
                }
                if k == "CLAUDE_CODE_OAUTH_TOKEN" {
                    entries.insert(
                        0,
                        ("anthropic".into(), "anthropic:oauth".into(), val.into()),
                    );
                    continue;
                }
                for kind in crate::config::PROVIDER_KINDS {
                    if crate::config::provider_key_vars(kind).contains(&k) {
                        let name = format!("{kind}:env");
                        let already = entries.iter().any(|(p, _, v)| p == kind && v == val);
                        if !already {
                            entries.push((kind.to_string(), name, val.to_string()));
                        }
                        break;
                    }
                }
            }
        }
    }
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (provider, _, key) in entries {
        match out.iter_mut().find(|(p, _)| *p == provider) {
            Some((_, keys)) => {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            None => out.push((provider, vec![key])),
        }
    }
    out
}

fn natural(s: &str) -> (String, u64) {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let stem: String = s.chars().filter(|c| !c.is_ascii_digit()).collect();
    (stem, digits.parse().unwrap_or(0))
}

pub fn set_primary(toml: &str, spec: &str) -> String {
    let (kind, model) = match spec.split_once('/') {
        Some((k, m)) if crate::config::known_kind(k) => (Some(k), m),
        _ => (None, spec),
    };
    let mut out = String::new();
    for line in toml.lines() {
        if line.starts_with("model = ") {
            out.push_str(&format!("model = {}\n", toml_str(model)));
            continue;
        }
        if let Some(k) = kind {
            if line.starts_with("kind = ") {
                out.push_str(&format!("kind = {}\n", toml_str(k)));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

pub fn set_fallbacks(toml: &str, list: &[String]) -> String {
    let has_line = toml.lines().any(|l| l.starts_with("fallbacks = "));
    let mut out = String::new();
    let mut written = false;
    for line in toml.lines() {
        if line.starts_with("fallbacks = ") {
            if !written && !list.is_empty() {
                out.push_str(&format!("fallbacks = {}\n", toml_list(list)));
            }
            written = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if !has_line && !written && line.starts_with("model = ") && !list.is_empty() {
            out.push_str(&format!("fallbacks = {}\n", toml_list(list)));
            written = true;
        }
    }
    out
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

const PERSONA_FILES: [&str; 5] = ["SOUL.md", "AGENTS.md", "USER.md", "IDENTITY.md", "TOOLS.md"];

pub fn gateway_workspace(v: &Value, gateway_dir: &std::path::Path) -> std::path::PathBuf {
    let raw = v["agents"]["defaults"]["workspace"].as_str().unwrap_or("");
    if raw.is_empty() {
        gateway_dir.join("workspace")
    } else {
        crate::config::expanduser(raw)
    }
}

pub fn carry_persona(old_workspace: &std::path::Path, workspace: &std::path::Path) -> Vec<String> {
    use std::fs;
    let mut notes = Vec::new();
    if !old_workspace.is_dir() {
        return notes;
    }
    let persona_dir = workspace.to_path_buf();
    let mut carried: Vec<&str> = Vec::new();
    for name in PERSONA_FILES {
        let src = old_workspace.join(name);
        let Ok(content) = fs::read(&src) else {
            continue;
        };
        if fs::create_dir_all(&persona_dir).is_err() {
            break;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&persona_dir, fs::Permissions::from_mode(0o700));
        }
        let dst = persona_dir.join(name);
        if fs::write(&dst, &content).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o600));
            }
            carried.push(name);
        }
    }
    if !carried.is_empty() {
        notes.push(format!("persona carried: {}", carried.join(", ")));
    }

    if let Ok(mem) = fs::read_to_string(old_workspace.join("MEMORY.md")) {
        let dst = workspace.join("MEMORY.md");
        let existing = fs::read_to_string(&dst).unwrap_or_default();
        if existing.contains(mem.trim()) && !mem.trim().is_empty() {
        } else {
            let merged = if existing.is_empty() {
                mem
            } else {
                format!("{existing}\n# Migrated from OpenClaw MEMORY.md\n{mem}")
            };
            let _ = fs::create_dir_all(workspace);
            if fs::write(&dst, merged).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o600));
                }
                notes.push("MEMORY.md carried into phoenix memory".into());
            }
        }
    }

    let src_daily = old_workspace.join("memory");
    if let Ok(entries) = fs::read_dir(&src_daily) {
        let dst_daily = workspace.join("memory");
        let mut copied = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Some(name) = p.file_name() else { continue };
            let dst = dst_daily.join(name);
            if dst.exists() {
                continue;
            }
            if fs::create_dir_all(&dst_daily).is_err() {
                break;
            }
            if let Ok(content) = fs::read(&p) {
                if fs::write(&dst, content).is_ok() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o600));
                    }
                    copied += 1;
                }
            }
        }
        if copied > 0 {
            notes.push(format!("daily notes carried: {copied} file(s)"));
        }
    }
    notes
}

pub fn from_gateway(v: &Value) -> Migration {
    let mut notes = Vec::new();
    let mut secrets: Vec<(String, String)> = Vec::new();
    let mut out = String::from("# Generated by `phoenix migrate` from an AI gateway config.\n");

    let primary = v["agents"]["defaults"]["model"]["primary"]
        .as_str()
        .unwrap_or("");
    let (provider, model, base_url) = if primary.is_empty() {
        notes.push("no agents.defaults.model.primary found; provider left at defaults".into());
        (String::new(), String::new(), String::new())
    } else {
        map_model(primary, &mut notes)
    };
    out.push_str("\n[provider]\n");
    if !provider.is_empty() {
        out.push_str(&format!("kind = {}\n", toml_str(&provider)));
        out.push_str(&format!("model = {}\n", toml_str(&model)));
    }
    if !base_url.is_empty() {
        out.push_str(&format!("base_url = {}\n", toml_str(&base_url)));
    }
    out.push_str("# api_key comes from the PHOENIX_API_KEY env var; never stored here.\n");

    let vars = crate::config::provider_key_vars(&provider);
    let found = vars
        .iter()
        .find(|v| std::env::var(v).map(|s| !s.is_empty()).unwrap_or(false));
    if let Some(var) = found {
        notes.push(format!(
            "{var} is already set in this environment; phoenix will use it \
automatically (PHOENIX_API_KEY overrides it)"
        ));
    } else {
        notes.push("export PHOENIX_API_KEY with the API key for the provider above".into());
    }

    let fallbacks = str_list(&v["agents"]["defaults"]["model"]["fallbacks"]);
    let mut kept = Vec::new();
    for fb in &fallbacks {
        match fb.split_once('/') {
            Some((p, rest)) if p == provider => kept.push(rest.to_string()),
            Some((p, _)) if crate::config::known_kind(p) => {
                kept.push(fb.clone());
                notes.push(format!(
                    "fallback '{fb}' switches provider; its key comes from [provider.keys] or the provider's env var"
                ));
            }
            _ => notes.push(format!(
                "fallback '{fb}' has an unknown provider prefix; set provider.base_url support first, skipped"
            )),
        }
    }
    if !kept.is_empty() {
        out.push_str(&format!("fallbacks = {}\n", toml_list(&kept)));
    }

    out.push_str("\n[agent]\nsessions = true\n");

    out.push_str(&format!("workspace = {}\n", toml_str("~/phoenix")));
    let ask = v["tools"]["exec"]["ask"].as_str().unwrap_or("off");
    let ask_on = ask != "off";
    if ask_on {
        out.push_str("\n[security]\n");
        {
            out.push_str("approvals = true\n");
            notes.push(format!(
                "gateway exec ask was {ask:?}; carried over as security.approvals = true: \
serve chats queue shell commands until you send /approve. Delete the line to run directly"
            ));
        }
    }
    if !ask_on {
        notes.push(
            "exec approvals stay off (phoenix default): serve chats run shell commands \
directly. Set security.approvals = true to queue them for /approve"
                .into(),
        );
    }

    let tg = &v["channels"]["telegram"];
    if tg.is_object() {
        let allowed = str_list(&tg["allowFrom"]);
        out.push_str("\n[telegram]\n");
        match tg["botToken"].as_str() {
            Some(tok) if !tok.is_empty() => {
                out.push_str("# token lives in the encrypted secret store, never in this file.\n");
                secrets.push(("PHOENIX_TELEGRAM_TOKEN".to_string(), tok.to_string()));
                notes.push("telegram bot token encrypted in the secret store".into());
            }
            _ => {
                out.push_str("# token comes from PHOENIX_TELEGRAM_TOKEN; the gateway kept it as a secret reference.\n");
                notes.push("export PHOENIX_TELEGRAM_TOKEN with your Telegram bot token".into());
            }
        }
        out.push_str(&format!("allowed_chat_ids = {}\n", toml_list(&allowed)));
        if allowed.is_empty() {
            notes.push(
                "telegram allowFrom was empty; phoenix fails closed until allowed_chat_ids is set"
                    .into(),
            );
        }
    }

    for (chan, section, allow_key, fields) in [
        (
            "discord",
            "discord",
            "allowed_channel_ids",
            &[("botToken", "PHOENIX_DISCORD_TOKEN")][..],
        ),
        (
            "slack",
            "slack",
            "allowed_channel_ids",
            &[
                ("appToken", "PHOENIX_SLACK_APP_TOKEN"),
                ("botToken", "PHOENIX_SLACK_BOT_TOKEN"),
            ][..],
        ),
        (
            "matrix",
            "matrix",
            "allowed_rooms",
            &[("accessToken", "PHOENIX_MATRIX_TOKEN")][..],
        ),
        (
            "mattermost",
            "mattermost",
            "allowed_channel_ids",
            &[("token", "PHOENIX_MATTERMOST_TOKEN")][..],
        ),
    ] {
        let node = &v["channels"][chan];
        if !node.is_object() {
            continue;
        }
        out.push_str(&format!("\n[{section}]\n"));
        for (json_key, var) in fields {
            match node[json_key].as_str() {
                Some(tok) if !tok.is_empty() => {
                    out.push_str(&format!(
                        "# {json_key} lives in the encrypted secret store, never in this file.\n"
                    ));
                    secrets.push((var.to_string(), tok.to_string()));
                    notes.push(format!("{chan} token encrypted in the secret store"));
                }
                _ => {
                    out.push_str(&format!("# {json_key} comes from {var}.\n"));
                }
            }
        }
        let allowed = str_list(&node["allowFrom"]);
        out.push_str(&format!("{allow_key} = {}\n", toml_list(&allowed)));
        if allowed.is_empty() {
            notes.push(format!(
                "{chan} allowFrom was empty; phoenix fails closed until {allow_key} is set"
            ));
        }
    }

    let wa = &v["channels"]["whatsapp"];
    if wa.is_object() {
        let allowed: Vec<String> = str_list(&wa["allowFrom"])
            .iter()
            .map(|n| n.trim_start_matches('+').to_string())
            .collect();
        out.push_str("\n[whatsapp]\n");
        out.push_str("# token comes from PHOENIX_WHATSAPP_TOKEN.\n");
        out.push_str("# phone_id / verify_token come from your Meta app; see README.\n");
        out.push_str(&format!("allowed_numbers = {}\n", toml_list(&allowed)));
        notes.push(
            "the gateway's WhatsApp uses QR pairing; phoenix uses the Business Cloud API. \
Create a Meta app, then set PHOENIX_WHATSAPP_TOKEN, whatsapp.phone_id, and whatsapp.verify_token"
                .into(),
        );
    }

    Migration {
        toml: out,
        notes,
        secrets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn migrated_workspace_is_the_user_phoenix_folder() {
        let v = json!({
            "agents": {"defaults": {
                "model": {"primary": "anthropic/claude-sonnet-5"},
                "workspace": "/root/.openclaw/workspace"
            }}
        });
        let m = from_gateway(&v);
        let cfg = crate::config::parse(&m.toml).expect("migrated config must parse");
        assert_eq!(
            cfg.workspace,
            crate::config::home_dir().join("phoenix"),
            "workspace must be the user's own phoenix folder\n{}",
            m.toml
        );
        assert!(
            crate::config::unknown_keys(&m.toml).is_empty(),
            "migrated config must use known keys only: {:?}",
            crate::config::unknown_keys(&m.toml)
        );
    }

    #[test]
    fn carry_persona_copies_identity_memory_and_dailies() {
        let base = std::env::temp_dir().join(format!("phx-persona-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let ws = base.join("workspace");
        let home = base.join("nest");
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        std::fs::write(ws.join("SOUL.md"), "# SOUL\nBobby lives").unwrap();
        std::fs::write(ws.join("AGENTS.md"), "# AGENTS\nrules").unwrap();
        std::fs::write(ws.join("MEMORY.md"), "- big fact").unwrap();
        std::fs::write(ws.join("memory/2026-07-27.md"), "- daily fact").unwrap();

        let notes = carry_persona(&ws, &home.join("ws"));
        assert!(notes.iter().any(|n| n.contains("SOUL.md")), "{notes:?}");
        assert_eq!(
            std::fs::read_to_string(home.join("ws/SOUL.md")).unwrap(),
            "# SOUL\nBobby lives"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("ws/MEMORY.md")).unwrap(),
            "- big fact"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("ws/memory/2026-07-27.md")).unwrap(),
            "- daily fact"
        );

        let _ = carry_persona(&ws, &home.join("ws"));
        assert_eq!(
            std::fs::read_to_string(home.join("ws/MEMORY.md")).unwrap(),
            "- big fact"
        );

        std::fs::write(home.join("ws/MEMORY.md"), "- phoenix note").unwrap();
        let _ = carry_persona(&ws, &home.join("ws"));
        let merged = std::fs::read_to_string(home.join("ws/MEMORY.md")).unwrap();
        assert!(merged.starts_with("- phoenix note"), "{merged}");
        assert!(merged.contains("Migrated from OpenClaw"), "{merged}");
        assert!(merged.contains("- big fact"), "{merged}");
    }

    #[test]
    fn carry_persona_missing_workspace_is_noop() {
        let base = std::env::temp_dir().join(format!("phx-persona-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let notes = carry_persona(&base.join("nope"), &base.join("ws"));
        assert!(notes.is_empty());
        assert!(!base.join("nest").exists());
    }

    #[test]
    fn gateway_workspace_resolution() {
        let v = json!({"agents": {"defaults": {"workspace": "/tmp/ws"}}});
        assert_eq!(
            gateway_workspace(&v, std::path::Path::new("/gw")),
            std::path::PathBuf::from("/tmp/ws")
        );
        let v = json!({});
        assert_eq!(
            gateway_workspace(&v, std::path::Path::new("/gw")),
            std::path::PathBuf::from("/gw/workspace")
        );
    }

    #[test]
    fn resolves_file_backed_secret_token() {
        let dir = std::env::temp_dir().join(format!("phx-migrate-sec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("secrets.json"),
            r#"{"channels":{"telegram":{"botToken":"123:secret"}}}"#,
        )
        .unwrap();
        let v = json!({"channels": {"telegram": {"botToken": {
            "source": "file", "provider": "filemain",
            "id": "/channels/telegram/botToken"}}}});
        assert_eq!(
            resolve_secret_token(&v, &dir),
            Some("123:secret".to_string())
        );

        let v2 = json!({"channels": {"telegram": {"botToken": {"source": "env"}}}});
        assert_eq!(resolve_secret_token(&v2, &dir), None);
        let v3 = json!({"channels": {"telegram": {"botToken": "plain"}}});
        assert_eq!(resolve_secret_token(&v3, &dir), None);

        let v4 = v.clone();
        assert_eq!(resolve_secret_token(&v4, &dir.join("nope")), None);
    }

    fn sample() -> Value {
        json!({
            "agents": {"defaults": {
                "model": {"primary": "anthropic/claude-sonnet-4-6",
                           "fallbacks": ["anthropic/claude-haiku-4-5", "openrouter/meta/llama-3.3-70b"]},
                "workspace": "~/work"
            }},
            "channels": {
                "telegram": {"enabled": true, "botToken": {"source": "env"},
                              "allowFrom": ["1868769425"]},
                "whatsapp": {"allowFrom": ["+4915551234567"]}
            }
        })
    }

    #[test]
    fn maps_provider_and_model() {
        let m = from_gateway(&sample());
        assert!(m.toml.contains("kind = \"anthropic\""));
        assert!(m.toml.contains("model = \"claude-sonnet-4-6\""));
        assert!(m
            .toml
            .contains("fallbacks = [\"claude-haiku-4-5\", \"openrouter/meta/llama-3.3-70b\"]"));
        assert!(m.notes.iter().any(|n| n.contains("switches provider")));
    }

    #[test]
    fn secret_refs_never_copied() {
        let m = from_gateway(&sample());
        assert!(!m.toml.contains("source"));
        assert!(m.toml.contains("PHOENIX_TELEGRAM_TOKEN"));
        assert!(m.toml.contains("allowed_chat_ids = [\"1868769425\"]"));
    }

    #[test]
    fn provider_keys_carry_from_the_env_file_without_a_secrets_json() {
        let dir = std::env::temp_dir().join(format!("phx-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("gateway.systemd.env"),
            "# a comment\nexport ANTHROPIC_API_KEY=sk-ant-1\nOPENROUTER_API_KEY=\"sk-or-1\"\nEMPTY_KEY=\nUNRELATED=nope\n",
        )
        .unwrap();
        let got = collect_keys(&dir);
        assert!(
            got.contains(&("anthropic".to_string(), vec!["sk-ant-1".to_string()])),
            "an api key in the env file must migrate even with no secrets.json: {got:?}"
        );
        assert!(
            got.contains(&("openrouter".to_string(), vec!["sk-or-1".to_string()])),
            "quoted values carry too: {got:?}"
        );
        assert!(
            !got.iter().any(|(_, v)| v.iter().any(|k| k == "nope")),
            "a non-provider variable is not an api key: {got:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_carried_bot_token_never_lands_in_the_config_file() {
        let mut v = sample();
        v["channels"]["telegram"]["botToken"] = json!("123:abc");
        let m = from_gateway(&v);
        assert!(
            !m.toml.contains("123:abc"),
            "the bot token is a secret; it belongs in the encrypted store: {}",
            m.toml
        );
        assert_eq!(
            m.secrets,
            vec![("PHOENIX_TELEGRAM_TOKEN".to_string(), "123:abc".to_string())],
            "the token must be handed to the caller to encrypt"
        );
        assert!(m.notes.iter().any(|n| n.contains("secret store")));
    }

    #[test]
    fn whatsapp_allowlist_strips_plus() {
        let m = from_gateway(&sample());
        assert!(m.toml.contains("allowed_numbers = [\"4915551234567\"]"));
        assert!(m.notes.iter().any(|n| n.contains("Cloud API")));
    }

    #[test]
    fn nvidia_maps_to_compat_endpoint() {
        let v = json!({"agents": {"defaults": {"model": {"primary": "nvidia/meta/llama-3.1-70b-instruct"}}}});
        let m = from_gateway(&v);
        assert!(m.toml.contains("kind = \"nvidia\""));
        assert!(m.toml.contains("model = \"meta/llama-3.1-70b-instruct\""));
        assert!(m
            .toml
            .contains("base_url = \"https://integrate.api.nvidia.com/v1\""));
    }

    #[test]
    fn unknown_provider_gets_base_url_note() {
        let v = json!({"agents": {"defaults": {"model": {"primary": "acme/foo-1"}}}});
        let m = from_gateway(&v);
        assert!(m.notes.iter().any(|n| n.contains("base_url")));
    }

    #[test]
    fn empty_config_still_renders() {
        let m = from_gateway(&json!({}));
        assert!(m.toml.contains("[provider]"));
        assert!(m.notes.iter().any(|n| n.contains("primary")));
    }

    #[test]
    fn workspace_and_sessions_carry() {
        let m = from_gateway(&sample());
        assert!(m.toml.contains("sessions = true"));
        assert!(m.toml.contains("workspace = \"~/phoenix\""));
    }

    #[test]
    fn exec_ask_on_maps_to_approvals_true() {
        for ask in ["on", "always"] {
            let mut v = sample();
            v["tools"] = json!({"exec": {"ask": ask}});
            let m = from_gateway(&v);
            assert!(m.toml.contains("approvals = true"), "ask={ask}");
            assert!(m.notes.iter().any(|n| n.contains("/approve")));
        }
    }

    #[test]
    fn exec_ask_off_or_missing_maps_to_nothing() {
        let mut v = sample();
        v["tools"] = json!({"exec": {"ask": "off"}});
        for cfg in [&v, &sample()] {
            let m = from_gateway(cfg);
            assert!(!m.toml.contains("approvals"));
            assert!(m
                .notes
                .iter()
                .any(|n| n.contains("approvals stay off (phoenix default)")));
        }
    }
}
