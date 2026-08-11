use std::fs;
use std::path::Path;

use serde_json::{json, Value};

fn board_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub const STATUSES: [&str; 4] = ["todo", "doing", "blocked", "done"];
pub const PRIORITIES: [&str; 3] = ["low", "normal", "high"];

#[derive(Debug, Clone)]
pub struct Card {
    pub id: u64,
    pub title: String,
    pub notes: String,
    pub status: String,
    pub priority: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Default)]
pub struct Board {
    pub next_id: u64,
    pub cards: Vec<Card>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn card_to_json(c: &Card) -> Value {
    json!({
        "id": c.id,
        "title": c.title,
        "notes": c.notes,
        "status": c.status,
        "priority": c.priority,
        "created_ms": c.created_ms,
        "updated_ms": c.updated_ms,
    })
}

fn card_from_json(v: &Value) -> Option<Card> {
    Some(Card {
        id: v["id"].as_u64()?,
        title: v["title"].as_str()?.to_string(),
        notes: v["notes"].as_str().unwrap_or("").to_string(),
        status: v["status"].as_str().unwrap_or("todo").to_string(),
        priority: v["priority"].as_str().unwrap_or("normal").to_string(),
        created_ms: v["created_ms"].as_u64().unwrap_or(0),
        updated_ms: v["updated_ms"].as_u64().unwrap_or(0),
    })
}

pub fn load(path: &Path) -> Board {
    let Ok(text) = fs::read_to_string(path) else {
        return Board {
            next_id: 1,
            cards: Vec::new(),
        };
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Board {
            next_id: 1,
            cards: Vec::new(),
        };
    };
    let cards: Vec<Card> = v["cards"]
        .as_array()
        .map(|a| a.iter().filter_map(card_from_json).collect())
        .unwrap_or_default();
    let max_id = cards.iter().map(|c| c.id).max().unwrap_or(0);
    Board {
        next_id: v["next_id"].as_u64().unwrap_or(max_id + 1).max(max_id + 1),
        cards,
    }
}

pub fn save(path: &Path, board: &Board) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let v = json!({
        "next_id": board.next_id,
        "cards": board.cards.iter().map(card_to_json).collect::<Vec<_>>(),
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    crate::security::write_atomic(path, body.as_bytes(), Some(0o600)).map_err(|e| e.to_string())
}

pub fn add(path: &Path, title: &str, notes: &str, priority: &str) -> Result<u64, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("empty title".into());
    }
    if !PRIORITIES.contains(&priority) {
        return Err(format!("priority must be one of {PRIORITIES:?}"));
    }
    let _guard = board_guard();
    let mut board = load(path);
    let id = board.next_id;
    board.next_id += 1;
    let now = now_ms();
    board.cards.push(Card {
        id,
        title: title.to_string(),
        notes: notes.trim().to_string(),
        status: "todo".into(),
        priority: priority.to_string(),
        created_ms: now,
        updated_ms: now,
    });
    save(path, &board)?;
    Ok(id)
}

pub fn list(path: &Path, status: Option<&str>) -> Result<String, String> {
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("status must be one of {STATUSES:?}"));
        }
    }
    let board = load(path);
    let prio_rank = |p: &str| PRIORITIES.iter().position(|x| *x == p).unwrap_or(1);
    let mut cards: Vec<&Card> = board
        .cards
        .iter()
        .filter(|c| status.is_none_or(|s| c.status == s))
        .collect();
    cards.sort_by_key(|c| {
        (
            c.status == "done",
            std::cmp::Reverse(prio_rank(&c.priority)),
            c.created_ms,
        )
    });
    if cards.is_empty() {
        return Ok("(no cards)".into());
    }
    Ok(cards
        .iter()
        .map(|c| {
            let notes = if c.notes.is_empty() {
                String::new()
            } else {
                format!(" | {}", c.notes)
            };
            format!(
                "#{} [{}] ({}) {}{notes}",
                c.id, c.status, c.priority, c.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn update(
    path: &Path,
    id: u64,
    status: Option<&str>,
    title: Option<&str>,
    notes: Option<&str>,
    priority: Option<&str>,
) -> Result<String, String> {
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("status must be one of {STATUSES:?}"));
        }
    }
    if let Some(p) = priority {
        if !PRIORITIES.contains(&p) {
            return Err(format!("priority must be one of {PRIORITIES:?}"));
        }
    }
    if let Some(t) = title {
        if t.trim().is_empty() {
            return Err("empty title".into());
        }
    }
    let _guard = board_guard();
    let mut board = load(path);
    let Some(card) = board.cards.iter_mut().find(|c| c.id == id) else {
        return Err(format!("no card #{id}"));
    };
    if let Some(s) = status {
        card.status = s.to_string();
    }
    if let Some(t) = title {
        card.title = t.trim().to_string();
    }
    if let Some(n) = notes {
        card.notes = n.trim().to_string();
    }
    if let Some(p) = priority {
        card.priority = p.to_string();
    }
    card.updated_ms = now_ms();
    let line = format!(
        "#{} [{}] ({}) {}",
        card.id, card.status, card.priority, card.title
    );
    save(path, &board)?;
    Ok(line)
}

pub fn show(path: &Path, id: u64) -> Result<String, String> {
    let board = load(path);
    let Some(c) = board.cards.iter().find(|c| c.id == id) else {
        return Err(format!("no card #{id}"));
    };
    Ok(format!(
        "#{} {}\n  status    {}\n  priority  {}\n  notes     {}\n",
        c.id,
        c.title,
        c.status,
        c.priority,
        if c.notes.is_empty() {
            "(none)"
        } else {
            &c.notes
        }
    ))
}

pub fn list_json(path: &Path, status: Option<&str>) -> Result<String, String> {
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("status must be one of {STATUSES:?}"));
        }
    }
    let board = load(path);
    let cards: Vec<Value> = board
        .cards
        .iter()
        .filter(|c| status.is_none_or(|s| c.status == s))
        .map(card_to_json)
        .collect();
    let mut out =
        serde_json::to_string_pretty(&json!({"cards": cards})).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

pub fn ready(path: &Path, limit: usize) -> Vec<Card> {
    let board = load(path);
    let prio_rank = |p: &str| PRIORITIES.iter().position(|x| *x == p).unwrap_or(1);
    let mut cards: Vec<Card> = board
        .cards
        .into_iter()
        .filter(|c| c.status == "todo")
        .collect();
    cards.sort_by_key(|c| (std::cmp::Reverse(prio_rank(&c.priority)), c.created_ms));
    cards.truncate(limit);
    cards
}

pub fn dispatch_text(cards: &[Card]) -> String {
    if cards.is_empty() {
        return "no todo cards are ready to dispatch\n".to_string();
    }
    let mut out = format!("{} card(s) ready\n", cards.len());
    for c in cards {
        out.push_str(&format!(
            "  #{:<4}({:<6}) {}\n",
            c.id,
            c.priority,
            crate::security::one_line(&c.title, 60)
        ));
    }
    out.push_str("\nstart one with: phoenix run \"work card #ID\"\n");
    out
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;

    #[test]
    fn parallel_adds_never_reuse_an_id_or_lose_a_card() {
        let d = std::env::temp_dir().join(format!("px-board-par-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let path = d.join("board.json");

        let mut handles = Vec::new();
        for i in 0..12 {
            let p = path.clone();
            handles.push(std::thread::spawn(move || {
                add(&p, &format!("card {i}"), "", "normal")
            }));
        }
        let ids: Vec<u64> = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(Result::ok)
            .collect();

        assert_eq!(ids.len(), 12, "every add must succeed");
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 12, "ids must be unique: {ids:?}");
        assert_eq!(load(&path).cards.len(), 12, "no card may be lost");

        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
        let _ = fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpfile() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-board-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.join("board.json")
    }

    #[test]
    fn add_list_update_roundtrip() {
        let p = tmpfile();
        assert_eq!(list(&p, None).unwrap(), "(no cards)");
        let a = add(&p, "ship v1", "freeze config", "high").unwrap();
        let b = add(&p, "walk dog", "", "normal").unwrap();
        assert_eq!((a, b), (1, 2));
        let out = list(&p, None).unwrap();
        assert_eq!(
            out,
            "#1 [todo] (high) ship v1 | freeze config\n#2 [todo] (normal) walk dog"
        );
        let line = update(&p, 2, Some("done"), None, None, None).unwrap();
        assert_eq!(line, "#2 [done] (normal) walk dog");

        let out = list(&p, None).unwrap();
        assert!(out.starts_with("#1"), "got: {out}");
        assert_eq!(
            list(&p, Some("done")).unwrap(),
            "#2 [done] (normal) walk dog"
        );

        let line = update(
            &p,
            1,
            Some("doing"),
            Some("ship v1.0"),
            Some("soon"),
            Some("low"),
        )
        .unwrap();
        assert_eq!(line, "#1 [doing] (low) ship v1.0");
    }

    #[test]
    fn validation_and_missing_cards() {
        let p = tmpfile();
        assert!(add(&p, "  ", "", "normal").is_err());
        assert!(add(&p, "x", "", "urgent").is_err());
        assert!(list(&p, Some("archived")).is_err());
        assert!(update(&p, 99, Some("done"), None, None, None)
            .unwrap_err()
            .contains("no card #99"));
        add(&p, "x", "", "low").unwrap();
        assert!(update(&p, 1, Some("bogus"), None, None, None).is_err());
        assert!(update(&p, 1, None, Some(" "), None, None).is_err());
    }

    #[test]
    fn ids_survive_reload_and_corrupt_file_is_empty_board() {
        let p = tmpfile();
        add(&p, "one", "", "normal").unwrap();
        update(&p, 1, Some("done"), None, None, None).unwrap();
        add(&p, "two", "", "normal").unwrap();
        let board = load(&p);
        assert_eq!(board.next_id, 3);
        fs::write(&p, "not json").unwrap();
        assert_eq!(list(&p, None).unwrap(), "(no cards)");
    }

    #[cfg(unix)]
    #[test]
    fn board_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmpfile();
        add(&p, "secret task", "", "normal").unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[cfg(test)]
mod board_view_tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-boardview-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.join("board.json")
    }

    #[test]
    fn show_prints_one_card_and_refuses_unknown_ids() {
        let p = tmp();
        let id = add(&p, "write the docs", "with examples", "high").unwrap();
        let text = show(&p, id).unwrap();
        assert!(text.contains("write the docs"), "{text}");
        assert!(text.contains("with examples"), "{text}");
        assert!(text.contains("high"), "{text}");
        assert!(show(&p, 999).is_err());
    }

    #[test]
    fn show_says_none_when_a_card_has_no_notes() {
        let p = tmp();
        let id = add(&p, "bare", "", "normal").unwrap();
        assert!(show(&p, id).unwrap().contains("(none)"));
    }

    #[test]
    fn the_json_listing_carries_every_field_and_honours_the_status_filter() {
        let p = tmp();
        let id = add(&p, "a card", "notes", "high").unwrap();
        let raw = list_json(&p, None).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["cards"][0]["id"], id);
        assert_eq!(v["cards"][0]["priority"], "high");
        assert_eq!(v["cards"][0]["status"], "todo");
        let done = list_json(&p, Some("done")).unwrap();
        let dv: Value = serde_json::from_str(&done).unwrap();
        assert_eq!(dv["cards"].as_array().map(Vec::len), Some(0));
        assert!(list_json(&p, Some("nonsense")).is_err());
    }

    #[test]
    fn dispatch_picks_todo_cards_by_priority_then_age() {
        let p = tmp();
        let low = add(&p, "low one", "", "low").unwrap();
        let high = add(&p, "high one", "", "high").unwrap();
        let done = add(&p, "finished", "", "high").unwrap();
        update(&p, done, Some("done"), None, None, None).unwrap();
        let ready = ready(&p, 10);
        assert_eq!(ready.len(), 2);
        assert_eq!(ready.first().map(|c| c.id), Some(high));
        assert_eq!(ready.get(1).map(|c| c.id), Some(low));
        assert!(!ready.iter().any(|c| c.id == done));
    }

    #[test]
    fn dispatch_respects_the_limit_and_reports_an_empty_board() {
        let p = tmp();
        for i in 0..5 {
            add(&p, &format!("card {i}"), "", "normal").unwrap();
        }
        assert_eq!(ready(&p, 2).len(), 2);
        let text = dispatch_text(&ready(&p, 2));
        assert!(text.contains("2 card(s) ready"), "{text}");
        assert!(text.contains("phoenix run"), "{text}");
        assert!(dispatch_text(&[]).contains("no todo cards"));
    }
}
