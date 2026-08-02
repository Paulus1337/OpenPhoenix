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
    pub sessions_dir: Option<PathBuf>,

    pub embed: Option<EmbedConfig>,
}

impl Memory {
    pub fn new(privacy: &str) -> Self {
        Memory::with_home(privacy, &config::home())
    }

    pub fn in_workspace(privacy: &str, workspace: &Path) -> Self {
        Memory {
            privacy: privacy.to_string(),
            path: workspace.join("MEMORY.md"),
            sessions_dir: Some(config::home().join("sessions")),
            embed: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(privacy: &str, home: &Path) -> Self {
        Memory {
            privacy: privacy.to_string(),
            path: home.join("memory.md"),
            sessions_dir: Some(home.join("sessions")),
            embed: None,
        }
    }

    fn daily_path(&self) -> Option<PathBuf> {
        let t = now_local();
        Some(
            self.path
                .parent()?
                .join("memory")
                .join(format!("{:04}-{:02}-{:02}.md", t.year, t.mon, t.mday)),
        )
    }

    pub fn enabled(&self) -> bool {
        self.privacy == "recall"
    }

    pub fn remember(&self, note: &str) -> String {
        self.remember_from("operator", note)
    }

    pub fn remember_from(&self, source: &str, note: &str) -> String {
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
        let line = format!("- [{stamp}] [{source}] {note}\n");
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
        if let Some(daily) = self.daily_path() {
            if let Some(dir) = daily.parent() {
                let _ = fs::create_dir_all(dir);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
                }
            }
            let wrote = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&daily)
                .and_then(|mut fh| fh.write_all(line.as_bytes()));
            if wrote.is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&daily, fs::Permissions::from_mode(0o600));
                }
            }
        }
        "noted".into()
    }

    fn transcript_lines(&self) -> Vec<String> {
        let Some(dir) = &self.sessions_dir else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let label = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("session")
                .to_string();
            let Ok(text) = fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(arr) = v.as_array() else { continue };
            for m in arr {
                let role = m["role"].as_str().unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                let content = m["content"].as_str().unwrap_or("");
                for line in content.lines() {
                    let t = line.trim();
                    if !t.is_empty() {
                        out.push(format!("[{label}:{role}] {t}"));
                    }
                }
            }
        }
        out
    }

    fn note_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(content) = fs::read_to_string(&self.path) {
            out.extend(content.lines().map(str::to_string));
        }
        if let Some(parent) = self.path.parent() {
            if let Ok(entries) = fs::read_dir(parent.join("memory")) {
                let mut files: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
                    .collect();
                files.sort();
                for f in files {
                    if let Ok(content) = fs::read_to_string(&f) {
                        let day = f
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        out.extend(
                            content
                                .lines()
                                .filter(|l| !l.trim().is_empty())
                                .map(|l| format!("[{day}] {l}")),
                        );
                    }
                }
            }
        }
        out
    }

    pub fn recall(&self, query: &str) -> String {
        if !self.enabled() {
            return "no memories".into();
        }
        let mut corpus = self.note_lines();
        corpus.extend(self.transcript_lines());
        corpus.retain(|l| !l.trim().is_empty());
        const MAX_CORPUS: usize = 4000;
        if corpus.len() > MAX_CORPUS {
            corpus.drain(..corpus.len() - MAX_CORPUS);
        }
        if corpus.is_empty() {
            return "no memories".into();
        }
        if let Some(embed) = &self.embed {
            let lines: Vec<&str> = corpus.iter().map(String::as_str).collect();
            match embeddings::rank(embed, &lines, query) {
                Ok(hits) if hits.is_empty() => return "no matching memories".into(),
                Ok(hits) => return hits.join("\n"),
                Err(_) => {}
            }
        }
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.chars().count() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        let hits: Vec<&String> = corpus
            .iter()
            .filter(|line| {
                let low = line.to_lowercase();
                words.is_empty() || words.iter().any(|w| low.contains(w))
            })
            .collect();
        if hits.is_empty() {
            return "no matching memories".into();
        }
        let start = hits.len().saturating_sub(20);
        hits[start..]
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn wipe(&self) -> String {
        if self.path.exists() {
            let _ = fs::remove_file(&self.path);
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::remove_dir_all(parent.join("memory"));
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
    fn notes_carry_their_source_tag() {
        let mem = Memory::with_home("recall", &tmpdir());
        assert_eq!(mem.remember("operator fact"), "noted");
        assert_eq!(mem.remember_from("agent", "agent fact"), "noted");
        let content = fs::read_to_string(&mem.path).unwrap();
        assert!(content.contains("[operator] operator fact"), "{content}");
        assert!(content.contains("[agent] agent fact"), "{content}");
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
    fn daily_notes_mirror_and_recall() {
        let home = tmpdir();
        let mem = Memory::with_home("recall", &home);
        mem.remember("daily mirror check");
        let dailies: Vec<_> = fs::read_dir(home.join("memory"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(dailies.len(), 1);
        let content = fs::read_to_string(&dailies[0]).unwrap();
        assert!(content.contains("daily mirror check"));
        mem.wipe();
        assert!(!home.join("memory").exists());
    }

    #[test]
    fn recall_searches_transcripts() {
        let home = tmpdir();
        let mem = Memory::with_home("recall", &home);
        crate::sessions::save(
            &home.join("sessions"),
            "tg-42",
            &[
                crate::providers::Msg::User {
                    content: "remember the zanzibar deploy plan".into(),
                    images: Vec::new(),
                },
                crate::providers::Msg::Assistant {
                    content: "zanzibar deploy: staged rollout agreed".into(),
                    tool_calls: Vec::new(),
                },
            ],
        )
        .unwrap();
        let out = mem.recall("zanzibar");
        assert!(out.contains("[tg-42:user]"), "got: {out}");
        assert!(out.contains("staged rollout"), "got: {out}");
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
