use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::daemon::Shared;

use super::{gather, text};

pub const CONNECTIONS: usize = 8;
pub const HEAD_BYTES: usize = 8192;
pub const READ: Duration = Duration::from_secs(5);
pub const WRITE: Duration = Duration::from_secs(10);
pub const FRESH: Duration = Duration::from_secs(1);

static OPEN: AtomicUsize = AtomicUsize::new(0);
static CACHE: Mutex<Option<(Instant, Arc<String>)>> = Mutex::new(None);

struct Slot;

impl Slot {
    fn take() -> Option<Self> {
        OPEN.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |open| {
            (open < CONNECTIONS).then_some(open + 1)
        })
        .ok()
        .map(|_| Slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Head {
    Metrics,
    Elsewhere,
    Malformed,
}

fn head(stream: &mut TcpStream) -> Option<Head> {
    let deadline = Instant::now() + READ;
    let mut bytes = Vec::with_capacity(1024);
    let mut piece = [0u8; 1024];
    loop {
        if bytes.windows(4).any(|window| window == b"\r\n\r\n")
            || bytes.windows(2).any(|window| window == b"\n\n")
        {
            break;
        }
        if bytes.len() >= HEAD_BYTES {
            return Some(Head::Malformed);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return None;
        }
        let room = (HEAD_BYTES - bytes.len()).min(piece.len());
        match stream.read(&mut piece[..room]) {
            Ok(0) => return None,
            Ok(read) => bytes.extend_from_slice(&piece[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let Ok(line) = std::str::from_utf8(line) else {
        return Some(Head::Malformed);
    };
    let words: Vec<&str> = line.split_whitespace().collect();
    let [method, target, version] = words.as_slice() else {
        return Some(Head::Malformed);
    };
    if !version.starts_with("HTTP/1.") {
        return Some(Head::Malformed);
    }
    let path = target.split('?').next().unwrap_or_default();
    Some(if *method == "GET" && path == "/metrics" {
        Head::Metrics
    } else {
        Head::Elsewhere
    })
}

fn fresh(shared: &Shared) -> Arc<String> {
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, text)) = cache.as_ref()
        && at.elapsed() < FRESH
    {
        return Arc::clone(text);
    }
    let text = Arc::new(gather::render(shared));
    *cache = Some((Instant::now(), Arc::clone(&text)));
    text
}

fn answer(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn served(shared: &Shared, mut stream: TcpStream) -> io::Result<()> {
    stream.set_write_timeout(Some(WRITE))?;
    let plain = "text/plain; charset=utf-8";
    match head(&mut stream) {
        None => {}
        Some(Head::Metrics) => {
            let body = fresh(shared);
            answer(&mut stream, "200 OK", text::CONTENT_TYPE, &body)?;
        }
        Some(Head::Elsewhere) => answer(&mut stream, "404 Not Found", plain, "not found\n")?,
        Some(Head::Malformed) => answer(&mut stream, "400 Bad Request", plain, "bad request\n")?,
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

pub fn serve(shared: Arc<Shared>, listener: TcpListener) -> io::Result<()> {
    std::thread::Builder::new().spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let Some(slot) = Slot::take() else {
                drop(stream);
                continue;
            };
            let shared = Arc::clone(&shared);
            let _ = std::thread::Builder::new().spawn(move || {
                let _slot = slot;
                let _ = served(&shared, stream);
            });
        }
    })?;
    Ok(())
}
