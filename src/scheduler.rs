use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Job;
use crate::security::redact;

pub fn post_webhook(url: &str, job: &str, result: &str) -> Result<(), String> {
    let payload = serde_json::json!({ "job": job, "result": result });
    ureq::post(url)
        .timeout(Duration::from_secs(30))
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string())
        .map_err(|e| redact(&e.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct Tm {
    pub min: i64,
    pub hour: i64,
    pub mday: i64,
    pub mon: i64,
    pub year: i64,
    pub wday: i64,
}

#[cfg(unix)]
pub fn now_local() -> Tm {
    #[allow(deprecated)]
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&epoch, &mut tm);
    }
    Tm {
        min: tm.tm_min as i64,
        hour: tm.tm_hour as i64,
        mday: tm.tm_mday as i64,
        mon: (tm.tm_mon + 1) as i64,
        year: (tm.tm_year + 1900) as i64,
        wday: tm.tm_wday as i64,
    }
}

#[cfg(windows)]
pub fn now_local() -> Tm {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime16 {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millis: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(st: *mut SystemTime16);
    }
    let mut st = SystemTime16::default();
    unsafe { GetLocalTime(&mut st) };
    Tm {
        min: st.minute as i64,
        hour: st.hour as i64,
        mday: st.day as i64,
        mon: st.month as i64,
        year: st.year as i64,
        wday: st.day_of_week as i64,
    }
}

fn field_match(expr: &str, value: i64, lo: i64, hi: i64) -> Result<bool, String> {
    for part in expr.split(',') {
        let part = part.trim();
        let mut step: i64 = 1;
        let mut base = part;
        if let Some((head, s)) = part.split_once('/') {
            base = head;
            step = s
                .trim()
                .parse()
                .map_err(|_| format!("bad cron step: {part:?}"))?;
            if step < 1 {
                return Err(format!("bad cron step: {part:?}"));
            }
        }
        let (lo2, hi2) = if base == "*" || base.is_empty() {
            (lo, hi)
        } else if let Some((a, b)) = base.split_once('-') {
            (
                a.trim()
                    .parse()
                    .map_err(|_| format!("bad cron field: {part:?}"))?,
                b.trim()
                    .parse()
                    .map_err(|_| format!("bad cron field: {part:?}"))?,
            )
        } else {
            let v: i64 = base
                .trim()
                .parse()
                .map_err(|_| format!("bad cron field: {part:?}"))?;
            (v, v)
        };
        if lo2 <= value && value <= hi2 && (value - lo2) % step == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn cron_due(expr: &str, t: &Tm) -> Result<bool, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("bad cron expression: {expr:?}"));
    }
    Ok(field_match(fields[0], t.min, 0, 59)?
        && field_match(fields[1], t.hour, 0, 23)?
        && field_match(fields[2], t.mday, 1, 31)?
        && field_match(fields[3], t.mon, 1, 12)?
        && field_match(fields[4], t.wday, 0, 6)?)
}

pub fn cron_valid(expr: &str) -> Result<(), String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("bad cron expression: {expr:?}"));
    }
    field_match(fields[0], 0, 0, 59)?;
    field_match(fields[1], 0, 0, 23)?;
    field_match(fields[2], 1, 1, 31)?;
    field_match(fields[3], 1, 1, 12)?;
    field_match(fields[4], 0, 0, 6)?;
    Ok(())
}

pub struct Scheduler {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Scheduler {
    pub fn start<R, D>(jobs: &[Job], run_job: R, deliver: D) -> Option<Scheduler>
    where
        R: Fn(&str) -> String + Send + 'static,
        D: Fn(&Job, &str) + Send + 'static,
    {
        let jobs: Vec<Job> = jobs
            .iter()
            .filter(|j| !j.cron.is_empty() && !j.prompt.is_empty())
            .cloned()
            .collect();
        if jobs.is_empty() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("phoenix-cron".into())
            .spawn(move || {
                let mut last_min: i64 = -1;
                'outer: while !stop2.load(Ordering::Relaxed) {
                    for _ in 0..20 {
                        thread::sleep(Duration::from_millis(250));
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                    }
                    let now = now_local();
                    if now.min == last_min {
                        continue;
                    }
                    last_min = now.min;
                    for job in &jobs {
                        match cron_due(&job.cron, &now) {
                            Ok(true) => {
                                let result = run_job(&job.prompt);
                                deliver(job, &result);
                            }
                            Ok(false) => {}
                            Err(e) => deliver(job, &format!("job failed: {e}")),
                        }
                    }
                }
            })
            .ok()?;
        Some(Scheduler {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for Scheduler {
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

    fn sunday_7am() -> Tm {
        Tm {
            min: 0,
            hour: 7,
            mday: 26,
            mon: 7,
            year: 2026,
            wday: 0,
        }
    }

    #[test]
    fn exact_time_matches() {
        let t = sunday_7am();
        assert!(cron_due("0 7 * * *", &t).unwrap());
        assert!(!cron_due("30 7 * * *", &t).unwrap());
        assert!(!cron_due("0 8 * * *", &t).unwrap());
    }

    #[test]
    fn steps_match() {
        let t = sunday_7am();
        assert!(cron_due("*/15 * * * *", &t).unwrap());
        let t2 = Tm { min: 7, ..t };
        assert!(!cron_due("*/15 * * * *", &t2).unwrap());
        let t3 = Tm { min: 45, ..t };
        assert!(cron_due("*/15 * * * *", &t3).unwrap());
    }

    #[test]
    fn ranges_match() {
        let t = sunday_7am();
        assert!(cron_due("* 6-9 * * *", &t).unwrap());
        assert!(!cron_due("* 8-9 * * *", &t).unwrap());
        assert!(cron_due("* 5-9/2 * * *", &t).unwrap());
    }

    #[test]
    fn lists_match() {
        let t = sunday_7am();
        assert!(cron_due("0,30 * * * *", &t).unwrap());
        assert!(cron_due("15,45,0 7 * * *", &t).unwrap());
        assert!(!cron_due("15,45 * * * *", &t).unwrap());
    }

    #[test]
    fn day_of_week_sunday_is_zero() {
        let t = sunday_7am();
        assert!(cron_due("0 7 * * 0", &t).unwrap());
        assert!(!cron_due("0 7 * * 1", &t).unwrap());
    }

    #[test]
    fn day_and_month_fields() {
        let t = sunday_7am();
        assert!(cron_due("* * 26 7 *", &t).unwrap());
        assert!(!cron_due("* * 27 * *", &t).unwrap());
        assert!(!cron_due("* * * 8 *", &t).unwrap());
    }

    #[test]
    fn webhook_posts_json() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let got = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();

            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = s.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(pos) = text.find("\r\n\r\n") {
                    let cl = text
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= pos + 4 + cl {
                        break;
                    }
                }
            }
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            String::from_utf8_lossy(&buf).to_string()
        });
        post_webhook(&format!("http://{addr}"), "brief", "all good").unwrap();
        let req = got.join().unwrap();
        assert!(req.contains("application/json"));
        assert!(req.contains(r#""job":"brief""#));
        assert!(req.contains(r#""result":"all good""#));
    }

    #[test]
    fn webhook_error_is_reported() {
        assert!(post_webhook("http://127.0.0.1:1", "j", "r").is_err());
    }

    #[test]
    fn scheduler_start_stop_and_filtering() {
        let jobs = vec![Job {
            name: "j".into(),
            cron: "* * * * *".into(),
            prompt: "p".into(),
            chat_ids: Vec::new(),
            webhook: String::new(),
        }];
        let s = Scheduler::start(&jobs, |_p| String::new(), |_j, _r| {}).unwrap();
        drop(s);
        let empty = vec![Job {
            name: "x".into(),
            cron: String::new(),
            prompt: String::new(),
            chat_ids: Vec::new(),
            webhook: String::new(),
        }];
        assert!(Scheduler::start(&empty, |_p| String::new(), |_j, _r| {}).is_none());
    }

    #[test]
    fn cron_valid_checks_every_field() {
        assert!(cron_valid("0 7 * * *").is_ok());
        assert!(cron_valid("*/15 9-17 1,15 * 1-5").is_ok());
        assert!(cron_valid("* * *").is_err());

        assert!(cron_valid("30 badhour * * *").is_err());
        assert!(cron_valid("* */0 * * *").is_err());
    }

    #[test]
    fn bad_expressions_error() {
        let t = sunday_7am();
        assert!(cron_due("* * *", &t).is_err());
        assert!(cron_due("* * * * * *", &t).is_err());
        assert!(cron_due("x * * * *", &t).is_err());
        assert!(cron_due("*/0 * * * *", &t).is_err());
        assert!(cron_due("1-x * * * *", &t).is_err());
    }
}
