use std::fs;
use std::process::{exit, Command, Stdio};

const IMG: &str = "openphoenix:smoke";
const NAME: &str = "phoenix-smoke";
const NET: &str = "phoenix-smoke-net";
const MOCK: &str = "phoenix-smoke-mock";
const MOCKPORT: u16 = 18999;
const PASS: &str = "hunter2";

fn sh(cmd: &str) -> (bool, String) {
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn must(cmd: &str, expect: &str, label: &str) {
    let (ok, out) = sh(cmd);
    if !ok || !out.to_lowercase().contains(&expect.to_lowercase()) {
        eprintln!("FAIL: {label}\n{out}");
        cleanup();
        exit(1);
    }
    println!("ok: {label}");
}

fn cleanup() {
    let _ = sh(&format!("docker rm -f {NAME} {MOCK} >/dev/null 2>&1"));
    let _ = sh(&format!("docker network rm {NET} >/dev/null 2>&1"));
}

fn main() {
    let vol = String::from_utf8(
        Command::new("mktemp").args(["-d", "/tmp/phoenix-smoke.XXXXXX"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    cleanup();
    let (ok, out) = sh(&format!(
        "docker build -q -f docker/Dockerfile --build-arg TARGETARCH=amd64 -t {IMG} ."
    ));
    if !ok {
        eprintln!("FAIL: docker build\n{out}");
        exit(1);
    }
    let _ = sh(&format!("docker network create {NET}"));

    must(&format!("docker run --rm {IMG} --version"), "openphoenix", "--version runs in scratch");

    let _ = sh(&format!("docker run --rm -v {vol}:/data {IMG} init"));
    if !fs::metadata(format!("{vol}/.openphoenix/config.toml")).is_ok() {
        eprintln!("FAIL: init did not write config");
        cleanup();
        exit(1);
    }
    println!("ok: init writes config in volume");

    let (ok, out) = sh(
        "rustc -O --target x86_64-unknown-linux-musl scripts/mock_provider.rs -o /tmp/phoenix-mock-provider",
    );
    if !ok {
        eprintln!("FAIL: build mock provider\n{out}");
        cleanup();
        exit(1);
    }
    let (ok, out) = sh(&format!(
        "docker run -d --name {MOCK} --network {NET} -v /tmp/phoenix-mock-provider:/mock:ro alpine:3 /mock {MOCKPORT}"
    ));
    if !ok {
        eprintln!("FAIL: start mock provider\n{out}");
        cleanup();
        exit(1);
    }
    std::thread::sleep(std::time::Duration::from_secs(2));

    let sha = String::from_utf8(
        Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {PASS} | sha256sum | cut -d' ' -f1"))
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    fs::write(
        format!("{vol}/.openphoenix/config.toml"),
        format!(
            "[provider]\nkind = \"custom\"\nbase_url = \"http://{MOCK}:{MOCKPORT}/v1\"\napi_key = \"smoke\"\nmodel = \"mock-model\"\n\n[agent]\nworkspace = \"/data/work\"\nsessions = true\n\n[http]\nenabled = true\nport = 8787\ntoken = \"smoketoken\"\nweb = true\nusername = \"bob\"\npassword = \"sha256:{sha}\"\n\n[canvas]\nenabled = true\n\n[board]\nenabled = true\n"
        ),
    )
    .unwrap();

    must(
        &format!("docker run --rm --network {NET} -v {vol}:/data {IMG} run \"say hi\""),
        "SMOKE-REPLY",
        "one-shot run round-trips the provider",
    );
    must(&format!("docker run --rm -v {vol}:/data {IMG} doctor && echo DOCTOR-OK"), "DOCTOR-OK", "doctor runs");
    must(&format!("docker run --rm -v {vol}:/data {IMG} jobs"), "no jobs", "jobs listing");
    must(
        &format!("docker run --rm -v {vol}:/data {IMG} sessions && echo SESS-OK"),
        "SESS-OK",
        "sessions listing",
    );

    let (ok, out) = sh(&format!("docker run -d --name {NAME} --network {NET} -v {vol}:/data {IMG} serve"));
    if !ok {
        eprintln!("FAIL: serve start\n{out}");
        cleanup();
        exit(1);
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    let curl = format!("docker run --rm --network container:{NAME} curlimages/curl:latest -s");

    must(
        &format!("{curl} -f -H \"Authorization: Bearer smoketoken\" http://127.0.0.1:8787/health"),
        "\"ok\":true",
        "http /health with bearer token",
    );
    must(
        &format!("{curl} -o /dev/null -w '%{{http_code}}' http://127.0.0.1:8787/health"),
        "401",
        "http auth fail-closed",
    );
    must(
        &format!(
            "{curl} -f -X POST -H \"Authorization: Bearer smoketoken\" -H \"Content-Type: application/json\" -d '{{\"prompt\":\"ping\"}}' http://127.0.0.1:8787/run"
        ),
        "SMOKE-REPLY",
        "http /run full agent round trip",
    );
    must(
        &format!("{curl} -o /dev/null -w '%{{http_code}}' http://127.0.0.1:8787/"),
        "401",
        "web UI 401 anon",
    );
    must(&format!("{curl} -f -u bob:{PASS} http://127.0.0.1:8787/"), "<!doctype", "web UI with creds");
    must(
        &format!("{curl} -o /dev/null -w '%{{http_code}}' http://127.0.0.1:8787/canvas"),
        "401",
        "canvas 401 anon",
    );
    must(
        &format!("{curl} -f -u bob:{PASS} http://127.0.0.1:8787/canvas"),
        "canvas is empty",
        "canvas placeholder",
    );
    must(
        &format!("{curl} -f -u bob:{PASS} http://127.0.0.1:8787/canvas/version"),
        "\"v\":",
        "canvas version",
    );
    must(&format!("{curl} -f http://127.0.0.1:8787/robots.txt"), "Disallow: /", "robots deny-all");

    let (_, out) = sh(&format!("docker run --rm -v {vol}:/data {IMG} skill search memory 2>&1"));
    let low = out.to_lowercase();
    if !(low.contains("slug") || low.contains("no results") || low.contains("found") || out.contains('/')) {
        eprintln!("FAIL: clawhub tls search\n{out}");
        cleanup();
        exit(1);
    }
    println!("ok: outbound TLS works from scratch (ClawHub search)");

    cleanup();
    println!("ALL CONTAINER SMOKE TESTS PASSED");
}
