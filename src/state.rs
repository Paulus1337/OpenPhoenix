use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

pub const VERSION: u64 = 1;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cooldown {
    pub until: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub activation: BTreeMap<String, String>,
    pub last_seen: BTreeMap<String, u64>,
    pub cooldowns: BTreeMap<String, Cooldown>,
}

pub struct State {
    path: PathBuf,
    inner: Mutex<Snapshot>,
}

fn now() -> u64 {
    crate::scheduler::now_epoch()
}

impl Snapshot {
    fn from_json(v: &Value) -> Self {
        let mut out = Snapshot::default();
        if v["v"].as_u64() != Some(VERSION) {
            return out;
        }
        if let Some(map) = v["activation"].as_object() {
            for (k, val) in map {
                if let Some(mode) = val.as_str() {
                    out.activation.insert(k.clone(), mode.to_string());
                }
            }
        }
        if let Some(map) = v["last_seen"].as_object() {
            for (k, val) in map {
                if let Some(ts) = val.as_u64() {
                    out.last_seen.insert(k.clone(), ts);
                }
            }
        }
        if let Some(map) = v["cooldowns"].as_object() {
            for (k, val) in map {
                let until = val["until"].as_u64().unwrap_or(0);
                if until <= now() {
                    continue;
                }
                out.cooldowns.insert(
                    k.clone(),
                    Cooldown {
                        until,
                        reason: val["reason"].as_str().unwrap_or("").to_string(),
                    },
                );
            }
        }
        out
    }

    fn to_json(&self) -> Value {
        let cools: BTreeMap<&String, Value> = self
            .cooldowns
            .iter()
            .filter(|(_, c)| c.until > now())
            .map(|(k, c)| (k, json!({"until": c.until, "reason": c.reason})))
            .collect();
        json!({
            "v": VERSION,
            "activation": self.activation,
            "last_seen": self.last_seen,
            "cooldowns": cools,
        })
    }
}

impl State {
    pub fn at(path: &Path) -> Self {
        let mut snap = Snapshot::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => {
                    let ver = v["v"].as_u64().unwrap_or(0);
                    if ver == VERSION {
                        snap = Snapshot::from_json(&v);
                    } else {
                        let backup = path.with_extension(format!("v{ver}.bak.json"));
                        if std::fs::copy(path, &backup).is_ok() {
                            eprintln!(
                                "state: {} is schema v{ver}, this build speaks v{VERSION}; \
kept your data at {} and started fresh",
                                path.display(),
                                backup.display()
                            );
                        }
                    }
                }
                Err(_) => {
                    let backup = path.with_extension("corrupt.bak.json");
                    if std::fs::copy(path, &backup).is_ok() {
                        eprintln!(
                            "state: {} is unreadable; kept a copy at {} and started fresh",
                            path.display(),
                            backup.display()
                        );
                    }
                }
            }
        }
        State {
            path: path.to_path_buf(),
            inner: Mutex::new(snap),
        }
    }

    pub fn default_path() -> PathBuf {
        crate::config::home().join("state.json")
    }

    pub fn load() -> Self {
        State::at(&State::default_path())
    }

    fn with<T>(&self, f: impl FnOnce(&mut Snapshot) -> T) -> T {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    fn flush(&self) -> Result<(), String> {
        let body = self.with(|s| s.to_json()).to_string();
        crate::security::write_atomic(&self.path, body.as_bytes(), Some(0o600))
            .map_err(|e| e.to_string())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.with(|s| s.clone())
    }

    pub fn activation(&self, chat: &str) -> Option<String> {
        self.with(|s| s.activation.get(chat).cloned())
    }

    pub fn set_activation(&self, chat: &str, mode: &str) -> Result<(), String> {
        self.with(|s| s.activation.insert(chat.to_string(), mode.to_string()));
        self.flush()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_seen(&self, chat: &str) -> Option<u64> {
        self.with(|s| s.last_seen.get(chat).copied())
    }

    pub fn touch(&self, chat: &str) -> Option<u64> {
        let previous = self.with(|s| s.last_seen.insert(chat.to_string(), now()));
        let _ = self.flush();
        previous
    }

    pub fn cooling(&self, key: &str) -> Option<Cooldown> {
        let hit = self.with(|s| s.cooldowns.get(key).cloned())?;
        if hit.until > now() {
            Some(hit)
        } else {
            self.with(|s| s.cooldowns.remove(key));
            None
        }
    }

    pub fn cool_down(&self, key: &str, secs: u64, reason: &str) -> Result<(), String> {
        self.with(|s| {
            s.cooldowns.insert(
                key.to_string(),
                Cooldown {
                    until: now().saturating_add(secs),
                    reason: reason.to_string(),
                },
            )
        });
        self.flush()
    }

    pub fn clear(&self) -> Result<(), String> {
        self.with(|s| *s = Snapshot::default());
        self.flush()
    }

    pub fn report(&self) -> Value {
        let s = self.snapshot();
        let live: Vec<Value> = s
            .cooldowns
            .iter()
            .filter(|(_, c)| c.until > now())
            .map(|(k, c)| {
                json!({
                    "key": k,
                    "seconds_left": c.until.saturating_sub(now()),
                    "reason": c.reason,
                })
            })
            .collect();
        json!({
            "v": VERSION,
            "path": self.path.display().to_string(),
            "activation": s.activation,
            "tracked_chats": s.last_seen.len(),
            "cooldowns": live,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("phx-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn activation_survives_a_restart() {
        let d = dir("activation");
        let p = d.join("state.json");
        State::at(&p).set_activation("chat:1", "mention").unwrap();

        let reloaded = State::at(&p);
        assert_eq!(reloaded.activation("chat:1").as_deref(), Some("mention"));
        assert_eq!(reloaded.activation("chat:2"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_newer_schema_is_backed_up_not_wiped() {
        let d = dir("schema");
        let p = d.join("state.json");
        let future = json!({"v": VERSION + 9, "precious": {"jobs": [1, 2, 3]}});
        std::fs::write(&p, future.to_string()).unwrap();
        let s = State::at(&p);
        assert!(s.snapshot().cooldowns.is_empty(), "fresh start expected");
        let backup = p.with_extension(format!("v{}.bak.json", VERSION + 9));
        let kept = std::fs::read_to_string(&backup).expect("backup must exist");
        assert!(kept.contains("precious"), "original bytes must survive");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_state_is_backed_up_not_silently_discarded() {
        let d = dir("corruptstate");
        let p = d.join("state.json");
        std::fs::write(&p, "{ not json at all").unwrap();
        let s = State::at(&p);
        assert!(s.snapshot().activation.is_empty());
        let backup = p.with_extension("corrupt.bak.json");
        assert!(backup.exists(), "corrupt bytes must be kept for recovery");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_cooldown_expires_on_its_own() {
        let d = dir("cooldown");
        let p = d.join("state.json");
        let s = State::at(&p);
        s.cool_down("anthropic:key0", 60, "rate_limit").unwrap();
        let hit = s.cooling("anthropic:key0").expect("still cooling");
        assert_eq!(hit.reason, "rate_limit");

        s.cool_down("anthropic:key1", 0, "expired").unwrap();
        assert!(
            s.cooling("anthropic:key1").is_none(),
            "an elapsed cooldown must not block a key forever"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_expired_cooldown_is_never_reloaded_from_disk() {
        let d = dir("stale");
        let p = d.join("state.json");
        let stale = json!({
            "v": VERSION,
            "activation": {},
            "last_seen": {},
            "cooldowns": {"dead:key": {"until": 1, "reason": "auth"}},
        });
        std::fs::write(&p, stale.to_string()).unwrap();

        let s = State::at(&p);
        assert!(
            s.cooling("dead:key").is_none(),
            "a stale cooldown must not survive a restart"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn touch_reports_the_previous_visit_then_records_this_one() {
        let d = dir("touch");
        let p = d.join("state.json");
        let s = State::at(&p);
        assert_eq!(s.touch("chat:1"), None);
        assert!(s.touch("chat:1").is_some());
        assert!(State::at(&p).last_seen("chat:1").is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_corrupt_or_foreign_file_reads_as_empty_rather_than_failing() {
        let d = dir("corrupt");
        let p = d.join("state.json");
        std::fs::write(&p, "not json at all").unwrap();
        assert!(State::at(&p).snapshot().activation.is_empty());

        std::fs::write(
            &p,
            json!({"v": 999, "activation": {"c": "always"}}).to_string(),
        )
        .unwrap();
        assert!(
            State::at(&p).activation("c").is_none(),
            "a future version must not be half-read"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clear_wipes_everything_the_operator_can_see() {
        let d = dir("clear");
        let p = d.join("state.json");
        let s = State::at(&p);
        s.set_activation("chat:1", "always").unwrap();
        s.cool_down("k", 60, "rate_limit").unwrap();
        s.touch("chat:1");

        s.clear().unwrap();
        let after = State::at(&p);
        assert!(after.activation("chat:1").is_none());
        assert!(after.cooling("k").is_none());
        assert!(after.last_seen("chat:1").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_report_names_live_cooldowns_for_an_operator() {
        let d = dir("report");
        let p = d.join("state.json");
        let s = State::at(&p);
        s.cool_down("anthropic:key0", 30, "rate_limit").unwrap();
        s.set_activation("chat:9", "mention").unwrap();

        let r = s.report();
        assert_eq!(r["v"], VERSION);
        assert_eq!(r["activation"]["chat:9"], "mention");
        let cools = r["cooldowns"].as_array().expect("array");
        assert_eq!(cools.len(), 1);
        assert_eq!(cools[0]["key"], "anthropic:key0");
        assert_eq!(cools[0]["reason"], "rate_limit");
        assert!(cools[0]["seconds_left"].as_u64().unwrap_or(0) > 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg(unix)]
    fn the_state_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("perms");
        let p = d.join("state.json");
        State::at(&p).set_activation("c", "mention").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_writers_do_not_lose_entries() {
        let d = dir("parallel");
        let p = d.join("state.json");
        let s = std::sync::Arc::new(State::at(&p));
        let mut handles = Vec::new();
        for i in 0..12 {
            let s = s.clone();
            handles.push(std::thread::spawn(move || {
                let _ = s.set_activation(&format!("chat:{i}"), "mention");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(s.snapshot().activation.len(), 12);
        let _ = std::fs::remove_dir_all(&d);
    }
}
