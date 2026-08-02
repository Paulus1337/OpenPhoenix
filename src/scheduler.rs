use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Job;
use crate::security::redact;

pub fn post_webhook(url: &str, job: &str, result: &str) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("webhook must be an http(s) URL".into());
    }
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

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

impl Tm {
    pub fn weekday(&self) -> &'static str {
        WEEKDAYS[(self.wday.rem_euclid(7)) as usize]
    }

    pub fn stamp(&self) -> String {
        format!(
            "{} {:04}-{:02}-{:02} {:02}:{:02}",
            self.weekday(),
            self.year,
            self.mon,
            self.mday,
            self.hour,
            self.min
        )
    }

    pub fn iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02} ({})",
            self.year,
            self.mon,
            self.mday,
            self.hour,
            self.min,
            self.weekday()
        )
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn time_ago(secs: u64) -> String {
    match secs {
        0..=44 => format!("{secs}s"),
        45..=5399 => format!("{}m", (secs as f64 / 60.0).round() as u64),
        5400..=86399 => format!("{}h", (secs as f64 / 3600.0).round() as u64),
        _ => format!("{}d", (secs as f64 / 86400.0).round() as u64),
    }
}

fn civil_from_secs(local: i64) -> Tm {
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let hour = secs / 3600;
    let min = (secs % 3600) / 60;
    let wday = (days + 4).rem_euclid(7);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let mon = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mon <= 2 { y + 1 } else { y };
    Tm {
        min,
        hour,
        mday,
        mon,
        year,
        wday,
    }
}

struct ZoneTable {
    transitions: Vec<(i64, i64)>,
    first_offset: i64,
}

fn be_u32(b: &[u8], at: usize) -> Option<u64> {
    let raw: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(raw)))
}

fn be_i32(b: &[u8], at: usize) -> Option<i64> {
    let raw: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(i64::from(i32::from_be_bytes(raw)))
}

fn be_i64(b: &[u8], at: usize) -> Option<i64> {
    let raw: [u8; 8] = b.get(at..at + 8)?.try_into().ok()?;
    Some(i64::from_be_bytes(raw))
}

struct TzCounts {
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

fn tz_header(data: &[u8], at: usize) -> Option<(TzCounts, u8)> {
    if data.get(at..at + 4)? != b"TZif" {
        return None;
    }
    let version = *data.get(at + 4)?;
    let counts = TzCounts {
        isutcnt: be_u32(data, at + 20)? as usize,
        isstdcnt: be_u32(data, at + 24)? as usize,
        leapcnt: be_u32(data, at + 28)? as usize,
        timecnt: be_u32(data, at + 32)? as usize,
        typecnt: be_u32(data, at + 36)? as usize,
        charcnt: be_u32(data, at + 40)? as usize,
    };
    if counts.timecnt > 100_000 || counts.typecnt > 1_000 {
        return None;
    }
    Some((counts, version))
}

fn tz_block(data: &[u8], at: usize, c: &TzCounts, wide: bool) -> Option<ZoneTable> {
    let tsize = if wide { 8 } else { 4 };
    let times = at;
    let idxs = times + c.timecnt * tsize;
    let types = idxs + c.timecnt;
    let mut offsets = Vec::with_capacity(c.typecnt);
    let mut first_std: Option<i64> = None;
    for k in 0..c.typecnt {
        let rec = types + k * 6;
        let utoff = be_i32(data, rec)?;
        let isdst = *data.get(rec + 4)?;
        if isdst == 0 && first_std.is_none() {
            first_std = Some(utoff);
        }
        offsets.push(utoff);
    }
    let mut transitions = Vec::with_capacity(c.timecnt);
    for k in 0..c.timecnt {
        let when = if wide {
            be_i64(data, times + k * 8)?
        } else {
            be_i32(data, times + k * 4)?
        };
        let idx = *data.get(idxs + k)? as usize;
        transitions.push((when, *offsets.get(idx)?));
    }
    let first_offset = first_std.or_else(|| offsets.first().copied()).unwrap_or(0);
    Some(ZoneTable {
        transitions,
        first_offset,
    })
}

fn parse_tzif(data: &[u8]) -> Option<ZoneTable> {
    let (v1, version) = tz_header(data, 0)?;
    let v1_len = v1.timecnt * 4
        + v1.timecnt
        + v1.typecnt * 6
        + v1.charcnt
        + v1.leapcnt * 8
        + v1.isstdcnt
        + v1.isutcnt;
    if version >= b'2' {
        let at = 44 + v1_len;
        if let Some((v2, _)) = tz_header(data, at) {
            return tz_block(data, at + 44, &v2, true);
        }
    }
    tz_block(data, 44, &v1, false)
}

fn zone_file_path() -> Option<std::path::PathBuf> {
    match std::env::var("TZ") {
        Ok(tz) => {
            let name = tz.strip_prefix(':').unwrap_or(&tz).to_string();
            if name.is_empty() || name.contains("..") {
                return None;
            }
            let p = std::path::PathBuf::from(&name);
            if p.is_absolute() {
                return Some(p);
            }
            Some(std::path::Path::new("/usr/share/zoneinfo").join(name))
        }
        Err(_) => Some(std::path::PathBuf::from("/etc/localtime")),
    }
}

fn load_zone() -> ZoneTable {
    let empty = ZoneTable {
        transitions: Vec::new(),
        first_offset: 0,
    };
    let Some(path) = zone_file_path() else {
        return empty;
    };
    let Ok(data) = std::fs::read(&path) else {
        return empty;
    };
    parse_tzif(&data).unwrap_or(empty)
}

static ZONE: OnceLock<ZoneTable> = OnceLock::new();

fn offset_in(zone: &ZoneTable, epoch: i64) -> i64 {
    let n = zone.transitions.partition_point(|(t, _)| *t <= epoch);
    if n == 0 {
        zone.first_offset
    } else {
        zone.transitions[n - 1].1
    }
}

pub fn local_at(epoch_secs: u64) -> Tm {
    let epoch = i64::try_from(epoch_secs).unwrap_or(i64::MAX / 4);
    let zone = ZONE.get_or_init(load_zone);
    civil_from_secs(epoch.saturating_add(offset_in(zone, epoch)))
}

pub fn now_local() -> Tm {
    local_at(now_epoch())
}

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAY_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn parse_cron_value(raw: &str, lo: i64, hi: i64) -> Result<i64, String> {
    let t = raw.trim();
    if let Ok(v) = t.parse::<i64>() {
        if hi == 6 && v == 7 {
            return Ok(0);
        }
        return Ok(v);
    }
    let low = t.to_ascii_lowercase();
    let names: &[&str] = if hi == 12 {
        &MONTH_NAMES
    } else if hi == 6 {
        &DAY_NAMES
    } else {
        return Err(format!("bad cron field: {raw:?}"));
    };
    names
        .iter()
        .position(|n| *n == low)
        .map(|i| i as i64 + if hi == 12 { 1 } else { 0 })
        .ok_or_else(|| format!("bad cron field: {raw:?}"))
        .and_then(|v| {
            if v < lo || v > hi {
                Err(format!("cron field out of range: {raw:?}"))
            } else {
                Ok(v)
            }
        })
}

pub fn expand_cron_alias(expr: &str) -> String {
    match expr.trim().to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *".into(),
        "@daily" | "@midnight" => "0 0 * * *".into(),
        "@weekly" => "0 0 * * 0".into(),
        "@monthly" => "0 0 1 * *".into(),
        "@yearly" | "@annually" => "0 0 1 1 *".into(),
        _ => expr.trim().to_string(),
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
            (parse_cron_value(a, lo, hi)?, parse_cron_value(b, lo, hi)?)
        } else {
            let v = parse_cron_value(base, lo, hi)?;
            (v, v)
        };
        if lo2 < lo || hi2 > hi || lo2 > hi2 {
            return Err(format!("cron field out of range: {part:?}"));
        }
        if lo2 <= value && value <= hi2 && (value - lo2) % step == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn cron_due(expr: &str, t: &Tm) -> Result<bool, String> {
    let expanded = expand_cron_alias(expr);
    let fields: Vec<&str> = expanded.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("bad cron expression: {expr:?}"));
    }
    if !field_match(fields[0], t.min, 0, 59)? {
        return Ok(false);
    }
    if !field_match(fields[1], t.hour, 0, 23)? {
        return Ok(false);
    }
    if !field_match(fields[3], t.mon, 1, 12)? {
        return Ok(false);
    }
    let dom_restricted = fields[2] != "*";
    let dow_restricted = fields[4] != "*";
    let dom = field_match(fields[2], t.mday, 1, 31)?;
    let dow = field_match(fields[4], t.wday, 0, 6)?;
    Ok(match (dom_restricted, dow_restricted) {
        (true, true) => dom || dow,
        _ => dom && dow,
    })
}

pub fn cron_valid(expr: &str) -> Result<(), String> {
    let expanded = expand_cron_alias(expr);
    let fields: Vec<&str> = expanded.split_whitespace().collect();
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

pub const CATCHUP_MINUTES: u64 = 5;

pub fn precheck_passes(job: &Job, cwd: &std::path::Path) -> bool {
    if job.precheck.is_empty() {
        return true;
    }
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&job.precheck)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn script_result(job: &Job, cwd: &std::path::Path) -> Option<String> {
    if job.script.is_empty() {
        return None;
    }
    Some(
        match std::process::Command::new("sh")
            .arg("-c")
            .arg(&job.script)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                let trimmed = text.trim().to_string();
                if o.status.success() {
                    if trimmed.is_empty() {
                        "(script ok, no output)".to_string()
                    } else {
                        trimmed
                    }
                } else {
                    format!(
                        "job failed: script exited {}\n{trimmed}",
                        o.status.code().unwrap_or(-1)
                    )
                }
            }
            Err(e) => format!("job failed: {e}"),
        },
    )
}

pub fn postcondition(job: &Job, result: &str) -> String {
    if job.expect.is_empty() || result.contains(&job.expect) {
        return result.to_string();
    }
    format!(
        "job failed postcondition: the result does not contain \"{}\"\n{result}",
        job.expect
    )
}

const NEXT_FIRE_SCAN_MINUTES: u64 = 366 * 24 * 60;

pub fn next_fire(expr: &str, from_epoch: u64) -> Result<Option<Tm>, String> {
    cron_valid(expr)?;
    let start = from_epoch / 60 + 1;
    for minute in start..start + NEXT_FIRE_SCAN_MINUTES {
        let t = local_at(minute * 60);
        if cron_due(expr, &t)? {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

pub fn sweep_due<'a>(jobs: &'a [Job], slots: &[Tm]) -> (Vec<&'a Job>, Vec<(&'a Job, String)>) {
    let mut due = Vec::new();
    let mut errs = Vec::new();
    for job in jobs {
        for slot in slots {
            match cron_due(&job.cron, slot) {
                Ok(true) => {
                    due.push(job);
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    errs.push((job, e));
                    break;
                }
            }
        }
    }
    (due, errs)
}

pub struct Scheduler {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Scheduler {
    pub fn start<R, D>(jobs: &[Job], run_job: R, deliver: D) -> Option<Scheduler>
    where
        R: Fn(&Job) -> Option<String> + Send + 'static,
        D: Fn(&Job, &str) + Send + 'static,
    {
        let jobs: Vec<Job> = jobs
            .iter()
            .filter(|j| !j.cron.is_empty() && (!j.prompt.is_empty() || !j.script.is_empty()))
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
                let mut last_epoch: u64 = 0;
                'outer: while !stop2.load(Ordering::Relaxed) {
                    for _ in 0..20 {
                        thread::sleep(Duration::from_millis(250));
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                    }
                    let now_secs = now_epoch();
                    let this_min = now_secs / 60;
                    if this_min == last_epoch {
                        continue;
                    }
                    let first = if last_epoch == 0 {
                        this_min
                    } else {
                        (last_epoch + 1).max(this_min.saturating_sub(CATCHUP_MINUTES))
                    };
                    last_epoch = this_min;
                    let slots: Vec<Tm> = (first..=this_min).map(|m| local_at(m * 60)).collect();
                    let (due, errs) = sweep_due(&jobs, &slots);
                    for (job, e) in errs {
                        deliver(job, &format!("job failed: {e}"));
                    }
                    for job in due {
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                        let Some(result) = run_job(job) else {
                            continue;
                        };
                        deliver(job, &postcondition(job, &result));
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
mod cron_semantics_tests {
    use super::*;

    fn tm(min: i64, hour: i64, mday: i64, mon: i64, wday: i64) -> Tm {
        Tm {
            min,
            hour,
            mday,
            mon,
            year: 2026,
            wday,
        }
    }

    #[test]
    fn dom_and_dow_use_or_when_both_restricted() {
        let expr = "0 12 13 * 5";
        assert!(
            cron_due(expr, &tm(0, 12, 13, 7, 1)).unwrap(),
            "13th matches"
        );
        assert!(
            cron_due(expr, &tm(0, 12, 3, 7, 5)).unwrap(),
            "friday matches"
        );
        assert!(
            !cron_due(expr, &tm(0, 12, 3, 7, 1)).unwrap(),
            "neither matches"
        );
    }

    #[test]
    fn dom_and_dow_use_and_when_one_is_wildcard() {
        assert!(cron_due("0 12 * * 5", &tm(0, 12, 3, 7, 5)).unwrap());
        assert!(!cron_due("0 12 * * 5", &tm(0, 12, 3, 7, 4)).unwrap());
        assert!(cron_due("0 12 13 * *", &tm(0, 12, 13, 7, 4)).unwrap());
        assert!(!cron_due("0 12 13 * *", &tm(0, 12, 14, 7, 4)).unwrap());
    }

    #[test]
    fn month_and_day_names_are_accepted() {
        assert!(cron_due("0 9 * jul mon", &tm(0, 9, 27, 7, 1)).unwrap());
        assert!(cron_due("0 9 * JUL MON", &tm(0, 9, 27, 7, 1)).unwrap());
        assert!(!cron_due("0 9 * jan *", &tm(0, 9, 27, 7, 1)).unwrap());
        assert!(cron_due("0 9 * * mon-fri", &tm(0, 9, 27, 7, 3)).unwrap());
        assert!(!cron_due("0 9 * * mon-fri", &tm(0, 9, 27, 7, 0)).unwrap());
    }

    #[test]
    fn sunday_accepts_both_zero_and_seven() {
        assert!(cron_due("0 9 * * 7", &tm(0, 9, 26, 7, 0)).unwrap());
        assert!(cron_due("0 9 * * 0", &tm(0, 9, 26, 7, 0)).unwrap());
    }

    #[test]
    fn shorthand_aliases_expand() {
        assert_eq!(expand_cron_alias("@daily"), "0 0 * * *");
        assert_eq!(expand_cron_alias("@WEEKLY"), "0 0 * * 0");
        assert!(cron_valid("@hourly").is_ok());
        assert!(cron_due("@daily", &tm(0, 0, 5, 7, 3)).unwrap());
        assert!(!cron_due("@daily", &tm(1, 0, 5, 7, 3)).unwrap());
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        assert!(cron_valid("0 25 * * *").is_err());
        assert!(cron_valid("0 0 32 * *").is_err());
        assert!(cron_valid("0 0 * 13 *").is_err());
        assert!(cron_valid("0 0 * * 8").is_err());
        assert!(cron_valid("0 0 * * bogus").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tzif_fixture() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"TZif2");
        d.extend_from_slice(&[0u8; 15]);
        for c in [0u32, 0, 0, 0, 1, 0] {
            d.extend_from_slice(&c.to_be_bytes());
        }
        d.extend_from_slice(&0i32.to_be_bytes());
        d.push(0);
        d.push(0);
        d.extend_from_slice(b"TZif2");
        d.extend_from_slice(&[0u8; 15]);
        for c in [0u32, 0, 0, 1, 2, 0] {
            d.extend_from_slice(&c.to_be_bytes());
        }
        d.extend_from_slice(&1000i64.to_be_bytes());
        d.push(1);
        d.extend_from_slice(&0i32.to_be_bytes());
        d.push(0);
        d.push(0);
        d.extend_from_slice(&3600i32.to_be_bytes());
        d.push(1);
        d.push(0);
        d
    }

    #[test]
    fn tzif_v2_transitions_are_parsed_and_applied() {
        let zone = parse_tzif(&tzif_fixture()).expect("fixture must parse");
        assert_eq!(zone.first_offset, 0);
        assert_eq!(zone.transitions, vec![(1000, 3600)]);
        assert_eq!(offset_in(&zone, 999), 0);
        assert_eq!(offset_in(&zone, 1000), 3600);
        assert_eq!(offset_in(&zone, 50_000), 3600);
    }

    #[test]
    fn civil_conversion_matches_known_dates() {
        let t = civil_from_secs(0);
        assert_eq!(
            (t.year, t.mon, t.mday, t.hour, t.min, t.wday),
            (1970, 1, 1, 0, 0, 4)
        );
        let t = civil_from_secs(1_722_384_000);
        assert_eq!((t.year, t.mon, t.mday, t.wday), (2024, 7, 31, 3));
        let t = civil_from_secs(1_709_209_800);
        assert_eq!(
            (t.year, t.mon, t.mday, t.hour, t.min, t.wday),
            (2024, 2, 29, 12, 30, 4)
        );
        let t = civil_from_secs(-86_400);
        assert_eq!((t.year, t.mon, t.mday, t.wday), (1969, 12, 31, 3));
    }

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
    fn a_missed_minute_is_still_reachable_from_its_epoch_slot() {
        let slot = local_at(0);
        assert_eq!(slot.year, 1970);
        assert!(cron_due("* * * * *", &slot).unwrap());
    }

    #[test]
    fn catchup_replays_only_the_bounded_window_after_a_long_sleep() {
        let slept_minutes = 10_000u64;
        let this_min = slept_minutes;
        let last = 1u64;
        let first = (last + 1).max(this_min.saturating_sub(CATCHUP_MINUTES));
        let replayed = this_min - first + 1;
        assert!(
            replayed <= CATCHUP_MINUTES + 1,
            "a resumed laptop replayed {replayed} minutes"
        );

        let last_recent = this_min - 3;
        let first_recent = (last_recent + 1).max(this_min.saturating_sub(CATCHUP_MINUTES));
        assert_eq!(
            this_min - first_recent + 1,
            3,
            "a short gap must replay every missed minute exactly once"
        );
    }

    #[test]
    fn prechecks_gate_and_scripts_run_without_a_model() {
        let cwd = std::env::temp_dir();
        let mut job = Job {
            webhook: String::new(),
            name: "gated".into(),
            cron: "* * * * *".into(),
            prompt: String::new(),
            chat_ids: Vec::new(),
            expect: String::new(),
            can_act: true,
            precheck: "true".into(),
            script: "echo did the thing".into(),
            model: String::new(),
        };
        assert!(precheck_passes(&job, &cwd));
        assert_eq!(
            script_result(&job, &cwd).as_deref(),
            Some("did the thing"),
            "script output is the delivered result"
        );

        job.precheck = "false".into();
        assert!(
            !precheck_passes(&job, &cwd),
            "a failing precheck skips the run"
        );

        job.precheck = String::new();
        assert!(precheck_passes(&job, &cwd), "no precheck means run");

        job.script = "exit 3".into();
        let out = script_result(&job, &cwd).unwrap();
        assert!(out.starts_with("job failed: script exited 3"), "{out}");

        job.script = "true".into();
        assert_eq!(
            script_result(&job, &cwd).as_deref(),
            Some("(script ok, no output)")
        );

        job.script = String::new();
        assert!(
            script_result(&job, &cwd).is_none(),
            "no script means the model path runs"
        );
    }

    #[test]
    fn next_fire_names_the_upcoming_slot_and_rejects_junk() {
        let t = next_fire("* * * * *", 0).unwrap().expect("every minute");
        assert_eq!((t.year, t.mon, t.mday, t.hour, t.min), (1970, 1, 1, 0, 1));
        let t = next_fire("30 9 * * *", 0).unwrap().expect("daily slot");
        assert_eq!((t.hour, t.min), (9, 30));
        assert!(next_fire("not a cron", 0).is_err());
        assert!(
            next_fire("0 0 30 2 *", 0).unwrap().is_none(),
            "february 30th never comes"
        );
    }

    #[test]
    fn a_postcondition_flags_a_result_missing_the_expected_marker() {
        let mut job = Job {
            webhook: String::new(),
            name: "backup".into(),
            cron: "0 3 * * *".into(),
            prompt: "run the backup and say BACKUP_OK".into(),
            chat_ids: Vec::new(),
            expect: "BACKUP_OK".into(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        };
        let good = postcondition(&job, "all done, BACKUP_OK, 42 files");
        assert_eq!(good, "all done, BACKUP_OK, 42 files");
        let bad = postcondition(&job, "I think it worked");
        assert!(bad.starts_with("job failed postcondition"), "{bad}");
        assert!(bad.contains("BACKUP_OK"), "the missing marker is named");
        assert!(bad.contains("I think it worked"), "the raw result is kept");
        job.expect = String::new();
        assert_eq!(
            postcondition(&job, "anything"),
            "anything",
            "no marker configured means no check"
        );
    }

    #[test]
    fn a_catchup_sweep_runs_a_job_once_even_when_several_minutes_match() {
        let job = Job {
            webhook: String::new(),
            name: "every-minute".into(),
            cron: "* * * * *".into(),
            prompt: "tick".into(),
            chat_ids: Vec::new(),
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        };
        let jobs = vec![job];
        let slots: Vec<Tm> = (0..6).map(|m| local_at(m * 60)).collect();
        let (due, errs) = sweep_due(&jobs, &slots);
        assert_eq!(
            due.len(),
            1,
            "missed minutes must not cascade duplicate runs"
        );
        assert!(errs.is_empty());
    }

    #[test]
    fn a_sweep_reports_a_broken_expression_once_and_still_runs_the_rest() {
        let broken = Job {
            webhook: String::new(),
            name: "broken".into(),
            cron: "not a cron".into(),
            prompt: "x".into(),
            chat_ids: Vec::new(),
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        };
        let fine = Job {
            webhook: String::new(),
            name: "fine".into(),
            cron: "* * * * *".into(),
            prompt: "y".into(),
            chat_ids: Vec::new(),
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        };
        let jobs = vec![broken, fine];
        let slots: Vec<Tm> = (0..3).map(|m| local_at(m * 60)).collect();
        let (due, errs) = sweep_due(&jobs, &slots);
        assert_eq!(errs.len(), 1, "a bad expression is reported once per sweep");
        assert_eq!(errs[0].0.name, "broken");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "fine");
    }

    #[test]
    fn a_sweep_skips_jobs_whose_minutes_did_not_match() {
        let hourly = Job {
            webhook: String::new(),
            name: "hourly".into(),
            cron: "0 * * * *".into(),
            prompt: "z".into(),
            chat_ids: Vec::new(),
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        };
        let jobs = vec![hourly];
        let slots: Vec<Tm> = (1..4).map(|m| local_at(m * 60)).collect();
        let (due, errs) = sweep_due(&jobs, &slots);
        assert!(due.is_empty());
        assert!(errs.is_empty());
        let with_top: Vec<Tm> = (0..4).map(|m| local_at(m * 60)).collect();
        let (due, _) = sweep_due(&jobs, &with_top);
        assert_eq!(
            due.len(),
            1,
            "the matching minute inside the sweep still fires"
        );
    }

    #[test]
    fn local_at_and_now_local_agree_on_the_current_minute() {
        let now = now_local();
        let same = local_at(now_epoch());
        assert_eq!(
            (now.min, now.hour, now.mday),
            (same.min, same.hour, same.mday)
        );
        assert_eq!(
            (now.mon, now.year, now.wday),
            (same.mon, same.year, same.wday)
        );
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
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        }];
        let s = Scheduler::start(&jobs, |_j| Some(String::new()), |_j, _r| {}).unwrap();
        drop(s);
        let empty = vec![Job {
            name: "x".into(),
            cron: String::new(),
            prompt: String::new(),
            chat_ids: Vec::new(),
            webhook: String::new(),
            expect: String::new(),
            can_act: true,
            precheck: String::new(),
            script: String::new(),
            model: String::new(),
        }];
        assert!(Scheduler::start(&empty, |_j| Some(String::new()), |_j, _r| {}).is_none());
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
