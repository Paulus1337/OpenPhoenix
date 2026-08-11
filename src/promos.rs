use std::io::Read;
use std::time::Duration;

use serde_json::Value;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub struct Promo {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub summary: String,
    pub base_url: String,
    pub expires: String,
}

pub fn parse_catalog(body: &str) -> Result<Vec<Promo>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad promo catalog: {e}"))?;
    let arr = v
        .get("promos")
        .and_then(Value::as_array)
        .ok_or("promo catalog has no promos array")?;
    let mut out = Vec::new();
    for p in arr {
        let Some(id) = p.get("id").and_then(Value::as_str) else {
            continue;
        };
        let provider = p
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.trim().is_empty() || !crate::config::known_kind(&provider) {
            continue;
        }
        let base_url = p
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !base_url.is_empty() && crate::ssrf::check_url(&base_url).is_err() {
            continue;
        }
        out.push(Promo {
            id: id.to_string(),
            provider,
            model: p
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            summary: p
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            base_url,
            expires: p
                .get("expires")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

pub fn catalog_url(cfg: &Config) -> String {
    format!("{}/api/v1/promos", cfg.clawhub_url.trim_end_matches('/'))
}

pub fn fetch(cfg: &Config) -> Result<Vec<Promo>, String> {
    let url = catalog_url(cfg);
    crate::ssrf::check_url(&url).map_err(|e| format!("promo catalog refused: {e}"))?;
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| crate::security::redact(&e.to_string()))?;
    let mut buf = String::new();
    resp.into_reader()
        .take(1 << 20)
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    parse_catalog(&buf)
}

pub fn find<'a>(promos: &'a [Promo], id: &str) -> Option<&'a Promo> {
    promos.iter().find(|p| p.id.eq_ignore_ascii_case(id.trim()))
}

pub fn claim_plan(promo: &Promo) -> Vec<String> {
    let mut steps = vec![format!("set [provider] kind = \"{}\"", promo.provider)];
    if !promo.model.is_empty() {
        steps.push(format!("set [provider] model = \"{}\"", promo.model));
    }
    if !promo.base_url.is_empty() {
        steps.push(format!("set [provider] base_url = \"{}\"", promo.base_url));
    }
    let vars = crate::config::provider_key_vars(&promo.provider);
    match vars.first() {
        Some(v) => steps.push(format!("export the promo key as {v}")),
        None => steps.push("export the promo key as PHOENIX_API_KEY".into()),
    }
    steps
}

pub fn list_text(promos: &[Promo]) -> String {
    if promos.is_empty() {
        return "no promotions on offer\n".to_string();
    }
    let mut out = format!("{} promotion(s)\n", promos.len());
    for p in promos {
        let when = if p.expires.is_empty() {
            String::new()
        } else {
            format!("  (until {})", p.expires)
        };
        out.push_str(&format!("  {:<20}{} {}{when}\n", p.id, p.provider, p.model));
        if !p.summary.is_empty() {
            out.push_str(&format!(
                "      {}\n",
                crate::security::one_line(&p.summary, 70)
            ));
        }
    }
    out.push_str("\nclaim one with: phoenix promos claim ID\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{"promos":[
        {"id":"free-gemini","provider":"google","model":"gemini-3-flash","summary":"free tier","expires":"2027-01-01"},
        {"id":"nvidia-trial","provider":"nvidia","model":"nemotron","base_url":"https://integrate.api.nvidia.com/v1"}
    ]}"#;

    #[test]
    fn a_catalog_parses_into_promos() {
        let promos = parse_catalog(CATALOG).unwrap();
        assert_eq!(promos.len(), 2);
        assert_eq!(
            find(&promos, "FREE-GEMINI").map(|p| p.provider.as_str()),
            Some("google")
        );
        assert!(find(&promos, "nope").is_none());
    }

    #[test]
    fn junk_and_unknown_providers_are_dropped_not_trusted() {
        let raw = r#"{"promos":[
            {"id":"bad","provider":"totally-made-up","model":"x"},
            {"id":"","provider":"google"},
            {"provider":"google"},
            {"id":"ok","provider":"google"}
        ]}"#;
        let promos = parse_catalog(raw).unwrap();
        assert_eq!(promos.len(), 1);
        assert_eq!(promos.first().map(|p| p.id.as_str()), Some("ok"));
    }

    #[test]
    fn a_promo_pointing_at_a_private_address_is_dropped() {
        let raw = r#"{"promos":[
            {"id":"ssrf","provider":"openai","base_url":"http://169.254.169.254/v1"},
            {"id":"loopback","provider":"openai","base_url":"http://127.0.0.1:8080/v1"},
            {"id":"nohost","provider":"openai","base_url":"http://localhost:8080/v1"},
            {"id":"fine","provider":"openai"}
        ]}"#;
        let promos = parse_catalog(raw).unwrap();
        assert_eq!(
            promos.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["fine"],
            "link-local, loopback and localhost promos are dropped; \
 a promo with no base_url needs no lookup, so this stays offline and cannot flake on DNS"
        );
    }

    #[test]
    fn a_malformed_catalog_is_an_error_not_an_empty_list() {
        assert!(parse_catalog("{oops").is_err());
        assert!(parse_catalog("{}").is_err());
        assert!(parse_catalog(r#"{"promos":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn the_claim_plan_never_writes_a_key_into_the_config() {
        let promos = parse_catalog(CATALOG).unwrap();
        let promo = find(&promos, "free-gemini").unwrap();
        let steps = claim_plan(promo);
        assert!(
            steps.iter().any(|s| s.contains("kind = \"google\"")),
            "{steps:?}"
        );
        assert!(
            steps.iter().any(|s| s.contains("gemini-3-flash")),
            "{steps:?}"
        );
        assert!(
            steps.iter().any(|s| s.contains("export the promo key")),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.contains("api_key =")),
            "a key must never be written into config: {steps:?}"
        );
    }

    #[test]
    fn the_catalog_url_hangs_off_the_configured_clawhub() {
        let cfg = Config {
            clawhub_url: "https://hub.example/".into(),
            ..Config::default()
        };
        assert_eq!(catalog_url(&cfg), "https://hub.example/api/v1/promos");
    }

    #[test]
    fn list_text_names_every_promo_and_the_claim_command() {
        let promos = parse_catalog(CATALOG).unwrap();
        let text = list_text(&promos);
        assert!(text.contains("free-gemini"), "{text}");
        assert!(text.contains("until 2027-01-01"), "{text}");
        assert!(text.contains("promos claim"), "{text}");
        assert!(list_text(&[]).contains("no promotions"));
    }
}
