use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

pub const STALE_SECS: u64 = 30;

static STOPPING: AtomicBool = AtomicBool::new(false);

static INTERRUPT_PENDING: AtomicBool = AtomicBool::new(false);

static INTERRUPT_ARMED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_sig: libc::c_int) {
    STOPPING.store(true, Ordering::SeqCst);
}

extern "C" fn on_interrupt(_sig: libc::c_int) {
    if INTERRUPT_ARMED.load(Ordering::SeqCst) {
        INTERRUPT_PENDING.store(true, Ordering::SeqCst);
        return;
    }
    const BYE: &[u8] = b"\n\x1b[?25h";
    #[cfg(unix)]
    let count = BYE.len();
    #[cfg(not(unix))]
    let count = BYE.len() as u32;
    unsafe {
        let _ = libc::write(2, BYE.as_ptr() as *const libc::c_void, count);
        libc::_exit(130);
    }
}

#[cfg(unix)]
pub fn install_stop_handler() {
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_stop as *const () as libc::sighandler_t);
    }
}

#[cfg(windows)]
pub fn install_stop_handler() {
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
    }
}

pub fn install_interrupt_handler() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_interrupt as *const () as libc::sighandler_t,
        );
    }
}

pub fn interrupt_pending_swap() -> bool {
    INTERRUPT_PENDING.swap(false, Ordering::SeqCst)
}

pub fn set_interrupt_pending(pending: bool) {
    INTERRUPT_PENDING.store(pending, Ordering::SeqCst);
}

pub fn set_interrupt_armed(armed: bool) {
    INTERRUPT_ARMED.store(armed, Ordering::SeqCst);
}

pub fn stopping() -> bool {
    STOPPING.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Owner {
    pub pid: u32,
    pub start: u64,
    pub created: u64,
    pub exe: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerState {
    Alive,
    Dead,
    Unknown,
}

pub fn default_path() -> PathBuf {
    crate::config::home().join("serve.lock")
}

fn now() -> u64 {
    crate::scheduler::now_epoch()
}

fn parse_owner(text: &str) -> Option<Owner> {
    let v: Value = serde_json::from_str(text).ok()?;
    let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
    if pid == 0 {
        return None;
    }
    Some(Owner {
        pid,
        start: v["start"].as_u64().unwrap_or(0),
        created: v["created"].as_u64().unwrap_or(0),
        exe: v["exe"].as_str().unwrap_or("").to_string(),
    })
}

pub fn read_owner(path: &Path) -> Option<Owner> {
    parse_owner(&fs::read_to_string(path).ok()?)
}

pub fn unparseable_lock_is_stale(path: &Path, now_secs: u64, stale_secs: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = now_secs;
    age > stale_secs
}

pub fn owner_state(owner: &Owner) -> OwnerState {
    if crate::tasks::zombie(owner.pid) {
        return OwnerState::Dead;
    }
    match crate::tasks::proc_start(owner.pid) {
        Some(start) if owner.start != 0 => {
            if start == owner.start {
                OwnerState::Alive
            } else {
                OwnerState::Dead
            }
        }
        Some(_) => OwnerState::Unknown,
        None => OwnerState::Dead,
    }
}

pub fn reclaimable(owner: &Owner, state: OwnerState, now_secs: u64, stale_secs: u64) -> bool {
    match state {
        OwnerState::Dead => true,
        OwnerState::Alive => false,
        OwnerState::Unknown => {
            owner.created != 0 && now_secs.saturating_sub(owner.created) > stale_secs
        }
    }
}

#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
    held: bool,
}

impl Lock {
    pub fn acquire(path: &Path) -> Result<Lock, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let pid = std::process::id();
        let start = crate::tasks::proc_start(pid).unwrap_or(0);
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let body = serde_json::to_string(&json!({
            "pid": pid,
            "start": start,
            "created": now(),
            "exe": exe,
        }))
        .unwrap_or_default();

        for _ in 0..2 {
            match Self::claim(path, &body) {
                Ok(()) => {
                    return Ok(Lock {
                        path: path.to_path_buf(),
                        held: true,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let Some(owner) = read_owner(path) else {
                        if !unparseable_lock_is_stale(path, now(), STALE_SECS) {
                            return Err(format!(
                                "another process is claiming the serve lock right now; \
retry in a moment or remove {}",
                                path.display()
                            ));
                        }
                        fs::remove_file(path).map_err(|e| e.to_string())?;
                        continue;
                    };
                    let state = owner_state(&owner);
                    if !reclaimable(&owner, state, now(), STALE_SECS) {
                        return Err(format!(
                            "phoenix serve is already running (pid {}); stop it first or remove {}",
                            owner.pid,
                            path.display()
                        ));
                    }
                    fs::remove_file(path).map_err(|e| e.to_string())?;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("could not acquire the serve lock; two processes are racing".into())
    }

    fn claim(path: &Path, body: &str) -> std::io::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "claim.{}.{:?}.{}",
            std::process::id(),
            std::thread::current().id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&tmp);
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut fh = opts.open(&tmp)?;
            fh.write_all(body.as_bytes())?;
            fh.sync_all()?;
        }
        let result = fs::hard_link(&tmp, path);
        let _ = fs::remove_file(&tmp);
        result
    }

    pub fn release(&mut self) {
        if !self.held {
            return;
        }
        self.held = false;
        if read_owner(&self.path).is_some_and(|o| o.pid == std::process::id()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        self.release();
    }
}

pub fn report(path: &Path) -> Value {
    match read_owner(path) {
        None => json!({"running": false}),
        Some(owner) => {
            let state = owner_state(&owner);
            json!({
                "running": state == OwnerState::Alive,
                "pid": owner.pid,
                "state": match state {
                    OwnerState::Alive => "alive",
                    OwnerState::Dead => "dead",
                    OwnerState::Unknown => "unknown",
                },
                "uptime_secs": now().saturating_sub(owner.created),
                "exe": owner.exe,
                "stale": reclaimable(&owner, state, now(), STALE_SECS),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-daemon-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_owner(path: &Path, owner: &Value) {
        fs::write(path, serde_json::to_string(owner).unwrap()).unwrap();
    }

    #[test]
    fn second_instance_is_refused_while_the_first_holds_the_lock() {
        let d = tmpdir("dup");
        let p = d.join("serve.lock");
        let _first = Lock::acquire(&p).unwrap();
        let err = Lock::acquire(&p).unwrap_err();
        assert!(err.contains("already running"), "got: {err}");
        assert!(err.contains(&std::process::id().to_string()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn release_lets_the_next_instance_start() {
        let d = tmpdir("release");
        let p = d.join("serve.lock");
        let mut first = Lock::acquire(&p).unwrap();
        first.release();
        assert!(!p.exists(), "lock file must be gone after release");
        let _second = Lock::acquire(&p).unwrap();
        assert!(p.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn drop_releases_the_lock() {
        let d = tmpdir("drop");
        let p = d.join("serve.lock");
        {
            let _l = Lock::acquire(&p).unwrap();
            assert!(p.exists());
        }
        assert!(!p.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_dead_owner_is_reclaimed() {
        let d = tmpdir("dead");
        let p = d.join("serve.lock");
        write_owner(
            &p,
            &json!({"pid": 4_000_000, "start": 999, "created": now(), "exe": "/x/phoenix"}),
        );
        let _l = Lock::acquire(&p).unwrap();
        assert_eq!(read_owner(&p).unwrap().pid, std::process::id());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_recycled_pid_is_not_mistaken_for_the_owner() {
        let d = tmpdir("recycle");
        let p = d.join("serve.lock");
        let me = std::process::id();
        let real = crate::tasks::proc_start(me).unwrap_or(0);
        write_owner(
            &p,
            &json!({"pid": me, "start": real + 1, "created": now(), "exe": "/x/phoenix"}),
        );
        let owner = read_owner(&p).unwrap();
        assert_eq!(owner_state(&owner), OwnerState::Dead);
        let _l = Lock::acquire(&p).unwrap();
        assert_eq!(read_owner(&p).unwrap().start, real);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_live_owner_is_never_reclaimed_however_old() {
        let me = std::process::id();
        let start = crate::tasks::proc_start(me).unwrap_or(0);
        let owner = Owner {
            pid: me,
            start,
            created: 0,
            exe: String::new(),
        };
        assert_eq!(owner_state(&owner), OwnerState::Alive);
        assert!(!reclaimable(
            &owner,
            OwnerState::Alive,
            now() + 86_400,
            STALE_SECS
        ));
    }

    #[test]
    fn an_unknown_owner_is_reclaimed_only_once_stale() {
        let owner = Owner {
            pid: 123,
            start: 0,
            created: 1_000,
            exe: String::new(),
        };
        assert!(!reclaimable(&owner, OwnerState::Unknown, 1_010, STALE_SECS));
        assert!(reclaimable(&owner, OwnerState::Unknown, 1_100, STALE_SECS));
        let no_ts = Owner {
            created: 0,
            ..owner
        };
        assert!(!reclaimable(
            &no_ts,
            OwnerState::Unknown,
            9_999_999,
            STALE_SECS
        ));
    }

    #[cfg(unix)]
    fn age_lock_file(path: &Path, secs: u64) {
        let target = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
        let epoch = target
            .duration_since(std::time::UNIX_EPOCH)
            .expect("post-epoch")
            .as_secs();
        let stamp = format!("@{epoch}");
        let out = std::process::Command::new("touch")
            .args(["-d", &stamp])
            .arg(path)
            .output()
            .expect("touch must run");
        assert!(out.status.success(), "touch -d {stamp} failed");
    }

    #[test]
    fn a_fresh_unreadable_lock_is_never_stolen() {
        let d = tmpdir("corrupt");
        let p = d.join("serve.lock");
        fs::write(&p, "not json at all").unwrap();
        let err = match Lock::acquire(&p) {
            Ok(_) => panic!("a lock being written right now must not be stolen"),
            Err(e) => e,
        };
        assert!(err.contains("claiming the serve lock"), "{err}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "not json at all");
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn an_aged_out_unreadable_lock_is_reclaimed() {
        let d = tmpdir("corrupt-old");
        let p = d.join("serve.lock");
        fs::write(&p, "not json at all").unwrap();
        age_lock_file(&p, STALE_SECS * 4);
        let _l = Lock::acquire(&p).expect("an abandoned torn lock must not brick serve");
        assert_eq!(read_owner(&p).unwrap().pid, std::process::id());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_fresh_lock_without_a_pid_is_never_stolen() {
        let d = tmpdir("nopid");
        let p = d.join("serve.lock");
        write_owner(&p, &json!({"created": now()}));
        assert!(read_owner(&p).is_none());
        assert!(
            Lock::acquire(&p).is_err(),
            "a payload-less lock may be a racing writer, not a dead owner"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn an_aged_out_lock_without_a_pid_is_reclaimed() {
        let d = tmpdir("nopid-old");
        let p = d.join("serve.lock");
        write_owner(&p, &json!({"created": now()}));
        age_lock_file(&p, STALE_SECS * 4);
        let _l = Lock::acquire(&p).expect("an abandoned payload-less lock must be reclaimable");
        assert_eq!(read_owner(&p).unwrap().pid, std::process::id());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_claim_is_never_visible_without_its_owner_payload() {
        let d = tmpdir("atomic-claim");
        let p = d.join("serve.lock");
        let _l = Lock::acquire(&p).unwrap();
        let owner = read_owner(&p).expect("the lock must carry its owner the instant it exists");
        assert_eq!(owner.pid, std::process::id());
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("claim."))
            .collect();
        assert!(leftovers.is_empty(), "claim temp files must be cleaned up");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn release_never_deletes_a_lock_owned_by_someone_else() {
        let d = tmpdir("steal");
        let p = d.join("serve.lock");
        let mut mine = Lock::acquire(&p).unwrap();
        write_owner(
            &p,
            &json!({"pid": 4_000_001, "start": 5, "created": now(), "exe": "/other"}),
        );
        mine.release();
        assert!(p.exists(), "must not delete another process's lock");
        assert_eq!(read_owner(&p).unwrap().pid, 4_000_001);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn racing_threads_never_hold_the_lock_at_the_same_time() {
        use std::sync::atomic::AtomicI32;
        use std::sync::Arc;

        let d = tmpdir("race");
        let p = d.join("serve.lock");
        let holders = Arc::new(AtomicI32::new(0));
        let peak = Arc::new(AtomicI32::new(0));
        let wins = Arc::new(AtomicI32::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let p2 = p.clone();
            let holders = Arc::clone(&holders);
            let peak = Arc::clone(&peak);
            let wins = Arc::clone(&wins);
            handles.push(std::thread::spawn(move || {
                if let Ok(l) = Lock::acquire(&p2) {
                    wins.fetch_add(1, Ordering::SeqCst);
                    let n = holders.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    holders.fetch_sub(1, Ordering::SeqCst);
                    drop(l);
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "two processes held the serve lock at once"
        );
        assert!(
            wins.load(Ordering::SeqCst) >= 1,
            "at least one thread must get in"
        );
        assert!(!p.exists(), "lock must be free once everyone is done");
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("perms");
        let p = d.join("serve.lock");
        let _l = Lock::acquire(&p).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn report_describes_running_and_absent_states() {
        let d = tmpdir("report");
        let p = d.join("serve.lock");
        assert_eq!(report(&p)["running"], json!(false));
        let _l = Lock::acquire(&p).unwrap();
        let r = report(&p);
        assert_eq!(r["running"], json!(true));
        assert_eq!(r["state"], json!("alive"));
        assert_eq!(r["pid"], json!(std::process::id()));
        assert_eq!(r["stale"], json!(false));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_stop_signal_flips_the_shutdown_flag() {
        install_stop_handler();
        assert!(!stopping());
        let me = std::process::id().to_string();
        let sent = std::process::Command::new("kill")
            .args(["-s", "TERM", &me])
            .output()
            .expect("kill must run");
        assert!(sent.status.success(), "kill -s TERM failed");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !stopping() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(stopping(), "SIGTERM must request a clean stop");
        STOPPING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_zombie_owner_counts_as_dead() {
        let d = tmpdir("zombie");
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let owner = Owner {
            pid,
            start: 1,
            created: now(),
            exe: String::new(),
        };
        let mut state = owner_state(&owner);
        let mut seen_zombie = crate::tasks::zombie(pid);
        for _ in 0..100 {
            if seen_zombie && state == OwnerState::Dead {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            seen_zombie |= crate::tasks::zombie(pid);
            state = owner_state(&owner);
        }
        #[cfg(target_os = "linux")]
        assert!(seen_zombie, "child should be an unreaped zombie");
        assert_eq!(
            state,
            OwnerState::Dead,
            "an exited owner must never hold the lock"
        );
        assert!(reclaimable(&owner, state, now(), STALE_SECS));
        let _ = child.wait();
        let _ = fs::remove_dir_all(&d);
    }
}
