use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Cx {
    pub phx: PathBuf,
}

impl Cx {
    pub fn detect() -> Result<Cx, String> {
        let phx = PathBuf::from(
            std::env::var("PHX").unwrap_or_else(|_| "/usr/local/bin/phoenix".to_string()),
        );
        if !phx.is_file() {
            return Err(format!("phoenix binary not found at {}", phx.display()));
        }
        Ok(Cx { phx })
    }
}

pub struct T {
    name: String,
    pass: u32,
    fail: u32,
}

impl T {
    pub fn new(name: &str) -> T {
        T {
            name: name.to_string(),
            pass: 0,
            fail: 0,
        }
    }

    pub fn ok(&mut self, d: &str) {
        self.pass += 1;
        println!("  ok   {d}");
    }

    pub fn bad(&mut self, d: &str) {
        self.fail += 1;
        println!("  FAIL {d}");
    }

    pub fn check(&mut self, d: &str, cond: bool) {
        if cond {
            self.ok(d);
        } else {
            self.bad(d);
        }
    }

    pub fn finish(self) -> bool {
        println!("{}: {} passed, {} failed", self.name, self.pass, self.fail);
        self.pass > 0 && self.fail == 0
    }
}

pub fn tmpdir_in(bases: &[&str], prefix: &str) -> Result<PathBuf, String> {
    for base in bases {
        let bp = Path::new(base);
        if !bp.is_dir() {
            continue;
        }
        for i in 0..10_000u32 {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let p = bp.join(format!("{prefix}-{}-{n}-{i}", std::process::id()));
            if fs::create_dir(&p).is_ok() {
                return Ok(p);
            }
        }
    }
    Err(format!("could not create a temp dir for {prefix}"))
}

pub fn tmpdir(prefix: &str) -> Result<PathBuf, String> {
    let sys = std::env::temp_dir();
    let sys_s = sys.to_string_lossy().to_string();
    tmpdir_in(&[sys_s.as_str()], prefix)
}

pub fn fresh_home() -> Result<PathBuf, String> {
    tmpdir("phx-home")
}

pub struct RunOut {
    pub rc: i32,
    pub out: String,
    pub err: String,
}

impl RunOut {
    pub fn all(&self) -> String {
        format!("{}{}", self.out, self.err)
    }
}

fn read_thread<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = r.read_to_end(&mut b);
        b
    })
}

fn join_read(h: std::thread::JoinHandle<Vec<u8>>) -> String {
    h.join()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

pub fn run_in(
    home: &Path,
    bin: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    stdin: Option<&[u8]>,
    timeout_ms: u64,
) -> RunOut {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("HOME", home)
        .env_remove("PHOENIX_HOME")
        .env_remove("PHOENIX_STATE_DIR")
        .env_remove("PHOENIX_CONFIG_PATH")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunOut {
                rc: -1,
                out: String::new(),
                err: format!("spawn: {e}"),
            }
        }
    };
    if let Some(data) = stdin {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(data);
        }
    }
    let out_h = child.stdout.take().map(read_thread);
    let err_h = child.stderr.take().map(read_thread);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let rc = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-9),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break 124;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break -1,
        }
    };
    RunOut {
        rc,
        out: out_h.map(join_read).unwrap_or_default(),
        err: err_h.map(join_read).unwrap_or_default(),
    }
}

pub fn phx(cx: &Cx, home: &Path, args: &[&str]) -> RunOut {
    run_in(home, &cx.phx, args, &[], None, 60_000)
}

pub fn phx_env(cx: &Cx, home: &Path, args: &[&str], envs: &[(&str, &str)]) -> RunOut {
    run_in(home, &cx.phx, args, envs, None, 60_000)
}

pub struct Serve {
    child: Option<Child>,
    pub log: PathBuf,
}

pub fn serve(cx: &Cx, home: &Path, envs: &[(&str, &str)]) -> Result<Serve, String> {
    let log = home.join("serve.log");
    let f = fs::File::create(&log).map_err(|e| e.to_string())?;
    let f2 = f.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&cx.phx);
    cmd.arg("serve")
        .env("HOME", home)
        .env_remove("PHOENIX_HOME")
        .env_remove("PHOENIX_STATE_DIR")
        .env_remove("PHOENIX_CONFIG_PATH")
        .stdout(Stdio::from(f))
        .stderr(Stdio::from(f2))
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|e| format!("spawn serve: {e}"))?;
    Ok(Serve {
        child: Some(child),
        log,
    })
}

impl Serve {
    pub fn term(&mut self) -> Option<i32> {
        let pid = self.child.as_ref().map(|c| c.id())?;
        #[cfg(unix)]
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        #[cfg(windows)]
        let _ = pid;
        let mut c = self.child.take()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match c.try_wait() {
                Ok(Some(st)) => return Some(st.code().unwrap_or(-9)),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = c.kill();
                        let _ = c.wait();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

pub fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(20)));
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    let has_conn = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("connection"));
    if !has_conn {
        req.push_str("Connection: close\r\n");
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    let b = body.unwrap_or(&[]);
    if body.is_some() || method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    if !b.is_empty() {
        s.write_all(b).map_err(|e| e.to_string())?;
    }
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("no status line")?;
    let bodytext = text
        .split_once("\r\n\r\n")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    Ok((code, bodytext))
}

pub fn http_code(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> u16 {
    http(port, method, path, headers, body)
        .map(|(c, _)| c)
        .unwrap_or(0)
}

pub fn wait_http(port: u16, path: &str) -> bool {
    for _ in 0..50 {
        if let Ok((code, _)) = http(port, "GET", path, &[], None) {
            if (200..300).contains(&code) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

pub fn read_s(p: &Path) -> String {
    fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

pub fn write_s(p: &Path, s: &str) -> Result<(), String> {
    fs::write(p, s).map_err(|e| format!("write {}: {e}", p.display()))
}

pub fn append_s(p: &Path, s: &str) -> Result<(), String> {
    let mut f = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(p)
        .map_err(|e| e.to_string())?;
    f.write_all(s.as_bytes()).map_err(|e| e.to_string())
}

pub fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

pub fn file_has(p: &Path, needle: &str) -> bool {
    contains_bytes(&fs::read(p).unwrap_or_default(), needle.as_bytes())
}

pub fn dir_has(dir: &Path, needle: &str) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if dir_has(&p, needle) {
                return true;
            }
        } else if file_has(&p, needle) {
            return true;
        }
    }
    false
}

pub fn line_starts(text: &str, prefix: &str) -> bool {
    text.lines().any(|l| l.starts_with(prefix))
}

#[cfg(unix)]
pub fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0)
}

#[cfg(not(unix))]
pub fn mode_of(_p: &Path) -> u32 {
    0
}

#[cfg(unix)]
pub fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
pub fn chmod(_p: &Path, _mode: u32) {}

pub fn mock_config(home: &Path, base: &str, model: &str) -> Result<(), String> {
    let d = home.join(".openphoenix");
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let cfg = d.join("config.toml");
    write_s(
        &cfg,
        &format!(
            "[provider]\nkind = \"openai\"\nmodel = \"{model}\"\napi_key = \"sk-e2e\"\nbase_url = \"{base}/v1\"\n"
        ),
    )?;
    chmod(&cfg, 0o600);
    Ok(())
}

pub fn cfg_path(home: &Path) -> PathBuf {
    home.join(".openphoenix").join("config.toml")
}

pub fn write_config(home: &Path, contents: &str) -> Result<(), String> {
    let d = home.join(".openphoenix");
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let cfg = d.join("config.toml");
    write_s(&cfg, contents)?;
    chmod(&cfg, 0o600);
    Ok(())
}

pub fn kv_line(text: &str, key: &str, want: &str) -> bool {
    text.lines().any(|l| {
        l.strip_prefix(key)
            .map(|rest| {
                let r = rest.trim_start();
                r.strip_prefix('=')
                    .map(|v| v.trim_start().starts_with(want))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    })
}

pub fn has_em_dash(b: &[u8]) -> bool {
    contains_bytes(b, &[0xe2, 0x80, 0x94])
}

fn alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

pub fn has_cred_shapes(b: &[u8]) -> bool {
    let n = b.len();
    for i in 0..n {
        let rest = &b[i..];
        if rest.starts_with(b"sk-") && i + 23 <= n && b[i + 3..i + 23].iter().all(|&c| alnum(c)) {
            return true;
        }
        if rest.starts_with(b"ghp_") && i + 40 <= n && b[i + 4..i + 40].iter().all(|&c| alnum(c)) {
            return true;
        }
        if rest.starts_with(b"xoxb-") {
            let run = b[i + 5..]
                .iter()
                .take_while(|&&c| alnum(c) || c == b'-')
                .count();
            if run >= 10 {
                return true;
            }
        }
        if rest.starts_with(b"AKIA")
            && i + 20 <= n
            && b[i + 4..i + 20]
                .iter()
                .all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

pub fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b0 = u32::from(c[0]);
        let b1 = u32::from(c.get(1).copied().unwrap_or(0));
        let b2 = u32::from(c.get(2).copied().unwrap_or(0));
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(A[(v >> 18) as usize & 63]));
        out.push(char::from(A[(v >> 12) as usize & 63]));
        out.push(if c.len() > 1 {
            char::from(A[(v >> 6) as usize & 63])
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            char::from(A[v as usize & 63])
        } else {
            '='
        });
    }
    out
}

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                if let (Some(hi), Some(lo)) = (hexval(b[i + 1]), hexval(b[i + 2])) {
                    out.push(hi * 16 + lo);
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn form_params(body: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        m.insert(pct_decode(k), serde_json::Value::String(pct_decode(v)));
    }
    m
}

pub fn sha256_hex(b: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, b)
        .as_ref()
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

pub fn replace_in_file(p: &Path, from: &str, to: &str) -> Result<(), String> {
    let s = read_s(p);
    write_s(p, &s.replace(from, to))
}

pub fn replace_line_prefix(p: &Path, prefix: &str, full_line: &str) -> Result<(), String> {
    let s = read_s(p);
    let out: Vec<String> = s
        .lines()
        .map(|l| {
            if l.starts_with(prefix) {
                full_line.to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    write_s(p, &format!("{}\n", out.join("\n")))
}
