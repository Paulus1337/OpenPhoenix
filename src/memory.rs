use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config;
use crate::embeddings::{self, EmbedConfig};
use crate::scheduler::now_local;
use crate::security::redact;

pub struct Memory {
    pub privacy: String,
    pub path: PathBuf,

    pub embed: Option<EmbedConfig>,
}

impl Memory {
    pub fn new(privacy: &str) -> Self {
        Memory {
            privacy: privacy.to_string(),
            path: config::home().join("memory.md"),
            embed: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(privacy: &str, home: &Path) -> Self {
        Memory {
            privacy: privacy.to_string(),
            path: home.join("memory.md"),
            embed: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.privacy == "recall"
    }

    pub fn remember(&self, note: &str) -> String {
        if !self.enabled() {
            return "memory disabled in this privacy mode".into();
        }
        let note = redact(note.trim());
        if note.is_empty() {
            return "empty note ignored".into();
        }
        if let Some(dir) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(dir) {
                return format!("error: {e}");
            }
        }
        let t = now_local();
        let stamp = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.year, t.mon, t.mday, t.hour, t.min
        );
        let line = format!("- [{stamp}] {note}\n");
        let res = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut fh| fh.write_all(line.as_bytes()));
        if let Err(e) = res {
            return format!("error: {e}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        "noted".into()
    }

    pub fn recall(&self, query: &str) -> String {
        if !self.enabled() || !self.path.exists() {
            return "no memories".into();
        }
        let content = fs::read_to_string(&self.path).unwrap_or_default();
        if let Some(embed) = &self.embed {
            let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
            if !lines.is_empty() {
                match embeddings::rank(embed, &lines, query) {
                    Ok(hits) if hits.is_empty() => return "no matching memories".into(),
                    Ok(hits) => return hits.join("\n"),
                    Err(_) => {}
                }
            }
        }
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.chars().count() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        let hits: Vec<&str> = content
            .lines()
            .filter(|line| {
                let low = line.to_lowercase();
                words.is_empty() || words.iter().any(|w| low.contains(w))
            })
            .collect();
        if hits.is_empty() {
            return "no matching memories".into();
        }
        let start = hits.len().saturating_sub(20);
        hits[start..].join("\n")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn wipe(&self) -> String {
        if self.path.exists() {
            let _ = fs::remove_file(&self.path);
        }
        "memory wiped".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-mem-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn recall_mode_roundtrip() {
        let mem = Memory::with_home("recall", &tmpdir());
        assert_eq!(mem.remember("phoenix rises from ashes"), "noted");
        assert!(mem.recall("phoenix").contains("phoenix"));
        assert_eq!(mem.wipe(), "memory wiped");
        assert_eq!(mem.recall("phoenix"), "no memories");
    }

    #[test]
    fn lines_are_timestamped() {
        let mem = Memory::with_home("recall", &tmpdir());
        mem.remember("a note");
        let content = fs::read_to_string(&mem.path).unwrap();
        assert!(content.starts_with("- ["), "got: {content}");
        assert!(content.contains("] a note"));
    }

    #[test]
    fn ghost_and_session_disabled() {
        let dir = tmpdir();
        for mode in ["ghost", "session"] {
            let mem = Memory::with_home(mode, &dir);
            assert!(mem.remember("secret").contains("disabled"));
            assert!(!mem.path.exists());
        }
    }

    #[test]
    fn empty_note_ignored() {
        let mem = Memory::with_home("recall", &tmpdir());
        assert_eq!(mem.remember("   "), "empty note ignored");
    }

    #[test]
    fn secrets_redacted_before_disk() {
        let mem = Memory::with_home("recall", &tmpdir());
        let token = format!("ghp_{}", "a".repeat(36));
        mem.remember(&format!("token is {token}"));
        let content = fs::read_to_string(&mem.path).unwrap();
        assert!(!content.contains(&token));
        assert!(content.contains("[redacted]"));
    }

    #[test]
    fn no_matching_memories() {
        let mem = Memory::with_home("recall", &tmpdir());
        mem.remember("alpha beta");
        assert_eq!(mem.recall("zzzquery"), "no matching memories");
    }

    #[test]
    fn embeddings_error_falls_back_to_substring() {
        let dir = tmpdir();
        let mut mem = Memory::with_home("recall", &dir);
        mem.remember("alpha borrow checker note");

        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        mem.embed = Some(EmbedConfig {
            model: "test-embed".into(),
            base_url: format!("http://{addr}/v1"),
            api_key: String::new(),
            index_path: dir.join("memory.embeddings.jsonl"),
        });
        let out = mem.recall("alpha");
        assert!(out.contains("alpha borrow checker note"), "got: {out}");
        assert!(!dir.join("memory.embeddings.jsonl").exists());
    }

    #[cfg(unix)]
    #[test]
    fn memory_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let mem = Memory::with_home("recall", &tmpdir());
        mem.remember("perm check");
        let mode = fs::metadata(&mem.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
