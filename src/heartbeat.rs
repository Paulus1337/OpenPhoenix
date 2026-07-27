use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn should_deliver(reply: &str) -> bool {
    !reply.trim().starts_with("HEARTBEAT_OK")
}

pub struct Heartbeat {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    pub fn start<R, D>(minutes: u32, run: R, deliver: D) -> Option<Heartbeat>
    where
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
                'outer: loop {
                    for _ in 0..slices {
                        thread::sleep(Duration::from_millis(250));
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                    }
                    let reply = run();
                    if should_deliver(&reply) {
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
        assert!(should_deliver(""));
        assert!(should_deliver("ok"));
    }

    #[test]
    fn zero_minutes_is_disabled() {
        assert!(Heartbeat::start(0, String::new, |_| {}).is_none());
    }

    #[test]
    fn start_stop_promptly() {
        let hb = Heartbeat::start(60, String::new, |_| {}).unwrap();
        drop(hb);
    }
}
