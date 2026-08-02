use crate::agent::Agent;
use crate::config::{self, Config};
use crate::providers;
use crate::tools::Toolbox;

pub const CONVERGED_MARKER: &str = "[[COLAB_CONVERGED]]";
const DEFAULT_MAX_ROUNDS: u32 = 6;

pub struct Round {
    pub speaker: String,
    pub text: String,
}

pub struct ColabResult {
    pub rounds: Vec<Round>,
    pub final_text: String,
    pub converged: bool,
}

fn colab_system_note(self_label: &str, other_label: &str) -> String {
    format!(
        "\n\nYou are one of two models collaborating on this task, alongside {other_label}. \
You are {self_label}. You will see the other model's replies appended to the conversation \
labeled with its name. Build on their work, correct mistakes you see, and add what they \
missed; do not just repeat what they already said. When the task is genuinely complete and \
you have nothing to add or change, end your reply with the exact line {CONVERGED_MARKER} \
on its own line. Do not print that line unless you mean it."
    )
}

fn build_partner(cfg: &Config, spec: &str, toolbox: Toolbox) -> Result<Agent, String> {
    let Some((kind, model)) = spec.split_once('/') else {
        return Err(format!(
            "colab needs a \"provider/model\" spec, got '{spec}' with no '/'"
        ));
    };
    if !config::known_kind(kind) {
        return Err(format!(
            "colab does not know provider '{kind}' (from '{spec}'); \
run `phoenix models` to see known provider kinds"
        ));
    }
    if model.trim().is_empty() {
        return Err(format!(
            "colab spec '{spec}' has no model name after the '/'"
        ));
    }
    let mut pcfg = cfg.clone();
    config::switch_provider(&mut pcfg, kind);
    pcfg.model = model.to_string();
    let provider = providers::make(&pcfg).map_err(|e| e.to_string())?;
    Ok(Agent::new(pcfg, Box::new(provider), toolbox))
}

pub fn run(
    a: &mut Agent,
    b_spec: &str,
    b_toolbox: Toolbox,
    task: &str,
    max_rounds: u32,
    mut on_round: impl FnMut(&Round),
) -> Result<ColabResult, String> {
    let max_rounds = if max_rounds == 0 {
        DEFAULT_MAX_ROUNDS
    } else {
        max_rounds
    };
    let label_a = format!("{}/{}", a.cfg.provider, a.cfg.model);
    let mut b = build_partner(&a.cfg, b_spec, b_toolbox)?;
    let label_b = format!("{}/{}", b.cfg.provider, b.cfg.model);
    if label_a == label_b {
        return Err(format!(
            "colab needs two different models, got the same one twice: {label_a}"
        ));
    }

    let note_a = colab_system_note(&label_a, &label_b);
    let note_b = colab_system_note(&label_b, &label_a);

    let mut rounds: Vec<Round> = Vec::new();
    let mut converged = false;
    let mut last_text = String::new();
    let mut turn = format!("{task}{note_a}");
    let mut speaker_is_a = true;

    for i in 0..max_rounds * 2 {
        if converged {
            break;
        }
        let (label, reply) = if speaker_is_a {
            let out = a.run(&turn);
            (label_a.clone(), out)
        } else {
            let out = b.run(&turn);
            (label_b.clone(), out)
        };
        let hit = reply.contains(CONVERGED_MARKER);
        let clean: String = reply.replace(CONVERGED_MARKER, "").trim().to_string();
        let round = Round {
            speaker: label,
            text: clean.clone(),
        };
        on_round(&round);
        rounds.push(round);
        last_text = clean.clone();
        if hit && i > 0 {
            converged = true;
            break;
        }
        let handoff_note = if speaker_is_a { &note_b } else { &note_a };
        turn =
            format!("The other model just replied:\n\n{clean}\n\nContinue the task.{handoff_note}");
        speaker_is_a = !speaker_is_a;
    }

    Ok(ColabResult {
        rounds,
        final_text: last_text,
        converged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatBackend, ProviderError, Reply};
    use serde_json::Value;
    use std::cell::Cell;
    use std::rc::Rc;

    fn tmpdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "phx-colab-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_cfg(provider: &str, model: &str) -> Config {
        Config {
            provider: provider.into(),
            model: model.into(),
            workspace: tmpdir(),
            privacy: "ghost".into(),
            ..Default::default()
        }
    }

    fn make_toolbox(cfg: &Config) -> Toolbox {
        let memory = crate::memory::Memory::in_workspace(&cfg.privacy, &cfg.workspace);
        Toolbox::new(cfg, memory, None, None).unwrap()
    }

    struct ScriptedProvider {
        replies: Rc<Vec<&'static str>>,
        idx: Rc<Cell<usize>>,
    }

    impl ChatBackend for ScriptedProvider {
        fn chat(
            &mut self,
            _c: &Config,
            _s: &str,
            _h: &[crate::providers::Msg],
            _t: &[Value],
        ) -> Result<Reply, ProviderError> {
            let i = self.idx.get();
            self.idx.set(i + 1);
            let text = self
                .replies
                .get(i)
                .copied()
                .unwrap_or("nothing more to add");
            Ok(Reply::text_only(text))
        }
        fn chat_stream(
            &mut self,
            c: &Config,
            s: &str,
            h: &[crate::providers::Msg],
            t: &[Value],
            _on_text: &mut dyn FnMut(&str),
        ) -> Result<Reply, ProviderError> {
            self.chat(c, s, h, t)
        }
    }

    fn build_agent_with(cfg: &Config, replies: Vec<&'static str>) -> Agent {
        let toolbox = make_toolbox(cfg);
        let provider = ScriptedProvider {
            replies: Rc::new(replies),
            idx: Rc::new(Cell::new(0)),
        };
        Agent::new(cfg.clone(), Box::new(provider), toolbox)
    }

    #[test]
    fn two_different_models_alternate_until_convergence() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(
            &cfg_a,
            vec![
                "first idea from a",
                "looks complete now [[COLAB_CONVERGED]]",
            ],
        );
        let cfg_b_toolbox = make_cfg("anthropic", "claude-b");
        let b_toolbox = make_toolbox(&cfg_b_toolbox);

        let result = run(
            &mut a,
            "anthropic/claude-b",
            b_toolbox,
            "do the task",
            3,
            |_| {},
        );
        let result = result.unwrap_or_else(|e| panic!("colab must run: {e}"));
        assert!(result.rounds.len() >= 2, "must have at least 2 rounds");
        assert!(
            result.rounds[0].text.contains("first idea from a"),
            "{}",
            result.rounds[0].text
        );
    }

    #[test]
    fn same_model_twice_is_refused() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(&cfg_a, vec!["x"]);
        let toolbox = make_toolbox(&cfg_a);
        let err = run(&mut a, "openai/gpt-a", toolbox, "task", 2, |_| {});
        assert!(err.is_err(), "same model twice must be refused");
    }

    #[test]
    fn max_rounds_caps_the_exchange_even_without_convergence() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(&cfg_a, vec!["a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"]);
        let cfg_b = make_cfg("anthropic", "claude-b");
        let b_toolbox = make_toolbox(&cfg_b);
        let result = run(&mut a, "anthropic/claude-b", b_toolbox, "task", 2, |_| {});
        let result = result.unwrap();
        assert!(
            result.rounds.len() <= 4,
            "max_rounds=2 means at most 4 total turns, got {}",
            result.rounds.len()
        );
        assert!(!result.converged);
    }

    #[test]
    fn converged_marker_is_stripped_from_visible_text() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(&cfg_a, vec!["done here [[COLAB_CONVERGED]]"]);
        let cfg_b = make_cfg("anthropic", "claude-b");
        let b_toolbox = make_toolbox(&cfg_b);
        let result = run(&mut a, "anthropic/claude-b", b_toolbox, "task", 3, |_| {});
        let result = result.unwrap();
        assert!(!result.rounds[0].text.contains(CONVERGED_MARKER));
    }

    #[test]
    fn on_round_callback_fires_for_every_round() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(&cfg_a, vec!["a1", "a2 [[COLAB_CONVERGED]]"]);
        let cfg_b = make_cfg("anthropic", "claude-b");
        let b_toolbox = make_toolbox(&cfg_b);
        let count = Rc::new(Cell::new(0));
        let count2 = count.clone();
        let _ = run(
            &mut a,
            "anthropic/claude-b",
            b_toolbox,
            "task",
            3,
            move |_| {
                count2.set(count2.get() + 1);
            },
        );
        assert!(count.get() >= 2);
    }

    #[test]
    fn unresolvable_partner_spec_is_a_clean_error_not_a_panic() {
        let cfg_a = make_cfg("openai", "gpt-a");
        let mut a = build_agent_with(&cfg_a, vec!["x"]);
        let toolbox = make_toolbox(&cfg_a);
        let result = run(
            &mut a,
            "totally-unknown-provider/model-x",
            toolbox,
            "task",
            2,
            |_| {},
        );
        assert!(result.is_err());
    }
}
