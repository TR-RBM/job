use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::streams::Recorder;

mod client;
pub mod key;
mod messages;
pub use client::{attach, require_terminal};
pub use messages::{help, message};

pub const PROTOCOL: u32 = 1;

const MAX_FRAME: usize = 1024 * 1024;
const MAX_PENDING: usize = 2 * MAX_FRAME;
const MAX_INPUT: usize = 64 * 1024;
const MAX_CLIENTS: usize = 64;
const INPUT: u8 = 1;
const RESIZE: u8 = 2;
const OUTPUT: u8 = 3;
const EXIT: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

impl Default for Size {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

impl Size {
    pub fn valid(self) -> bool {
        self.rows > 0 && self.cols > 0 && self.rows <= 200 && self.cols <= 500
    }

    pub fn current() -> Self {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_row > 0
            && size.ws_col > 0
        {
            Self {
                rows: size.ws_row.clamp(1, 200),
                cols: size.ws_col.clamp(1, 500),
            }
        } else {
            Self::default()
        }
    }

    fn apply(self, fd: i32) -> io::Result<()> {
        let size = libc::winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct Pty {
    master: File,
    slave: Option<File>,
    listener: UnixListener,
    path: PathBuf,
    size: Size,
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Pty {
    pub fn open(path: &Path, size: Size) -> io::Result<Self> {
        if !size.valid() {
            return Err(io::Error::other(message("invalid terminal size", &[])));
        }
        let (mut master, mut slave) = (-1, -1);
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        nonblocking(master.as_raw_fd())?;
        size.apply(master.as_raw_fd())?;
        let listener = UnixListener::bind(path)?;
        crate::service::access::shared().share_socket(path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            master,
            slave: Some(slave),
            listener,
            path: path.to_owned(),
            size,
        })
    }

    pub fn configure(&self, command: &mut Command) -> io::Result<()> {
        let slave = self
            .slave
            .as_ref()
            .ok_or_else(|| io::Error::other(message("terminal slave is closed", &[])))?;
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?))
            .env("TERM", "xterm-256color");
        Ok(())
    }

    pub fn serve(mut self, log: Recorder, ended: Receiver<i32>) -> crate::streams::Outcome {
        let log = OutputLog::start(log);
        drop(self.slave.take());
        let mut parser = vt100::Parser::new_with_callbacks(
            self.size.rows,
            self.size.cols,
            0,
            Replies::default(),
        );
        let mut clients: Vec<Peer> = Vec::new();
        let mut input = VecDeque::new();
        let mut ending = None;
        let mut closed = false;
        let mut buffer = [0u8; 8192];
        loop {
            if ending.is_none()
                && let Ok(code) = ended.try_recv()
            {
                ending = Some((code, Instant::now() + Duration::from_secs(1)));
            }
            if ending.is_none() {
                for _ in 0..MAX_CLIENTS {
                    let Ok((stream, _)) = self.listener.accept() else {
                        break;
                    };
                    if clients.len() < MAX_CLIENTS
                        && own_user(&stream)
                        && stream.set_nonblocking(true).is_ok()
                    {
                        clients.push(Peer::new(stream));
                    }
                }
            }
            for peer in &mut clients {
                if peer.read().is_err() {
                    peer.dead = true;
                    continue;
                }
                for _ in 0..16 {
                    let message = match take_frame(&mut peer.rx) {
                        Ok(Some(message)) => message,
                        Ok(None) => break,
                        Err(_) => {
                            peer.dead = true;
                            break;
                        }
                    };
                    match message {
                        (RESIZE, bytes) if bytes.len() == 4 => {
                            let size = Size {
                                rows: u16::from_be_bytes([bytes[0], bytes[1]]),
                                cols: u16::from_be_bytes([bytes[2], bytes[3]]),
                            };
                            if !size.valid() {
                                peer.dead = true;
                                break;
                            }
                            if peer.size.is_none() {
                                peer.send(OUTPUT, &parser.screen().state_formatted());
                            }
                            peer.size = Some(size);
                        }
                        (INPUT, bytes)
                            if peer.size.is_some()
                                && bytes.len() <= 8192
                                && input.len() + bytes.len() <= MAX_INPUT =>
                        {
                            input.extend(bytes)
                        }
                        _ => {
                            peer.dead = true;
                            break;
                        }
                    }
                }
            }
            clients.retain(|peer| !peer.dead);
            let sizes: Vec<_> = clients.iter().filter_map(|peer| peer.size).collect();
            if !sizes.is_empty() {
                let size = Size {
                    rows: sizes.iter().map(|s| s.rows).min().unwrap(),
                    cols: sizes.iter().map(|s| s.cols).min().unwrap(),
                };
                if size != self.size && size.apply(self.master.as_raw_fd()).is_ok() {
                    self.size = size;
                    parser.screen_mut().set_size(size.rows, size.cols);
                    broadcast(&mut clients, &parser.screen().state_formatted());
                }
            }
            if !input.is_empty() {
                match self.master.write(input.make_contiguous()) {
                    Ok(n) => {
                        input.drain(..n);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        input.clear();
                    }
                }
            }
            match self.master.read(&mut buffer) {
                Ok(0) => closed = true,
                Ok(n) => {
                    log.write(&buffer[..n]);
                    let previous = parser.screen().clone();
                    parser.process(&buffer[..n]);
                    let replies = std::mem::take(&mut parser.callbacks_mut().bytes);
                    if input.len() + replies.len() <= MAX_INPUT {
                        input.extend(replies);
                    }
                    broadcast(&mut clients, &parser.screen().state_diff(&previous));
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => closed = true,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => closed = true,
            }
            for peer in &mut clients {
                if peer.flush().is_err() {
                    peer.dead = true;
                }
            }
            clients.retain(|peer| {
                !peer.dead
                    && (peer.size.is_some() || peer.connected.elapsed() < Duration::from_secs(10))
            });
            if let Some((code, deadline)) = ending
                && (closed || Instant::now() >= deadline)
            {
                for peer in &mut clients {
                    peer.send(EXIT, &code.to_be_bytes());
                }
                let deadline = Instant::now() + Duration::from_millis(200);
                while Instant::now() < deadline
                    && clients.iter().any(|peer| !peer.tx.is_empty() && !peer.dead)
                {
                    for peer in &mut clients {
                        if peer.flush().is_err() {
                            peer.dead = true;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                break;
            }
            let mut polls = vec![libc::pollfd {
                fd: if closed { -1 } else { self.master.as_raw_fd() },
                events: if closed { 0 } else { libc::POLLIN },
                revents: 0,
            }];
            polls.push(libc::pollfd {
                fd: self.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
            polls.extend(clients.iter().map(|p| libc::pollfd {
                fd: p.stream.as_raw_fd(),
                events: libc::POLLIN | if p.tx.is_empty() { 0 } else { libc::POLLOUT },
                revents: 0,
            }));
            unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as _, 20) };
        }
        log.finish()
    }
}

struct OutputLog {
    sender: std::sync::mpsc::SyncSender<Vec<u8>>,
    omitted: std::sync::Arc<std::sync::atomic::AtomicU64>,
    writer: std::thread::JoinHandle<crate::streams::Outcome>,
}

impl OutputLog {
    fn start(mut log: Recorder) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
        let omitted = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let losses = omitted.clone();
        let writer = std::thread::spawn(move || {
            for bytes in receiver {
                if log.write_all(&bytes).is_err() {
                    losses.fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let lost = losses.load(std::sync::atomic::Ordering::Relaxed);
            if lost > 0 {
                log.omit(lost);
                let _ = log.write_all(message("\n[job: terminal log omitted {lost} bytes because storage could not keep up]\n", &[("lost", lost.to_string())]).as_bytes());
            }
            log.finish()
        });
        Self {
            sender,
            omitted,
            writer,
        }
    }

    fn write(&self, bytes: &[u8]) {
        if self.sender.try_send(bytes.to_vec()).is_err() {
            self.omitted
                .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn finish(self) -> crate::streams::Outcome {
        drop(self.sender);
        self.writer
            .join()
            .unwrap_or_else(|_| crate::streams::Outcome::incomplete())
    }
}

fn own_user(stream: &UnixStream) -> bool {
    crate::service::access::shared().admits(stream).is_some()
}

#[derive(Default)]
struct Replies {
    bytes: Vec<u8>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if self.bytes.len() > MAX_INPUT {
            return;
        }
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            (None, 'n', 5) => self.bytes.extend(b"\x1b[0n"),
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.bytes
                    .extend(format!("\x1b[{};{}R", row + 1, col + 1).bytes());
            }
            (None, 'c', 0) => self.bytes.extend(b"\x1b[?1;2c"),
            (Some(b'>'), 'c', 0) => self.bytes.extend(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

struct Peer {
    stream: UnixStream,
    size: Option<Size>,
    rx: Vec<u8>,
    tx: VecDeque<u8>,
    dead: bool,
    connected: Instant,
}

impl Peer {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            size: None,
            rx: Vec::new(),
            tx: VecDeque::new(),
            dead: false,
            connected: Instant::now(),
        }
    }

    fn send(&mut self, kind: u8, bytes: &[u8]) {
        let bytes = frame(kind, bytes);
        if bytes.len() > MAX_FRAME + 5 || self.tx.len() + bytes.len() > MAX_PENDING {
            self.dead = true;
        } else {
            self.tx.extend(bytes);
        }
    }

    fn read(&mut self) -> io::Result<()> {
        let mut bytes = [0; 8192];
        match self.stream.read(&mut bytes) {
            Ok(0) => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(n) if self.rx.len() + n <= MAX_INPUT => {
                self.rx.extend_from_slice(&bytes[..n]);
                Ok(())
            }
            Ok(_) => Err(io::Error::other(message(
                "terminal input buffer exceeded",
                &[],
            ))),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.tx.is_empty() {
            return Ok(());
        }
        match self.stream.write(self.tx.make_contiguous()) {
            Ok(0) => Err(io::Error::from(io::ErrorKind::WriteZero)),
            Ok(n) => {
                self.tx.drain(..n);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn broadcast(clients: &mut [Peer], bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    for peer in clients.iter_mut().filter(|peer| peer.size.is_some()) {
        peer.send(OUTPUT, bytes);
    }
}

fn frame(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + bytes.len());
    frame.push(kind);
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    frame
}

fn take_frame(bytes: &mut Vec<u8>) -> io::Result<Option<(u8, Vec<u8>)>> {
    if bytes.len() < 5 {
        return Ok(None);
    }
    let len = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::other(message("terminal frame too large", &[])));
    }
    if bytes.len() < 5 + len {
        return Ok(None);
    }
    let kind = bytes[0];
    let body = bytes[5..5 + len].to_vec();
    bytes.drain(..5 + len);
    Ok(Some((kind, body)))
}

pub fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\t' {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_snapshot_restores_cursor_and_contents() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[2J\x1b[10;12Hhello\x1b[4;5H");
        let mut restored = vt100::Parser::new(24, 80, 0);
        restored.process(&parser.screen().state_formatted());
        assert_eq!(restored.screen().contents(), parser.screen().contents());
        assert_eq!(restored.screen().cursor_position(), (3, 4));
    }

    #[test]
    fn cursor_queries_are_answered_once_by_the_server() {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, Replies::default());
        parser.process(b"\x1b[3;7H\x1b[6n");
        assert_eq!(parser.callbacks().bytes, b"\x1b[3;7R");
        assert!(
            !parser
                .screen()
                .state_formatted()
                .windows(4)
                .any(|s| s == b"\x1b[6n")
        );
    }

    #[test]
    fn output_does_not_forward_clipboard_requests() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"safe\x1b]52;c;YWJj\x07");
        assert_eq!(parser.screen().contents(), "safe");
        assert!(
            !parser
                .screen()
                .state_formatted()
                .windows(3)
                .any(|s| s == b"]52")
        );
    }

    #[test]
    fn oversized_frames_are_rejected_before_allocation() {
        let mut bytes = vec![INPUT];
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(take_frame(&mut bytes).is_err());
    }

    #[test]
    fn slow_client_is_disconnected_when_its_buffer_is_full() {
        let (stream, _other) = UnixStream::pair().unwrap();
        let mut peer = Peer::new(stream);
        peer.tx.resize(MAX_PENDING, 0);
        peer.send(OUTPUT, b"more");
        assert!(peer.dead);
        assert_eq!(peer.tx.len(), MAX_PENDING);
    }
}
