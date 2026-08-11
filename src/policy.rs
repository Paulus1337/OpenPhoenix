use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::Config;

pub const RULES: &[&str] = &[
    "channels.denied",
    "channels.allowed",
    "sandbox.required",
    "network.private_denied",
    "tools.denied",
    "approvals.required",
    "audit.required",
    "secrets.not_in_config",
    "http.token_required",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub rule: String,
    pub pass: bool,
    pub detail: String,
}

pub fn policy_path() -> PathBuf {
    crate::config::home().join("policy.json")
}

pub fn known_rule(name: &str) -> bool {
    RULES.contains(&name)
}

pub fn parse(raw: &str) -> Result<Vec<Rule>, String> {
    let v: Value =
        serde_json::from_str(raw).map_err(|e| format!("policy is not valid json: {e}"))?;
    let obj = v
        .get("rules")
        .and_then(Value::as_object)
        .ok_or("policy has no rules object")?;
    let mut out = Vec::new();
    for (name, val) in obj {
        if !known_rule(name) {
            return Err(format!(
                "unknown policy rule '{name}': expected one of {RULES:?}"
            ));
        }
        let values = match val {
            Value::Bool(b) => vec![b.to_string()],
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            other => return Err(format!("rule '{name}' has an unusable value: {other}")),
        };
        out.push(Rule {
            name: name.clone(),
            values,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn load(path: &Path) -> Result<Vec<Rule>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&raw)
}

fn wants_true(rule: &Rule) -> bool {
    rule.values.iter().any(|v| v == "true")
}

fn configured_channels(cfg: &Config) -> Vec<&'static str> {
    let mut out = Vec::new();
    if !cfg.telegram_token.is_empty() {
        out.push("telegram");
    }
    if !cfg.wa_token.is_empty() {
        out.push("whatsapp");
    }
    if !cfg.discord_token.is_empty() {
        out.push("discord");
    }
    if !cfg.slack_bot_token.is_empty() {
        out.push("slack");
    }
    if !cfg.signal_account.is_empty() {
        out.push("signal");
    }
    if !cfg.irc_server.is_empty() {
        out.push("irc");
    }
    if !cfg.matrix_token.is_empty() {
        out.push("matrix");
    }
    if !cfg.mattermost_token.is_empty() {
        out.push("mattermost");
    }
    if cfg.imessage_enabled {
        out.push("imessage");
    }
    out
}

pub fn evaluate(cfg: &Config, rules: &[Rule], config_raw: &str) -> Vec<Verdict> {
    let live = configured_channels(cfg);
    let mut out = Vec::new();
    for rule in rules {
        let v = match rule.name.as_str() {
            "channels.denied" => {
                let bad: Vec<&str> = rule
                    .values
                    .iter()
                    .filter(|w| live.iter().any(|l| l.eq_ignore_ascii_case(w)))
                    .map(String::as_str)
                    .collect();
                Verdict {
                    rule: rule.name.clone(),
                    pass: bad.is_empty(),
                    detail: if bad.is_empty() {
                        "no denied channel is configured".into()
                    } else {
                        format!("configured but denied: {}", bad.join(", "))
                    },
                }
            }
            "channels.allowed" => {
                let bad: Vec<&str> = live
                    .iter()
                    .filter(|l| !rule.values.iter().any(|w| w.eq_ignore_ascii_case(l)))
                    .copied()
                    .collect();
                Verdict {
                    rule: rule.name.clone(),
                    pass: bad.is_empty(),
                    detail: if bad.is_empty() {
                        "every configured channel is on the allowed list".into()
                    } else {
                        format!("configured but not allowed: {}", bad.join(", "))
                    },
                }
            }
            "sandbox.required" => {
                let on = crate::sandbox::policy(cfg).enabled();
                Verdict {
                    rule: rule.name.clone(),
                    pass: on || !wants_true(rule),
                    detail: if on {
                        format!("sandbox runtime is {}", cfg.sandbox_runtime)
                    } else {
                        "sandbox is off; shell runs on the host".into()
                    },
                }
            }
            "network.private_denied" => Verdict {
                rule: rule.name.clone(),
                pass: !cfg.allow_private_network || !wants_true(rule),
                detail: if cfg.allow_private_network {
                    "security.allow_private_network is on".into()
                } else {
                    "private and loopback addresses are refused".into()
                },
            },
            "tools.denied" => {
                let missing: Vec<&str> = rule
                    .values
                    .iter()
                    .filter(|t| !cfg.deny_tools.iter().any(|d| d == *t))
                    .map(String::as_str)
                    .collect();
                Verdict {
                    rule: rule.name.clone(),
                    pass: missing.is_empty(),
                    detail: if missing.is_empty() {
                        "every named tool is denied".into()
                    } else {
                        format!("not in security.deny_tools: {}", missing.join(", "))
                    },
                }
            }
            "approvals.required" => Verdict {
                rule: rule.name.clone(),
                pass: cfg.approvals || !wants_true(rule),
                detail: if cfg.approvals {
                    "approvals are on".into()
                } else {
                    "security.approvals is off".into()
                },
            },
            "audit.required" => Verdict {
                rule: rule.name.clone(),
                pass: cfg.audit_log || !wants_true(rule),
                detail: if cfg.audit_log {
                    "the audit log is on".into()
                } else {
                    "security.audit_log is off".into()
                },
            },
            "secrets.not_in_config" => {
                let leaked = crate::security::config_has_inline_secret(config_raw);
                Verdict {
                    rule: rule.name.clone(),
                    pass: !leaked || !wants_true(rule),
                    detail: if leaked {
                        "a secret is written into the config file".into()
                    } else {
                        "no secret sits in the config file".into()
                    },
                }
            }
            "http.token_required" => {
                let need = wants_true(rule);
                let ok = !cfg.http_enabled || !cfg.http_token.is_empty() || !need;
                Verdict {
                    rule: rule.name.clone(),
                    pass: ok,
                    detail: if !cfg.http_enabled {
                        "the http server is off".into()
                    } else if cfg.http_token.is_empty() {
                        "the http server is on with no bearer token".into()
                    } else {
                        "the http server requires a bearer token".into()
                    },
                }
            }
            other => Verdict {
                rule: other.to_string(),
                pass: false,
                detail: "unknown rule".into(),
            },
        };
        out.push(v);
    }
    out
}

pub fn attestation(verdicts: &[Verdict]) -> String {
    let mut body = String::new();
    for v in verdicts {
        body.push_str(&format!("{}={}\n", v.rule, v.pass));
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, body.as_bytes());
    let mut out = String::new();
    for b in digest.as_ref() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn report_text(verdicts: &[Verdict], json: bool) -> String {
    if json {
        let arr: Vec<Value> = verdicts
            .iter()
            .map(|v| serde_json::json!({"rule": v.rule, "pass": v.pass, "detail": v.detail}))
            .collect();
        let failed = verdicts.iter().filter(|v| !v.pass).count();
        let mut s = serde_json::to_string_pretty(&serde_json::json!({
            "verdicts": arr,
            "failed": failed,
            "attestation": attestation(verdicts),
        }))
        .unwrap_or_default();
        s.push('\n');
        return s;
    }
    if verdicts.is_empty() {
        return "no policy rules to check\n".to_string();
    }
    let mut out = String::new();
    for v in verdicts {
        out.push_str(&format!(
            "  {}  {:<24}{}\n",
            if v.pass { "pass" } else { "FAIL" },
            v.rule,
            v.detail
        ));
    }
    let failed = verdicts.iter().filter(|v| !v.pass).count();
    out.push_str(&format!(
        "\n{} of {} rules pass\nattestation {}\n",
        verdicts.len() - failed,
        verdicts.len(),
        attestation(verdicts)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_from(raw: &str) -> Config {
        crate::config::parse(raw).unwrap()
    }

    #[test]
    fn a_policy_parses_booleans_strings_and_lists() {
        let rules = parse(
            r#"{"rules":{"audit.required":true,"channels.denied":["telegram"],"sandbox.required":"true"}}"#,
        )
        .unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules.first().map(|r| r.name.as_str()),
            Some("audit.required")
        );
    }

    #[test]
    fn an_unknown_rule_is_refused_rather_than_silently_passing() {
        let err = parse(r#"{"rules":{"make.coffee":true}}"#).unwrap_err();
        assert!(err.contains("unknown policy rule"), "{err}");
        assert!(parse("{oops").is_err());
        assert!(parse("{}").is_err());
    }

    #[test]
    fn a_denied_channel_that_is_configured_fails() {
        let cfg = cfg_from("[telegram]\ntoken = \"t\"\nallowed_chat_ids = [\"1\"]\n");
        let rules = parse(r#"{"rules":{"channels.denied":["telegram"]}}"#).unwrap();
        let v = evaluate(&cfg, &rules, "");
        assert!(!v.first().map(|x| x.pass).unwrap_or(true));
        let clean = evaluate(&cfg_from(""), &rules, "");
        assert!(clean.first().map(|x| x.pass).unwrap_or(false));
    }

    #[test]
    fn an_allowed_list_fails_when_an_extra_channel_is_configured() {
        let cfg = cfg_from(
            "[telegram]\ntoken = \"t\"\nallowed_chat_ids = [\"1\"]\n[discord]\ntoken = \"d\"\n",
        );
        let rules = parse(r#"{"rules":{"channels.allowed":["telegram"]}}"#).unwrap();
        let v = evaluate(&cfg, &rules, "");
        assert!(!v.first().map(|x| x.pass).unwrap_or(true));
        assert!(v
            .first()
            .map(|x| x.detail.contains("discord"))
            .unwrap_or(false));
    }

    #[test]
    fn sandbox_approvals_audit_and_private_network_rules_read_the_real_config() {
        let strict = parse(
            r#"{"rules":{"sandbox.required":true,"approvals.required":true,"audit.required":true,"network.private_denied":true}}"#,
        )
        .unwrap();
        let loose = cfg_from("[security]\naudit_log = false\nallow_private_network = true\n");
        let v = evaluate(&loose, &strict, "");
        assert_eq!(v.iter().filter(|x| x.pass).count(), 0, "{v:?}");

        let tight = cfg_from(
            "[security]\napprovals = true\naudit_log = true\nallow_private_network = false\n\
[sandbox]\nruntime = \"docker\"\n",
        );
        let v2 = evaluate(&tight, &strict, "");
        assert_eq!(v2.iter().filter(|x| !x.pass).count(), 0, "{v2:?}");
    }

    #[test]
    fn a_denied_tool_rule_checks_the_real_deny_list() {
        let rules = parse(r#"{"rules":{"tools.denied":["shell","browser_open"]}}"#).unwrap();
        let missing = evaluate(
            &cfg_from("[security]\ndeny_tools = [\"shell\"]\n"),
            &rules,
            "",
        );
        assert!(!missing.first().map(|x| x.pass).unwrap_or(true));
        assert!(missing
            .first()
            .map(|x| x.detail.contains("browser_open"))
            .unwrap_or(false));
        let full = evaluate(
            &cfg_from("[security]\ndeny_tools = [\"shell\", \"browser_open\"]\n"),
            &rules,
            "",
        );
        assert!(full.first().map(|x| x.pass).unwrap_or(false));
    }

    #[test]
    fn an_http_server_without_a_token_fails_the_token_rule() {
        let rules = parse(r#"{"rules":{"http.token_required":true}}"#).unwrap();
        let open = cfg_from("[http]\nenabled = true\n");
        assert!(!evaluate(&open, &rules, "")
            .first()
            .map(|x| x.pass)
            .unwrap_or(true));
        let closed = cfg_from("[http]\nenabled = true\ntoken = \"t\"\n");
        assert!(evaluate(&closed, &rules, "")
            .first()
            .map(|x| x.pass)
            .unwrap_or(false));
        let off = cfg_from("");
        assert!(evaluate(&off, &rules, "")
            .first()
            .map(|x| x.pass)
            .unwrap_or(false));
    }

    #[test]
    fn an_inline_secret_fails_the_secrets_rule() {
        let rules = parse(r#"{"rules":{"secrets.not_in_config":true}}"#).unwrap();
        let raw = "[provider]\napi_key = \"sk-live-1234\"\n";
        assert!(!evaluate(&cfg_from(raw), &rules, raw)
            .first()
            .map(|x| x.pass)
            .unwrap_or(true));
        let clean = "[provider]\nkind = \"ollama\"\n";
        assert!(evaluate(&cfg_from(clean), &rules, clean)
            .first()
            .map(|x| x.pass)
            .unwrap_or(false));
    }

    #[test]
    fn the_attestation_is_stable_and_changes_with_a_verdict() {
        let rules = parse(r#"{"rules":{"audit.required":true}}"#).unwrap();
        let a = evaluate(&cfg_from("[security]\naudit_log = true\n"), &rules, "");
        let b = evaluate(&cfg_from("[security]\naudit_log = true\n"), &rules, "");
        let c = evaluate(&cfg_from("[security]\naudit_log = false\n"), &rules, "");
        assert_eq!(attestation(&a), attestation(&b));
        assert_ne!(attestation(&a), attestation(&c));
        assert_eq!(attestation(&a).len(), 64);
    }

    #[test]
    fn the_report_counts_passes_and_the_json_form_carries_the_attestation() {
        let rules = parse(r#"{"rules":{"audit.required":true}}"#).unwrap();
        let v = evaluate(&cfg_from("[security]\naudit_log = false\n"), &rules, "");
        let text = report_text(&v, false);
        assert!(text.contains("FAIL"), "{text}");
        assert!(text.contains("0 of 1 rules pass"), "{text}");
        let raw = report_text(&v, true);
        let j: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(j["failed"], 1);
        assert_eq!(j["attestation"], attestation(&v));
        assert!(report_text(&[], false).contains("no policy rules"));
    }
}
