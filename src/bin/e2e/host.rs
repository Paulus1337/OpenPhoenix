use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub fn run() -> Result<ExitCode, String> {
    let root = repo_root()?;
    build(&root)?;
    let release = target_dir(&root).join("release");
    let phoenix = std::env::var("PHOENIX_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| release.join(executable("phoenix")));
    let runner = release.join(executable("phoenix-e2e"));
    let mut command = Command::new(&runner);
    command.arg("runner").env("PHX", &phoenix);
    if let Ok(filter) = std::env::var("FILTER") {
        if !filter.is_empty() {
            command.env("FILTER", filter);
        }
    }
    match command
        .status()
        .map_err(|error| format!("run {}: {error}", runner.display()))?
    {
        status if status.success() => Ok(ExitCode::SUCCESS),
        _ => Ok(ExitCode::from(1)),
    }
}

fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let mut directory = std::env::current_dir().map_err(|error| error.to_string())?;
    loop {
        if directory.join("Cargo.toml").is_file() {
            return Ok(directory);
        }
        if !directory.pop() {
            return Err("run from inside the repository: Cargo.toml not found".to_string());
        }
    }
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target"))
}

fn build(root: &Path) -> Result<(), String> {
    println!("e2e: compiling the local release binaries");
    let status = Command::new("cargo")
        .args(["build", "--release", "--locked"])
        .current_dir(root)
        .status()
        .map_err(|error| format!("cargo build: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("local release build failed".to_string())
    }
}
