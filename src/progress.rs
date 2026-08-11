use serde_json::Value;

pub const PRIMARY_MARK: &str = "🐦‍🔥";
pub const PARTNER_MARK: &str = "🪶";

const DETAIL_LIMIT: usize = 900;
const COMPACT_DETAIL_LIMIT: usize = 160;
const REASONING_ENTRY_LIMIT: usize = 1600;
const REASONING_LOG_LIMIT: usize = 12_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Primary,
    Partner,
}

#[derive(Clone)]
struct Lane {
    label: String,
    activity: String,
    detail: String,
}

pub struct ProgressBoard {
    primary: Option<Lane>,
    partner: Option<Lane>,
    latest: Option<Role>,
    detailed: bool,
}

impl Default for ProgressBoard {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ProgressBoard {
    pub fn new(detailed: bool) -> Self {
        ProgressBoard {
            primary: None,
            partner: None,
            latest: None,
            detailed,
        }
    }
    pub fn update(&mut self, name: &str, args: &Value) -> String {
        if let Some(detailed) = args.get("_reasoning_visible").and_then(Value::as_bool) {
            self.detailed = detailed;
        }
        if name == "colab_start" {
            self.primary = label_arg(args, "primary").map(|label| Lane {
                label,
                activity: "🧠 preparing assigned work…".to_string(),
                detail: "Establishing the shared task and team split".to_string(),
            });
            self.partner = label_arg(args, "partner").map(|label| Lane {
                label,
                activity: "🧠 preparing assigned work…".to_string(),
                detail: "Establishing the shared task and team split".to_string(),
            });
            self.latest = None;
            return self.render();
        }

        let speaker = args
            .get("_speaker")
            .and_then(Value::as_str)
            .map(normalize_speaker)
            .filter(|value| !value.is_empty());
        let role = role_arg(args);
        let (activity, detail) = activity(name, args);
        let Some(role) = role else {
            self.primary = None;
            self.partner = None;
            let limit = if self.detailed {
                DETAIL_LIMIT
            } else {
                COMPACT_DETAIL_LIMIT
            };
            return render_single(&activity, &detail, limit);
        };
        let label = speaker.unwrap_or_else(|| match role {
            Role::Primary => "Primary".to_string(),
            Role::Partner => "Partner".to_string(),
        });
        let lane = Lane {
            label,
            activity,
            detail,
        };
        match role {
            Role::Primary => self.primary = Some(lane),
            Role::Partner => self.partner = Some(lane),
        }
        self.latest = Some(role);
        self.render()
    }

    fn render(&self) -> String {
        let limit = if self.detailed {
            DETAIL_LIMIT
        } else {
            COMPACT_DETAIL_LIMIT
        };
        let mut lanes = Vec::new();
        if let Some(primary) = &self.primary {
            lanes.push(render_lane(PRIMARY_MARK, primary, limit));
        }
        if let Some(partner) = &self.partner {
            lanes.push(render_lane(PARTNER_MARK, partner, limit));
        }
        if lanes.is_empty() {
            return "🧠 working…".to_string();
        }
        lanes.join("\n")
    }
}

pub fn compact_transcript(rounds: &[crate::colab::Round], primary: &str) -> String {
    let mut lines = Vec::new();
    for round in rounds {
        let speaker = normalize_speaker(&round.speaker);
        if speaker.is_empty() {
            continue;
        }
        let mark = mark_for(&speaker, primary);
        let reasoning = round.thinking.trim();
        let (activity, detail) = if reasoning.is_empty() {
            ("🧠 thinking…", round.text.trim())
        } else {
            ("🧠 thinking…", reasoning)
        };
        let detail = detail_line(detail, COMPACT_DETAIL_LIMIT);
        if detail.is_empty() {
            continue;
        }
        lines.push(format!("{mark} {speaker}: {activity}\n{detail}"));
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

pub fn public_reasoning(
    rounds: &[crate::colab::Round],
    primary: &str,
    events: &[(u64, String, Value)],
) -> String {
    let mut entries = Vec::new();
    if events.is_empty() {
        for round in rounds {
            let speaker = normalize_speaker(&round.speaker);
            let mark = mark_for(&speaker, primary);
            let reasoning = round.thinking.trim();
            let said = clipped(round.text.trim(), REASONING_ENTRY_LIMIT);
            if reasoning.is_empty() {
                if said.is_empty() {
                    continue;
                }
                entries.push(format!(
                    "{mark} {speaker}: 🗣️ public rationale (this model returned no reasoning)\n{said}"
                ));
                continue;
            }
            let thought = clipped(reasoning, REASONING_ENTRY_LIMIT);
            entries.push(format!("{mark} {speaker}: 🧠 thinking…\n{thought}"));
            if !said.is_empty() {
                entries.push(format!("{mark} {speaker}: 💬 said…\n{said}"));
            }
        }
    } else {
        let mut ordered = events.to_vec();
        ordered.sort_by_key(|event| event.0);
        for (_, name, args) in ordered {
            if name == "colab_start" || name == "colab_say" {
                continue;
            }
            let speaker = args
                .get("_speaker")
                .and_then(Value::as_str)
                .map(normalize_speaker)
                .unwrap_or_default();
            if speaker.is_empty() {
                continue;
            }
            let (verb, detail) = activity(&name, &args);
            let detail = one_line(&detail, REASONING_ENTRY_LIMIT);
            entries.push(if detail.is_empty() {
                format!("{} {speaker}: {verb}", mark_for(&speaker, primary))
            } else {
                format!(
                    "{} {speaker}: {verb}\n{detail}",
                    mark_for(&speaker, primary)
                )
            });
        }
    }

    let mut body = String::new();
    for entry in entries {
        if body.chars().count() + entry.chars().count() + 2 > REASONING_LOG_LIMIT {
            break;
        }
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&entry);
    }
    if body.is_empty() {
        String::new()
    } else {
        format!("Team work log\n\n{body}")
    }
}

fn mark_for(speaker: &str, primary: &str) -> &'static str {
    if speaker.starts_with(primary) {
        PRIMARY_MARK
    } else {
        PARTNER_MARK
    }
}

fn render_lane(mark: &str, lane: &Lane, limit: usize) -> String {
    let head = format!("{mark} {}: {}", lane.label, lane.activity);
    let detail = detail_line(&lane.detail, limit);
    format!("{head}\n{detail}")
}

fn one_line(value: &str, limit: usize) -> String {
    let flat = value
        .split('\n')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    clipped(flat.trim(), limit)
}

fn detail_line(value: &str, limit: usize) -> String {
    let detail = one_line(value, limit);
    if detail.is_empty() {
        "Working on the assigned task".to_string()
    } else {
        detail
    }
}

fn render_single(activity: &str, detail: &str, limit: usize) -> String {
    format!("{activity}\n{}", detail_line(detail, limit))
}

fn role_arg(args: &Value) -> Option<Role> {
    match args.get("_role").and_then(Value::as_str) {
        Some("main") | Some("primary") => Some(Role::Primary),
        Some("partner") => Some(Role::Partner),
        _ => None,
    }
}

fn label_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(normalize_speaker)
        .filter(|value| !value.is_empty())
}

fn normalize_speaker(value: &str) -> String {
    value
        .trim()
        .strip_prefix("partner:")
        .unwrap_or(value.trim())
        .trim_end_matches(" (planning)")
        .trim_end_matches(" planning")
        .trim_end_matches(" (solo fallback)")
        .to_string()
}

fn activity(name: &str, args: &Value) -> (String, String) {
    let detail = detail(args);
    let text = if name == "thinking" || name == "colab_reasoning" || name == "colab_say" {
        "🧠 thinking…"
    } else if name == "colab_status" {
        "🧠 coordinating…"
    } else if name == "tool_result" {
        "🔥 command finished…"
    } else if name.starts_with("browser") {
        "🔥 using the browser…"
    } else if name.starts_with("agent") || name == "subtask" {
        "🔥 working with a helper…"
    } else {
        match name {
            "shell" => "🔥 running a command…",
            "write_file" | "edit_file" => "🔥 writing code…",
            "read_file" | "list_dir" | "grep" | "glob" => "🔥 reading code…",
            "web_search" => "🔥 searching the web…",
            "web_fetch" | "fetch" | "http_get" => "🔥 reading a web page…",
            "memory_save" | "memory_search" => "🔥 checking memory…",
            "image" | "image_generate" => "🔥 creating an image…",
            "tts" | "speak" => "🔥 creating audio…",
            "video" | "video_generate" => "🔥 creating video…",
            "music" | "music_generate" => "🔥 creating music…",
            "transcribe" => "🔥 transcribing audio…",
            "task_add" | "task_list" | "cron_add" => "🔥 organizing tasks…",
            _ => "🔥 working…",
        }
    };
    (text.to_string(), detail)
}

fn detail(args: &Value) -> String {
    let value = [
        "result", "command", "task", "query", "url", "path", "prompt", "id", "note",
    ]
    .iter()
    .find_map(|key| args.get(*key).and_then(Value::as_str))
    .unwrap_or("")
    .trim();
    clipped(value, DETAIL_LIMIT)
}

fn clipped(value: &str, limit: usize) -> String {
    let clean = value.replace('\r', "");
    let count = clean.chars().count();
    if count <= limit {
        return clean;
    }
    let mut short: String = clean.chars().take(limit).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_lanes_use_primary_and_partner_marks() {
        let mut board = ProgressBoard::new(true);
        let start = board.update(
            "colab_start",
            &serde_json::json!({"primary":"openai/gpt", "partner":"anthropic/claude"}),
        );
        assert!(start.contains("🐦‍🔥 openai/gpt"), "{start}");
        assert!(start.contains("🪶 anthropic/claude"), "{start}");
        let partner = board.update(
            "shell",
            &serde_json::json!({"_speaker":"partner:anthropic/claude", "_role":"partner", "command":"cargo test"}),
        );
        assert!(partner.contains("🪶 anthropic/claude: 🔥 running a command…"));
        assert!(partner.contains("🐦‍🔥 openai/gpt: 🧠 preparing assigned work…"));
        assert!(partner.contains("cargo test"));
    }

    #[test]
    fn reasoning_off_shows_both_lanes_with_one_detail_line_each() {
        let mut board = ProgressBoard::new(false);
        let start = board.update(
            "colab_start",
            &serde_json::json!({"primary":"openai/gpt-5.6-sol", "partner":"anthropic/claude-opus-5"}),
        );
        assert_eq!(start.lines().count(), 4, "{start}");
        let command = board.update(
            "shell",
            &serde_json::json!({"_speaker":"openai/gpt-5.6-sol", "_role":"main", "command":"cargo test --all-targets"}),
        );
        let lines: Vec<&str> = command.lines().collect();
        assert_eq!(lines.len(), 4, "{command}");
        assert_eq!(
            lines[0],
            "🐦\u{200d}🔥 openai/gpt-5.6-sol: 🔥 running a command…"
        );
        assert_eq!(lines[1], "cargo test --all-targets");
        assert_eq!(
            lines[2],
            "🪶 anthropic/claude-opus-5: 🧠 preparing assigned work…"
        );
        let partner = board.update(
            "colab_reasoning",
            &serde_json::json!({"_speaker":"partner:anthropic/claude-opus-5", "_role":"partner", "note":"weighing the renderer contract"}),
        );
        let lines: Vec<&str> = partner.lines().collect();
        assert_eq!(lines.len(), 4, "{partner}");
        assert_eq!(lines[2], "🪶 anthropic/claude-opus-5: 🧠 thinking…");
        assert_eq!(lines[3], "weighing the renderer contract");
        assert!(
            lines[0].starts_with("🐦\u{200d}🔥 openai/gpt-5.6-sol:"),
            "the other seat must never vanish: {partner}"
        );
    }

    #[test]
    fn reasoning_off_keeps_each_detail_to_a_single_short_line() {
        let mut board = ProgressBoard::new(false);
        board.update(
            "colab_start",
            &serde_json::json!({"primary":"openai/gpt", "partner":"anthropic/claude"}),
        );
        let out = board.update(
            "shell",
            &serde_json::json!({"_speaker":"openai/gpt", "_role":"main",
                "command":format!("cargo test {}", "x".repeat(400))}),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(
            lines[1].chars().count() <= COMPACT_DETAIL_LIMIT + 1,
            "{out}"
        );
        assert!(lines[1].ends_with('…'), "{out}");
        let multi = board.update(
            "colab_reasoning",
            &serde_json::json!({"_speaker":"partner:anthropic/claude", "_role":"partner",
                "note":"first thought\nsecond thought\nthird thought"}),
        );
        let lines: Vec<&str> = multi.lines().collect();
        assert_eq!(lines.len(), 4, "{multi}");
        assert_eq!(lines[3], "first thought second thought third thought");
    }

    #[test]
    fn reasoning_on_keeps_the_same_shape_with_fuller_detail() {
        let mut board = ProgressBoard::new(true);
        board.update(
            "colab_start",
            &serde_json::json!({"primary":"openai/gpt", "partner":"anthropic/claude"}),
        );
        let long = "y".repeat(400);
        let out = board.update(
            "shell",
            &serde_json::json!({"_speaker":"openai/gpt", "_role":"main",
                "command":format!("cargo test {long}")}),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(
            lines[1].contains(&long),
            "reasoning on must not truncate: {out}"
        );
    }

    #[test]
    fn public_reasoning_is_detailed_bounded_and_role_aware() {
        let rounds = vec![
            crate::colab::Round {
                speaker: "anthropic/claude (planning)".to_string(),
                text: "I will inspect the renderer because that keeps transport concerns separate."
                    .to_string(),
                thinking: String::new(),
            },
            crate::colab::Round {
                speaker: "openai/gpt".to_string(),
                text: "I verified the shared event boundary and implemented the Rust model."
                    .to_string(),
                thinking: String::new(),
            },
        ];
        let events = vec![
            (
                1,
                "colab_reasoning".to_string(),
                serde_json::json!({"_speaker":"partner:anthropic/claude", "_role":"partner", "note":"I will inspect the renderer because that keeps transport concerns separate."}),
            ),
            (
                2,
                "shell".to_string(),
                serde_json::json!({"_speaker":"openai/gpt", "_role":"main", "command":"cargo test"}),
            ),
            (
                3,
                "tool_result".to_string(),
                serde_json::json!({"_speaker":"openai/gpt", "_role":"main", "result":"all tests passed"}),
            ),
            (
                4,
                "colab_reasoning".to_string(),
                serde_json::json!({"_speaker":"openai/gpt", "_role":"main", "note":"I verified the shared event boundary and implemented the Rust model."}),
            ),
        ];
        let log = public_reasoning(&rounds, "openai/gpt", &events);
        assert!(log.contains("🪶 anthropic/claude: 🧠 thinking…"));
        assert!(log.contains("🐦‍🔥 openai/gpt: 🧠 thinking…"));
        assert!(log.contains("transport concerns separate"));
        assert!(log.contains("🔥 running a command…\ncargo test"));
        assert!(log.contains("🔥 command finished…\nall tests passed"));
        let command = log.find("cargo test").unwrap();
        let result = log.find("all tests passed").unwrap();
        let rationale = log.find("I verified the shared event boundary").unwrap();
        assert!(command < result && result < rationale, "{log}");
    }

    #[test]
    fn tool_result_keeps_one_safe_detail_line() {
        let mut board = ProgressBoard::new(true);
        let line = board.update(
            "tool_result",
            &serde_json::json!({"result":"tests passed\n12 assertions", "outcome":"ok"}),
        );
        assert_eq!(line.lines().count(), 2, "{line}");
        assert_eq!(line.lines().next(), Some("🔥 command finished…"));
        assert!(line.ends_with("tests passed 12 assertions"), "{line}");
    }

    #[test]
    fn an_event_without_arguments_still_has_exactly_one_detail_line() {
        let line = ProgressBoard::default().update("image_generate", &serde_json::json!({}));
        assert_eq!(line.lines().count(), 2, "{line}");
        assert_eq!(line.lines().nth(1), Some("Working on the assigned task"));
    }

    #[test]
    fn reasoning_off_single_model_keeps_one_detail_line() {
        let line = ProgressBoard::default().update(
            "shell",
            &serde_json::json!({"command":"cargo test --all-targets"}),
        );
        assert_eq!(line, "🔥 running a command…\ncargo test --all-targets");
    }

    #[test]
    fn reasoning_on_keeps_single_model_command_detail() {
        let mut board = ProgressBoard::new(true);
        let line = board.update(
            "shell",
            &serde_json::json!({"command":"cargo test --all-targets"}),
        );
        assert_eq!(line, "🔥 running a command…\ncargo test --all-targets");
    }
}

#[cfg(test)]
mod format_proof {
    use super::*;

    #[test]
    fn renders_the_exact_requested_transcript_shape() {
        let mut board = ProgressBoard::new(false);
        board.update(
            "colab_start",
            &serde_json::json!({"primary":"openai/gpt-5.6-sol","partner":"anthropic/claude-opus-5"}),
        );
        let a = board.update(
            "shell",
            &serde_json::json!({"_speaker":"openai/gpt-5.6-sol","_role":"main","command":"cargo test"}),
        );
        let b = board.update(
            "shell",
            &serde_json::json!({"_speaker":"partner:anthropic/claude-opus-5","_role":"partner","command":"cargo clippy"}),
        );
        let c = board.update(
            "thinking",
            &serde_json::json!({"_speaker":"openai/gpt-5.6-sol","_role":"main","note":"tracing the scheduler"}),
        );
        println!("--- primary runs a command ---\n{a}");
        println!("--- partner runs a command ---\n{b}");
        println!("--- primary thinks ---\n{c}");
        for frame in [&a, &b, &c] {
            assert_eq!(
                frame.lines().count(),
                4,
                "two lanes, one detail each: {frame}"
            );
        }
        assert!(a.starts_with("🐦\u{200d}🔥 openai/gpt-5.6-sol: 🔥 running a command…\ncargo test"));
        assert!(b.contains("🪶 anthropic/claude-opus-5: 🔥 running a command…\ncargo clippy"));
        assert!(
            c.starts_with("🐦\u{200d}🔥 openai/gpt-5.6-sol: 🧠 thinking…\ntracing the scheduler")
        );
    }
}

#[cfg(test)]
mod reasoning_display {
    use super::*;

    fn round(speaker: &str, text: &str, thinking: &str) -> crate::colab::Round {
        crate::colab::Round {
            speaker: speaker.to_string(),
            text: text.to_string(),
            thinking: thinking.to_string(),
        }
    }

    #[test]
    fn reasoning_shows_the_models_actual_thinking_not_only_its_answer() {
        let rounds = vec![round(
            "alpha/one",
            "I will take the scheduler.",
            "Weighing which half has fewer merge conflicts before answering.",
        )];
        let log = public_reasoning(&rounds, "alpha/one", &[]);
        assert!(log.contains("🧠 thinking…"), "{log}");
        assert!(
            log.contains("Weighing which half has fewer merge conflicts"),
            "provider reasoning must be shown: {log}"
        );
        assert!(log.contains("💬 said…"), "{log}");
        assert!(log.contains("I will take the scheduler."), "{log}");
        let thinking_at = log.find("Weighing which half").unwrap_or_default();
        let said_at = log.find("I will take the scheduler.").unwrap_or_default();
        assert!(thinking_at < said_at, "thinking precedes the answer: {log}");
    }

    #[test]
    fn a_model_without_reasoning_is_labelled_honestly_and_never_fabricated() {
        let rounds = vec![round("beta/two", "Renderer is done.", "")];
        let log = public_reasoning(&rounds, "alpha/one", &[]);
        assert!(
            log.contains("returned no reasoning"),
            "absence must be explicit: {log}"
        );
        assert!(!log.contains("🧠 thinking…"), "never fake thinking: {log}");
        assert!(log.contains("Renderer is done."), "{log}");
    }

    #[test]
    fn both_seats_reasoning_appears_with_their_own_marks() {
        let rounds = vec![
            round("alpha/one", "answer one", "reasoning one"),
            round("beta/two", "answer two", "reasoning two"),
        ];
        let log = public_reasoning(&rounds, "alpha/one", &[]);
        assert!(
            log.contains(&format!("{PRIMARY_MARK} alpha/one: 🧠 thinking…")),
            "{log}"
        );
        assert!(
            log.contains(&format!("{PARTNER_MARK} beta/two: 🧠 thinking…")),
            "{log}"
        );
        assert!(
            log.contains("reasoning one") && log.contains("reasoning two"),
            "{log}"
        );
    }
}

#[cfg(test)]
mod compact_transcript_tests {
    use super::*;

    fn round(speaker: &str, text: &str, thinking: &str) -> crate::colab::Round {
        crate::colab::Round {
            speaker: speaker.to_string(),
            text: text.to_string(),
            thinking: thinking.to_string(),
        }
    }

    #[test]
    fn reasoning_off_keeps_a_two_line_entry_for_every_seat() {
        let rounds = vec![
            round(
                "openai/gpt-5.6-sol (planning)",
                "answer one",
                "tracing the scheduler",
            ),
            round(
                "anthropic/claude-opus-5 (planning)",
                "answer two",
                "auditing the renderer",
            ),
        ];
        let out = compact_transcript(&rounds, "openai/gpt-5.6-sol");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert_eq!(lines[0], "🐦\u{200d}🔥 openai/gpt-5.6-sol: 🧠 thinking…");
        assert_eq!(lines[1], "tracing the scheduler");
        assert_eq!(lines[2], "🪶 anthropic/claude-opus-5: 🧠 thinking…");
        assert_eq!(lines[3], "auditing the renderer");
    }

    #[test]
    fn reasoning_off_details_stay_on_one_short_line() {
        let rounds = vec![round(
            "openai/gpt",
            "answer",
            &format!("first\nsecond\n{}", "x".repeat(400)),
        )];
        let out = compact_transcript(&rounds, "openai/gpt");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert!(
            lines[1].chars().count() <= COMPACT_DETAIL_LIMIT + 1,
            "{out}"
        );
        assert!(lines[1].ends_with('…'), "{out}");
    }

    #[test]
    fn a_seat_without_reasoning_still_appears_with_its_answer() {
        let rounds = vec![round("anthropic/claude", "the renderer is fixed", "")];
        let out = compact_transcript(&rounds, "openai/gpt");
        assert!(
            out.starts_with("🪶 anthropic/claude: 🧠 thinking…"),
            "{out}"
        );
        assert!(out.contains("the renderer is fixed"), "{out}");
        assert_eq!(out.lines().count(), 2, "{out}");
    }

    #[test]
    fn reasoning_on_shows_strictly_more_than_reasoning_off() {
        let rounds = vec![round(
            "openai/gpt",
            "the answer",
            "the private-safe rationale",
        )];
        let off = compact_transcript(&rounds, "openai/gpt");
        let on = public_reasoning(&rounds, "openai/gpt", &[]);
        assert!(
            on.chars().count() > off.chars().count(),
            "on={on}\noff={off}"
        );
        assert!(on.contains("💬 said…"), "{on}");
        assert!(!off.contains("💬 said…"), "{off}");
    }
}
