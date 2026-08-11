use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const NATIVE_INTERVAL: Duration = Duration::from_secs(6);
pub const FALLBACK_DELAY: Duration = Duration::from_secs(5);
pub const FALLBACK_NOTICE: &str = "🔥 Still working on it…";
pub const MATRIX_INTERVAL: Duration = Duration::from_secs(20);
pub const WHATSAPP_INTERVAL: Duration = Duration::from_secs(20);

pub struct Working {
    active: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Working {
    pub fn native(
        interval: Duration,
        mut pulse: impl FnMut() + Send + 'static,
        cleanup: impl FnOnce() + Send + 'static,
    ) -> Working {
        pulse();
        Working::start(interval, interval, true, pulse, cleanup)
    }

    pub fn delayed(notify: impl FnOnce() + Send + 'static) -> Working {
        Working::delayed_for(FALLBACK_DELAY, notify)
    }

    fn delayed_for(delay: Duration, notify: impl FnOnce() + Send + 'static) -> Working {
        let mut notify = Some(notify);
        Working::start(
            delay,
            Duration::ZERO,
            false,
            move || {
                if let Some(notify) = notify.take() {
                    notify();
                }
            },
            || {},
        )
    }

    fn start(
        delay: Duration,
        interval: Duration,
        already_started: bool,
        mut pulse: impl FnMut() + Send + 'static,
        cleanup: impl FnOnce() + Send + 'static,
    ) -> Working {
        let active = Arc::new(AtomicBool::new(true));
        let thread_active = active.clone();
        let worker = std::thread::Builder::new()
            .name("phoenix-working".to_string())
            .spawn(move || {
                let mut started = already_started;
                if !wait_until_stopped(&thread_active, delay) {
                    while thread_active.load(Ordering::Acquire) {
                        pulse();
                        started = true;
                        if interval.is_zero() || wait_until_stopped(&thread_active, interval) {
                            break;
                        }
                    }
                }
                if started {
                    cleanup();
                }
            })
            .ok();
        Working { active, worker }
    }

    pub fn finish(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        self.finish();
    }
}

fn wait_until_stopped(active: &AtomicBool, duration: Duration) -> bool {
    if duration.is_zero() {
        return !active.load(Ordering::Acquire);
    }
    let started = std::time::Instant::now();
    while active.load(Ordering::Acquire) {
        let left = duration.saturating_sub(started.elapsed());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(25)));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn delayed_indicator_is_silent_for_a_quick_turn() {
        let notices = Arc::new(AtomicUsize::new(0));
        let seen = notices.clone();
        let mut working = Working::delayed_for(Duration::from_millis(50), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(10));
        working.finish();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(notices.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn delayed_indicator_runs_once_for_a_long_turn() {
        let notices = Arc::new(AtomicUsize::new(0));
        let seen = notices.clone();
        let mut working = Working::delayed_for(Duration::from_millis(10), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(45));
        working.finish();
        assert_eq!(notices.load(Ordering::SeqCst), 1);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(notices.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_a_delayed_indicator_cancels_it() {
        let notices = Arc::new(AtomicUsize::new(0));
        let seen = notices.clone();
        let working = Working::delayed_for(Duration::from_millis(40), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        drop(working);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(notices.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn native_indicator_repeats_and_cleans_up() {
        let pulses = Arc::new(AtomicUsize::new(0));
        let cleaned = Arc::new(AtomicBool::new(false));
        let seen = pulses.clone();
        let done = cleaned.clone();
        let mut working = Working::native(
            Duration::from_millis(15),
            move || {
                seen.fetch_add(1, Ordering::SeqCst);
            },
            move || done.store(true, Ordering::SeqCst),
        );
        assert_eq!(pulses.load(Ordering::SeqCst), 1);
        std::thread::sleep(Duration::from_millis(45));
        working.finish();
        assert!(pulses.load(Ordering::SeqCst) >= 2);
        assert!(cleaned.load(Ordering::SeqCst));
        let stopped_at = pulses.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(pulses.load(Ordering::SeqCst), stopped_at);
    }
}
