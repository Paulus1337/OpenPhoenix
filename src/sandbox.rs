use std::path::Path;
use std::process::Command;

use crate::config::Config;

pub const RUNTIMES: &[&str] = &["none", "docker", "podman"];
pub const NETWORK_MODES: &[&str] = &["none", "host"];
pub const DEFAULT_IMAGE: &str = "debian:bookworm-slim";
pub const DEFAULT_MEMORY: &str = "512m";
pub const DEFAULT_CPUS: &str = "1";

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub runtime: String,
    pub image: String,
    pub network: String,
    pub memory: String,
    pub cpus: String,
    pub read_only: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            runtime: "none".into(),
            image: DEFAULT_IMAGE.into(),
            network: "none".into(),
            memory: DEFAULT_MEMORY.into(),
            cpus: DEFAULT_CPUS.into(),
            read_only: false,
        }
    }
}

pub fn policy(cfg: &Config) -> Policy {
    Policy {
        runtime: cfg.sandbox_runtime.clone(),
        image: cfg.sandbox_image.clone(),
        network: cfg.sandbox_network.clone(),
        memory: cfg.sandbox_memory.clone(),
        cpus: cfg.sandbox_cpus.clone(),
        read_only: cfg.sandbox_read_only,
    }
}

pub fn validate(p: &Policy) -> Vec<String> {
    let mut out = Vec::new();
    if !RUNTIMES.contains(&p.runtime.as_str()) {
        out.push(format!(
            "sandbox.runtime '{}' is unknown: expected one of {RUNTIMES:?}",
            p.runtime
        ));
    }
    if !NETWORK_MODES.contains(&p.network.as_str()) {
        out.push(format!(
            "sandbox.network '{}' is unknown: expected one of {NETWORK_MODES:?}",
            p.network
        ));
    }
    if p.enabled() && p.image.trim().is_empty() {
        out.push("sandbox.image is empty; set an image to run commands in".into());
    }
    if p.enabled() && p.network == "host" {
        out.push(
            "sandbox.network = \"host\" gives the container your network; \
prefer \"none\" unless a task truly needs it"
                .into(),
        );
    }
    out
}

impl Policy {
    pub fn enabled(&self) -> bool {
        self.runtime != "none" && RUNTIMES.contains(&self.runtime.as_str())
    }

    pub fn args(&self, workspace: &Path, command: &str) -> Result<Vec<String>, String> {
        if !self.enabled() {
            return Err("sandbox is off".into());
        }
        if self.image.trim().is_empty() {
            return Err("sandbox.image is empty".into());
        }
        if !NETWORK_MODES.contains(&self.network.as_str()) {
            return Err(format!("sandbox.network '{}' is unknown", self.network));
        }
        let mount = workspace.to_string_lossy().to_string();
        if mount.trim().is_empty() {
            return Err("workspace path is empty".into());
        }
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "-i".into(),
            "--network".into(),
            self.network.clone(),
            "--memory".into(),
            self.memory.clone(),
            "--cpus".into(),
            self.cpus.clone(),
            "--pids-limit".into(),
            "256".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
        ];
        if self.read_only {
            args.push("--read-only".into());
            args.push("--tmpfs".into());
            args.push("/tmp:rw,noexec,nosuid,size=64m".into());
        }
        args.push("-v".into());
        args.push(format!("{mount}:/work"));
        args.push("-w".into());
        args.push("/work".into());
        args.push(self.image.clone());
        args.push("sh".into());
        args.push("-c".into());
        args.push(command.to_string());
        Ok(args)
    }
}

pub fn runtime_available(runtime: &str) -> bool {
    if !RUNTIMES.contains(&runtime) || runtime == "none" {
        return false;
    }
    Command::new(runtime)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn status_text(p: &Policy) -> String {
    let mut out = String::new();
    if !p.enabled() {
        out.push_str("sandbox: off (shell runs directly on this host)\n");
        out.push_str("  turn it on with [sandbox] runtime = \"docker\" or \"podman\"\n");
        return out;
    }
    let present = runtime_available(&p.runtime);
    out.push_str(&format!(
        "sandbox: {} ({})\n",
        p.runtime,
        if present { "available" } else { "NOT FOUND" }
    ));
    out.push_str(&format!("  image      {}\n", p.image));
    out.push_str(&format!("  network    {}\n", p.network));
    out.push_str(&format!("  memory     {}\n", p.memory));
    out.push_str(&format!("  cpus       {}\n", p.cpus));
    out.push_str(&format!("  read_only  {}\n", p.read_only));
    for w in validate(p) {
        out.push_str(&format!("  warning: {w}\n"));
    }
    if !present {
        out.push_str(&format!(
            "  install {} or set [sandbox] runtime = \"none\"\n",
            p.runtime
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Policy {
        Policy {
            runtime: "docker".into(),
            ..Policy::default()
        }
    }

    #[test]
    fn the_default_policy_is_off_so_nothing_changes_until_asked() {
        let p = Policy::default();
        assert!(!p.enabled());
        assert!(p.args(Path::new("/work"), "ls").is_err());
        assert!(status_text(&p).contains("off"));
    }

    #[test]
    fn an_enabled_policy_drops_capabilities_and_isolates_the_network() {
        let args = on().args(Path::new("/home/me/work"), "echo hi").unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--cap-drop ALL"), "{joined}");
        assert!(
            joined.contains("--security-opt no-new-privileges"),
            "{joined}"
        );
        assert!(joined.contains("--network none"), "{joined}");
        assert!(joined.contains("--pids-limit 256"), "{joined}");
        assert!(joined.contains("--memory 512m"), "{joined}");
        assert!(joined.ends_with("echo hi"), "{joined}");
        assert!(joined.contains("/home/me/work:/work"), "{joined}");
    }

    #[test]
    fn read_only_adds_a_writable_tmpfs_only() {
        let p = Policy {
            read_only: true,
            ..on()
        };
        let joined = p.args(Path::new("/w"), "ls").unwrap().join(" ");
        assert!(joined.contains("--read-only"), "{joined}");
        assert!(
            joined.contains("/tmp:rw,noexec,nosuid,size=64m"),
            "{joined}"
        );
    }

    #[test]
    fn the_command_is_never_split_into_shell_words_by_us() {
        let nasty = "echo 'a b'; rm -rf /nope";
        let args = on().args(Path::new("/w"), nasty).unwrap();
        assert_eq!(args.last().map(String::as_str), Some(nasty));
        assert_eq!(args.iter().filter(|a| *a == "-c").count(), 1);
    }

    #[test]
    fn unknown_runtimes_and_networks_are_refused_not_guessed() {
        let bad = Policy {
            runtime: "chroot".into(),
            ..Policy::default()
        };
        assert!(!bad.enabled());
        assert!(validate(&bad).iter().any(|w| w.contains("runtime")));
        let badnet = Policy {
            network: "bridge".into(),
            ..on()
        };
        assert!(badnet.args(Path::new("/w"), "ls").is_err());
        assert!(validate(&badnet).iter().any(|w| w.contains("network")));
    }

    #[test]
    fn host_networking_is_allowed_but_warned_about() {
        let p = Policy {
            network: "host".into(),
            ..on()
        };
        assert!(p.args(Path::new("/w"), "ls").is_ok());
        assert!(validate(&p).iter().any(|w| w.contains("host")));
    }

    #[test]
    fn an_empty_image_is_refused() {
        let p = Policy {
            image: "  ".into(),
            ..on()
        };
        assert!(p.args(Path::new("/w"), "ls").is_err());
        assert!(validate(&p).iter().any(|w| w.contains("image")));
    }

    #[test]
    fn status_names_the_runtime_and_every_limit() {
        let text = status_text(&on());
        for needle in ["docker", "image", "network", "memory", "cpus", "read_only"] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }

    #[test]
    fn the_none_runtime_never_counts_as_available() {
        assert!(!runtime_available("none"));
        assert!(!runtime_available("nosuchruntime"));
    }
}
