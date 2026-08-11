use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct Req {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct Resp {
    pub code: u16,
    pub ctype: String,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(code: u16, v: &serde_json::Value) -> Resp {
        Resp {
            code,
            ctype: "application/json".to_string(),
            body: v.to_string().into_bytes(),
        }
    }

    pub fn bytes(code: u16, ctype: &str, body: Vec<u8>) -> Resp {
        Resp {
            code,
            ctype: ctype.to_string(),
            body,
        }
    }
}

pub type Handler = Arc<dyn Fn(&Req) -> Resp + Send + Sync>;

pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub fn start(handler: Handler) -> Result<Server, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::spawn(move || {
        for conn in listener.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = conn else { continue };
            let h = handler.clone();
            std::thread::spawn(move || handle(stream, h));
        }
    });
    Ok(Server {
        port,
        stop,
        thread: Some(thread),
    })
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn handle(stream: TcpStream, h: Handler) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(rs) = stream.try_clone() else { return };
    let mut reader = BufReader::new(rs);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    let mut clen = 0usize;
    loop {
        let mut hl = String::new();
        if reader.read_line(&mut hl).unwrap_or(0) == 0 {
            break;
        }
        let t = hl.trim();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                clen = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; clen.min(10_000_000)];
    if !body.is_empty() && reader.read_exact(&mut body).is_err() {
        return;
    }
    let req = Req {
        method,
        path,
        headers,
        body,
    };
    let resp = h(&req);
    let mut w = stream;
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.code,
        status_text(resp.code),
        resp.ctype,
        resp.body.len()
    );
    let _ = w.write_all(head.as_bytes());
    let _ = w.write_all(&resp.body);
}
