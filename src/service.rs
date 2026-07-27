use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::config;

const UNIT_NAME: &str = "phoenix.service";

pub fn systemd_available() -> bool {
    std::path::Path::new("/run/systemd/system").exists()
}

fn is_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|v| v == "0"))
        })
        .unwrap_or(false)
}

fn unit_path(user_mode: bool) -> PathBuf {
    if user_mode {
        config::home_dir()
            .join(".config/systemd/user")
            .join(UNIT_NAME)
    } else {
        PathBuf::from("/etc/systemd/system").join(UNIT_NAME)
    }
}

pub fn unit_content(exe: &str, phoenix_home: &str, user_mode: bool) -> String {
    let wanted_by = if user_mode {
        "default.target"
    } else {
        "multi-user.target"
    };
    format!(
        "[Unit]\n\
Description=OpenPhoenix gateway (serve)\n\
Documentation=https://github.com/Paulus1337/OpenPhoenix\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
ExecStart={exe} serve\n\
Environment=PHOENIX_HOME={phoenix_home}\n\
EnvironmentFile=-{phoenix_home}/env\n\
Restart=on-failure\n\
RestartSec=5\n\
NoNewPrivileges=true\n\
\n\
[Install]\n\
WantedBy={wanted_by}\n"
    )
}

fn systemctl(user_mode: bool, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("systemctl");
    if user_mode {
        cmd.arg("--user");
    }
    cmd.args(args);
    let out = cmd.output().map_err(|e| format!("systemctl: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

pub fn install() -> Result<String, String> {
    if !systemd_available() {
        return Err("systemd not detected; run `phoenix serve` under your own supervisor".into());
    }
    let user_mode = !is_root();
    let exe = env::current_exe()
        .map_err(|e| e.to_string())?
        .display()
        .to_string();
    let home = config::home();
    fs::create_dir_all(&home).map_err(|e| e.to_string())?;

    let envf = home.join("env");
    if !envf.exists() {
        fs::write(
            &envf,
            "# Secrets for the phoenix service (mode 600). One KEY=value per line.\n\
# PHOENIX_API_KEY=\n# PHOENIX_TELEGRAM_TOKEN=\n",
        )
        .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&envf, fs::Permissions::from_mode(0o600));
        }
    }

    let path = unit_path(user_mode);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(
        &path,
        unit_content(&exe, &home.display().to_string(), user_mode),
    )
    .map_err(|e| format!("write {}: {e}", path.display()))?;
    systemctl(user_mode, &["daemon-reload"])?;
    systemctl(user_mode, &["enable", "--now", UNIT_NAME])?;
    Ok(format!(
        "service installed and started ({})\n  unit:    {}\n  secrets: {}\n  status:  phoenix service status",
        if user_mode { "user" } else { "system" },
        path.display(),
        envf.display(),
    ))
}

fn uninstall() -> Result<String, String> {
    let user_mode = !is_root();
    let _ = systemctl(user_mode, &["disable", "--now", UNIT_NAME]);
    let path = unit_path(user_mode);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    systemctl(user_mode, &["daemon-reload"])?;
    Ok("service stopped and removed".into())
}

pub fn state() -> String {
    if !systemd_available() {
        return "no systemd".into();
    }
    let user_mode = !is_root();
    match systemctl(user_mode, &["is-active", UNIT_NAME]) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            let t = e.trim();
            if t == "inactive" || t == "failed" || t == "activating" {
                t.to_string()
            } else {
                "not installed".into()
            }
        }
    }
}

pub fn cmd_service(words: &[String]) -> u8 {
    let usage = "usage: phoenix service install|uninstall|start|stop|restart|status|logs";
    let Some(action) = words.first().map(String::as_str) else {
        eprintln!("{usage}");
        return 2;
    };
    if !systemd_available() {
        eprintln!("systemd not detected; run `phoenix serve` under your own supervisor");
        return 2;
    }
    let user_mode = !is_root();
    let result = match action {
        "install" => install(),
        "uninstall" => uninstall(),
        "start" | "stop" | "restart" => {
            systemctl(user_mode, &[action, UNIT_NAME]).map(|_| format!("service {action}: ok"))
        }
        "status" => systemctl(user_mode, &["status", "--no-pager", "--full", UNIT_NAME])
            .or_else(|e| if e.contains("Loaded:") { Ok(e) } else { Err(e) }),
        "logs" => {
            let mut cmd = Command::new("journalctl");
            if user_mode {
                cmd.args(["--user-unit", UNIT_NAME]);
            } else {
                cmd.args(["-u", UNIT_NAME]);
            }
            cmd.args(["-n", "50", "--no-pager"]);
            match cmd.output() {
                Ok(o) => Ok(format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                )),
                Err(e) => Err(format!("journalctl: {e}")),
            }
        }
        _ => {
            eprintln!("{usage}");
            return 2;
        }
    };
    match result {
        Ok(out) => {
            println!("{}", out.trim_end());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_pins_home_and_exec() {
        let u = unit_content("/usr/local/bin/phoenix", "/root/.openphoenix", false);
        assert!(u.contains("ExecStart=/usr/local/bin/phoenix serve"));
        assert!(u.contains("Environment=PHOENIX_HOME=/root/.openphoenix"));
        assert!(u.contains("EnvironmentFile=-/root/.openphoenix/env"));
        assert!(u.contains("WantedBy=multi-user.target"));
        assert!(u.contains("NoNewPrivileges=true"));
    }

    #[test]
    fn user_unit_targets_default() {
        let u = unit_content("/x/phoenix", "/home/u/.openphoenix", true);
        assert!(u.contains("WantedBy=default.target"));
    }
}
