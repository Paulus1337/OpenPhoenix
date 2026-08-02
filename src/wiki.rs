use std::path::{Path, PathBuf};

pub const MAX_TITLE: usize = 64;
pub const MAX_PAGE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub slug: String,
    pub title: String,
    pub links: Vec<String>,
    pub bytes: u64,
}

pub fn vault_dir() -> PathBuf {
    crate::config::home().join("wiki")
}

pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in title.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && out.len() < MAX_TITLE {
            out.push('-');
            last_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    s.chars().take(MAX_TITLE).collect()
}

pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_TITLE
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

pub fn page_path(dir: &Path, slug: &str) -> Result<PathBuf, String> {
    if !valid_slug(slug) {
        return Err(format!(
            "'{slug}' is not a valid page name: lowercase letters, digits and dashes only"
        ));
    }
    Ok(dir.join(format!("{slug}.md")))
}

pub fn parse_links(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = rest.get(start + 2..).unwrap_or("");
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = after.get(..end).unwrap_or("");
        let target = inner.split('|').next().unwrap_or("").trim();
        let slug = slugify(target);
        if !slug.is_empty() && !out.contains(&slug) {
            out.push(slug);
        }
        rest = after.get(end + 2..).unwrap_or("");
    }
    out
}

pub fn first_heading(body: &str) -> String {
    body.lines()
        .find_map(|l| l.strip_prefix("# ").map(str::trim))
        .unwrap_or("")
        .to_string()
}

pub fn write(dir: &Path, slug: &str, body: &str) -> Result<PathBuf, String> {
    if body.trim().is_empty() {
        return Err("refusing to write an empty page".into());
    }
    if body.len() as u64 > MAX_PAGE_BYTES {
        return Err(format!("page is over the {MAX_PAGE_BYTES} byte cap"));
    }
    let path = page_path(dir, slug)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    crate::security::write_atomic(&path, body.as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn read(dir: &Path, slug: &str) -> Result<String, String> {
    let path = page_path(dir, slug)?;
    std::fs::read_to_string(&path).map_err(|_| format!("no page named '{slug}'"))
}

pub fn remove(dir: &Path, slug: &str) -> Result<(), String> {
    let path = page_path(dir, slug)?;
    std::fs::remove_file(&path).map_err(|_| format!("no page named '{slug}'"))
}

pub fn pages(dir: &Path) -> Vec<Page> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Page> = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_slug(slug) {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let title = {
            let h = first_heading(&body);
            if h.is_empty() {
                slug.to_string()
            } else {
                h
            }
        };
        out.push(Page {
            slug: slug.to_string(),
            title,
            links: parse_links(&body),
            bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

pub fn search(dir: &Path, query: &str) -> Vec<(String, String)> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for p in pages(dir) {
        let Ok(body) = read(dir, &p.slug) else {
            continue;
        };
        for line in body.lines() {
            if line.to_lowercase().contains(&needle) {
                out.push((p.slug.clone(), crate::security::one_line(line, 80)));
                break;
            }
        }
    }
    out
}

pub fn broken_links(dir: &Path) -> Vec<String> {
    let all = pages(dir);
    let known: Vec<&str> = all.iter().map(|p| p.slug.as_str()).collect();
    let mut out = Vec::new();
    for p in &all {
        for l in &p.links {
            if !known.contains(&l.as_str()) {
                out.push(format!("{} links to missing page '{}'", p.slug, l));
            }
        }
    }
    out
}

pub fn orphans(dir: &Path) -> Vec<String> {
    let all = pages(dir);
    let mut linked: Vec<&str> = Vec::new();
    for p in &all {
        for l in &p.links {
            linked.push(l.as_str());
        }
    }
    all.iter()
        .filter(|p| !linked.contains(&p.slug.as_str()))
        .map(|p| p.slug.clone())
        .collect()
}

pub fn status_text(dir: &Path) -> String {
    let all = pages(dir);
    if all.is_empty() {
        return format!(
            "the wiki at {} is empty\nwrite one with: phoenix wiki write SLUG BODY\n",
            dir.display()
        );
    }
    let links: usize = all.iter().map(|p| p.links.len()).sum();
    let broken = broken_links(dir);
    let mut out = format!(
        "{} page(s), {links} link(s) at {}\n",
        all.len(),
        dir.display()
    );
    for p in &all {
        out.push_str(&format!(
            "  {:<28}{:<6}{}\n",
            p.slug,
            p.links.len(),
            crate::security::one_line(&p.title, 40)
        ));
    }
    if !broken.is_empty() {
        out.push_str(&format!("{} broken link(s)\n", broken.len()));
        for b in &broken {
            out.push_str(&format!("  {b}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-wiki-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn titles_turn_into_safe_slugs() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("  Rust & Safety!  "), "rust-safety");
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert_eq!(slugify("---"), "");
        assert!(slugify(&"x".repeat(200)).len() <= MAX_TITLE);
    }

    #[test]
    fn a_page_name_can_never_escape_the_vault() {
        let d = tmp();
        for bad in ["../escape", "/etc/passwd", "a/b", "..", "Upper", "-lead"] {
            assert!(page_path(&d, bad).is_err(), "{bad} was accepted");
            assert!(write(&d, bad, "body").is_err(), "{bad} was written");
        }
        let p = page_path(&d, "fine-page").unwrap();
        assert_eq!(p.parent(), Some(d.as_path()));
    }

    #[test]
    fn writing_reading_and_removing_a_page_round_trips() {
        let d = tmp();
        write(&d, "notes", "# Notes\nbody text\n").unwrap();
        assert!(read(&d, "notes").unwrap().contains("body text"));
        assert_eq!(pages(&d).len(), 1);
        remove(&d, "notes").unwrap();
        assert!(read(&d, "notes").is_err());
        assert!(remove(&d, "notes").is_err());
    }

    #[test]
    fn pages_are_written_0600_and_empty_bodies_are_refused() {
        let d = tmp();
        assert!(write(&d, "empty", "   ").is_err());
        let p = write(&d, "ok", "# Ok\ncontent").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn wiki_links_are_parsed_including_pipes_and_duplicates() {
        let links = parse_links("see [[Alpha]] and [[Beta Page|beta]] and [[Alpha]] again");
        assert_eq!(links, vec!["alpha", "beta-page"]);
        assert!(parse_links("no links here").is_empty());
        assert!(parse_links("[[unclosed").is_empty());
    }

    #[test]
    fn the_title_comes_from_the_first_heading_and_falls_back_to_the_slug() {
        let d = tmp();
        write(&d, "titled", "# Real Title\nbody").unwrap();
        write(&d, "untitled", "just body text").unwrap();
        let all = pages(&d);
        assert_eq!(
            all.iter()
                .find(|p| p.slug == "titled")
                .map(|p| p.title.as_str()),
            Some("Real Title")
        );
        assert_eq!(
            all.iter()
                .find(|p| p.slug == "untitled")
                .map(|p| p.title.as_str()),
            Some("untitled")
        );
    }

    #[test]
    fn broken_links_and_orphans_are_reported() {
        let d = tmp();
        write(&d, "index", "# Index\nsee [[Child]] and [[Missing]]").unwrap();
        write(&d, "child", "# Child\nhi").unwrap();
        let broken = broken_links(&d);
        assert_eq!(broken.len(), 1);
        assert!(broken
            .first()
            .map(|b| b.contains("missing"))
            .unwrap_or(false));
        assert_eq!(orphans(&d), vec!["index".to_string()]);
    }

    #[test]
    fn search_finds_the_first_matching_line_per_page() {
        let d = tmp();
        write(&d, "a", "# A\nthe secret plan\nmore").unwrap();
        write(&d, "b", "# B\nnothing here").unwrap();
        let hits = search(&d, "SECRET");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits.first().map(|h| h.0.as_str()), Some("a"));
        assert!(search(&d, "  ").is_empty());
        assert!(search(&d, "nowhere").is_empty());
    }

    #[test]
    fn a_stray_file_in_the_vault_is_ignored() {
        let d = tmp();
        std::fs::write(d.join("notes.txt"), "not a page").unwrap();
        std::fs::write(d.join("BAD NAME.md"), "# x").unwrap();
        write(&d, "good", "# Good\nyes").unwrap();
        assert_eq!(pages(&d).len(), 1);
    }

    #[test]
    fn status_names_every_page_and_reports_an_empty_vault() {
        let d = tmp();
        assert!(status_text(&d).contains("is empty"));
        write(&d, "one", "# One\n[[two]]").unwrap();
        let text = status_text(&d);
        assert!(text.contains("1 page(s)"), "{text}");
        assert!(text.contains("1 broken link"), "{text}");
    }
}
