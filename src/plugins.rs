use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub detail: String,
    pub enabled: bool,
    pub problem: String,
}

pub fn inventory(cfg: &Config) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for s in &cfg.mcp_servers {
        let problem = if s.command.trim().is_empty() {
            format!("mcp server '{}' has no command", s.name)
        } else {
            String::new()
        };
        out.push(Entry {
            kind: "mcp".into(),
            name: s.name.clone(),
            detail: s.command.clone(),
            enabled: s.enabled,
            problem,
        });
    }
    let hook_problems = crate::hooks::problems(&cfg.hooks);
    for h in &cfg.hooks {
        let problem = hook_problems
            .iter()
            .find(|p| p.contains(&format!("'{}'", h.name)))
            .cloned()
            .unwrap_or_default();
        out.push(Entry {
            kind: "hook".into(),
            name: h.name.clone(),
            detail: format!("{} -> {}", h.event, h.command),
            enabled: h.enabled,
            problem,
        });
    }
    let dirs = [
        cfg.workspace.join("skills"),
        crate::config::home().join("skills"),
    ];
    for dir in &dirs {
        let (skills, problems) = crate::skills::scan_dir(dir);
        for s in skills {
            if out
                .iter()
                .any(|e| e.kind == "skill" && e.name.eq_ignore_ascii_case(&s.name))
            {
                continue;
            }
            out.push(Entry {
                kind: "skill".into(),
                name: s.name.clone(),
                detail: crate::security::one_line(&s.description, 60),
                enabled: true,
                problem: String::new(),
            });
        }
        for p in problems {
            out.push(Entry {
                kind: "skill".into(),
                name: "(unreadable)".into(),
                detail: dir.display().to_string(),
                enabled: false,
                problem: p,
            });
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    out
}

pub fn problems(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| !e.problem.is_empty())
        .map(|e| e.problem.clone())
        .collect()
}

pub fn list_text(entries: &[Entry], json: bool) -> String {
    if json {
        let arr: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "kind": e.kind,
                    "name": e.name,
                    "detail": e.detail,
                    "enabled": e.enabled,
                    "problem": e.problem,
                })
            })
            .collect();
        let mut s = serde_json::to_string_pretty(&serde_json::json!({"extensions": arr}))
            .unwrap_or_default();
        s.push('\n');
        return s;
    }
    if entries.is_empty() {
        return "no extensions loaded\n\
phoenix is one static binary: extensions are mcp servers, hooks and skills\n"
            .to_string();
    }
    let mut out = format!("{} extension(s)\n", entries.len());
    for e in entries {
        let state = if !e.problem.is_empty() {
            "broken"
        } else if e.enabled {
            "on"
        } else {
            "off"
        };
        out.push_str(&format!(
            "  {:<6}{:<24}{:<8}{}\n",
            e.kind,
            crate::security::one_line(&e.name, 22),
            state,
            crate::security::one_line(&e.detail, 44)
        ));
    }
    for p in problems(entries) {
        out.push_str(&format!("  problem: {p}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(raw: &str) -> Config {
        let mut c = crate::config::parse(raw).unwrap();
        c.workspace = std::env::temp_dir().join(format!(
            "px-plugins-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&c.workspace);
        std::fs::create_dir_all(&c.workspace).unwrap();
        c
    }

    #[test]
    fn an_empty_setup_lists_nothing_and_says_what_an_extension_is() {
        let c = cfg_with("");
        let text = list_text(&inventory(&c), false);
        assert!(text.contains("no extensions"), "{text}");
        assert!(text.contains("mcp servers, hooks and skills"), "{text}");
    }

    #[test]
    fn mcp_servers_and_hooks_appear_with_their_state() {
        let c = cfg_with(
            "[mcp.servers.files]\ncommand = \"mcp-files\"\n\
[mcp.servers.off]\ncommand = \"x\"\nenabled = false\n\
[hooks.notify]\nevent = \"turn_end\"\ncommand = \"/bin/true\"\n",
        );
        let inv = inventory(&c);
        let text = list_text(&inv, false);
        assert!(text.contains("mcp   files"), "{text}");
        assert!(text.contains("hook  notify"), "{text}");
        assert!(inv.iter().any(|e| e.name == "off" && !e.enabled));
        assert!(text.contains(" on "), "{text}");
        assert!(text.contains(" off "), "{text}");
    }

    #[test]
    fn a_broken_hook_is_reported_as_broken_not_as_on() {
        let c = cfg_with("[hooks.bad]\nevent = \"nonsense\"\ncommand = \"/bin/true\"\n");
        let inv = inventory(&c);
        assert_eq!(problems(&inv).len(), 1);
        let text = list_text(&inv, false);
        assert!(text.contains("broken"), "{text}");
        assert!(text.contains("problem:"), "{text}");
    }

    #[test]
    fn an_mcp_server_without_a_command_is_broken() {
        let c = cfg_with("[mcp.servers.empty]\ncommand = \"\"\n");
        assert_eq!(problems(&inventory(&c)).len(), 1);
    }

    #[test]
    fn workspace_skills_are_listed_once() {
        let c = cfg_with("");
        let dir = c.workspace.join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("weather.md"),
            "---\nname: weather\ndescription: forecast lookups\n---\nask nicely\n",
        )
        .unwrap();
        let inv = inventory(&c);
        assert_eq!(inv.iter().filter(|e| e.kind == "skill").count(), 1);
        let text = list_text(&inv, false);
        assert!(text.contains("weather"), "{text}");
        assert!(text.contains("forecast lookups"), "{text}");
    }

    #[test]
    fn the_json_form_carries_every_field() {
        let c = cfg_with("[mcp.servers.files]\ncommand = \"mcp-files\"\n");
        let raw = list_text(&inventory(&c), true);
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let first = &v["extensions"][0];
        assert_eq!(first["kind"], "mcp");
        assert_eq!(first["name"], "files");
        assert_eq!(first["enabled"], true);
        assert_eq!(first["problem"], "");
    }

    #[test]
    fn entries_are_sorted_by_kind_then_name() {
        let c = cfg_with(
            "[mcp.servers.zeta]\ncommand = \"z\"\n[mcp.servers.alpha]\ncommand = \"a\"\n\
[hooks.h]\nevent = \"turn_end\"\ncommand = \"/bin/true\"\n",
        );
        let inv = inventory(&c);
        let order: Vec<String> = inv
            .iter()
            .map(|e| format!("{}/{}", e.kind, e.name))
            .collect();
        assert_eq!(order, vec!["hook/h", "mcp/alpha", "mcp/zeta"]);
    }
}
