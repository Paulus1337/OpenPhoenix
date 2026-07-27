use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let port: u16 = env::args().nth(1).unwrap_or_else(|| "18999".into()).parse().unwrap();
    let listener = TcpListener::bind(("0.0.0.0", port)).unwrap();
    let body = r#"{"choices":[{"message":{"content":"SMOKE-REPLY"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    for stream in listener.incoming() {
        let mut s = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut buf = [0u8; 65536];
        let mut read = 0usize;
        let (mut need, mut head_end) = (0usize, 0usize);
        loop {
            match s.read(&mut buf[read..]) {
                Ok(0) => break,
                Ok(n) => read += n,
                Err(_) => break,
            }
            if head_end == 0 {
                if let Some(p) = find(&buf[..read], b"\r\n\r\n") {
                    head_end = p + 4;
                    let head = String::from_utf8_lossy(&buf[..p]);
                    for line in head.lines() {
                        let l = line.to_ascii_lowercase();
                        if let Some(v) = l.strip_prefix("content-length:") {
                            need = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
            if head_end > 0 && read >= head_end + need {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = s.write_all(resp.as_bytes());
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
