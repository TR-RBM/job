use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use super::message;

pub const SOCKET: &str = "stdin.sock";
pub const GRACE: Duration = Duration::from_secs(10);
const STEP: Duration = Duration::from_millis(25);

pub fn check(declared: &crate::model::Declared) -> Result<(), String> {
    if !declared.stdin {
        return Ok(());
    }
    if declared.terminal.is_some() {
        return Err(message(
            "--stdin goes with pipe mode; with --pty the terminal is the input of the Job",
        ));
    }
    if declared.on.is_some() {
        return Err(message(
            "--stdin is not carried to another host; leave out --on",
        ));
    }
    Ok(())
}

pub fn wanted(job: &crate::model::Job) -> bool {
    job.spec.declared.stdin
        && job.spec.declared.terminal.is_none()
        && job.spec.declared.on.is_none()
}

pub fn listen(directory: &Path) -> std::io::Result<UnixListener> {
    let path = directory.join(SOCKET);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    crate::service::access::shared().share_socket(&path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn feed(listener: UnixListener, directory: &Path, mut sink: std::process::ChildStdin) {
    let path = directory.join(SOCKET);
    std::thread::spawn(move || {
        let started = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= GRACE {
                        break None;
                    }
                    std::thread::sleep(STEP);
                }
                Err(_) => break None,
            }
        };
        drop(listener);
        let _ = std::fs::remove_file(&path);
        if let Some(mut stream) = stream
            && stream.set_nonblocking(false).is_ok()
        {
            let mut bytes = [0u8; 8192];
            loop {
                match stream.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(count) => {
                        if sink.write_all(&bytes[..count]).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
        drop(sink);
    });
}

pub fn pump(id: u64) {
    let path = crate::store::Store {
        root: crate::store::Store::default_root(),
    }
    .job_dir(id)
    .join(SOCKET);
    std::thread::spawn(move || {
        let mut stream = loop {
            match UnixStream::connect(&path) {
                Ok(stream) => break stream,
                Err(_) => std::thread::sleep(STEP),
            }
        };
        let mut bytes = [0u8; 8192];
        let mut input = std::io::stdin().lock();
        loop {
            match input.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => {
                    if stream.write_all(&bytes[..count]).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = stream.shutdown(std::net::Shutdown::Write);
    });
}
