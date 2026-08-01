use crate::config::Config;

const BASE: &str = "You are Phoenix, a capable personal AI agent (OpenPhoenix runtime).
Act, verify, report. Use tools to check facts instead of guessing.
Workspace: {workspace}
Privacy mode: {privacy} ({privacy_note})
Rules:
- Never fabricate tool output or file contents.
- Never write secrets to disk or echo them back.
- Destructive actions require an explicit user request.
- Prefer the smallest action that completes the task.";

pub fn privacy_note(privacy: &str) -> &'static str {
    match privacy {
        "ghost" => {
            "nothing is persisted; this task leaves no trace. Do not reference past sessions."
        }
        "session" => "conversation lives in RAM for this session only; nothing written to disk.",
        "recall" => "you may use remember/recall tools for durable notes the user can audit.",
        _ => "",
    }
}

pub fn lean_style(lean: &str) -> &'static str {
    match lean {
        "lean" => {
            "\nOutput style: concise. No preamble, no filler, no restating the question.
Answer first, detail only if needed. Keep code and commands exact."
        }
        "grunt" => {
            "\nOutput style: maximum compression. Short sentences. Drop articles and filler.
No apologies, no hedging, no summaries of what you just did.
Keep code, commands, paths, errors byte-exact. Technical accuracy 100%.
Example: \"New object ref each render. Wrap in useMemo.\" Nothing lost, half the tokens."
        }
        _ => "",
    }
}

const PERSONA_CAP: usize = 16 * 1024;

const PERSONA_FIRST: [&str; 5] = ["SOUL.md", "AGENTS.md", "IDENTITY.md", "USER.md", "TOOLS.md"];

pub fn persona_text(dir: &std::path::Path) -> String {
    let mut names: Vec<String> = PERSONA_FIRST.iter().map(|s| (*s).to_string()).collect();
    let mut extras: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".md")
                && n != "PROMPT.md"
                && !PERSONA_FIRST.contains(&n.as_str())
                && e.path().is_file()
            {
                extras.push(n);
            }
        }
    }
    extras.sort();
    names.extend(extras);
    let mut out = String::new();
    for name in names {
        let Ok(content) = std::fs::read_to_string(dir.join(&name)) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let clipped: String = content.chars().take(PERSONA_CAP).collect();
        let safe = crate::text::sanitize_prompt_literal(&clipped);
        out.push_str(&format!("\n\n## {name}\n{safe}"));
    }
    if out.is_empty() {
        out
    } else {
        format!("\n\n# Identity (carried from your previous agent){out}")
    }
}

pub fn tool_inventory(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    format!(
        "\nTools available to you right now: {}.\n\
That list is complete. Never claim or offer a capability that is not in it. \
If asked what you can do, describe only those tools.",
        names.join(", ")
    )
}

fn base_template(persona_dir: &std::path::Path) -> String {
    let override_path = persona_dir.join("PROMPT.md");
    if let Ok(text) = std::fs::read_to_string(&override_path) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(PERSONA_CAP).collect();
        }
    }
    BASE.to_string()
}

pub fn build_full(cfg: &Config, persona_dir: &std::path::Path, tools: &[String]) -> String {
    let p = base_template(persona_dir)
        .replace("{workspace}", &cfg.workspace.display().to_string())
        .replace("{privacy}", &cfg.privacy)
        .replace("{privacy_note}", privacy_note(&cfg.privacy));
    let style = lean_style(&cfg.lean);
    let inventory = if cfg.tool_list {
        tool_inventory(tools)
    } else {
        String::new()
    };
    let persona = persona_text(persona_dir);
    format!("{p}{style}{inventory}{persona}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_exactly_the_tools_that_exist() {
        let cfg = Config::default();
        let tools = vec!["shell".to_string(), "read_file".to_string()];
        let p = build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &tools);
        assert!(p.contains("shell, read_file"), "{p}");
        assert!(
            p.contains("That list is complete"),
            "the model must be told not to invent capabilities: {p}"
        );
        assert!(
            !p.contains("image_generate"),
            "a disabled tool must never be advertised"
        );
    }

    #[test]
    fn no_tool_inventory_line_when_there_are_no_tools() {
        assert!(tool_inventory(&[]).is_empty());
    }

    #[test]
    fn extra_persona_files_load_after_the_known_set() {
        let d = std::env::temp_dir().join(format!(
            "px-persona-extra-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SOUL.md"), "the soul").unwrap();
        std::fs::write(d.join("VALUE.md"), "the values").unwrap();
        std::fs::write(d.join("VESSEL.md"), "the vessel").unwrap();
        std::fs::write(d.join("PROMPT.md"), "base override, not persona").unwrap();
        std::fs::write(d.join("notes.txt"), "not markdown").unwrap();
        let p = persona_text(&d);
        assert!(p.contains("## SOUL.md"), "{p}");
        assert!(
            p.contains("## VALUE.md"),
            "any extra persona markdown loads: {p}"
        );
        assert!(p.contains("## VESSEL.md"), "{p}");
        assert!(
            !p.contains("base override"),
            "PROMPT.md is the template override, never persona: {p}"
        );
        assert!(!p.contains("notes.txt"), "{p}");
        let soul = p.find("## SOUL.md").unwrap();
        let value = p.find("## VALUE.md").unwrap();
        assert!(soul < value, "the known set keeps priority order");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_prompt_md_override_replaces_the_base_template() {
        let d = std::env::temp_dir().join(format!(
            "px-prompt-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("PROMPT.md"),
            "Custom base. Workspace: {workspace}. Mode: {privacy}.",
        )
        .unwrap();
        let cfg = Config::default();
        let p = build_full(&cfg, &d, &[]);
        assert!(p.starts_with("Custom base."), "{p}");
        assert!(
            !p.contains("You are Phoenix"),
            "the built-in template must step aside: {p}"
        );
        assert!(
            p.contains(&cfg.workspace.display().to_string()),
            "placeholders still expand in the override: {p}"
        );

        std::fs::write(d.join("PROMPT.md"), "   \n").unwrap();
        let p = build_full(&cfg, &d, &[]);
        assert!(
            p.contains("You are Phoenix"),
            "a blank override falls back to the built-in: {p}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tool_list_off_skips_the_inventory_line() {
        let cfg = Config {
            tool_list: false,
            ..Config::default()
        };
        let tools = vec!["shell".to_string(), "read_file".to_string()];
        let p = build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &tools);
        assert!(
            !p.contains("That list is complete"),
            "tool_list = false must drop the inventory block: {p}"
        );
        assert!(!p.contains("shell, read_file"), "{p}");
    }

    #[test]
    fn build_includes_workspace_and_privacy() {
        let cfg = Config {
            workspace: "/tmp/px-prompt".into(),
            privacy: "ghost".into(),
            ..Config::default()
        };
        let p = build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &[]);
        assert!(p.contains("/tmp/px-prompt"));
        assert!(p.contains("Privacy mode: ghost"));
        assert!(p.contains("leaves no trace"));
    }

    #[test]
    fn lean_levels_change_prompt() {
        let mut cfg = Config::default();
        let base_len = build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &[]).len();
        cfg.lean = "lean".into();
        assert!(
            build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &[])
                .contains("No preamble")
        );
        cfg.lean = "grunt".into();
        let grunt = build_full(&cfg, std::path::Path::new("/nonexistent-persona"), &[]);
        assert!(grunt.contains("maximum compression"));
        assert!(grunt.len() > base_len);
    }
}
