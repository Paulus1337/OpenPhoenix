use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;
use crate::security::redact;

const MAX_ARCHIVE: u64 = 20 * 1024 * 1024;

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn get_json(url: &str) -> Result<Value, String> {
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| redact(&e.to_string()))?;
    let mut buf = String::new();
    resp.into_reader()
        .take(1 << 20)
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&buf).map_err(|e| e.to_string())
}

pub fn search(cfg: &Config, query: &str) -> Result<String, String> {
    let url = format!(
        "{}/api/v1/search?q={}&limit=10",
        cfg.clawhub_url,
        urlencode(query)
    );
    let v = get_json(&url)?;
    let empty = Vec::new();
    let results = v["results"].as_array().unwrap_or(&empty);
    if results.is_empty() {
        return Ok("no skills found".to_string());
    }
    let mut out = Vec::new();
    for r in results {
        let reference = r["install"]["reference"].as_str().unwrap_or("?");
        let downloads = r["downloads"].as_u64().unwrap_or(0);
        let summary = r["native"]["skill"]["summary"].as_str().unwrap_or("");
        out.push(format!("{reference}  ({downloads} downloads)\n  {summary}"));
    }
    Ok(out.join("\n"))
}

pub fn split_reference(reference: &str) -> Result<(String, String), String> {
    match reference.split_once('/') {
        Some((owner, slug)) if !owner.is_empty() && !slug.is_empty() => {
            Ok((owner.to_string(), slug.to_string()))
        }
        _ => Err(format!(
            "expected OWNER/SLUG (like steipete/weather), got: {reference}"
        )),
    }
}

#[derive(Debug, PartialEq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
    Markdown,
    Unknown,
}

pub fn sniff(bytes: &[u8]) -> ArchiveKind {
    if bytes.starts_with(b"PK\x03\x04") {
        ArchiveKind::Zip
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        ArchiveKind::TarGz
    } else if bytes.starts_with(b"---") {
        ArchiveKind::Markdown
    } else {
        ArchiveKind::Unknown
    }
}

pub fn install(cfg: &Config, reference: &str, skills_dir: &Path) -> Result<String, String> {
    let (owner, slug) = split_reference(reference)?;
    let url = format!(
        "{}/api/v1/skills/{}/install?ownerHandle={}",
        cfg.clawhub_url,
        urlencode(&slug),
        urlencode(&owner)
    );
    let v = get_json(&url)?;
    if !v["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "registry refused install for {reference}: {}",
            v["error"].as_str().unwrap_or("unknown reason")
        ));
    }
    let version = v["archive"]["version"].as_str().unwrap_or("?").to_string();
    let download = v["archive"]["downloadUrl"]
        .as_str()
        .ok_or("registry response has no downloadUrl")?;

    let resp = ureq::get(download)
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| redact(&e.to_string()))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_ARCHIVE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ARCHIVE {
        return Err("archive larger than 20 MB, refusing".into());
    }
    if bytes.is_empty() {
        return Err("empty archive".into());
    }

    let target = skills_dir.join(&slug);
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    match sniff(&bytes) {
        ArchiveKind::Markdown => {
            std::fs::write(target.join("SKILL.md"), &bytes).map_err(|e| e.to_string())?;
        }
        ArchiveKind::Zip => extract(&bytes, &target, "unzip", &["-o", "-q", "{file}", "-d"])?,
        ArchiveKind::TarGz => extract(&bytes, &target, "tar", &["-xzf", "{file}", "-C"])?,
        ArchiveKind::Unknown => return Err("unrecognized archive format".into()),
    }
    Ok(format!(
        "installed {reference} {version} into {}\nnote: third-party skill, review {} before relying on it",
        target.display(),
        target.join("SKILL.md").display()
    ))
}

fn extract(bytes: &[u8], target: &Path, tool: &str, args: &[&str]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!(
        "phoenix-skill-{}-{}",
        std::process::id(),
        target.file_name().and_then(|n| n.to_str()).unwrap_or("x")
    ));
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    let tmp_s = tmp.to_string_lossy().to_string();
    let mut cmd = std::process::Command::new(tool);
    for a in args {
        if *a == "{file}" {
            cmd.arg(&tmp_s);
        } else {
            cmd.arg(a);
        }
    }
    cmd.arg(target);
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run {tool} (install it): {e}"));
    let _ = std::fs::remove_file(&tmp);
    let out = out?;
    if !out.status.success() {
        return Err(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_split() {
        assert_eq!(
            split_reference("steipete/weather").unwrap(),
            ("steipete".to_string(), "weather".to_string())
        );
        assert!(split_reference("weather").is_err());
        assert!(split_reference("/x").is_err());
        assert!(split_reference("x/").is_err());
    }

    #[test]
    fn sniff_formats() {
        assert_eq!(sniff(b"PK\x03\x04rest"), ArchiveKind::Zip);
        assert_eq!(sniff(&[0x1f, 0x8b, 0x08]), ArchiveKind::TarGz);
        assert_eq!(sniff(b"---\nname: x\n---\n"), ArchiveKind::Markdown);
        assert_eq!(sniff(b"hello"), ArchiveKind::Unknown);
    }

    #[test]
    fn urlencode_basics() {
        assert_eq!(urlencode("weather"), "weather");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }
}
