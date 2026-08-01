use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ring::signature::KeyPair;

use crate::util;

const DOCKERFILE: &str = "FROM debian:bookworm-slim\nCOPY phoenix /usr/local/bin/phoenix\nCOPY phoenix-e2e /usr/local/bin/phoenix-e2e\nCOPY signing.key /e2e/signing.key\nENV HOME=/data\nWORKDIR /data\nENTRYPOINT [\"/usr/local/bin/phoenix-e2e\", \"runner\"]\n";

const MUSL: &str = "x86_64-unknown-linux-musl";

pub fn run() -> Result<ExitCode, String> {
    let root = repo_root()?;
    let rng = ring::rand::SystemRandom::new();
    let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| "signing keygen failed".to_string())?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref())
        .map_err(|_| "signing key parse failed".to_string())?;
    let pub_hex: String = pair
        .public_key()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("e2e: ephemeral release key {pub_hex}");
    let (phx, runner) = binaries(&root, &pub_hex)?;
    let stage = util::tmpdir_in(&["/root/fuzz_ram", "/tmp"], "e2e-stage")?;
    let res = stage_and_run(&stage, &phx, &runner, doc.as_ref());
    let _ = fs::remove_dir_all(&stage);
    res
}

fn repo_root() -> Result<PathBuf, String> {
    let mut d = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        if d.join("Cargo.toml").is_file() {
            return Ok(d);
        }
        if !d.pop() {
            return Err("run from inside the repo: Cargo.toml not found".to_string());
        }
    }
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target"))
}

fn binaries(root: &Path, pub_hex: &str) -> Result<(PathBuf, PathBuf), String> {
    let tdir = target_dir(root).join(MUSL).join("release");
    if build_local(root, pub_hex) {
        let phx = match std::env::var("PHOENIX_BIN") {
            Ok(bin) => {
                println!("e2e: using PHOENIX_BIN {bin}");
                PathBuf::from(bin)
            }
            Err(_) => tdir.join("phoenix"),
        };
        return Ok((phx, tdir.join("phoenix-e2e")));
    }
    println!("e2e: local musl build unavailable, compiling in a rust container");
    build_docker(root, pub_hex)
}

fn build_local(root: &Path, pub_hex: &str) -> bool {
    println!("e2e: compiling the musl release locally");
    Command::new("cargo")
        .args(["build", "--release", "--locked", "--target", MUSL])
        .current_dir(root)
        .env("PHOENIX_SIGNING_PUBKEY", pub_hex)
        .status()
        .map(|st| st.success())
        .unwrap_or(false)
}

fn build_docker(root: &Path, pub_hex: &str) -> Result<(PathBuf, PathBuf), String> {
    let _ = Command::new("docker")
        .args(["volume", "create", "phoenix-e2e-cargo"])
        .output();
    let out = util::tmpdir_in(&["/root/fuzz_ram", "/tmp"], "e2e-dist")?;
    let script = format!(
        "rustup target add {MUSL} && apt-get update -qq && \
         apt-get install -y -qq --no-install-recommends musl-tools && \
         cargo build --release --locked --target {MUSL} && \
         cp /cache/target/{MUSL}/release/phoenix /out/phoenix && \
         cp /cache/target/{MUSL}/release/phoenix-e2e /out/phoenix-e2e"
    );
    let args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--network=host".into(),
        "-v".into(),
        "/etc/resolv.conf:/etc/resolv.conf:ro".into(),
        "-v".into(),
        format!("{}/Cargo.toml:/build/Cargo.toml:ro", root.display()),
        "-v".into(),
        format!("{}/Cargo.lock:/build/Cargo.lock:ro", root.display()),
        "-v".into(),
        format!("{}/src:/build/src:ro", root.display()),
        "-v".into(),
        format!("{}/assets:/build/assets:ro", root.display()),
        "-v".into(),
        format!("{}:/out", out.display()),
        "-v".into(),
        "phoenix-e2e-cargo:/cache".into(),
        "-e".into(),
        "CARGO_HOME=/cache/cargo".into(),
        "-e".into(),
        "CARGO_TARGET_DIR=/cache/target".into(),
        "-e".into(),
        format!("PHOENIX_SIGNING_PUBKEY={pub_hex}"),
        "-w".into(),
        "/build".into(),
        "rust:latest".into(),
        "bash".into(),
        "-ec".into(),
        script,
    ];
    let st = Command::new("docker")
        .args(&args)
        .status()
        .map_err(|e| format!("docker: {e}"))?;
    if !st.success() {
        return Err("container musl build failed".to_string());
    }
    Ok((out.join("phoenix"), out.join("phoenix-e2e")))
}

fn stage_and_run(
    stage: &Path,
    phx: &Path,
    runner: &Path,
    key_der: &[u8],
) -> Result<ExitCode, String> {
    fs::copy(phx, stage.join("phoenix"))
        .map_err(|e| format!("stage phoenix from {}: {e}", phx.display()))?;
    fs::copy(runner, stage.join("phoenix-e2e"))
        .map_err(|e| format!("stage phoenix-e2e from {}: {e}", runner.display()))?;
    fs::write(stage.join("signing.key"), key_der).map_err(|e| e.to_string())?;
    fs::write(stage.join("Dockerfile"), DOCKERFILE).map_err(|e| e.to_string())?;
    util::chmod(&stage.join("phoenix"), 0o755);
    util::chmod(&stage.join("phoenix-e2e"), 0o755);
    util::chmod(&stage.join("signing.key"), 0o644);
    let stage_s = stage.to_str().ok_or("stage path is not utf-8")?;
    let st = Command::new("docker")
        .args(["build", "-t", "phoenix-e2e", stage_s])
        .status()
        .map_err(|e| format!("docker build: {e}"))?;
    if !st.success() {
        return Err("docker build failed".to_string());
    }
    let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
    if let Ok(f) = std::env::var("FILTER") {
        if !f.is_empty() {
            args.push("-e".into());
            args.push(format!("FILTER={f}"));
        }
    }
    args.push("phoenix-e2e".into());
    let st = Command::new("docker")
        .args(&args)
        .status()
        .map_err(|e| format!("docker run: {e}"))?;
    if st.success() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}
