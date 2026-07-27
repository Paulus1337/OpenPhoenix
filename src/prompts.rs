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

pub fn build(cfg: &Config) -> String {
    let p = BASE
        .replace("{workspace}", &cfg.workspace.display().to_string())
        .replace("{privacy}", &cfg.privacy)
        .replace("{privacy_note}", privacy_note(&cfg.privacy));
    let style = lean_style(&cfg.lean);
    if style.is_empty() {
        p
    } else {
        format!("{p}{style}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_workspace_and_privacy() {
        let cfg = Config {
            workspace: "/tmp/px-prompt".into(),
            privacy: "ghost".into(),
            ..Config::default()
        };
        let p = build(&cfg);
        assert!(p.contains("/tmp/px-prompt"));
        assert!(p.contains("Privacy mode: ghost"));
        assert!(p.contains("leaves no trace"));
    }

    #[test]
    fn lean_levels_change_prompt() {
        let mut cfg = Config::default();
        let base_len = build(&cfg).len();
        cfg.lean = "lean".into();
        assert!(build(&cfg).contains("No preamble"));
        cfg.lean = "grunt".into();
        let grunt = build(&cfg);
        assert!(grunt.contains("maximum compression"));
        assert!(grunt.len() > base_len);
    }
}
