use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const STATUSES: &[&str] = &["pending", "done", "dismissed", "expired"];

#[derive(Debug, Clone, PartialEq)]
pub struct Commitment {
    pub id: u64,
    pub text: String,
    pub due_ms: u64,
    pub status: String,
    pub created_ms: u64,
    pub scope: String,
}

pub fn known_status(s: &str) -> bool {
    STATUSES.contains(&s)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn store_path() -> PathBuf {
    crate::config::home().join("commitments.json")
}

pub fn load(path: &Path) -> Vec<Commitment> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    v.get("items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let text = c.get("text").and_then(Value::as_str)?;
                    if text.trim().is_empty() {
                        return None;
                    }
                    let status = c
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .to_string();
                    Some(Commitment {
                        id: c.get("id").and_then(Value::as_u64).unwrap_or(0),
                        text: text.to_string(),
                        due_ms: c.get("due_ms").and_then(Value::as_u64).unwrap_or(0),
                        status: if known_status(&status) {
                            status
                        } else {
                            "pending".to_string()
                        },
                        created_ms: c.get("created_ms").and_then(Value::as_u64).unwrap_or(0),
                        scope: c
                            .get("scope")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(path: &Path, items: &[Commitment]) -> Result<(), String> {
    let arr: Vec<Value> = items
        .iter()
        .map(|c| {
            json!({"id": c.id, "text": c.text, "due_ms": c.due_ms,
                   "status": c.status, "created_ms": c.created_ms, "scope": c.scope})
        })
        .collect();
    let doc = json!({"v": 1, "items": arr});
    let body = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn parse_due(raw: &str) -> Result<u64, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("empty duration".into());
    }
    let (num, mult) = match t.chars().last() {
        Some('m') => (&t[..t.len() - 1], 60_000u64),
        Some('h') => (&t[..t.len() - 1], 3_600_000),
        Some('d') => (&t[..t.len() - 1], 86_400_000),
        _ => (t, 60_000),
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("cannot read '{raw}' as a duration: try 30m, 2h, or 3d"))?;
    if n == 0 {
        return Err("duration must be greater than zero".into());
    }
    n.checked_mul(mult)
        .ok_or_else(|| format!("duration '{raw}' is too large"))
}

pub fn add(path: &Path, text: &str, due_in_ms: u64, scope: &str) -> Result<u64, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("a commitment needs text".into());
    }
    let mut items = load(path);
    let id = items.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let now = now_ms();
    items.push(Commitment {
        id,
        text: crate::security::redact(text),
        due_ms: now + due_in_ms,
        status: "pending".into(),
        created_ms: now,
        scope: scope.to_string(),
    });
    save(path, &items)?;
    Ok(id)
}

pub fn set_status(path: &Path, id: u64, status: &str) -> Result<(), String> {
    if !known_status(status) {
        return Err(format!("status must be one of {STATUSES:?}"));
    }
    let mut items = load(path);
    let Some(c) = items.iter_mut().find(|c| c.id == id) else {
        return Err(format!("no commitment #{id}"));
    };
    c.status = status.to_string();
    save(path, &items)
}

pub fn due_now(items: &[Commitment], now: u64) -> Vec<&Commitment> {
    let mut out: Vec<&Commitment> = items
        .iter()
        .filter(|c| c.status == "pending" && c.due_ms <= now)
        .collect();
    out.sort_by_key(|c| c.due_ms);
    out
}

fn when(due_ms: u64, now: u64) -> String {
    if due_ms <= now {
        let late = (now - due_ms) / 1000;
        return format!("due {}", crate::scheduler::time_ago(late));
    }
    let left = (due_ms - now) / 60_000;
    if left < 60 {
        format!("in {left}m")
    } else if left < 1440 {
        format!("in {}h", left / 60)
    } else {
        format!("in {}d", left / 1440)
    }
}

pub fn list_text(items: &[Commitment], status: Option<&str>, now: u64) -> String {
    let shown: Vec<&Commitment> = items
        .iter()
        .filter(|c| status.map(|s| c.status == s).unwrap_or(true))
        .collect();
    if shown.is_empty() {
        return match status {
            Some(s) => format!("no {s} commitments\n"),
            None => "no commitments yet; add one with `phoenix commitments add`\n".to_string(),
        };
    }
    let mut out = format!("{} commitments\n", shown.len());
    for c in shown {
        let mark = if c.status == "pending" && c.due_ms <= now {
            "!"
        } else {
            " "
        };
        out.push_str(&format!(
            "{mark} #{:<4}{:<11}{:<12}{}\n",
            c.id,
            c.status,
            when(c.due_ms, now),
            crate::security::one_line(&c.text, 60)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("phx-commit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn durations_accept_minutes_hours_and_days() {
        assert_eq!(parse_due("30m").expect("m"), 30 * 60_000);
        assert_eq!(parse_due("2h").expect("h"), 2 * 3_600_000);
        assert_eq!(parse_due("3d").expect("d"), 3 * 86_400_000);
        assert_eq!(parse_due("45").expect("bare"), 45 * 60_000);
    }

    #[test]
    fn a_bad_duration_explains_the_format_instead_of_defaulting() {
        let e = parse_due("soon").expect_err("must fail");
        assert!(e.contains("30m"), "{e}");
        assert!(parse_due("0m").is_err(), "zero is not a deadline");
        assert!(parse_due("").is_err());
    }

    #[test]
    fn a_huge_duration_is_refused_rather_than_wrapping() {
        assert!(parse_due(&format!("{}d", u64::MAX)).is_err());
    }

    #[test]
    fn add_then_load_round_trips() {
        let p = tmp("round");
        let id = add(
            &p,
            "call the bank",
            parse_due("1h").expect("dur"),
            "telegram:1",
        )
        .expect("add");
        assert_eq!(id, 1);
        let items = load(&p);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "call the bank");
        assert_eq!(items[0].status, "pending");
        assert_eq!(items[0].scope, "telegram:1");
        assert!(items[0].due_ms > items[0].created_ms);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn ids_keep_climbing_so_a_removed_entry_never_reuses_one() {
        let p = tmp("ids");
        add(&p, "one", 60_000, "").expect("a");
        let second = add(&p, "two", 60_000, "").expect("b");
        assert_eq!(second, 2);
        let mut items = load(&p);
        items.retain(|c| c.id != 2);
        save(&p, &items).expect("save");
        let third = add(&p, "three", 60_000, "").expect("c");
        assert_eq!(third, 2, "max+1 over what remains");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_empty_commitment_is_refused() {
        let p = tmp("empty");
        assert!(add(&p, "   ", 60_000, "").is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn secrets_in_the_text_are_redacted_before_they_reach_disk() {
        let p = tmp("redact");
        add(&p, "renew sk-ant-api03-abcdefghijklmnop", 60_000, "").expect("add");
        let raw = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(
            !raw.contains("abcdefghijklmnop"),
            "a key pasted into a reminder must not persist: {raw}"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn only_pending_and_past_due_entries_come_back_as_due() {
        let now = 1_000_000u64;
        let items = vec![
            Commitment {
                id: 1,
                text: "late".into(),
                due_ms: now - 1,
                status: "pending".into(),
                created_ms: 0,
                scope: String::new(),
            },
            Commitment {
                id: 2,
                text: "future".into(),
                due_ms: now + 60_000,
                status: "pending".into(),
                created_ms: 0,
                scope: String::new(),
            },
            Commitment {
                id: 3,
                text: "already handled".into(),
                due_ms: now - 500,
                status: "done".into(),
                created_ms: 0,
                scope: String::new(),
            },
        ];
        let due = due_now(&items, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, 1);
    }

    #[test]
    fn due_entries_come_back_oldest_first() {
        let now = 1_000_000u64;
        let mk = |id: u64, due: u64| Commitment {
            id,
            text: format!("c{id}"),
            due_ms: due,
            status: "pending".into(),
            created_ms: 0,
            scope: String::new(),
        };
        let items = vec![mk(1, now - 10), mk(2, now - 900), mk(3, now - 100)];
        let due = due_now(&items, now);
        assert_eq!(
            due.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![2, 3, 1],
            "the longest-overdue follow-up is reported first"
        );
    }

    #[test]
    fn status_changes_persist_and_unknown_statuses_are_refused() {
        let p = tmp("status");
        let id = add(&p, "thing", 60_000, "").expect("add");
        set_status(&p, id, "done").expect("done");
        assert_eq!(load(&p)[0].status, "done");
        assert!(set_status(&p, id, "maybe").is_err());
        assert!(set_status(&p, 999, "done").is_err(), "unknown id");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_corrupt_store_reads_as_empty_rather_than_panicking() {
        let p = tmp("corrupt");
        std::fs::write(&p, "{not json").expect("write");
        assert!(load(&p).is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_entry_with_an_unknown_status_on_disk_reads_back_as_pending() {
        let p = tmp("badstatus");
        std::fs::write(
            &p,
            r#"{"v":1,"items":[{"id":1,"text":"x","due_ms":1,"status":"weird"}]}"#,
        )
        .expect("write");
        assert_eq!(load(&p)[0].status, "pending");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn overdue_entries_are_marked_in_the_listing() {
        let now = 1_000_000u64;
        let items = vec![Commitment {
            id: 1,
            text: "overdue thing".into(),
            due_ms: now - 60_000,
            status: "pending".into(),
            created_ms: 0,
            scope: String::new(),
        }];
        let text = list_text(&items, None, now);
        assert!(text.starts_with("1 commitments"), "{text}");
        assert!(
            text.contains('!'),
            "an overdue entry must stand out: {text}"
        );
    }

    #[test]
    fn an_empty_list_says_how_to_add_one() {
        assert!(list_text(&[], None, 0).contains("commitments add"));
        assert!(list_text(&[], Some("done"), 0).contains("no done"));
    }
}
