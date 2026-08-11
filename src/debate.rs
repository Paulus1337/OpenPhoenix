use crate::agent::Agent;

pub const AGREEMENT_MARKER: &str = "[[COLAB_AGREED]]";
pub const MAX_DEBATE_ROUNDS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatReply {
    pub label: String,
    pub text: String,
    pub thinking: String,
    pub failed: bool,
}

pub fn agreed(text: &str) -> bool {
    text.lines().any(|line| line.trim() == AGREEMENT_MARKER)
}

pub fn both_agreed(a: &SeatReply, b: &SeatReply) -> bool {
    !a.failed && !b.failed && agreed(&a.text) && agreed(&b.text)
}

pub fn strip_agreement(text: &str) -> String {
    text.lines()
        .filter(|line| line.trim() != AGREEMENT_MARKER)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn debate_prompt(task: &str, peer_label: &str, peer_last: Option<&str>, note: &str) -> String {
    match peer_last {
        None => format!(
            "The person's task:\n{task}\n\nJOINT PLANNING, both seats are thinking at the same time. \
State how you would approach this, what you believe the correct split is, and the strongest \
objection you expect from {peer_label}. Reason in depth about the plan itself; this is where the \
team spends its time. Do not start the work and do not use tools. If and only if you are already \
certain the plan is right, end with {AGREEMENT_MARKER} on its own line.{note}"
        ),
        Some(peer) => format!(
            "The person's task:\n{task}\n\n{peer_label} answered at the same time as you:\n{peer}\n\n\
JOINT PLANNING, keep debating as peers. Say plainly where you disagree and why, correct any real \
mistake, and adopt their better ideas instead of restating your own. Converge on one concrete plan \
with a non-duplicated split. Do not start the work and do not use tools. When the plan is genuinely \
settled, end with {AGREEMENT_MARKER} on its own line.{note}"
        ),
    }
}

pub fn work_prompt(task: &str, plan: &str, share: &str, note: &str) -> String {
    format!(
        "The person's task:\n{task}\n\nThe plan both seats agreed:\n{plan}\n\nYour assigned share:\n{share}\n\n\
Both seats are working at the same time right now. Do your share with real tool work and report \
concrete evidence. Do not wait for your teammate and do not redo their share.{note}"
    )
}

pub fn run_round(
    a: &mut Agent,
    b: &mut Agent,
    prompt_a: &str,
    prompt_b: &str,
    planning: bool,
) -> (SeatReply, SeatReply) {
    let prepare: fn(&mut Agent, &str) -> String = if planning {
        crate::colab::prepare_with_overflow_recovery
    } else {
        Agent::run_pinned
    };
    let label_a = format!("{}/{}", a.cfg.provider, a.cfg.model);
    let label_b = format!("{}/{}", b.cfg.provider, b.cfg.model);
    a.toolbox.set_speaker(&label_a);
    b.toolbox.set_speaker(&format!("partner:{label_b}"));
    let mut results = crate::parallel::run(vec![
        Box::new(|| prepare(a, prompt_a)) as crate::parallel::Job<'_, String>,
        Box::new(|| prepare(b, prompt_b)) as crate::parallel::Job<'_, String>,
    ]);
    let raw_b = results
        .pop()
        .unwrap_or_else(|| Err(Box::new("missing peer job") as Box<dyn std::any::Any + Send>));
    let raw_a = results
        .pop()
        .unwrap_or_else(|| Err(Box::new("missing peer job") as Box<dyn std::any::Any + Send>));
    a.toolbox.clear_speaker();
    b.toolbox.clear_speaker();
    let thinking_a = a.last_thinking.clone();
    let thinking_b = b.last_thinking.clone();
    (
        seat_reply(label_a, raw_a, thinking_a),
        seat_reply(label_b, raw_b, thinking_b),
    )
}

fn seat_reply(label: String, raw: std::thread::Result<String>, thinking: String) -> SeatReply {
    match raw {
        Ok(text) => {
            let failed = crate::colab::turn_failed(&text);
            SeatReply {
                label,
                text,
                thinking,
                failed,
            }
        }
        Err(_) => SeatReply {
            label,
            text: "provider error: the seat thread stopped unexpectedly".to_string(),
            thinking: String::new(),
            failed: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::providers::{ChatBackend, ProviderError, Reply};
    use serde_json::Value;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn tmpdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "phx-debate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&p).ok();
        p
    }

    fn cfg(provider: &str, model: &str) -> Config {
        Config {
            provider: provider.into(),
            model: model.into(),
            workspace: tmpdir(),
            privacy: "ghost".into(),
            ..Default::default()
        }
    }

    struct Slow {
        reply: &'static str,
        live: Arc<AtomicU32>,
        peak: Arc<AtomicU32>,
        calls: Arc<AtomicU32>,
        overlapped: Option<Arc<AtomicU32>>,
    }

    impl ChatBackend for Slow {
        fn chat(
            &mut self,
            _c: &Config,
            _s: &str,
            _h: &[crate::providers::Msg],
            _t: &[Value],
        ) -> Result<Reply, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
            while std::time::Instant::now() < deadline {
                if self.live.load(Ordering::SeqCst) > 1 {
                    if let Some(counter) = &self.overlapped {
                        counter.fetch_max(1, Ordering::SeqCst);
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(Reply::text_only(self.reply))
        }
    }

    struct Dead;
    impl ChatBackend for Dead {
        fn chat(
            &mut self,
            _c: &Config,
            _s: &str,
            _h: &[crate::providers::Msg],
            _t: &[Value],
        ) -> Result<Reply, ProviderError> {
            Err(ProviderError("HTTP 500: down".into()))
        }
    }

    fn agent(config: &Config, provider: Box<dyn ChatBackend>) -> Agent {
        let memory = crate::memory::Memory::in_workspace(&config.privacy, &config.workspace);
        let toolbox = crate::tools::Toolbox::new(config, memory, None, None)
            .unwrap_or_else(|e| panic!("toolbox: {e}"));
        Agent::new(config.clone(), provider, toolbox)
    }

    #[test]
    fn both_seats_run_at_the_same_time() {
        let live = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));
        let total = Arc::new(AtomicU32::new(0));
        let overlapped = Arc::new(AtomicU32::new(0));
        let cfg_a = cfg("openai", "gpt-a");
        let cfg_b = cfg("anthropic", "claude-b");
        let mut a = agent(
            &cfg_a,
            Box::new(Slow {
                reply: "primary plan",
                live: live.clone(),
                peak: peak.clone(),
                calls: total.clone(),
                overlapped: Some(overlapped.clone()),
            }),
        );
        let mut b = agent(
            &cfg_b,
            Box::new(Slow {
                reply: "partner plan",
                live: live.clone(),
                peak: peak.clone(),
                calls: total.clone(),
                overlapped: Some(overlapped.clone()),
            }),
        );
        let warm = std::time::Instant::now();
        let _ = run_round(&mut a, &mut b, "warm", "warm", true);
        let warm_cost = warm.elapsed();
        total.store(0, Ordering::SeqCst);
        overlapped.store(0, Ordering::SeqCst);
        peak.store(0, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let (ra, rb) = run_round(&mut a, &mut b, "plan", "plan", true);
        let elapsed = started.elapsed();
        let _ = (warm_cost, elapsed);
        assert_eq!(ra.text, "primary plan");
        assert_eq!(rb.text, "partner plan");
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "both seats must be inside the provider call at the same time"
        );
        let calls = total.load(Ordering::SeqCst);
        assert_eq!(calls, 2, "exactly one provider call per seat");
        let overlap = overlapped.load(Ordering::SeqCst);
        assert!(
            overlap >= 1,
            "the seats' provider calls must overlap in time, not run one after another"
        );
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "both seats must be inside their provider call at the same moment"
        );
    }

    #[test]
    fn a_failed_seat_does_not_stop_the_other() {
        let cfg_a = cfg("openai", "gpt-a");
        let cfg_b = cfg("anthropic", "claude-b");
        let mut a = agent(&cfg_a, Box::new(Dead));
        let mut b = agent(
            &cfg_b,
            Box::new(Slow {
                reply: "partner still worked",
                live: Arc::new(AtomicU32::new(0)),
                peak: Arc::new(AtomicU32::new(0)),
                calls: Arc::new(AtomicU32::new(0)),
                overlapped: None,
            }),
        );
        let (ra, rb) = run_round(&mut a, &mut b, "plan", "plan", true);
        assert!(ra.failed, "{ra:?}");
        assert!(!rb.failed, "{rb:?}");
        assert_eq!(rb.text, "partner still worked");
    }

    #[test]
    fn agreement_needs_both_seats() {
        let yes = SeatReply {
            label: "a".into(),
            text: format!("settled\n{AGREEMENT_MARKER}"),
            thinking: String::new(),
            failed: false,
        };
        let no = SeatReply {
            label: "b".into(),
            text: "still disagree about the split".into(),
            thinking: String::new(),
            failed: false,
        };
        assert!(!both_agreed(&yes, &no));
        assert!(both_agreed(&yes, &yes));
        let failed = SeatReply {
            failed: true,
            ..yes.clone()
        };
        assert!(
            !both_agreed(&yes, &failed),
            "a failed seat never counts as agreement"
        );
        assert_eq!(strip_agreement(&yes.text), "settled");
    }

    #[test]
    fn turn_taking_only_when_a_seat_is_above_the_shared_threshold() {
        assert_eq!(
            crate::usage::COLAB_DELEGATION_PERCENT,
            98.0,
            "seats work concurrently until a seat passes 98% usage"
        );
        let busy = |percent: f64| percent > crate::usage::COLAB_DELEGATION_PERCENT;
        assert!(!busy(10.0));
        assert!(
            !busy(98.0),
            "exactly at the threshold still works in parallel"
        );
        assert!(busy(98.1));
        assert!(busy(99.9));
    }

    #[test]
    fn debate_prompt_opens_symmetric_then_carries_the_peer_reply() {
        let opening = debate_prompt("ship it", "anthropic/claude", None, "");
        assert!(opening.contains("both seats are thinking at the same time"));
        assert!(opening.contains("strongest objection"));
        assert!(opening.contains("Do not start the work"));
        let reply = debate_prompt("ship it", "anthropic/claude", Some("I object to X"), "");
        assert!(reply.contains("answered at the same time as you"));
        assert!(reply.contains("I object to X"));
        assert!(reply.contains("where you disagree"));
        let work = work_prompt("ship it", "the plan", "renderer", "");
        assert!(work.contains("Both seats are working at the same time"));
        assert!(work.contains("Do not wait for your teammate"));
    }
}
