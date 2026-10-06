use super::*;
use crate::model::{Request, Response};
use std::sync::atomic::{AtomicI32, Ordering};

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

extern "C" fn interrupted(signal: i32) {
    INTERRUPTED.store(signal, Ordering::Relaxed);
}

struct LocalTerminal {
    original: libc::termios,
    signals: Vec<(i32, libc::sigaction)>,
}

impl LocalTerminal {
    fn enter() -> io::Result<Self> {
        require_terminal().map_err(io::Error::other)?;
        let mut original = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut original) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut terminal = Self {
            original,
            signals: Vec::new(),
        };
        INTERRUPTED.store(0, Ordering::Relaxed);
        for signal in [
            libc::SIGHUP,
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTSTP,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old = unsafe { std::mem::zeroed() };
            action.sa_sigaction = interrupted as *const () as usize;
            if unsafe { libc::sigaction(signal, &action, &mut old) } < 0 {
                return Err(io::Error::last_os_error());
            }
            terminal.signals.push((signal, old));
        }
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        io::stdout().write_all(b"\x1b[?1049h\x1b[2J\x1b[H")?;
        io::stdout().flush()?;
        Ok(terminal)
    }
}

impl Drop for LocalTerminal {
    fn drop(&mut self) {
        let _ = io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1049l");
        let _ = io::stdout().flush();
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.original) };
        for (signal, old) in &self.signals {
            unsafe { libc::sigaction(*signal, old, std::ptr::null_mut()) };
        }
    }
}

pub fn require_terminal() -> Result<(), String> {
    if unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 } {
        Ok(())
    } else {
        Err(message(
            "attach needs a terminal on stdin and stdout; use job submit --pty for a detached terminal",
            &[],
        ))
    }
}

fn connect(socket: &Path, id: u64) -> Result<UnixStream, String> {
    loop {
        let response =
            crate::client::call(socket, &Request::Attach { id }).map_err(|e| e.to_string())?;
        match response {
            Response::Attached {
                path,
                terminal_protocol,
            } if terminal_protocol == PROTOCOL => match UnixStream::connect(path) {
                Ok(stream) => return Ok(stream),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(e) => return Err(e.to_string()),
            },
            Response::StillRunning { .. } => {}
            Response::Finished { .. } => {
                return Err(message(
                    "job {id} has finished; use job log {id} full",
                    &[("id", id.to_string())],
                ));
            }
            Response::Error { message } => return Err(message),
            other => {
                return Err(message(
                    "unexpected attach response: {error}",
                    &[("error", format!("{other:?}"))],
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Escape {
    key: Option<u8>,
    pending: bool,
}

impl Escape {
    fn new(key: key::DetachKey) -> Self {
        Self {
            key: key.byte(),
            pending: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let Some(key) = self.key else {
            return (bytes.to_vec(), false);
        };
        let mut input = Vec::new();
        for &byte in bytes {
            if self.pending {
                self.pending = false;
                match byte {
                    b'd' => return (input, true),
                    other if other == key => input.push(key),
                    other => {
                        input.push(key);
                        input.push(other);
                    }
                }
            } else if byte == key {
                self.pending = true;
            } else {
                input.push(byte);
            }
        }
        (input, false)
    }
}

fn configured(socket: &Path) -> Option<key::DetachKey> {
    match crate::client::call(socket, &Request::Config { reload: false }) {
        Ok(Response::Configuration { effective }) => effective.config.terminal.detach_key,
        _ => None,
    }
}

pub fn attach(socket: &Path, id: u64, detach_key: Option<&str>) -> Result<u8, String> {
    let key = key::resolve(detach_key, || configured(socket))?;
    require_terminal()?;
    eprintln!(
        "{}",
        match key.label() {
            Some(label) => message(
                "[job] terminal {id}: shared input; detach with {key} then d",
                &[("id", id.to_string()), ("key", label)]
            ),
            None => message(
                "[job] terminal {id}: shared input; no detach key is set, close this terminal to detach",
                &[("id", id.to_string())]
            ),
        }
    );
    let mut stream = connect(socket, id)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    let terminal = LocalTerminal::enter().map_err(|e| e.to_string())?;
    let result = interact(&mut stream, key);
    drop(terminal);
    match result {
        Ok(Some(code)) => Ok(code),
        Ok(None) => {
            eprintln!(
                "{}",
                message(
                    "[job] detached from {id}; attach again with: job attach {id}",
                    &[("id", id.to_string())]
                )
            );
            Ok(0)
        }
        Err(e) => Err(message(
            "terminal connection ended: {error}; reconnect with job attach {id}",
            &[("id", id.to_string()), ("error", e.to_string())],
        )),
    }
}

fn interact(stream: &mut UnixStream, key: key::DetachKey) -> io::Result<Option<u8>> {
    let mut size = Size::current();
    send_size(stream, size)?;
    let mut rx = Vec::new();
    let mut escape = Escape::new(key);
    let mut bytes = [0; 8192];
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            return Ok(None);
        }
        let current = Size::current();
        if current != size {
            size = current;
            send_size(stream, size)?;
        }
        let mut polls = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if unsafe { libc::poll(polls.as_mut_ptr(), 2, 100) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if polls[0].revents != 0 {
            let n = io::stdin().read(&mut bytes)?;
            if n == 0 {
                return Ok(None);
            }
            let (input, detach) = escape.feed(&bytes[..n]);
            if !input.is_empty() {
                stream.write_all(&frame(INPUT, &input))?;
            }
            if detach {
                return Ok(None);
            }
        }
        if polls[1].revents != 0 {
            let n = stream.read(&mut bytes)?;
            if n == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            rx.extend_from_slice(&bytes[..n]);
            while let Some((kind, body)) = take_frame(&mut rx)? {
                match kind {
                    OUTPUT => {
                        io::stdout().write_all(&body)?;
                        io::stdout().flush()?;
                    }
                    EXIT if body.len() == 4 => {
                        return Ok(Some(
                            i32::from_be_bytes(body.try_into().unwrap()).clamp(0, 255) as u8,
                        ));
                    }
                    _ => return Err(io::Error::other(message("invalid terminal response", &[]))),
                }
            }
        }
    }
}

fn send_size(stream: &mut UnixStream, size: Size) -> io::Result<()> {
    let bytes = [size.rows.to_be_bytes(), size.cols.to_be_bytes()].concat();
    stream.write_all(&frame(RESIZE, &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_can_span_reads() {
        let mut escape = Escape::new(Default::default());
        assert_eq!(escape.feed(b"hello\x1d"), (b"hello".to_vec(), false));
        assert_eq!(escape.feed(b"dignored"), (vec![], true));
    }

    #[test]
    fn doubled_escape_is_literal() {
        assert_eq!(
            Escape::new(Default::default()).feed(b"\x1d\x1d"),
            (vec![0x1d], false)
        );
    }

    #[test]
    fn unknown_escape_preserves_input() {
        assert_eq!(
            Escape::new(Default::default()).feed(b"\x1dx"),
            (b"\x1dx".to_vec(), false)
        );
    }
}
