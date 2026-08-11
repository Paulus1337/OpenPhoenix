use std::path::{Path, PathBuf};

use serde_json::Value;

pub const MANIFEST: &str = "CLAW.md";
pub const MAX_SKILLS: usize = 32;
pub const MAX_JOBS: usize = 32;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Claw {
    pub name: String,
    pub version: String,
    pub summary: String,
    pub prompt: String,
    pub skills: Vec<String>,
    pub jobs: Vec<(String, String)>,
    pub mcp: Vec<String>,
}

pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.len() <= 48
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn frontmatter(text: &str) -> Result<(&str, &str), String> {
    let rest = text
        .strip_prefix("---")
        .ok_or("a CLAW.md must start with a --- line")?;
    rest.split_once("\n---")
        .ok_or_else(|| "frontmatter is never closed: add a --- line".to_string())
}

pub fn parse(text: &str) -> Result<Claw, String> {
    let (head, body) = frontmatter(text)?;
    let mut claw = Claw {
        version: "0".into(),
        ..Claw::default()
    };
    let mut list_key = String::new();
    for line in head.lines() {
        let trimmed = line.trim();
        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match list_key.as_str() {
                "skills" => claw.skills.push(item.to_string()),
                "mcp" => claw.mcp.push(item.to_string()),
                "jobs" => {
                    let Some((name, cron)) = item.split_once('=') else {
                        return Err(format!("job '{item}' must look like name=cron expression"));
                    };
                    claw.jobs
                        .push((name.trim().to_string(), cron.trim().to_string()));
                }
                _ => {}
            }
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let value = v.trim();
        if value.is_empty() {
            list_key = key.to_string();
            continue;
        }
        list_key.clear();
        match key {
            "name" => claw.name = value.to_string(),
            "version" => claw.version = value.to_string(),
            "summary" => claw.summary = value.to_string(),
            _ => {}
        }
    }
    if !valid_name(&claw.name) {
        return Err("a CLAW.md needs a valid name: field".into());
    }
    if claw.skills.len() > MAX_SKILLS {
        return Err(format!("a Claw may declare at most {MAX_SKILLS} skills"));
    }
    if claw.jobs.len() > MAX_JOBS {
        return Err(format!("a Claw may declare at most {MAX_JOBS} jobs"));
    }
    for (name, cron) in &claw.jobs {
        if name.is_empty() {
            return Err("a job needs a name".into());
        }
        crate::scheduler::cron_valid(cron)
            .map_err(|e| format!("job '{name}' has a bad schedule: {e}"))?;
    }
    for s in &claw.skills {
        if !valid_name(s) {
            return Err(format!("skill name '{s}' is not usable"));
        }
    }
    claw.prompt = body.trim_start_matches('-').trim().to_string();
    if claw.prompt.is_empty() {
        return Err(format!("Claw '{}' has an empty body", claw.name));
    }
    Ok(claw)
}

pub fn read(dir: &Path) -> Result<Claw, String> {
    let path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("no {MANIFEST} in {}", dir.display()))?;
    parse(&text)
}

pub fn plan(claw: &Claw, agents_dir: &Path) -> Vec<String> {
    let mut out = vec![format!(
        "create agent '{}' at {}",
        claw.name,
        agents_dir.join(&claw.name).display()
    )];
    if !claw.prompt.is_empty() {
        out.push(format!("write PROMPT.md ({} bytes)", claw.prompt.len()));
    }
    for s in &claw.skills {
        out.push(format!("install skill '{s}'"));
    }
    for (name, cron) in &claw.jobs {
        out.push(format!("add job '{name}' on '{cron}'"));
    }
    for m in &claw.mcp {
        out.push(format!("expect mcp server '{m}' in the config"));
    }
    out
}

pub fn install(claw: &Claw, agents_dir: &Path) -> Result<PathBuf, String> {
    if !valid_name(&claw.name) {
        return Err("refusing to install a Claw with an unusable name".into());
    }
    let dir = agents_dir.join(&claw.name);
    if dir.exists() {
        return Err(format!(
            "agent '{}' already exists at {}; a Claw never overwrites one",
            claw.name,
            dir.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::security::write_atomic(&dir.join("PROMPT.md"), claw.prompt.as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    let meta = serde_json::json!({
        "name": claw.name,
        "version": claw.version,
        "summary": claw.summary,
        "skills": claw.skills,
        "jobs": claw.jobs.iter().map(|(n, c)| serde_json::json!({"name": n, "cron": c})).collect::<Vec<Value>>(),
        "mcp": claw.mcp,
    });
    let body = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
    crate::security::write_atomic(&dir.join("claw.json"), body.as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn describe(claw: &Claw) -> String {
    let mut out = format!("{} {}\n", claw.name, claw.version);
    if !claw.summary.is_empty() {
        out.push_str(&format!("  {}\n", claw.summary));
    }
    out.push_str(&format!("  skills  {}\n", claw.skills.join(", ")));
    out.push_str(&format!(
        "  jobs    {}\n",
        claw.jobs
            .iter()
            .map(|(n, c)| format!("{n} ({c})"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!("  mcp     {}\n", claw.mcp.join(", ")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: researcher\nversion: 1.2\n\
summary: digs through papers\nskills:\n  - search\n  - summarize\n\
jobs:\n  - digest=0 9 * * *\nmcp:\n  - files\n---\n\
You are a careful researcher. Cite everything.\n";

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-claws-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_manifest_parses_every_section() {
        let c = parse(GOOD).unwrap();
        assert_eq!(c.name, "researcher");
        assert_eq!(c.version, "1.2");
        assert_eq!(c.skills, vec!["search", "summarize"]);
        assert_eq!(c.jobs, vec![("digest".into(), "0 9 * * *".into())]);
        assert_eq!(c.mcp, vec!["files"]);
        assert!(c.prompt.starts_with("You are a careful researcher"));
    }

    #[test]
    fn a_manifest_without_frontmatter_or_name_or_body_is_refused() {
        assert!(parse("no frontmatter here").is_err());
        assert!(parse("---\nname: x\n").is_err());
        assert!(parse("---\nsummary: nameless\n---\nbody").is_err());
        assert!(parse("---\nname: ok\n---\n   \n").is_err());
    }

    #[test]
    fn a_bad_cron_expression_is_caught_before_anything_is_written() {
        let bad = "---\nname: x\njobs:\n  - broken=not a cron\n---\nbody\n";
        let err = parse(bad).unwrap_err();
        assert!(err.contains("bad schedule"), "{err}");
        let shapeless = "---\nname: x\njobs:\n  - nocron\n---\nbody\n";
        assert!(parse(shapeless).is_err());
    }

    #[test]
    fn unusable_names_are_refused_for_the_claw_and_its_skills() {
        assert!(parse("---\nname: ../evil\n---\nbody").is_err());
        assert!(parse("---\nname: ok\nskills:\n  - ../evil\n---\nbody").is_err());
        assert!(parse(&format!("---\nname: {}\n---\nbody", "x".repeat(49))).is_err());
    }

    #[test]
    fn the_declared_limits_are_enforced() {
        let many: String = (0..MAX_SKILLS + 1).map(|i| format!("  - s{i}\n")).collect();
        let raw = format!("---\nname: big\nskills:\n{many}---\nbody\n");
        assert!(parse(&raw).is_err());
    }

    #[test]
    fn installing_writes_a_prompt_and_metadata_and_never_overwrites() {
        let d = tmp();
        let c = parse(GOOD).unwrap();
        let dir = install(&c, &d).unwrap();
        assert!(dir.join("PROMPT.md").is_file());
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("claw.json")).unwrap()).unwrap();
        assert_eq!(meta["name"], "researcher");
        assert_eq!(meta["jobs"][0]["cron"], "0 9 * * *");
        assert!(install(&c, &d).is_err(), "a second install must refuse");
    }

    #[test]
    fn installed_files_are_0600() {
        let d = tmp();
        let dir = install(&parse(GOOD).unwrap(), &d).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in ["PROMPT.md", "claw.json"] {
                let mode = std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{f}");
            }
        }
    }

    #[test]
    fn the_plan_lists_every_side_effect_before_any_of_them_happen() {
        let c = parse(GOOD).unwrap();
        let d = tmp();
        let steps = plan(&c, &d);
        assert!(steps
            .iter()
            .any(|s| s.contains("create agent 'researcher'")));
        assert!(steps.iter().any(|s| s.contains("install skill 'search'")));
        assert!(steps.iter().any(|s| s.contains("add job 'digest'")));
        assert!(steps.iter().any(|s| s.contains("mcp server 'files'")));
        assert!(!d.join("researcher").exists(), "plan must not write");
    }

    #[test]
    fn reading_from_a_directory_needs_the_manifest() {
        let d = tmp();
        assert!(read(&d).is_err());
        std::fs::write(d.join(MANIFEST), GOOD).unwrap();
        assert_eq!(read(&d).unwrap().name, "researcher");
    }

    #[test]
    fn describe_names_every_declared_part() {
        let text = describe(&parse(GOOD).unwrap());
        assert!(text.contains("researcher 1.2"), "{text}");
        assert!(text.contains("search, summarize"), "{text}");
        assert!(text.contains("digest (0 9 * * *)"), "{text}");
    }
}
