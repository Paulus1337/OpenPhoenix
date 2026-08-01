use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::security::sha256_hex;

const REPO: &str = "Paulus1337/OpenPhoenix";
const MAX_BINARY: u64 = 100 * 1024 * 1024;

pub const RELEASE_PUBKEY_HEX: &str = match option_env!("PHOENIX_SIGNING_PUBKEY") {
    Some(k) => k,
    None => "f05658487d50f60ca46927f182d97826f9356d363715719b31ff324146bfa71d",
};

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let b = s.trim().as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        out.push((val(pair[0])? << 4) | val(pair[1])?);
    }
    Some(out)
}

pub fn verify_signature(bytes: &[u8], sig: &[u8], pubkey_hex: &str) -> Result<(), String> {
    let pk = hex_bytes(pubkey_hex).ok_or("release public key is not valid hex")?;
    if pk.len() != 32 {
        return Err("release public key must be 32 bytes of hex".into());
    }
    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pk);
    key.verify(bytes, sig).map_err(|_| {
        "signature check failed: this download was not signed by the release key".to_string()
    })
}

fn probe(exe: &Path) -> Result<(), String> {
    let mut child = std::process::Command::new(exe)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("probe launch: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => return Err(format!("probe exited with {st}")),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("probe timed out after 10 seconds".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("probe wait: {e}")),
        }
    }
}

pub fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("phoenix-linux-x86_64"),
        ("linux", "aarch64") => Some("phoenix-linux-arm64"),
        ("macos", "x86_64") => Some("phoenix-macos-x86_64"),
        ("macos", "aarch64") => Some("phoenix-macos-arm64"),
        ("windows", "x86_64") => Some("phoenix-windows-x86_64.exe"),
        _ => None,
    }
}

pub fn parse_sums(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?;
        (file.trim_start_matches('*') == name && hash.len() == 64)
            .then(|| hash.to_ascii_lowercase())
    })
}

pub fn verify(bytes: &[u8], expected: &str) -> Result<(), String> {
    let got = sha256_hex(bytes);
    if got == expected.to_ascii_lowercase() {
        Ok(())
    } else {
        Err(format!("checksum mismatch: expected {expected}, got {got}"))
    }
}

pub fn check_cache_path() -> std::path::PathBuf {
    crate::config::home().join("update-check.json")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn record_check(path: &Path, latest_tag: &str, up_to_date: bool) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let doc = serde_json::json!({
        "checked_at": unix_now(),
        "latest_tag": latest_tag,
        "up_to_date": up_to_date,
    });
    let _ = fs::write(path, doc.to_string());
}

pub fn last_check(path: &Path) -> Option<(u64, String, bool)> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some((
        v["checked_at"].as_u64()?,
        v["latest_tag"].as_str()?.to_string(),
        v["up_to_date"].as_bool()?,
    ))
}

pub fn swap_in(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = target.parent().ok_or("binary has no parent directory")?;
    let tmp = dir.join(".phoenix-update.tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("swap {}: {e}", target.display())
    })
}

fn get(url: &str) -> Result<ureq::Response, String> {
    ureq::get(url)
        .set("User-Agent", "openphoenix-update")
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, r) => {
                format!("HTTP {code}: {}", r.into_string().unwrap_or_default())
            }
            other => other.to_string(),
        })
}

fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = get(url)?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BINARY)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

use std::io::Read;

fn asset_url<'a>(release: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    release["assets"].as_array()?.iter().find_map(|a| {
        (a["name"].as_str() == Some(name)).then(|| a["browser_download_url"].as_str())?
    })
}

pub fn run(check_only: bool) -> Result<String, String> {
    let name = asset_name(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        format!(
            "no release asset for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let api_base = std::env::var("PHOENIX_UPDATE_BASE")
        .unwrap_or_else(|_| "https://api.github.com".to_string());
    let release: serde_json::Value = serde_json::from_str(
        &get(&format!("{api_base}/repos/{REPO}/releases/latest"))?
            .into_string()
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("release JSON: {e}"))?;
    let tag = release["tag_name"].as_str().unwrap_or("?").to_string();

    let sums_url = asset_url(&release, "SHA256SUMS").ok_or("release has no SHA256SUMS asset")?;
    let sig_name = format!("{name}.sig");
    let sig_url = asset_url(&release, &sig_name)
        .ok_or_else(|| format!("release has no {sig_name}: refusing an unsigned update"))?
        .to_string();
    let sums = String::from_utf8(get_bytes(sums_url)?).map_err(|e| e.to_string())?;
    let expected = parse_sums(&sums, name).ok_or_else(|| format!("{name} not in SHA256SUMS"))?;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let current = fs::read(&exe).map_err(|e| e.to_string())?;
    if sha256_hex(&current) == expected {
        record_check(&check_cache_path(), &tag, true);
        return Ok(format!("already flying the latest build ({tag}, {name})"));
    }
    record_check(&check_cache_path(), &tag, false);
    if check_only {
        return Ok(format!(
            "update available: {tag} ({name}); run `phoenix update` to take it"
        ));
    }

    let bin_url =
        asset_url(&release, name).ok_or_else(|| format!("release has no asset named {name}"))?;
    let bytes = get_bytes(bin_url)?;
    verify(&bytes, &expected)?;
    let sig = get_bytes(&sig_url)?;
    verify_signature(&bytes, &sig, RELEASE_PUBKEY_HEX)?;
    swap_in(&exe, &bytes)?;
    if let Err(probe_err) = probe(&exe) {
        swap_in(&exe, &current)?;
        return Err(format!(
            "the new build failed its health probe ({probe_err}); the previous build was restored"
        ));
    }
    record_check(&check_cache_path(), &tag, true);
    Ok(format!(
        "reborn: {} updated to {tag} ({} bytes, checksum and signature verified)",
        exe.display(),
        bytes.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing_accepts_valid_and_rejects_junk() {
        assert_eq!(hex_bytes("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(hex_bytes("ABcd"), Some(vec![171, 205]));
        assert!(hex_bytes("abc").is_none(), "odd length");
        assert!(hex_bytes("zz").is_none(), "non-hex");
        assert_eq!(hex_bytes(""), Some(Vec::new()));
    }

    #[test]
    fn signature_roundtrip_verifies_and_rejects_tampering() {
        let rng = ring::rand::SystemRandom::new();
        let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("keygen");
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref()).expect("pair");
        use ring::signature::KeyPair;
        let pub_hex: String = pair
            .public_key()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let payload = b"phoenix release bytes";
        let sig = pair.sign(payload);
        assert!(verify_signature(payload, sig.as_ref(), &pub_hex).is_ok());
        let mut bad = sig.as_ref().to_vec();
        bad[0] ^= 1;
        assert!(verify_signature(payload, &bad, &pub_hex).is_err());
        assert!(verify_signature(b"other bytes", sig.as_ref(), &pub_hex).is_err());
        assert!(verify_signature(payload, sig.as_ref(), "deadbeef").is_err());
    }

    #[test]
    fn check_cache_round_trips_and_rejects_junk() {
        let dir = std::env::temp_dir().join(format!(
            "px-upd-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let p = dir.join("update-check.json");
        record_check(&p, "v0.0.2", false);
        let (at, tag, ok) = last_check(&p).expect("cache reads back");
        assert_eq!(tag, "v0.0.2");
        assert!(!ok, "first record says an update is out");
        assert!(at > 0, "timestamp is set");
        record_check(&p, "v0.0.2", true);
        assert!(last_check(&p).expect("cache reads back").2);
        let _ = fs::write(&p, "not json");
        assert!(last_check(&p).is_none(), "junk cache reads as absent");
        assert!(last_check(&dir.join("absent.json")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pinned_release_key_is_wellformed() {
        let pk = hex_bytes(RELEASE_PUBKEY_HEX).expect("pinned key must be hex");
        assert_eq!(pk.len(), 32, "ed25519 public keys are 32 bytes");
    }

    #[test]
    fn asset_names_cover_release_matrix() {
        assert_eq!(asset_name("linux", "x86_64"), Some("phoenix-linux-x86_64"));
        assert_eq!(asset_name("linux", "aarch64"), Some("phoenix-linux-arm64"));
        assert_eq!(asset_name("macos", "aarch64"), Some("phoenix-macos-arm64"));
        assert_eq!(
            asset_name("windows", "x86_64"),
            Some("phoenix-windows-x86_64.exe")
        );
        assert_eq!(asset_name("freebsd", "x86_64"), None);
    }

    #[test]
    fn sums_parsing_finds_named_entry() {
        let sums = format!(
            "{}  phoenix-linux-x86_64\n{}  SHA256SUMS.other\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(
            parse_sums(&sums, "phoenix-linux-x86_64"),
            Some("a".repeat(64))
        );
        assert_eq!(parse_sums(&sums, "phoenix-macos-arm64"), None);
        assert_eq!(
            parse_sums("short  phoenix-linux-x86_64", "phoenix-linux-x86_64"),
            None
        );
    }

    #[test]
    fn sums_parsing_accepts_binary_marker() {
        let sums = format!("{}  *phoenix-linux-arm64\n", "c".repeat(64));
        assert_eq!(
            parse_sums(&sums, "phoenix-linux-arm64"),
            Some("c".repeat(64))
        );
    }

    #[test]
    fn verify_matches_and_rejects() {
        let hash = sha256_hex(b"phoenix");
        assert!(verify(b"phoenix", &hash).is_ok());
        assert!(verify(b"phoenix", &hash.to_ascii_uppercase()).is_ok());
        let err = verify(b"other", &hash).unwrap_err();
        assert!(err.contains("checksum mismatch"));
    }

    #[test]
    fn swap_replaces_target_atomically() {
        let d = std::env::temp_dir().join(format!("phx-upd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let target = d.join("phoenix");
        fs::write(&target, b"old").unwrap();
        swap_in(&target, b"new-binary").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-binary");
        assert!(!d.join(".phoenix-update.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn asset_url_reads_release_json() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"assets":[{"name":"SHA256SUMS","browser_download_url":"https://x/s"},
                {"name":"phoenix-linux-x86_64","browser_download_url":"https://x/b"}]}"#,
        )
        .unwrap();
        assert_eq!(asset_url(&v, "SHA256SUMS"), Some("https://x/s"));
        assert_eq!(asset_url(&v, "phoenix-linux-x86_64"), Some("https://x/b"));
        assert_eq!(asset_url(&v, "nope"), None);
    }
}
