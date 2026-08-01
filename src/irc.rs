use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;

pub struct Irc {
    server: String,
    port: u16,
    tls: bool,
    nick: String,
    channels: Vec<String>,
    allowed: crate::allowlist::Allowlist,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Ping(String),
    Msg {
        sender: String,
        target: String,
        text: String,
    },
    Welcome,
    Other,
}

pub fn parse_line(line: &str, me: &str) -> Event {
    let line = line.trim_end();
    if let Some(tok) = line.strip_prefix("PING ") {
        return Event::Ping(tok.trim_start_matches(':').to_string());
    }
    let Some(rest) = line.strip_prefix(':') else {
        return Event::Other;
    };
    let mut parts = rest.splitn(2, ' ');
    let prefix = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    if rest.starts_with("001 ") || rest.starts_with("001\t") {
        return Event::Welcome;
    }
    if let Some(msg) = rest.strip_prefix("PRIVMSG ") {
        let Some((target, text)) = msg.split_once(" :") else {
            return Event::Other;
        };
        let sender = prefix.split('!').next().unwrap_or("").to_string();
        let target = target.trim();
        let reply_to = if target.eq_ignore_ascii_case(me) {
            sender.clone()
        } else {
            target.to_string()
        };
        return Event::Msg {
            sender,
            target: reply_to,
            text: text.to_string(),
        };
    }
    Event::Other
}

pub const IRC_LINE_BYTES: usize = 512;

pub fn inbound_text(sender: &str, text: &str, elapsed: Option<u64>) -> String {
    if crate::looks_like_command(text) {
        return text.to_string();
    }
    crate::text::format_envelope(
        "IRC",
        sender,
        &crate::scheduler::now_local().stamp(),
        elapsed,
        text,
    )
}

pub fn payload_budget(target: &str) -> usize {
    let overhead = "PRIVMSG ".len() + target.len() + " :".len() + "\r\n".len();
    IRC_LINE_BYTES.saturating_sub(overhead).max(1)
}

pub fn chunks_for(text: &str, budget: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut rest = line;
        while rest.len() > budget {
            let mut cut = 0usize;
            let mut last_space = 0usize;
            for (i, c) in rest.char_indices() {
                let end = i + c.len_utf8();
                if end > budget {
                    break;
                }
                cut = end;
                if c.is_whitespace() {
                    last_space = i;
                }
            }
            if cut == 0 {
                break;
            }
            let (piece, next) = if last_space > 0 {
                (&rest[..last_space], &rest[last_space..])
            } else {
                (&rest[..cut], &rest[cut..])
            };
            out.push(piece.to_string());
            rest = next.trim_start();
        }
        if !rest.is_empty() {
            out.push(rest.to_string());
        }
    }
    out
}

#[cfg(test)]
pub fn chunks(text: &str) -> Vec<String> {
    chunks_for(text, payload_budget("#channel"))
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(t) => t.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(t) => t.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(t) => t.flush(),
        }
    }
}

impl Irc {
    pub fn wanted(cfg: &Config) -> bool {
        !cfg.irc_server.is_empty()
    }

    pub fn new(cfg: &Config) -> Result<Irc, String> {
        if cfg.irc_server.is_empty() {
            return Err("irc: server missing".into());
        }
        if cfg.irc_allowed.is_empty() {
            return Err("irc: allowed_nicks is empty; refusing to serve everyone".into());
        }
        Ok(Irc {
            server: cfg.irc_server.clone(),
            port: cfg.irc_port,
            tls: cfg.irc_tls,
            nick: if cfg.irc_nick.is_empty() {
                "phoenix".to_string()
            } else {
                cfg.irc_nick.clone()
            },
            channels: cfg.irc_channels.clone(),
            allowed: crate::allowlist::Allowlist::new(&cfg.irc_allowed),
        })
    }

    fn connect(&self) -> Result<Stream, String> {
        let tcp = TcpStream::connect((self.server.as_str(), self.port))
            .map_err(|e| format!("irc: connect {}:{}: {e}", self.server, self.port))?;
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(Duration::from_secs(600))).ok();
        if !self.tls {
            return Ok(Stream::Plain(tcp));
        }
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(self.server.clone())
            .map_err(|e| e.to_string())?;
        let conn =
            rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
        Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
    }

    fn session(
        &self,
        handler: &mut dyn FnMut(&str, &str) -> String,
        last_seen: &mut std::collections::HashMap<String, u64>,
    ) -> Result<(), String> {
        let stream = self.connect()?;
        let mut writer = stream;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        write!(
            writer,
            "NICK {}\r\nUSER {} 0 * :openphoenix\r\n",
            self.nick, self.nick
        )
        .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        loop {
            buf.clear();
            loop {
                match writer.read(&mut byte) {
                    Ok(0) => return Err("irc: server closed connection".into()),
                    Ok(_) => {
                        if byte[0] == b'\n' {
                            break;
                        }
                        if buf.len() < 8192 {
                            buf.push(byte[0]);
                        }
                    }
                    Err(e) => return Err(format!("irc: read: {e}")),
                }
            }
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            match parse_line(&line, &self.nick) {
                Event::Ping(tok) => {
                    write!(writer, "PONG :{tok}\r\n").map_err(|e| e.to_string())?;
                    writer.flush().map_err(|e| e.to_string())?;
                }
                Event::Welcome => {
                    for ch in &self.channels {
                        write!(writer, "JOIN {ch}\r\n").map_err(|e| e.to_string())?;
                    }
                    writer.flush().map_err(|e| e.to_string())?;
                }
                Event::Msg {
                    sender,
                    target,
                    text,
                } => {
                    if !self.allowed.allows(&sender) {
                        continue;
                    }
                    let now = crate::scheduler::now_epoch();
                    let elapsed = last_seen
                        .insert(sender.clone(), now)
                        .map(|prev| now.saturating_sub(prev));
                    let text = inbound_text(&sender, &text, elapsed);
                    let reply = handler(&sender, &text);
                    for chunk in chunks_for(&reply, payload_budget(&target)) {
                        write!(writer, "PRIVMSG {target} :{chunk}\r\n")
                            .map_err(|e| e.to_string())?;
                    }
                    writer.flush().map_err(|e| e.to_string())?;
                }
                Event::Other => {}
            }
        }
    }

    pub fn serve(&self, handler: &mut dyn FnMut(&str, &str) -> String) {
        let mut backoff = 1u64;
        let mut last_seen: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        loop {
            match self.session(handler, &mut last_seen) {
                Ok(()) => backoff = 1,
                Err(e) => {
                    eprintln!("{e}; reconnecting in {backoff}s");
                    std::thread::sleep(Duration::from_secs(backoff));
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    #[test]
    fn inbound_text_envelopes_chat_but_not_commands() {
        let out = inbound_text("paulus", "hello", Some(45));
        assert!(out.starts_with("[IRC paulus +1m "), "{out}");
        assert!(out.ends_with("] hello"), "{out}");
        assert_eq!(inbound_text("paulus", "/reset", None), "/reset");
    }

    #[test]
    fn parse_ping_welcome_privmsg() {
        assert_eq!(
            parse_line("PING :irc.libera.chat", "phoenix"),
            Event::Ping("irc.libera.chat".into())
        );
        assert_eq!(
            parse_line(":irc.libera.chat 001 phoenix :Welcome", "phoenix"),
            Event::Welcome
        );
        assert_eq!(
            parse_line(":paulus!u@host PRIVMSG #ops :deploy status?", "phoenix"),
            Event::Msg {
                sender: "paulus".into(),
                target: "#ops".into(),
                text: "deploy status?".into()
            }
        );
        assert_eq!(
            parse_line(":paulus!u@host PRIVMSG phoenix :hi", "phoenix"),
            Event::Msg {
                sender: "paulus".into(),
                target: "paulus".into(),
                text: "hi".into()
            }
        );
        assert_eq!(parse_line("NOTICE stuff", "phoenix"), Event::Other);
    }

    #[test]
    fn chunking_splits_long_lines() {
        let long = "x".repeat(900);
        let budget = payload_budget("#ops");
        let c = chunks_for(&long, budget);
        assert!(c.len() >= 2, "a 900 byte line must split");
        assert!(c.iter().all(|p| p.len() <= budget));
        assert_eq!(chunks("a\n\nb"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn every_wire_line_fits_the_512_byte_protocol_limit() {
        for target in ["#a", "#ops", "#a-very-long-channel-name-for-testing"] {
            let budget = payload_budget(target);
            for body in [
                "x".repeat(2000),
                "word ".repeat(400),
                "\u{6f22}\u{5b57}".repeat(300),
                "\u{1f525}".repeat(200),
            ] {
                for chunk in chunks_for(&body, budget) {
                    let wire = format!("PRIVMSG {target} :{chunk}\r\n");
                    assert!(
                        wire.len() <= IRC_LINE_BYTES,
                        "wire line {} bytes for target {target}",
                        wire.len()
                    );
                }
            }
        }
    }

    #[test]
    fn multibyte_characters_are_never_split_in_half() {
        let body = "\u{6f22}\u{5b57}".repeat(300);
        let out = chunks_for(&body, payload_budget("#ops"));
        let rejoined: String = out.concat();
        assert!(
            rejoined.chars().all(|c| c == '\u{6f22}' || c == '\u{5b57}'),
            "a character was torn across a chunk boundary"
        );
        assert!(!out.is_empty());
    }

    #[test]
    fn a_long_target_shrinks_the_payload_budget() {
        let short = payload_budget("#a");
        let long = payload_budget("#a-very-long-channel-name-for-testing");
        assert!(long < short, "budget must account for the target length");
        assert!(short <= IRC_LINE_BYTES);
    }

    #[test]
    fn session_roundtrip_against_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut s = s;
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert!(line.starts_with("NICK phx"), "got: {line}");
            line.clear();
            r.read_line(&mut line).unwrap();
            assert!(line.starts_with("USER phx"), "got: {line}");
            s.write_all(b":srv 001 phx :Welcome\r\n").unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), "JOIN #ops");
            s.write_all(b"PING :tok123\r\n").unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), "PONG :tok123");
            s.write_all(b":stranger!u@h PRIVMSG #ops :ignore me\r\n")
                .unwrap();
            s.write_all(b":paulus!u@h PRIVMSG #ops :ping\r\n").unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            let got = line.trim_end();
            assert!(
                got.starts_with("PRIVMSG #ops :echo:[IRC paulus "),
                "got: {got}"
            );
            assert!(got.ends_with("] ping"), "got: {got}");
        });
        let irc = Irc {
            server: "127.0.0.1".into(),
            port: addr.port(),
            tls: false,
            nick: "phx".into(),
            channels: vec!["#ops".into()],
            allowed: crate::allowlist::Allowlist::new(&["paulus".to_string()]),
        };
        let mut handler = |_s: &str, t: &str| format!("echo:{t}");
        let mut last_seen = std::collections::HashMap::new();
        let err = irc.session(&mut handler, &mut last_seen).unwrap_err();
        assert!(err.contains("closed") || err.contains("read"), "{err}");
        server.join().unwrap();
    }
}
