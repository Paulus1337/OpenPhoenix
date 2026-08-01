use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::security::redact;

pub const TOP_K: usize = 8;
pub const MIN_SIMILARITY: f32 = 0.25;
pub const EMBED_BATCH: usize = 96;

#[derive(Debug, Clone)]
pub struct EmbedConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub index_path: PathBuf,
}

impl EmbedConfig {
    pub fn from_config(cfg: &Config) -> EmbedConfig {
        let base = if !cfg.mem_embed_base_url.is_empty() {
            cfg.mem_embed_base_url.clone()
        } else if !cfg.base_url.is_empty() {
            cfg.base_url.clone()
        } else {
            "https://api.openai.com/v1".to_string()
        };
        EmbedConfig {
            model: cfg.mem_embed_model.clone(),
            base_url: base,
            api_key: cfg.api_key.clone(),
            index_path: crate::config::home().join("memory.embeddings.jsonl"),
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
pub fn line_hash(line: &str) -> String {
    format!("{:016x}", fnv1a(line.as_bytes()))
}

pub fn cache_key(model: &str, line: &str) -> String {
    format!(
        "{:016x}{:016x}",
        fnv1a(model.as_bytes()),
        fnv1a(line.as_bytes())
    )
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn embed(cfg: &EmbedConfig, inputs: &[&str]) -> Result<Vec<Vec<f32>>, String> {
    let url = format!("{}/embeddings", cfg.base_url.trim_end_matches('/'));
    let payload = json!({"model": cfg.model, "input": inputs});
    let mut req = ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .set("Content-Type", "application/json");
    if !cfg.api_key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", cfg.api_key));
    }
    let resp = req
        .send_string(&payload.to_string())
        .map_err(|e| redact(&e.to_string()))?;
    let text = resp.into_string().map_err(|e| redact(&e.to_string()))?;
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("bad JSON from embeddings: {e}"))?;
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or("malformed embeddings response")?;
    let mut out = vec![Vec::new(); inputs.len()];
    for item in data {
        let idx = item["index"].as_u64().unwrap_or(0) as usize;
        let vec: Vec<f32> = item["embedding"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_f64)
                    .map(|x| x as f32)
                    .collect()
            })
            .unwrap_or_default();
        if idx < out.len() {
            out[idx] = vec;
        }
    }
    if out.iter().any(Vec::is_empty) {
        return Err("embeddings response missing vectors".into());
    }
    Ok(out)
}

fn load_index(path: &PathBuf) -> HashMap<String, Vec<f32>> {
    let mut index = HashMap::new();
    let Ok(content) = fs::read_to_string(path) else {
        return index;
    };
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(hash) = v["hash"].as_str() else {
            continue;
        };
        let Some(arr) = v["vector"].as_array() else {
            continue;
        };
        let vec: Vec<f32> = arr
            .iter()
            .filter_map(Value::as_f64)
            .map(|x| x as f32)
            .collect();
        if !vec.is_empty() {
            index.insert(hash.to_string(), vec);
        }
    }
    index
}

fn append_index(path: &PathBuf, entries: &[(String, Vec<f32>)]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut fh = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    for (hash, vec) in entries {
        let line = json!({"hash": hash, "vector": vec}).to_string();
        writeln!(fh, "{line}").map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn rank(cfg: &EmbedConfig, lines: &[&str], query: &str) -> Result<Vec<String>, String> {
    let mut index = load_index(&cfg.index_path);
    let mut missing: Vec<&str> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for line in lines {
        let h = cache_key(&cfg.model, line);
        if !index.contains_key(&h) && seen.insert(h) {
            missing.push(line);
        }
    }
    if !missing.is_empty() {
        for chunk in missing.chunks(EMBED_BATCH) {
            let vecs = embed(cfg, chunk)?;
            let entries: Vec<(String, Vec<f32>)> = chunk
                .iter()
                .map(|l| cache_key(&cfg.model, l))
                .zip(vecs)
                .collect();
            append_index(&cfg.index_path, &entries)?;
            index.extend(entries);
        }
    }
    let qv = embed(cfg, &[query])?.remove(0);
    let mut scored: Vec<(f32, &str)> = lines
        .iter()
        .filter_map(|l| {
            index
                .get(&cache_key(&cfg.model, l))
                .filter(|v| v.len() == qv.len())
                .map(|v| (cosine(&qv, v), *l))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(b.1))
    });
    Ok(scored
        .into_iter()
        .take(TOP_K)
        .filter(|(s, _)| *s >= MIN_SIMILARITY)
        .map(|(_, l)| l.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write as IoWrite};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "px-embed-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn canned(text: &str) -> Vec<f32> {
        if text.contains("alpha") {
            vec![1.0, 0.0, 0.0]
        } else if text.contains("beta") {
            vec![0.8, 0.6, 0.0]
        } else if text.contains("gamma") {
            vec![0.0, 0.0, 1.0]
        } else {
            vec![0.0, 1.0, 0.0]
        }
    }

    fn mock_embed_server(embedded: Arc<Mutex<Vec<String>>>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let mut reader = BufReader::new(&stream);
                let mut content_len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let t = line.trim_end().to_ascii_lowercase();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("content-length:") {
                        content_len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut buf = vec![0u8; content_len];
                let _ = reader.read_exact(&mut buf);
                let req: Value = serde_json::from_slice(&buf).unwrap_or_default();
                let empty = Vec::new();
                let inputs = req["input"].as_array().unwrap_or(&empty);
                let data: Vec<Value> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, inp)| {
                        let text = inp.as_str().unwrap_or("");
                        embedded.lock().unwrap().push(text.to_string());
                        json!({"index": i, "embedding": canned(text)})
                    })
                    .collect();
                let body = json!({"data": data}).to_string();
                let mut s = &stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        addr
    }

    fn make_cfg(addr: SocketAddr, dir: &std::path::Path) -> EmbedConfig {
        EmbedConfig {
            model: "test-embed".into(),
            base_url: format!("http://{addr}/v1"),
            api_key: "k".into(),
            index_path: dir.join("memory.embeddings.jsonl"),
        }
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn line_hash_is_stable_and_distinct() {
        assert_eq!(line_hash("abc"), line_hash("abc"));
        assert_ne!(line_hash("abc"), line_hash("abd"));
        assert_eq!(line_hash("abc").len(), 16);
    }

    #[cfg(unix)]
    #[test]
    fn indexing_writes_file_with_600_perms() {
        use std::os::unix::fs::PermissionsExt;
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let dir = tmpdir();
        let cfg = make_cfg(addr, &dir);
        let lines = ["- alpha note", "- gamma note"];
        rank(&cfg, &lines, "query alpha").unwrap();
        let mode = fs::metadata(&cfg.index_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let content = fs::read_to_string(&cfg.index_path).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains(&line_hash("- alpha note")));
    }

    #[test]
    fn ranking_order_and_floor() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let cfg = make_cfg(addr, &tmpdir());
        let lines = ["- gamma fact", "- beta fact", "- alpha fact"];
        let hits = rank(&cfg, &lines, "tell me about alpha").unwrap();

        assert_eq!(
            hits,
            vec!["- alpha fact".to_string(), "- beta fact".to_string()]
        );
    }

    #[test]
    fn switching_models_does_not_reuse_the_old_models_vectors() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let dir = tmpdir();
        let mut cfg = make_cfg(addr, &dir);
        let lines = ["- alpha fact"];
        rank(&cfg, &lines, "alpha").unwrap();
        let after_first = embedded.lock().unwrap().len();

        cfg.model = "a-different-embedding-model".into();
        rank(&cfg, &lines, "alpha").unwrap();
        let second: Vec<String> = embedded.lock().unwrap()[after_first..].to_vec();
        assert!(
            second.contains(&"- alpha fact".to_string()),
            "a model switch must re-embed, not reuse incomparable vectors: {second:?}"
        );
    }

    #[test]
    fn cache_key_separates_models_and_lines() {
        assert_ne!(cache_key("m1", "line"), cache_key("m2", "line"));
        assert_ne!(cache_key("m1", "a"), cache_key("m1", "b"));
        assert_eq!(cache_key("m1", "line"), cache_key("m1", "line"));
    }

    #[test]
    fn a_cold_corpus_is_embedded_in_bounded_batches() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let cfg = make_cfg(addr, &tmpdir());
        let owned: Vec<String> = (0..250).map(|i| format!("- alpha fact {i}")).collect();
        let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
        let hits = rank(&cfg, &lines, "alpha").unwrap();
        assert!(!hits.is_empty());
        assert_eq!(
            embedded.lock().unwrap().len(),
            251,
            "every line must be embedded exactly once, plus the query"
        );
        let content = fs::read_to_string(&cfg.index_path).unwrap();
        assert_eq!(content.lines().count(), 250);
    }

    #[test]
    fn a_stale_vector_of_the_wrong_width_is_ignored() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let dir = tmpdir();
        let cfg = make_cfg(addr, &dir);
        append_index(
            &cfg.index_path,
            &[(
                cache_key(&cfg.model, "- alpha fact"),
                vec![1.0, 0.0, 0.0, 0.5, 0.25],
            )],
        )
        .unwrap();
        let hits = rank(&cfg, &["- alpha fact"], "alpha").unwrap();
        assert!(
            hits.is_empty(),
            "a vector with the wrong dimension count must not be scored: {hits:?}"
        );
    }

    #[test]
    fn ranking_is_deterministic_for_equal_scores() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let cfg = make_cfg(addr, &tmpdir());
        let lines = ["- alpha two", "- alpha one", "- alpha three"];
        let first = rank(&cfg, &lines, "alpha").unwrap();
        let second = rank(&cfg, &lines, "alpha").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.first().map(String::as_str), Some("- alpha one"));
    }

    #[test]
    fn hash_cache_skips_reembedding() {
        let embedded = Arc::new(Mutex::new(Vec::new()));
        let addr = mock_embed_server(Arc::clone(&embedded));
        let cfg = make_cfg(addr, &tmpdir());
        let lines = ["- alpha one", "- beta two"];
        rank(&cfg, &lines, "alpha").unwrap();
        let after_first = embedded.lock().unwrap().len();
        assert_eq!(after_first, 3);
        rank(&cfg, &lines, "alpha again").unwrap();
        let second: Vec<String> = embedded.lock().unwrap()[after_first..].to_vec();
        assert_eq!(second, vec!["alpha again".to_string()]);
    }

    #[test]
    fn server_error_is_reported_not_panicking() {
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let cfg = make_cfg(addr, &tmpdir());
        assert!(rank(&cfg, &["- alpha"], "alpha").is_err());
    }
}
