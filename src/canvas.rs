use std::fs;
use std::path::{Path, PathBuf};

pub fn state_path() -> PathBuf {
    crate::config::home().join("canvas.html")
}

pub fn version(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn present(path: &Path, html: &str) -> Result<(), String> {
    if html.trim().is_empty() {
        return Err("empty html".into());
    }
    if html.len() > 512 * 1024 {
        return Err("canvas document larger than 512 KB cap".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("html.tmp");
    fs::write(&tmp, html).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn hide(path: &Path) {
    let _ = fs::remove_file(path);
}

const RELOAD_JS: &str = "<script nonce=\"phoenix\">(function(){var v=null;setInterval(function(){\
fetch('/canvas/version',{credentials:'same-origin'}).then(function(r){return r.json()})\
.then(function(j){if(v===null){v=j.v;}else if(j.v!==v){location.reload();}})\
.catch(function(){});},2000);})();</script>";

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>Canvas</title></head><body style=\"font-family:sans-serif;color:#888;\
display:flex;align-items:center;justify-content:center;height:100vh;margin:0\">\
<p>(canvas is empty)</p></body></html>";

pub fn render(path: &Path) -> String {
    let doc = fs::read_to_string(path).unwrap_or_else(|_| PLACEHOLDER.to_string());
    format!("{doc}{RELOAD_JS}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "px-canvas-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.join("canvas.html")
    }

    #[test]
    fn present_render_hide_cycle() {
        let p = tmpfile();
        assert_eq!(version(&p), 0);
        assert!(render(&p).contains("(canvas is empty)"));
        present(&p, "<h1>dashboard</h1>").unwrap();
        assert!(version(&p) > 0);
        let page = render(&p);
        assert!(page.starts_with("<h1>dashboard</h1>"), "got: {page}");
        assert!(page.contains("/canvas/version"), "reload script missing");
        hide(&p);
        assert_eq!(version(&p), 0);
        assert!(render(&p).contains("(canvas is empty)"));
    }

    #[test]
    fn present_rejects_empty_and_oversized() {
        let p = tmpfile();
        assert!(present(&p, "  ").is_err());
        let big = "x".repeat(512 * 1024 + 1);
        assert!(present(&p, &big).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn canvas_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmpfile();
        present(&p, "<p>hi</p>").unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
