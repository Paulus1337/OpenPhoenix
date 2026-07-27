use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::Config;
use crate::discord::chunks_of;
use crate::security::redact;

const MAX_MSG: usize = 4000;

pub struct Signal {
    account: String,
    allowed: HashSet<String>,
    cli_path: String,
    port: u16,
}

pub fn classify(ev: &Value) -> Option<(String, String)> {
    let envelope = if ev["envelope"].is_object() {
        &ev["envelope"]
    } else {
        &ev["params"]["envelope"]
    };
    if !envelope.is_object() || envelope["syncMessage"].is_object() {
        return None;
    }
    let dm = &envelope["dataMessage"];
    if !dm.is_object() || dm["reaction"].is_object() {
        return None;
    }
    let text = dm["message"].as_str().unwrap_or("");
    let sender = envelope["sourceNumber"]
        .as_str()
        .or_else(|| envelope["sourceUuid"].as_str())
        .unwrap_or("");
    if text.is_empty() || sender.is_empty() {
        return None;
    }
    Some((sender.to_string(), text.to_string()))
}

pub fn parse_sse_data(lines: &[String]) -> Option<Value> {
    let mut data = String::new();
    for l in lines {
        if let Some(rest) = l.strip_prefix("data:") {
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data).ok()
}

impl Signal {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.signal_account.is_empty() || !cfg.signal_allowed.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Signal, String> {
        if cfg.signal_account.is_empty() {
            return Err("signal: account missing (E.164 like +4915551234567)".into());
        }
        if cfg.signal_allowed.is_empty() {
            return Err("signal: allowed_numbers is empty; refusing to serve everyone".into());
        }
        Ok(Signal {
            account: cfg.signal_account.clone(),
            allowed: cfg.signal_allowed.iter().cloned().collect(),
            cli_path: if cfg.signal_cli_path.is_empty() {
                "signal-cli".to_string()
            } else {
                cfg.signal_cli_path.clone()
            },
            port: cfg.signal_http_port,
        })
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn spawn_daemon(&self) -> Result<Child, String> {
        Command::new(&self.cli_path)
            .args([
                "-a",
                &self.account,
                "daemon",
                "--http",
                &format!("127.0.0.1:{}", self.port),
                "--no-receive-stdout",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("signal: cannot spawn {}: {e}", self.cli_path))
    }

    fn wait_ready(&self, deadline: Duration) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            let ok = ureq::get(&format!("{}/api/v1/check", self.base()))
                .timeout(Duration::from_secs(2))
                .call()
                .is_ok();
            if ok {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err("signal: daemon did not become ready in time".into())
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({ "jsonrpc": "2.0", "id": "1", "method": method, "params": params });
        let resp = ureq::post(&format!("{}/api/v1/rpc", self.base()))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(60))
            .send_string(&body.to_string())
            .map_err(|e| redact(&e.to_string()))?;
        if resp.status() == 201 {
            return Ok(Value::Null);
        }
        let mut buf = String::new();
        resp.into_reader()
            .take(1 << 20)
            .read_to_string(&mut buf)
            .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&buf).map_err(|e| e.to_string())?;
        if v["error"].is_object() {
            return Err(format!(
                "signal rpc {method}: {}",
                v["error"]["message"].as_str().unwrap_or("unknown error")
            ));
        }
        Ok(v["result"].clone())
    }

    pub fn send(&self, recipient: &str, text: &str) -> Result<(), String> {
        for part in chunks_of(text, MAX_MSG) {
            self.rpc("send", json!({ "recipient": [recipient], "message": part }))?;
        }
        Ok(())
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut backoff = 1u64;
        println!(
            "phoenix: serving signal as {} ({} allowed number(s))",
            self.account,
            self.allowed.len()
        );
        loop {
            match self.run_daemon(handler) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    println!("signal error: {}, restarting", redact(&e));
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    fn run_daemon(&self, handler: &mut dyn FnMut(&str, &str) -> String) -> Result<(), String> {
        let mut child = self.spawn_daemon()?;
        let result = (|| {
            self.wait_ready(Duration::from_secs(45))?;
            self.stream_events(&mut child, handler)
        })();
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    fn stream_events(
        &self,
        child: &mut Child,
        handler: &mut dyn FnMut(&str, &str) -> String,
    ) -> Result<(), String> {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(300))
            .build();
        let resp = agent
            .get(&format!("{}/api/v1/events", self.base()))
            .set("Accept", "text/event-stream")
            .call()
            .map_err(|e| redact(&e.to_string()))?;
        let mut reader = BufReader::new(resp.into_reader());
        let mut lines: Vec<String> = Vec::new();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("signal-cli exited ({status})"));
            }
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return Err("signal event stream closed".into()),
                Ok(_) => {}
                Err(e) => return Err(format!("signal event stream: {e}")),
            }
            let line = line.trim_end().to_string();
            if !line.is_empty() {
                lines.push(line);
                continue;
            }

            let ev = parse_sse_data(&lines);
            lines.clear();
            let Some(ev) = ev else { continue };
            let Some((sender, text)) = classify(&ev) else {
                continue;
            };
            if !self.allowed.contains(&sender) {
                continue;
            }
            let reply = handler(&sender, &text);
            if !reply.is_empty() {
                if let Err(e) = self.send(&sender, &reply) {
                    println!("signal send error: {}", redact(&e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(account: &str, allowed: &[&str]) -> Config {
        Config {
            signal_account: account.to_string(),
            signal_allowed: allowed.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn fail_closed_on_partial_config() {
        assert!(Signal::new(&cfg("", &["+491"])).is_err());
        assert!(Signal::new(&cfg("+490", &[])).is_err());
        assert!(Signal::new(&cfg("+490", &["+491"])).is_ok());
    }

    #[test]
    fn classify_plain_message() {
        let ev: Value = serde_json::from_str(
            r#"{"envelope":{"sourceNumber":"+4915551234567","sourceUuid":"u-1",
                "dataMessage":{"message":"hi phoenix","timestamp":1}},"account":"+490"}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ev),
            Some(("+4915551234567".to_string(), "hi phoenix".to_string()))
        );
    }

    #[test]
    fn classify_skips_noise() {
        let sync: Value = serde_json::from_str(
            r#"{"envelope":{"sourceNumber":"+49","syncMessage":{"sentMessage":{}}}}"#,
        )
        .unwrap();
        assert_eq!(classify(&sync), None);
        let receipt: Value =
            serde_json::from_str(r#"{"envelope":{"sourceNumber":"+49","receiptMessage":{}}}"#)
                .unwrap();
        assert_eq!(classify(&receipt), None);
        let reaction: Value = serde_json::from_str(
            r#"{"envelope":{"sourceNumber":"+49",
                "dataMessage":{"message":"","reaction":{"emoji":"👍"}}}}"#,
        )
        .unwrap();
        assert_eq!(classify(&reaction), None);
    }

    #[test]
    fn classify_jsonrpc_notification_shape() {
        let ev: Value = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
                "sourceNumber":"+4915551234567",
                "dataMessage":{"message":"wrapped"}},"account":"+490"}}"#,
        )
        .unwrap();
        assert_eq!(
            classify(&ev),
            Some(("+4915551234567".to_string(), "wrapped".to_string()))
        );
    }

    #[test]
    fn sse_data_lines_concatenate() {
        let lines = vec![
            "event: receive".to_string(),
            r#"data: {"envelope":"#.to_string(),
            r#"data: {"sourceNumber":"+49"}}"#.to_string(),
        ];
        let v = parse_sse_data(&lines).unwrap();
        assert_eq!(v["envelope"]["sourceNumber"].as_str(), Some("+49"));
        assert_eq!(parse_sse_data(&["event: x".to_string()]), None);
    }
}
