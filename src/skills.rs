use std::fs;
use std::path::Path;

const MAX_INJECT: usize = 8_000;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub body: String,
}

pub fn parse(text: &str) -> Option<Skill> {
    let rest = text.strip_prefix("---")?;
    let (head, body) = rest.split_once("\n---")?;
    let mut name = String::new();
    let mut description = String::new();
    let mut keywords: Vec<String> = Vec::new();
    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "name" => name = v.to_string(),
            "description" => description = v.to_string(),
            "keywords" => {
                keywords = v
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    if name.is_empty() {
        return None;
    }
    if keywords.is_empty() {
        keywords.push(name.to_lowercase());
    }
    Some(Skill {
        name,
        description,
        keywords,
        body: body.trim().to_string(),
    })
}

pub fn load_dir(dir: &Path) -> Vec<Skill> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().map(|x| x == "md").unwrap_or(false) {
            paths.push(p);
        } else if p.is_dir() {
            let nested = p.join("SKILL.md");
            if nested.is_file() {
                paths.push(nested);
            }
        }
    }
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        if let Ok(text) = fs::read_to_string(&p) {
            if let Some(s) = parse(&text) {
                out.push(s);
            }
        }
    }
    out
}

pub fn inject(skills: &[Skill], text: &str) -> String {
    let low = text.to_lowercase();
    let mut out = String::new();
    for s in skills {
        if s.keywords.iter().any(|k| low.contains(k.as_str())) {
            let entry = format!("\n\n[skill: {}] {}\n{}", s.name, s.description, s.body);
            if out.len() + entry.len() > MAX_INJECT {
                break;
            }
            out.push_str(&entry);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-skill-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const SAMPLE: &str = "---\nname: git-flow\ndescription: how we use git\nkeywords: git, commit, branch\n---\nAlways rebase before merging.";

    #[test]
    fn parse_roundtrip() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.name, "git-flow");
        assert_eq!(s.keywords, vec!["git", "commit", "branch"]);
        assert_eq!(s.body, "Always rebase before merging.");
    }

    #[test]
    fn parse_rejects_missing_front_matter() {
        assert!(parse("just a file").is_none());
        assert!(parse("---\ndescription: x\n---\nbody").is_none());
    }

    #[test]
    fn keywords_default_to_name() {
        let s = parse("---\nname: docker\n---\nUse compose.").unwrap();
        assert_eq!(s.keywords, vec!["docker"]);
    }

    #[test]
    fn load_and_inject() {
        let dir = tmpdir();
        fs::write(dir.join("a.md"), SAMPLE).unwrap();
        fs::write(dir.join("skip.txt"), SAMPLE).unwrap();
        let skills = load_dir(&dir);
        assert_eq!(skills.len(), 1);
        let add = inject(&skills, "please COMMIT this change");
        assert!(add.contains("git-flow"));
        assert!(add.contains("rebase"));
        assert_eq!(inject(&skills, "what is the weather"), "");
    }

    #[test]
    fn missing_dir_is_empty() {
        assert!(load_dir(Path::new("/nonexistent/px-skills")).is_empty());
    }

    #[test]
    fn injection_is_capped() {
        let big = format!(
            "---\nname: big\nkeywords: zzz\n---\n{}",
            "x".repeat(MAX_INJECT)
        );
        let skills = vec![parse(&big).unwrap(), parse(SAMPLE).unwrap()];
        let add = inject(&skills, "zzz git");
        assert!(add.len() <= MAX_INJECT);
    }
}
