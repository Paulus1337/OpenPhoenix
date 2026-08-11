#![forbid(unsafe_code)]

use std::process::ExitCode;

mod host;
mod httpd;
mod mocks;
mod tests_a;
mod tests_b;
mod util;

fn main() -> ExitCode {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "host".to_string());
    let res = match mode.as_str() {
        "host" => host::run(),
        "runner" => runner(),
        "mcp-echo" => mocks::mcp_echo(),
        other => Err(format!(
            "unknown mode {other}: expected host, runner or mcp-echo"
        )),
    };
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("e2e: {e}");
            ExitCode::from(1)
        }
    }
}

type TestFn = fn(&util::Cx) -> util::T;

fn runner() -> Result<ExitCode, String> {
    let cx = util::Cx::detect()?;
    let filter = std::env::var("FILTER").unwrap_or_default();
    let tests: &[(&str, TestFn)] = &[
        ("01-binary-smoke", tests_a::t01_binary_smoke),
        ("02-init-config", tests_a::t02_init_config),
        ("03-onboard-piped", tests_a::t03_onboard_piped),
        ("04-cli-commands", tests_a::t04_cli_commands),
        (
            "05-commands-reject-unknown",
            tests_a::t05_commands_reject_unknown,
        ),
        ("06-memory-ops", tests_a::t06_memory_ops),
        ("07-serve-startup", tests_a::t07_serve_startup),
        ("08-http-api", tests_a::t08_http_api),
        ("09-web-ui", tests_a::t09_web_ui),
        ("10-ssrf-block", tests_a::t10_ssrf_block),
        ("11-secret-scrub", tests_b::t11_secret_scrub),
        ("12-fail-closed", tests_b::t12_fail_closed),
        ("13-migration", tests_b::t13_migration),
        ("14-session-quarantine", tests_b::t14_session_quarantine),
        ("15-file-permissions", tests_b::t15_file_permissions),
        ("16-update", tests_b::t16_update),
        ("17-mock-provider", tests_b::t17_mock_provider),
        ("18-doctor", tests_b::t18_doctor),
        ("18-telegram-poll", tests_b::t19_telegram_poll),
        ("18-colab", tests_b::t20_colab),
    ];
    let mut results: Vec<(String, bool)> = Vec::new();
    for (name, f) in tests {
        if !filter.is_empty() && !name.contains(filter.as_str()) {
            continue;
        }
        println!("=== {name} ===");
        let good = f(&cx).finish();
        results.push(((*name).to_string(), good));
        println!();
    }
    if results.is_empty() {
        return Err(format!("FILTER matched no E2E scenario: {filter}"));
    }
    println!("=== E2E summary ===");
    let mut overall = true;
    for (name, good) in &results {
        println!("  {} {name}", if *good { "PASS" } else { "FAIL" });
        if !good {
            overall = false;
        }
    }
    if overall {
        println!("No ashes left behind.");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("Some feathers burned.");
        Ok(ExitCode::from(1))
    }
}
