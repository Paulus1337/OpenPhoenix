use std::path::{Path, PathBuf};

use serde_json::Value;

pub const SCHEME: &str = "px://";

#[derive(Debug, Clone, PartialEq)]
pub struct Addr {
    pub file: String,
    pub segments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Json,
    Jsonl,
    Toml,
    Markdown,
}

pub fn kind_of(file: &str) -> Result<Kind, String> {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "json" => Ok(Kind::Json),
        "jsonl" | "ndjson" => Ok(Kind::Jsonl),
        "toml" => Ok(Kind::Toml),
        "md" | "markdown" => Ok(Kind::Markdown),
        "" => Err(format!("'{file}' has no extension; cannot pick a reader")),
        other => Err(format!("no reader for .{other} files")),
    }
}

pub fn parse(raw: &str) -> Result<Addr, String> {
    let Some(rest) = raw.strip_prefix(SCHEME) else {
        return Err(format!("a path must start with {SCHEME}"));
    };
    if rest.trim().is_empty() {
        return Err("a path needs a file".into());
    }
    let mut parts = rest.split('/');
    let file = parts.next().unwrap_or("").trim().to_string();
    if file.is_empty() {
        return Err("a path needs a file".into());
    }
    if file.contains("..") || file.starts_with('.') {
        return Err(format!("'{file}' escapes the workspace"));
    }
    kind_of(&file)?;
    let segments: Vec<String> = parts
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Ok(Addr { file, segments })
}

pub fn resolve_file(workspace: &Path, addr: &Addr) -> Result<PathBuf, String> {
    let jail = crate::security::PathJail::new(workspace, false)
        .map_err(|e| format!("workspace unusable: {e}"))?;
    jail.resolve(&addr.file)
        .map_err(|_| format!("'{}' is outside the workspace", addr.file))
}

fn dig<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut node = root;
    for s in segments {
        node = match node {
            Value::Object(map) => map.get(s)?,
            Value::Array(arr) => {
                let i: usize = s.parse().ok()?;
                arr.get(i)?
            }
            _ => return None,
        };
    }
    Some(node)
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn markdown_section(body: &str, segments: &[String]) -> Option<String> {
    let Some(want) = segments.first() else {
        return Some(body.to_string());
    };
    let want = crate::wiki::slugify(want);
    let mut out: Vec<&str> = Vec::new();
    let mut depth = 0usize;
    let mut collecting = false;
    for line in body.lines() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let is_heading = hashes > 0 && line.chars().nth(hashes) == Some(' ');
        if is_heading {
            let title = line.get(hashes + 1..).unwrap_or("").trim();
            if collecting && hashes <= depth {
                break;
            }
            if !collecting && crate::wiki::slugify(title) == want {
                collecting = true;
                depth = hashes;
                continue;
            }
        }
        if collecting {
            out.push(line);
        }
    }
    if !collecting {
        return None;
    }
    let text = out.join("\n").trim().to_string();
    if segments.len() > 1 {
        return markdown_section(&text, segments.get(1..).unwrap_or(&[]));
    }
    Some(text)
}

pub fn resolve(workspace: &Path, raw: &str) -> Result<String, String> {
    let addr = parse(raw)?;
    let path = resolve_file(workspace, &addr)?;
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match kind_of(&addr.file)? {
        Kind::Json => {
            let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            dig(&v, &addr.segments)
                .map(render)
                .ok_or_else(|| format!("nothing at {raw}"))
        }
        Kind::Toml => {
            let t: toml::Value = toml::from_str(&body).map_err(|e| e.to_string())?;
            let v: Value = serde_json::to_value(t).map_err(|e| e.to_string())?;
            dig(&v, &addr.segments)
                .map(render)
                .ok_or_else(|| format!("nothing at {raw}"))
        }
        Kind::Jsonl => {
            let Some(first) = addr.segments.first() else {
                return Err("a jsonl path needs a record index or a [field=value] filter".into());
            };
            let rest = addr.segments.get(1..).unwrap_or(&[]);
            let records: Vec<Value> = body
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            if let Some(filter) = first.strip_prefix('[').and_then(|f| f.strip_suffix(']')) {
                let Some((k, want)) = filter.split_once('=') else {
                    return Err("a filter looks like [field=value]".into());
                };
                let hits: Vec<String> = records
                    .iter()
                    .filter(|r| r.get(k).map(render).as_deref() == Some(want))
                    .filter_map(|r| dig(r, rest).map(render))
                    .collect();
                if hits.is_empty() {
                    return Err(format!("nothing at {raw}"));
                }
                return Ok(hits.join("\n"));
            }
            let i: usize = first
                .parse()
                .map_err(|_| format!("'{first}' is not a record index"))?;
            records
                .get(i)
                .and_then(|r| dig(r, rest))
                .map(render)
                .ok_or_else(|| format!("nothing at {raw}"))
        }
        Kind::Markdown => {
            markdown_section(&body, &addr.segments).ok_or_else(|| format!("nothing at {raw}"))
        }
    }
}

pub fn validate_text(raw: &str) -> Result<String, String> {
    let addr = parse(raw)?;
    Ok(format!(
        "ok\n  file      {}\n  kind      {:?}\n  segments  {}\n",
        addr.file,
        kind_of(&addr.file)?,
        if addr.segments.is_empty() {
            "(whole file)".to_string()
        } else {
            addr.segments.join(" / ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-path-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_path_needs_the_scheme_a_file_and_a_known_kind() {
        assert!(parse("config.toml/provider").is_err());
        assert!(parse("px://").is_err());
        assert!(parse("px://noext/key").is_err());
        assert!(parse("px://data.xlsx/a").is_err());
        let a = parse("px://config.toml/provider/kind").unwrap();
        assert_eq!(a.file, "config.toml");
        assert_eq!(a.segments, vec!["provider", "kind"]);
    }

    #[test]
    fn a_path_can_never_climb_out_of_the_workspace() {
        for bad in [
            "px://../secrets.json/a",
            "px://.ssh/id.json",
            "px://a/../../b.json",
        ] {
            let parsed = parse(bad);
            if let Ok(addr) = parsed {
                assert!(
                    resolve_file(&ws(), &addr).is_err(),
                    "{bad} resolved inside the jail"
                );
            }
        }
    }

    #[test]
    fn toml_leaves_resolve_by_table_and_key() {
        let w = ws();
        std::fs::write(
            w.join("config.toml"),
            "[provider]\nkind = \"anthropic\"\nmodel = \"sonnet\"\n[security]\napprovals = true\n",
        )
        .unwrap();
        assert_eq!(
            resolve(&w, "px://config.toml/provider/kind").unwrap(),
            "anthropic"
        );
        assert_eq!(
            resolve(&w, "px://config.toml/security/approvals").unwrap(),
            "true"
        );
        assert!(resolve(&w, "px://config.toml/provider/nope").is_err());
    }

    #[test]
    fn json_arrays_are_addressed_by_index() {
        let w = ws();
        std::fs::write(
            w.join("state.json"),
            r#"{"cards":[{"id":1,"title":"first"},{"id":2,"title":"second"}]}"#,
        )
        .unwrap();
        assert_eq!(
            resolve(&w, "px://state.json/cards/1/title").unwrap(),
            "second"
        );
        assert!(resolve(&w, "px://state.json/cards/9/title").is_err());
        assert!(resolve(&w, "px://state.json/cards/x/title").is_err());
    }

    #[test]
    fn jsonl_records_resolve_by_index_and_by_filter() {
        let w = ws();
        std::fs::write(
            w.join("audit.jsonl"),
            "{\"event\":\"tool_call\",\"name\":\"shell\"}\n\
{\"event\":\"auth\",\"name\":\"http\"}\n\
{\"event\":\"tool_call\",\"name\":\"read_file\"}\n",
        )
        .unwrap();
        assert_eq!(resolve(&w, "px://audit.jsonl/1/name").unwrap(), "http");
        assert_eq!(
            resolve(&w, "px://audit.jsonl/[event=tool_call]/name").unwrap(),
            "shell\nread_file"
        );
        assert!(resolve(&w, "px://audit.jsonl/[event=nothing]/name").is_err());
        assert!(resolve(&w, "px://audit.jsonl/[broken]/name").is_err());
        assert!(resolve(&w, "px://audit.jsonl").is_err());
    }

    #[test]
    fn a_damaged_jsonl_line_is_skipped_not_fatal() {
        let w = ws();
        std::fs::write(
            w.join("log.jsonl"),
            "{not json\n{\"event\":\"ok\",\"name\":\"good\"}\n",
        )
        .unwrap();
        assert_eq!(
            resolve(&w, "px://log.jsonl/[event=ok]/name").unwrap(),
            "good"
        );
    }

    #[test]
    fn markdown_sections_resolve_by_slug_and_nest() {
        let w = ws();
        std::fs::write(
            w.join("AGENTS.md"),
            "# Title\nintro\n\n## Runtime Safety\nnever rm -rf\n\n### Shell\nask first\n\n## Other\nnope\n",
        )
        .unwrap();
        let sec = resolve(&w, "px://AGENTS.md/runtime-safety").unwrap();
        assert!(sec.contains("never rm -rf"), "{sec}");
        assert!(!sec.contains("nope"), "{sec}");
        let nested = resolve(&w, "px://AGENTS.md/runtime-safety/shell").unwrap();
        assert_eq!(nested, "ask first");
        assert!(resolve(&w, "px://AGENTS.md/missing-section").is_err());
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_empty_answer() {
        let w = ws();
        assert!(resolve(&w, "px://nothing.json/a").is_err());
    }

    #[test]
    fn validate_never_touches_the_filesystem() {
        let text = validate_text("px://config.toml/provider/kind").unwrap();
        assert!(text.contains("Toml"), "{text}");
        assert!(text.contains("provider / kind"), "{text}");
        assert!(validate_text("px://whole.json")
            .unwrap()
            .contains("(whole file)"));
        assert!(validate_text("nonsense").is_err());
    }
}
