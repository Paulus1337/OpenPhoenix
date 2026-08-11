use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn should_deliver(reply: &str) -> bool {
    let t = reply.trim();
    if t.starts_with("HEARTBEAT_OK") {
        return false;
    }
    let mut saw_ack = false;
    for line in t.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with("HEARTBEAT_OK") {
            saw_ack = true;
            continue;
        }
        return true;
    }
    !saw_ack
}

pub fn busy_window_secs(minutes: u32) -> u64 {
    (u64::from(minutes) * 60).min(600)
}

pub const OBSERVE_ONLY_DENIES: [&str; 13] = [
    "shell",
    "bg_start",
    "bg_cancel",
    "write_file",
    "send_message",
    "subagent",
    "browser_open",
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_screenshot",
    "browser_close",
];

pub fn file_warrants_a_beat(path: &std::path::Path) -> bool {
    match std::fs::read_to_string(path) {
        Err(_) => false,
        Ok(text) => text.lines().any(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        }),
    }
}

pub fn observe_only_denies(existing: &[String]) -> Vec<String> {
    let mut out: Vec<String> = existing.to_vec();
    for name in OBSERVE_ONLY_DENIES {
        if !out.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

pub fn beat_gate(reply: &str, last_failure: &mut Option<String>) -> bool {
    let failed = reply.trim_start().starts_with("heartbeat failed:");
    if failed {
        if last_failure.as_deref() == Some(reply.trim()) {
            return false;
        }
        *last_failure = Some(reply.trim().to_string());
    } else {
        *last_failure = None;
    }
    should_deliver(reply)
}

pub struct Heartbeat {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    pub fn start<B, R, D>(minutes: u32, busy: B, run: R, deliver: D) -> Option<Heartbeat>
    where
        B: Fn() -> bool + Send + 'static,
        R: Fn() -> String + Send + 'static,
        D: Fn(&str) + Send + 'static,
    {
        if minutes == 0 {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("phoenix-heartbeat".into())
            .spawn(move || {
                let slices = u64::from(minutes) * 60 * 4;
                let mut last_failure: Option<String> = None;
                'outer: loop {
                    for _ in 0..slices {
                        thread::sleep(Duration::from_millis(250));
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                    }
                    if busy() {
                        continue;
                    }
                    let reply = run();
                    if beat_gate(&reply, &mut last_failure) {
                        deliver(&reply);
                    }
                }
            })
            .ok()?;
        Some(Heartbeat {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_replies_are_suppressed() {
        assert!(!should_deliver("HEARTBEAT_OK"));
        assert!(!should_deliver("  HEARTBEAT_OK  "));
        assert!(!should_deliver("HEARTBEAT_OK, nothing to report"));
        assert!(!should_deliver("\nHEARTBEAT_OK\nall quiet"));
    }

    #[test]
    fn real_replies_are_delivered() {
        assert!(should_deliver("disk is 95% full"));
        assert!(should_deliver("the heartbeat found HEARTBEAT_OK issues"));
    }

    #[test]
    fn an_ack_after_a_reasoning_preamble_is_still_silent() {
        assert!(!should_deliver("\n\nHEARTBEAT_OK\n\n"));
        assert!(
            !should_deliver("HEARTBEAT_OK\n\nnothing needs attention"),
            "trailing filler after the ack must not force a delivery"
        );
        assert!(
            should_deliver("Disk is 98% full.\nHEARTBEAT_OK"),
            "a real finding before the ack must still be delivered"
        );
        assert!(should_deliver(""));
        assert!(should_deliver("ok"));
    }

    #[test]
    fn repeated_identical_failures_are_delivered_once() {
        let mut last = None;
        assert!(beat_gate("heartbeat failed: connect timeout", &mut last));
        assert!(!beat_gate("heartbeat failed: connect timeout", &mut last));
        assert!(!beat_gate("heartbeat failed: connect timeout", &mut last));
        assert!(
            beat_gate("heartbeat failed: dns error", &mut last),
            "a different failure is news"
        );
        assert!(
            beat_gate("disk is 95% full", &mut last),
            "a real finding after failures is delivered"
        );
        assert!(
            beat_gate("heartbeat failed: connect timeout", &mut last),
            "after recovery the old failure is news again"
        );
        assert!(!beat_gate("HEARTBEAT_OK", &mut last));
        assert!(
            !beat_gate("heartbeat failed: connect timeout", &mut last)
                || last.as_deref() == Some("heartbeat failed: connect timeout"),
            "ok resets the failure memory"
        );
    }

    #[test]
    fn zero_minutes_is_disabled() {
        assert!(Heartbeat::start(0, || false, String::new, |_| {}).is_none());
    }

    #[test]
    fn start_stop_promptly() {
        let hb = Heartbeat::start(60, || false, String::new, |_| {}).unwrap();
        drop(hb);
    }

    #[test]
    fn the_busy_window_caps_at_ten_minutes() {
        assert_eq!(busy_window_secs(1), 60);
        assert_eq!(busy_window_secs(10), 600);
        assert_eq!(busy_window_secs(60), 600);
        assert_eq!(busy_window_secs(0), 0);
    }

    #[test]
    fn a_missing_or_empty_heartbeat_file_skips_the_model_call() {
        let d = std::env::temp_dir().join(format!(
            "px-hb-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("HEARTBEAT.md");
        assert!(!file_warrants_a_beat(&f), "missing file means no beat");
        std::fs::write(&f, "").unwrap();
        assert!(!file_warrants_a_beat(&f), "empty file means no beat");
        std::fs::write(&f, "# only comments\n\n# more\n").unwrap();
        assert!(!file_warrants_a_beat(&f), "comment-only file means no beat");
        std::fs::write(&f, "# header\ncheck the disk\n").unwrap();
        assert!(file_warrants_a_beat(&f), "real content warrants a beat");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn observe_only_extends_without_duplicating() {
        let existing = vec!["SHELL".to_string(), "my_mcp_tool".to_string()];
        let out = observe_only_denies(&existing);
        assert_eq!(
            out.iter()
                .filter(|d| d.eq_ignore_ascii_case("shell"))
                .count(),
            1,
            "an operator entry already covering a name is kept, not doubled"
        );
        assert!(out.contains(&"my_mcp_tool".to_string()));
        assert!(out.contains(&"write_file".to_string()));
        assert!(out.contains(&"browser_close".to_string()));
        assert_eq!(out.len(), 2 + OBSERVE_ONLY_DENIES.len() - 1);
    }
}
