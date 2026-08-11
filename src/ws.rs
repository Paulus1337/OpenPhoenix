use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::media::{b64_decode, b64_encode};
use crate::security::sha1;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const MAX_FRAME: usize = 16 * 1024 * 1024;
const MAX_CONTROL_PAYLOAD: usize = 125;

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BIN: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xa;

#[derive(Debug, PartialEq)]
pub enum WsMsg {
    Text(String),
    Binary(Vec<u8>),

    Close(u16),
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn sock(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(t) => &t.sock,
        }
    }
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

pub fn urandom(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        return buf;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = nanos
        ^ (u64::from(std::process::id()) << 32)
        ^ (&buf as *const _ as u64)
        ^ 0x9e37_79b9_7f4a_7c15;
    for b in buf.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state >> 24) as u8;
    }
    buf
}

pub fn server_accept(key: &str) -> String {
    b64_encode(&sha1(format!("{key}{WS_GUID}").as_bytes()))
}

pub fn write_frame_server(w: &mut impl Write, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(10);
    head.push(0x80 | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        head.push(len as u8);
    } else if len <= 0xffff {
        head.push(126);
        head.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        head.push(127);
        head.extend_from_slice(&(len as u64).to_be_bytes());
    }
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

pub fn write_frame(w: &mut impl Write, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(14);
    head.push(0x80 | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        head.push(0x80 | len as u8);
    } else if len <= 0xffff {
        head.push(0x80 | 126);
        head.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        head.push(0x80 | 127);
        head.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = urandom(4);
    head.extend_from_slice(&mask);
    w.write_all(&head)?;
    let mut masked = payload.to_vec();
    for (i, b) in masked.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    w.write_all(&masked)?;
    w.flush()
}

pub fn read_raw_frame(r: &mut impl Read) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let fin = h[0] & 0x80 != 0;
    let opcode = h[0] & 0x0f;
    let masked = h[1] & 0x80 != 0;
    let mut len = (h[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        r.read_exact(&mut ext)?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        r.read_exact(&mut ext)?;
        let l = u64::from_be_bytes(ext);
        if l > MAX_FRAME as u64 {
            return Err(std::io::Error::other("frame too large"));
        }
        len = l as usize;
    }
    if len > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mask = if masked {
        let mut m = [0u8; 4];
        r.read_exact(&mut m)?;
        Some(m)
    } else {
        None
    };
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    Ok((fin, opcode, payload))
}

pub struct WsClient {
    stream: Stream,
}

impl std::fmt::Debug for WsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let peer = self.stream.sock().peer_addr();
        f.debug_struct("WsClient").field("peer", &peer).finish()
    }
}

impl WsClient {
    pub fn connect(url: &str) -> Result<WsClient, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("ws://") {
            (false, r)
        } else {
            return Err(format!("not a websocket url: {url}"));
        };
        let (hostport, path) = match rest.split_once('/') {
            Some((h, p)) => (h, format!("/{p}")),
            None => (rest, "/".to_string()),
        };
        let default_port = if tls { 443 } else { 80 };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port")?),
            None => (hostport, default_port),
        };
        let tcp = TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
        tcp.set_nodelay(true).ok();
        let mut stream = if tls {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|e| e.to_string())?;
            let conn =
                rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
            Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
        } else {
            Stream::Plain(tcp)
        };

        let key = b64_encode(&urandom(16));
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;

        let mut head = Vec::with_capacity(512);
        let mut one = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 16384 {
                return Err("oversized upgrade response".into());
            }
            stream.read_exact(&mut one).map_err(|e| e.to_string())?;
            head.push(one[0]);
        }
        let head = String::from_utf8_lossy(&head);
        if !head.starts_with("HTTP/1.1 101") {
            let line = head.lines().next().unwrap_or("");
            return Err(format!("upgrade refused: {line}"));
        }
        let want = b64_encode(&sha1(format!("{key}{WS_GUID}").as_bytes()));
        let got = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("sec-websocket-accept")
                    .then(|| v.trim().to_string())
            })
            .unwrap_or_default();
        if got != want {
            return Err("bad Sec-WebSocket-Accept".into());
        }
        Ok(WsClient { stream })
    }

    pub fn set_read_timeout(&mut self, d: Option<Duration>) -> Result<(), String> {
        self.stream
            .sock()
            .set_read_timeout(d)
            .map_err(|e| e.to_string())
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), String> {
        write_frame(&mut self.stream, OP_TEXT, text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn send_ping(&mut self) -> Result<(), String> {
        write_frame(&mut self.stream, OP_PING, b"").map_err(|e| e.to_string())
    }

    pub fn send_close(&mut self, code: u16) -> Result<(), String> {
        write_frame(&mut self.stream, OP_CLOSE, &code.to_be_bytes()).map_err(|e| e.to_string())
    }

    pub fn next(&mut self) -> Result<Option<WsMsg>, String> {
        let mut assembling: Option<(u8, Vec<u8>)> = None;
        loop {
            let (fin, opcode, payload) = match read_raw_frame(&mut self.stream) {
                Ok(f) => f,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(e.to_string()),
            };
            if matches!(opcode, OP_PING | OP_PONG | OP_CLOSE) {
                if !fin {
                    return Err("control frames must not be fragmented".into());
                }
                if payload.len() > MAX_CONTROL_PAYLOAD {
                    return Err("control frame payload over 125 bytes".into());
                }
            }
            match opcode {
                OP_PING => {
                    write_frame(&mut self.stream, OP_PONG, &payload).map_err(|e| e.to_string())?;
                }
                OP_PONG => {}
                OP_CLOSE => {
                    let code = if payload.len() >= 2 {
                        u16::from_be_bytes([payload[0], payload[1]])
                    } else {
                        1005
                    };
                    return Ok(Some(WsMsg::Close(code)));
                }
                OP_TEXT | OP_BIN => {
                    if assembling.is_some() {
                        return Err("data frame arrived while a fragmented message was open".into());
                    }
                    if fin {
                        return Ok(Some(finish(opcode, payload)?));
                    }
                    assembling = Some((opcode, payload));
                }
                OP_CONT => {
                    let Some((op, mut buf)) = assembling.take() else {
                        return Err("continuation without start".into());
                    };
                    if buf.len() + payload.len() > MAX_FRAME {
                        return Err("fragmented message too large".into());
                    }
                    buf.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(finish(op, buf)?));
                    }
                    assembling = Some((op, buf));
                }
                other => return Err(format!("unknown opcode {other}")),
            }
        }
    }
}

fn finish(opcode: u8, payload: Vec<u8>) -> Result<WsMsg, String> {
    if opcode == OP_TEXT {
        Ok(WsMsg::Text(
            String::from_utf8(payload).map_err(|_| "invalid utf8 in text frame")?,
        ))
    } else {
        Ok(WsMsg::Binary(payload))
    }
}

#[allow(unused_imports)]
use b64_decode as _b64_decode_reexport_check;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;

    #[test]
    fn urandom_returns_varied_bytes_of_the_requested_length() {
        for n in [1usize, 4, 16, 64] {
            assert_eq!(urandom(n).len(), n);
        }
        let a = urandom(32);
        let b = urandom(32);
        assert_ne!(a, b, "two draws must differ");
        assert!(
            a.iter().collect::<std::collections::HashSet<_>>().len() > 4,
            "a 32 byte draw must not be nearly constant: {a:?}"
        );
    }

    #[test]
    fn frame_roundtrip_masked() {
        let mut buf = Vec::new();
        write_frame(&mut buf, OP_TEXT, b"hello phoenix").unwrap();

        assert!(buf[1] & 0x80 != 0);
        let (fin, op, payload) = read_raw_frame(&mut Cursor::new(&buf)).unwrap();
        assert!(fin);
        assert_eq!(op, OP_TEXT);
        assert_eq!(payload, b"hello phoenix");
    }

    #[test]
    fn frame_length_forms() {
        let mid = vec![0x42u8; 300];
        let mut buf = Vec::new();
        write_frame(&mut buf, OP_BIN, &mid).unwrap();
        assert_eq!(buf[1] & 0x7f, 126);
        let (_, _, payload) = read_raw_frame(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(payload.len(), 300);

        let big = vec![0x0fu8; 70_000];
        let mut buf = Vec::new();
        write_frame(&mut buf, OP_BIN, &big).unwrap();
        assert_eq!(buf[1] & 0x7f, 127);
        let (_, _, payload) = read_raw_frame(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(payload.len(), 70_000);
    }

    #[test]
    fn oversized_control_payload_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut frame = vec![0x80 | OP_PING, 126];
            frame.extend_from_slice(&200u16.to_be_bytes());
            frame.extend_from_slice(&[0x41u8; 200]);
            let _ = s.write_all(&frame);
            let _ = s.flush();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let mut c = WsClient {
            stream: Stream::Plain(TcpStream::connect(addr).unwrap()),
        };
        let err = c.next().expect_err("a 200-byte ping must be refused");
        assert!(err.contains("control frame payload"), "{err}");
    }

    #[test]
    fn fragmented_control_frame_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.write_all(&[OP_PING, 0x02, 0x41, 0x42]);
            let _ = s.flush();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let mut c = WsClient {
            stream: Stream::Plain(TcpStream::connect(addr).unwrap()),
        };
        let err = c.next().expect_err("a fragmented ping must be refused");
        assert!(err.contains("must not be fragmented"), "{err}");
    }

    #[test]
    fn interleaved_data_frame_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.write_all(&[OP_TEXT, 0x01, b'a']);
            let _ = s.write_all(&[0x80 | OP_TEXT, 0x01, b'b']);
            let _ = s.flush();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let mut c = WsClient {
            stream: Stream::Plain(TcpStream::connect(addr).unwrap()),
        };
        let err = c
            .next()
            .expect_err("a new data frame mid-fragment must be refused");
        assert!(err.contains("fragmented message was open"), "{err}");
    }

    fn mock_ws_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut one = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut one).unwrap();
                head.push(one[0]);
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let key = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("sec-websocket-key")
                        .then(|| v.trim().to_string())
                })
                .unwrap();
            let accept = b64_encode(&sha1(format!("{key}{WS_GUID}").as_bytes()));
            write!(
                s,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            )
            .unwrap();

            let payload = b"welcome";
            let mut frame = vec![0x80 | OP_TEXT, payload.len() as u8];
            frame.extend_from_slice(payload);
            s.write_all(&frame).unwrap();

            let (_, op, echo) = read_raw_frame(&mut s).unwrap();
            let mut frame = vec![0x80 | op, echo.len() as u8];
            frame.extend_from_slice(&echo);
            s.write_all(&frame).unwrap();
        });
        format!("ws://{addr}/socket")
    }

    #[test]
    fn connect_handshake_and_messages() {
        let url = mock_ws_server();
        let mut ws = WsClient::connect(&url).unwrap();
        assert_eq!(ws.next().unwrap(), Some(WsMsg::Text("welcome".into())));
        ws.send_text("ping back").unwrap();
        assert_eq!(ws.next().unwrap(), Some(WsMsg::Text("ping back".into())));
    }

    #[test]
    fn connect_rejects_bad_accept() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut one = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut one).unwrap();
                head.push(one[0]);
            }
            let _ = write!(
                s,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
Connection: Upgrade\r\nSec-WebSocket-Accept: bogus\r\n\r\n"
            );
        });
        let err = WsClient::connect(&format!("ws://{addr}/")).unwrap_err();
        assert!(err.contains("Sec-WebSocket-Accept"), "got: {err}");
    }

    #[test]
    fn connect_rejects_non_101() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut one = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut one).unwrap();
                head.push(one[0]);
            }
            let _ = write!(s, "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        });
        let err = WsClient::connect(&format!("ws://{addr}/")).unwrap_err();
        assert!(err.contains("upgrade refused"), "got: {err}");
    }
}
