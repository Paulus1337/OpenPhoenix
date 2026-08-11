use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static NOTIFIED: AtomicBool = AtomicBool::new(false);

const FRAMES: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

fn clear_line() {
    eprint!("\r\x1b[2K");
    let _ = std::io::stderr().flush();
}

pub fn start(label: &str, tty: bool) {
    if !tty {
        return;
    }
    STOP.store(false, Ordering::SeqCst);
    NOTIFIED.store(false, Ordering::SeqCst);
    ACTIVE.store(true, Ordering::SeqCst);
    let label = label.to_string();
    std::thread::spawn(move || {
        let mut i = 0usize;
        loop {
            if STOP.load(Ordering::SeqCst) || NOTIFIED.load(Ordering::SeqCst) {
                break;
            }
            eprint!(
                "\r\x1b[38;5;208;1m{}\x1b[0m \x1b[2m{label}\x1b[0m",
                FRAMES[i % FRAMES.len()]
            );
            let _ = std::io::stderr().flush();
            i += 1;
            for _ in 0..9 {
                if STOP.load(Ordering::SeqCst) || NOTIFIED.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if ACTIVE.swap(false, Ordering::SeqCst) {
            clear_line();
        }
    });
}

pub fn notify_activity() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        NOTIFIED.store(true, Ordering::SeqCst);
        clear_line();
    }
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    if ACTIVE.swap(false, Ordering::SeqCst) {
        clear_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn start_with_no_tty_never_spawns_or_marks_active() {
        let _g = TEST_LOCK.lock().unwrap();
        stop();
        start("thinking", false);
        assert!(!ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn stop_is_safe_when_nothing_was_started() {
        let _g = TEST_LOCK.lock().unwrap();
        stop();
        stop();
        assert!(!ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn notify_activity_is_safe_when_nothing_was_started() {
        let _g = TEST_LOCK.lock().unwrap();
        stop();
        notify_activity();
        assert!(!ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn frames_are_all_single_char_braille_glyphs() {
        for f in FRAMES {
            assert_eq!(f.chars().count(), 1, "frame must be one glyph: {f}");
        }
    }
}
