use std::path::{Path, PathBuf};

pub const MAX_CELLS: usize = 32;
pub const BASE_PORT: u16 = 8790;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub name: String,
    pub dir: PathBuf,
    pub port: u16,
    pub has_config: bool,
}

pub fn root() -> PathBuf {
    crate::config::home().join("cells")
}

pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.len() <= 32
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !n.starts_with('-')
        && !n.ends_with('-')
}

pub fn cell_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "'{name}' is not a valid cell name: lowercase letters, digits and dashes only"
        ));
    }
    Ok(root.join(name))
}

pub fn port_for(root: &Path, name: &str) -> Result<u16, String> {
    let dir = cell_dir(root, name)?;
    let taken: Vec<u16> = list(root).iter().map(|c| c.port).collect();
    let _ = dir;
    let mut hash: u32 = 2166136261;
    for b in name.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(16777619);
    }
    let span = 500u32;
    let mut port = BASE_PORT + (hash % span) as u16;
    let mut tries = 0;
    while taken.contains(&port) && tries < span {
        port = BASE_PORT + ((u32::from(port - BASE_PORT) + 1) % span) as u16;
        tries += 1;
    }
    if tries >= span {
        return Err("no free port left in the fleet range".into());
    }
    Ok(port)
}

fn config_for(name: &str, port: u16) -> String {
    format!(
        "[provider]\nkind = \"ollama\"\n\n\
[agent]\nprivacy = \"session\"\nworkspace = \"workspace\"\n\n\
[security]\napprovals = true\naudit_log = true\n\n\
[http]\nenabled = false\nbind = \"127.0.0.1\"\nport = {port}\n\n\
[sandbox]\nruntime = \"none\"\n\n\
[pairing]\nenabled = false\n\n\
# cell {name}: every door starts closed\n"
    )
}

pub fn create(root: &Path, name: &str) -> Result<Cell, String> {
    let dir = cell_dir(root, name)?;
    if dir.exists() {
        return Err(format!("cell '{name}' already exists at {}", dir.display()));
    }
    if list(root).len() >= MAX_CELLS {
        return Err(format!("cell limit reached ({MAX_CELLS})"));
    }
    let port = port_for(root, name)?;
    std::fs::create_dir_all(dir.join("workspace")).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let body = config_for(name, port);
    crate::security::write_atomic(&dir.join("config.toml"), body.as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    Ok(Cell {
        name: name.to_string(),
        dir,
        port,
        has_config: true,
    })
}

pub fn list(root: &Path) -> Vec<Cell> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Cell> = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let cfg = dir.join("config.toml");
        let raw = std::fs::read_to_string(&cfg).unwrap_or_default();
        let port = raw
            .lines()
            .find_map(|l| {
                let t = l.trim();
                t.strip_prefix("port")
                    .and_then(|r| r.split('=').nth(1))
                    .and_then(|v| v.trim().parse::<u16>().ok())
            })
            .unwrap_or(0);
        out.push(Cell {
            name: name.to_string(),
            dir: dir.clone(),
            port,
            has_config: cfg.is_file(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn find(root: &Path, name: &str) -> Option<Cell> {
    list(root).into_iter().find(|c| c.name == name)
}

pub fn remove(root: &Path, name: &str, force: bool) -> Result<PathBuf, String> {
    let dir = cell_dir(root, name)?;
    if !dir.is_dir() {
        return Err(format!("no cell named '{name}'"));
    }
    if !force {
        return Err(format!(
            "removing cell '{name}' deletes {}; pass --force to confirm",
            dir.display()
        ));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn env_for(cell: &Cell) -> Vec<(String, String)> {
    vec![
        (
            "PHOENIX_STATE_DIR".to_string(),
            cell.dir.display().to_string(),
        ),
        (
            "PHOENIX_CONFIG_PATH".to_string(),
            cell.dir.join("config.toml").display().to_string(),
        ),
    ]
}

pub fn shell_text(cell: &Cell) -> String {
    let mut out = String::new();
    for (k, v) in env_for(cell) {
        out.push_str(&format!("export {k}=\"{v}\"\n"));
    }
    out
}

pub fn list_text(cells: &[Cell]) -> String {
    if cells.is_empty() {
        return "no fleet cells\ncreate one with: phoenix fleet create NAME\n".to_string();
    }
    let mut out = format!("{} cell(s)\n", cells.len());
    for c in cells {
        out.push_str(&format!(
            "  {:<20}port {:<6}{}{}\n",
            c.name,
            c.port,
            c.dir.display(),
            if c.has_config { "" } else { "  (no config)" }
        ));
    }
    out.push_str("\nrun one with: eval \"$(phoenix fleet env NAME)\" && phoenix status\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-fleet-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_cell_name_can_never_escape_the_fleet_root() {
        let r = tmp();
        for bad in ["../evil", "/etc", "a/b", "Upper", "-lead", "trail-", ""] {
            assert!(cell_dir(&r, bad).is_err(), "{bad} accepted");
            assert!(create(&r, bad).is_err(), "{bad} created");
        }
        let dir = cell_dir(&r, "tenant-a").unwrap();
        assert_eq!(dir.parent(), Some(r.as_path()));
    }

    #[test]
    fn creating_a_cell_writes_a_closed_config_and_a_workspace() {
        let r = tmp();
        let cell = create(&r, "tenant-a").unwrap();
        assert!(cell.dir.join("workspace").is_dir());
        let raw = std::fs::read_to_string(cell.dir.join("config.toml")).unwrap();
        assert!(raw.contains("enabled = false"), "{raw}");
        assert!(raw.contains("approvals = true"), "{raw}");
        assert!(raw.contains("bind = \"127.0.0.1\""), "{raw}");
        assert!(raw.contains(&format!("port = {}", cell.port)), "{raw}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cell.dir.join("config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            let dmode = std::fs::metadata(&cell.dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(dmode, 0o700);
        }
    }

    #[test]
    fn cells_never_share_a_port() {
        let r = tmp();
        let mut ports = Vec::new();
        for i in 0..8 {
            ports.push(create(&r, &format!("cell-{i}")).unwrap().port);
        }
        let mut sorted = ports.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ports.len(), "duplicate port in {ports:?}");
        assert!(ports.iter().all(|p| *p >= BASE_PORT), "{ports:?}");
    }

    #[test]
    fn a_cell_is_never_created_twice() {
        let r = tmp();
        create(&r, "dup").unwrap();
        assert!(create(&r, "dup").is_err());
        assert_eq!(list(&r).len(), 1);
    }

    #[test]
    fn listing_reads_the_port_back_and_ignores_stray_directories() {
        let r = tmp();
        let made = create(&r, "tenant-b").unwrap();
        std::fs::create_dir_all(r.join("NOT a cell")).unwrap();
        std::fs::write(r.join("loose.txt"), "x").unwrap();
        let cells = list(&r);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells.first().map(|c| c.port), Some(made.port));
        assert_eq!(
            find(&r, "tenant-b").map(|c| c.name),
            Some("tenant-b".into())
        );
        assert!(find(&r, "nope").is_none());
    }

    #[test]
    fn removal_needs_an_explicit_force() {
        let r = tmp();
        let cell = create(&r, "doomed").unwrap();
        let err = remove(&r, "doomed", false).unwrap_err();
        assert!(err.contains("--force"), "{err}");
        assert!(cell.dir.is_dir());
        remove(&r, "doomed", true).unwrap();
        assert!(!cell.dir.is_dir());
        assert!(remove(&r, "doomed", true).is_err());
    }

    #[test]
    fn the_env_points_state_and_config_at_the_cell_only() {
        let r = tmp();
        let cell = create(&r, "tenant-c").unwrap();
        let env = env_for(&cell);
        assert_eq!(env.len(), 2);
        for (_, v) in &env {
            assert!(v.starts_with(&cell.dir.display().to_string()), "{v}");
        }
        let text = shell_text(&cell);
        assert!(text.contains("export PHOENIX_STATE_DIR="), "{text}");
        assert!(text.contains("export PHOENIX_CONFIG_PATH="), "{text}");
    }

    #[test]
    fn the_cell_limit_is_enforced() {
        let r = tmp();
        for i in 0..MAX_CELLS {
            create(&r, &format!("c{i}")).unwrap();
        }
        assert!(create(&r, "over").is_err());
    }

    #[test]
    fn list_text_reports_an_empty_fleet_and_names_the_next_step() {
        let r = tmp();
        assert!(list_text(&list(&r)).contains("no fleet cells"));
        create(&r, "one").unwrap();
        let text = list_text(&list(&r));
        assert!(text.contains("1 cell(s)"), "{text}");
        assert!(text.contains("fleet env"), "{text}");
    }
}
