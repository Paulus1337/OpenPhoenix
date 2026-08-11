use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ring::aead::{Aad, Algorithm, LessSafeKey, Nonce, UnboundKey, NONCE_LEN};
use ring::aead::{AES_256_GCM, CHACHA20_POLY1305};
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};

pub const KEY_VAR: &str = "PHOENIX_SECRET_KEY";
const SALT_LEN: usize = 16;
const MAGIC: &[u8; 4] = b"PHXS";
const VERSION_CHACHA: u8 = 1;
const VERSION_AES_GCM: u8 = 2;
const VERSION: u8 = VERSION_AES_GCM;
const ITERATIONS: u32 = 600_000;

pub struct Store {
    path: PathBuf,
}

fn cipher_for(version: u8) -> Result<&'static Algorithm, String> {
    match version {
        VERSION_AES_GCM => Ok(&AES_256_GCM),
        VERSION_CHACHA => Ok(&CHACHA20_POLY1305),
        _ => Err("secret store version is not supported".into()),
    }
}

pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        *b = std::hint::black_box(0u8);
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    let _ = std::hint::black_box(&*buf);
}

pub struct SecretStr(String);

impl SecretStr {
    pub fn new(s: String) -> SecretStr {
        SecretStr(s)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretStr {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        wipe(&mut bytes);
        let _ = std::hint::black_box(&bytes);
    }
}

pub struct Secret(Vec<u8>);

impl Secret {
    fn new(v: Vec<u8>) -> Secret {
        Secret(v)
    }
    fn as_slice(&self) -> &[u8] {
        &self.0
    }
    fn as_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn derive(passphrase: &str, salt: &[u8], version: u8) -> Result<LessSafeKey, String> {
    let mut key = [0u8; 32];
    let iterations = std::num::NonZeroU32::new(ITERATIONS).ok_or("bad iteration count")?;
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        passphrase.as_bytes(),
        &mut key,
    );
    let unbound = UnboundKey::new(cipher_for(version)?, &key)
        .map_err(|_| "cannot build cipher".to_string())?;
    wipe(&mut key);
    Ok(LessSafeKey::new(unbound))
}

fn passphrase_from_env_file(dir: &Path) -> Option<SecretStr> {
    let text = std::fs::read_to_string(dir.join("env")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some(v) = line.strip_prefix(&format!("{KEY_VAR}=")) {
            let v = v.trim().trim_matches(['"', '\''].as_slice());
            if !v.is_empty() {
                return Some(SecretStr::new(v.to_string()));
            }
        }
    }
    None
}

fn passphrase_env() -> Option<SecretStr> {
    std::env::var(KEY_VAR)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(SecretStr::new)
}

fn passphrase_for(dir: &Path) -> Option<SecretStr> {
    passphrase_env().or_else(|| passphrase_from_env_file(dir))
}

impl Store {
    pub fn at(path: &Path) -> Self {
        Store {
            path: path.to_path_buf(),
        }
    }

    pub fn default_path() -> PathBuf {
        crate::config::home().join("secrets.enc")
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn passphrase(&self) -> Option<SecretStr> {
        passphrase_for(&self.dir())
    }

    pub fn locked(&self) -> bool {
        self.exists() && self.passphrase().is_none()
    }

    pub fn load(&self) -> Result<BTreeMap<String, String>, String> {
        if !self.exists() {
            return Ok(BTreeMap::new());
        }
        let Some(pass) = self.passphrase() else {
            return Err(format!(
                "{} holds encrypted secrets; set {KEY_VAR} to unlock it",
                self.path.display()
            ));
        };
        let raw = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        let head = MAGIC.len() + 1;
        if raw.len() < head + SALT_LEN + NONCE_LEN {
            return Err("secret store is truncated".into());
        }
        if raw.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err("secret store has a bad header".into());
        }
        let version = *raw.get(MAGIC.len()).ok_or("short header")?;
        let salt = raw.get(head..head + SALT_LEN).ok_or("short salt")?;
        let nonce_at = head + SALT_LEN;
        let nonce_bytes: [u8; NONCE_LEN] = raw
            .get(nonce_at..nonce_at + NONCE_LEN)
            .ok_or("short nonce")?
            .try_into()
            .map_err(|_| "short nonce".to_string())?;
        let mut body = Secret::new(
            raw.get(nonce_at + NONCE_LEN..)
                .ok_or("short body")?
                .to_vec(),
        );

        let key = derive(pass.as_str(), salt, version)?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let plain = key
            .open_in_place(nonce, Aad::empty(), body.as_mut())
            .map_err(|_| format!("cannot decrypt {}: wrong {KEY_VAR}?", self.path.display()))?;
        let text = std::str::from_utf8(plain).map_err(|_| "secret store is not UTF-8")?;
        serde_json::from_str(text).map_err(|e| format!("secret store is corrupt: {e}"))
    }

    pub fn save(&self, entries: &BTreeMap<String, String>) -> Result<(), String> {
        let Some(pass) = self.passphrase() else {
            return Err(format!(
                "refusing to write secrets to disk without {KEY_VAR}; \
export it first so the store can be encrypted"
            ));
        };
        let rng = SystemRandom::new();
        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt).map_err(|_| "no system randomness")?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| "no system randomness")?;

        let key = derive(pass.as_str(), &salt, VERSION)?;
        let mut body = Secret::new(serde_json::to_vec(entries).map_err(|e| e.to_string())?);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        key.seal_in_place_append_tag(nonce, Aad::empty(), body.as_mut())
            .map_err(|_| "cannot encrypt secrets".to_string())?;

        let mut out =
            Vec::with_capacity(MAGIC.len() + 1 + SALT_LEN + NONCE_LEN + body.as_slice().len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(body.as_slice());
        crate::security::write_atomic(&self.path, &out, Some(0o600)).map_err(|e| e.to_string())
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.load().ok()?.get(name).cloned()
    }

    pub fn put(&self, name: &str, value: &str) -> Result<(), String> {
        let mut all = self.load()?;
        all.insert(name.to_string(), value.to_string());
        self.save(&all)
    }

    pub fn names(&self) -> Result<Vec<String>, String> {
        self.load().map(|m| m.into_keys().collect())
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn ensure_passphrase() -> Result<(), String> {
    if let Some(p) = passphrase_for(&crate::config::home()) {
        std::env::set_var(KEY_VAR, p.as_str());
        return Ok(());
    }
    let envf = crate::config::home().join("env");
    let rng = SystemRandom::new();
    let mut b = [0u8; 32];
    rng.fill(&mut b).map_err(|_| "no system randomness")?;
    let pass = SecretStr::new(hex(&b));
    wipe(&mut b);
    if let Some(dir) = envf.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut text = SecretStr::new(std::fs::read_to_string(&envf).unwrap_or_default());
    if !text.0.is_empty() && !text.0.ends_with('\n') {
        text.0.push('\n');
    }
    text.0.push_str(&format!("{KEY_VAR}={}\n", pass.as_str()));
    crate::security::write_atomic(&envf, text.as_str().as_bytes(), Some(0o600))
        .map_err(|e| e.to_string())?;
    std::env::set_var(KEY_VAR, pass.as_str());
    Ok(())
}

pub fn stash_provider_keys(keys: &[(String, Vec<String>)]) -> Result<Vec<String>, String> {
    ensure_passphrase()?;
    let store = Store::at(&Store::default_path());
    let mut all = store.load()?;
    let mut notes = Vec::new();
    for (provider, ring) in keys {
        let var = crate::config::provider_key_vars(provider)
            .first()
            .copied()
            .unwrap_or("PHOENIX_API_KEY");
        for (i, k) in ring.iter().enumerate() {
            let name = if i == 0 {
                var.to_string()
            } else {
                format!("{var}_{}", i + 1)
            };
            all.insert(name, k.clone());
        }
        if !ring.is_empty() {
            notes.push(format!("{provider} ({})", ring.len()));
        }
    }
    store.save(&all)?;
    Ok(notes)
}

#[cfg(test)]
pub fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn stash_named(entries: &[(String, Vec<String>)]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    ensure_passphrase()?;
    let store = Store::at(&Store::default_path());
    let mut all = store.load()?;
    for (name, values) in entries {
        for (i, v) in values.iter().enumerate() {
            let key = if i == 0 {
                name.clone()
            } else {
                format!("{name}_{}", i + 1)
            };
            all.insert(key, v.clone());
        }
    }
    store.save(&all)
}

pub fn ring_extras(var: &str) -> Vec<String> {
    let store = Store::at(&Store::default_path());
    let Ok(all) = store.load() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 2usize;
    while let Some(v) = all.get(&format!("{var}_{i}")) {
        out.push(v.clone());
        i += 1;
    }
    out
}

pub fn vault_fetch(cmd_tpl: &str, name: &str) -> Option<String> {
    if cmd_tpl.trim().is_empty() || name.is_empty() {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let cmd = cmd_tpl.replace("{name}", name);
    #[cfg(unix)]
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    #[cfg(windows)]
    let out = std::process::Command::new("cmd")
        .arg("/C")
        .arg(&cmd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

pub fn resolve_chain(vault_cmd: &str, var: &str, name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(var) {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Some(v) = vault_fetch(vault_cmd, var) {
        return Some(v);
    }
    let store = Store::at(&Store::default_path());
    store.get(name).or_else(|| store.get(var))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_fetch_runs_the_command_with_the_name_substituted() {
        assert_eq!(
            vault_fetch("echo vk-{name}", "ANTHROPIC_API_KEY").as_deref(),
            Some("vk-ANTHROPIC_API_KEY")
        );
    }

    #[test]
    fn vault_fetch_refuses_suspicious_names_and_failures() {
        assert_eq!(vault_fetch("echo x", ""), None);
        assert_eq!(vault_fetch("echo x", "BAD;NAME"), None);
        assert_eq!(vault_fetch("echo x", "BAD NAME"), None);
        assert_eq!(vault_fetch("false", "GOOD_NAME"), None);
        assert_eq!(vault_fetch("", "GOOD_NAME"), None);
    }

    #[test]
    fn a_secret_stashed_under_its_env_var_name_still_resolves() {
        let _l = env_lock();
        let d = dir("varname");
        std::env::set_var("PHOENIX_STATE_DIR", &d);
        std::env::set_var(KEY_VAR, "test-passphrase");
        std::env::remove_var("PHOENIX_TELEGRAM_TOKEN");
        stash_named(&[(
            "PHOENIX_TELEGRAM_TOKEN".to_string(),
            vec!["123:bot".to_string()],
        )])
        .unwrap();
        assert_eq!(
            resolve_chain("", "PHOENIX_TELEGRAM_TOKEN", "telegram_token").as_deref(),
            Some("123:bot"),
            "`secret import` stashes under the env var name; the lookup must find it there \
 too, or serve starts with no token and systemd restarts it forever"
        );
        std::env::remove_var("PHOENIX_STATE_DIR");
        std::env::remove_var(KEY_VAR);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_chain_prefers_env_then_vault_then_store() {
        let _l = env_lock();
        std::env::set_var("PHX_CHAIN_TEST_VAR", "from-env");
        assert_eq!(
            resolve_chain("echo from-vault", "PHX_CHAIN_TEST_VAR", "x").as_deref(),
            Some("from-env")
        );
        std::env::remove_var("PHX_CHAIN_TEST_VAR");
        assert_eq!(
            resolve_chain("echo from-vault", "PHX_CHAIN_TEST_VAR", "x").as_deref(),
            Some("from-vault")
        );
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("phx-sec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::secrets::test_env_lock()
    }

    fn with_key<T>(key: &str, body: impl FnOnce() -> T) -> T {
        let _g = env_lock();
        std::env::set_var(KEY_VAR, key);
        let out = body();
        std::env::remove_var(KEY_VAR);
        out
    }

    fn without_key<T>(body: impl FnOnce() -> T) -> T {
        let _g = env_lock();
        std::env::remove_var(KEY_VAR);
        body()
    }

    #[test]
    fn a_secret_survives_a_save_and_load_round_trip() {
        let d = dir("roundtrip");
        let store = Store::at(&d.join("secrets.enc"));
        with_key("correct horse battery staple", || {
            store.put("anthropic", "sk-secret-value").unwrap();
            assert_eq!(store.get("anthropic").as_deref(), Some("sk-secret-value"));
            store.put("telegram", "123:abc").unwrap();
            assert_eq!(store.names().unwrap(), vec!["anthropic", "telegram"]);
        });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_stored_bytes_never_contain_the_plaintext() {
        let d = dir("opaque");
        let path = d.join("secrets.enc");
        let store = Store::at(&path);
        with_key("passphrase", || {
            store.put("anthropic", "sk-plaintext-marker").unwrap();
        });
        let raw = std::fs::read(&path).unwrap();
        let hay = String::from_utf8_lossy(&raw);
        assert!(!hay.contains("sk-plaintext-marker"), "plaintext on disk");
        assert!(!hay.contains("anthropic"), "key names leaked");
        assert_eq!(&raw[..4], MAGIC);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_wrong_passphrase_cannot_read_the_store() {
        let d = dir("wrongkey");
        let store = Store::at(&d.join("secrets.enc"));
        with_key("right one", || {
            store.put("k", "v").unwrap();
        });
        let err = with_key("wrong one", || store.load().unwrap_err());
        assert!(err.contains("cannot decrypt"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writing_without_a_key_is_refused_rather_than_written_plainly() {
        let d = dir("nokey");
        let path = d.join("secrets.enc");
        let store = Store::at(&path);
        let err = without_key(|| store.put("k", "v").unwrap_err());
        assert!(err.contains(KEY_VAR), "{err}");
        assert!(!path.exists(), "nothing may be written without a key");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_tampered_store_fails_closed() {
        let d = dir("tamper");
        let path = d.join("secrets.enc");
        let store = Store::at(&path);
        with_key("passphrase", || {
            store.put("k", "v").unwrap();
        });
        let mut raw = std::fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        std::fs::write(&path, &raw).unwrap();
        let err = with_key("passphrase", || store.load().unwrap_err());
        assert!(err.contains("cannot decrypt"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn two_saves_of_the_same_value_produce_different_ciphertext() {
        let d = dir("nonce");
        let a = d.join("a.enc");
        let b = d.join("b.enc");
        with_key("passphrase", || {
            Store::at(&a).put("k", "same").unwrap();
            Store::at(&b).put("k", "same").unwrap();
        });
        assert_ne!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "salt and nonce must be fresh each save"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg(unix)]
    fn the_store_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("perms");
        let path = d.join("secrets.enc");
        with_key("passphrase", || {
            Store::at(&path).put("k", "v").unwrap();
        });
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_env_file_beside_the_store_unlocks_it_without_an_exported_var() {
        let d = dir("envfile");
        let store = Store::at(&d.join("secrets.enc"));
        with_key("file-passphrase", || {
            store.put("ANTHROPIC_API_KEY", "sk-carried").unwrap();
        });
        std::fs::write(d.join("env"), "PHOENIX_SECRET_KEY=file-passphrase\n").unwrap();
        without_key(|| {
            assert!(
                !store.locked(),
                "a store whose env file holds the key is not locked"
            );
            assert_eq!(
                store.get("ANTHROPIC_API_KEY").as_deref(),
                Some("sk-carried"),
                "phoenix writes the unlock key to the env file, so it must read it back"
            );
        });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn env_file_parsing_tolerates_export_and_quotes() {
        for line in [
            "PHOENIX_SECRET_KEY=pw\n",
            "export PHOENIX_SECRET_KEY=pw\n",
            "PHOENIX_SECRET_KEY=\"pw\"\n",
            "PHOENIX_SECRET_KEY='pw'\n",
            "# comment\nOTHER=1\nPHOENIX_SECRET_KEY=pw\n",
        ] {
            let d = dir("envparse");
            std::fs::write(d.join("env"), line).unwrap();
            let got = without_key(|| passphrase_for(&d).map(|p| p.as_str().to_string()));
            assert_eq!(got.as_deref(), Some("pw"), "line {line:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn an_env_file_in_another_directory_never_unlocks_this_store() {
        let a = dir("iso-a");
        let b = dir("iso-b");
        std::fs::write(a.join("env"), "PHOENIX_SECRET_KEY=pw\n").unwrap();
        without_key(|| {
            assert!(passphrase_for(&b).is_none());
        });
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn an_exported_key_wins_over_the_env_file() {
        let d = dir("envprec");
        std::fs::write(d.join("env"), "PHOENIX_SECRET_KEY=from-file\n").unwrap();
        let got = with_key("from-env", || {
            passphrase_for(&d).map(|p| p.as_str().to_string())
        });
        assert_eq!(got.as_deref(), Some("from-env"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wiping_leaves_no_bytes_of_the_secret_behind() {
        let mut buf = b"sk-super-secret".to_vec();
        wipe(&mut buf);
        assert!(buf.iter().all(|b| *b == 0), "every byte must be zeroed");
    }

    #[test]
    fn a_missing_store_reads_as_empty_not_an_error() {
        let d = dir("missing");
        let store = Store::at(&d.join("nothing.enc"));
        without_key(|| {
            assert!(store.load().unwrap().is_empty());
            assert!(!store.locked());
        });
        let _ = std::fs::remove_dir_all(&d);
    }
}

pub fn sealed_token_get(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    let store = Store::at(&Store::default_path());
    store.get(name).filter(|value| !value.trim().is_empty())
}

pub fn sealed_token_put(name: &str, value: &str) -> Result<(), String> {
    let store = Store::at(&Store::default_path());
    store.put(name, value)
}

#[cfg(test)]
mod sealed_credentials {
    use super::*;

    #[test]
    fn host_environment_supplies_credentials_without_any_file() {
        let _guard = test_env_lock();
        let name = "PHOENIX_TEST_SEALED_TOKEN";
        std::env::set_var(name, "  env-supplied-token  ");
        let value = sealed_token_get(name);
        std::env::remove_var(name);
        assert_eq!(value.as_deref(), Some("env-supplied-token"));
    }

    #[test]
    fn an_empty_environment_value_is_not_treated_as_a_credential() {
        let _guard = test_env_lock();
        let name = "PHOENIX_TEST_EMPTY_TOKEN";
        std::env::set_var(name, "   ");
        let value = sealed_token_get(name);
        std::env::remove_var(name);
        assert!(value.is_none(), "blank env must not become a credential");
    }

    #[test]
    fn sealing_writes_no_readable_credential_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "phx-sealed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let path = dir.join("secrets.enc");
        let store = Store::at(&path);
        let _guard = test_env_lock();
        std::env::set_var(KEY_VAR, "unit-test-passphrase");
        let secret = "super-secret-refresh-material";
        let saved = store.put("PHOENIX_TEST_OAUTH", secret);
        std::env::remove_var(KEY_VAR);
        assert!(saved.is_ok(), "{saved:?}");
        let raw = std::fs::read(&path).unwrap_or_default();
        assert!(!raw.is_empty(), "the sealed store must exist");
        let hay = String::from_utf8_lossy(&raw);
        assert!(
            !hay.contains(secret),
            "credential material must never appear in cleartext on disk"
        );
        assert!(raw.starts_with(MAGIC), "sealed envelope header expected");
        assert_eq!(
            raw.get(MAGIC.len()).copied(),
            Some(VERSION_AES_GCM),
            "sealing must use the AES-256-GCM envelope"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sealed_material_is_unreadable_with_the_wrong_passphrase() {
        let dir = std::env::temp_dir().join(format!(
            "phx-wrongpass-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let path = dir.join("secrets.enc");
        let store = Store::at(&path);
        let _guard = test_env_lock();
        std::env::set_var(KEY_VAR, "correct-passphrase");
        let stored = store.put("PHOENIX_TEST_TOKEN", "sealed-material");
        std::env::set_var(KEY_VAR, "wrong-passphrase");
        let reopened = store.load();
        std::env::remove_var(KEY_VAR);
        assert!(stored.is_ok(), "{stored:?}");
        assert!(
            reopened.is_err(),
            "a wrong passphrase must not decrypt sealed credentials"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
