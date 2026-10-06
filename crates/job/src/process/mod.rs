mod messages;
pub use messages::message;

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub struct Handle {
    fd: OwnedFd,
    pub pid: i32,
    pub start_ticks: u64,
}

impl Handle {
    pub fn open(pid: i32, expected_ticks: Option<u64>) -> io::Result<Self> {
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        let stat =
            crate::procs::stat_of(pid).ok_or_else(|| io::Error::from_raw_os_error(libc::ESRCH))?;
        if expected_ticks.is_some_and(|expected| expected != stat.start_ticks) {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(Self {
            fd,
            pid,
            start_ticks: stat.start_ticks,
        })
    }

    pub fn signal(&self, signal: i32) -> io::Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn alive(&self) -> io::Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = unsafe { libc::poll(&mut poll, 1, 0) };
            if result >= 0 {
                return Ok(result == 0);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

pub fn boot_id() -> io::Result<String> {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let id = id.trim();
    if id.is_empty() {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(id.to_owned())
}

pub fn boot_token() -> io::Result<[u8; 16]> {
    u128::from_str_radix(&boot_id()?.replace('-', ""), 16)
        .map(u128::to_be_bytes)
        .map_err(io::Error::other)
}

pub fn missing_boot_identity() -> String {
    message(
        "running legacy Jobs have no boot identity; drain them with the previous version before upgrading",
        &[],
    )
}

pub fn parse_signal(value: &str) -> Result<i32, String> {
    let name = value.strip_prefix("SIG").unwrap_or(value);
    let number = match name {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "TERM" => libc::SIGTERM,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "STOP" => libc::SIGSTOP,
        "CONT" => libc::SIGCONT,
        "TSTP" => libc::SIGTSTP,
        "WINCH" => libc::SIGWINCH,
        _ => name.parse().map_err(|_| message("invalid signal", &[]))?,
    };
    if (0..=libc::SIGRTMAX()).contains(&number) {
        Ok(number)
    } else {
        Err(message("invalid signal", &[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn a_handle_detects_exit_and_cannot_signal_a_subsequent_process() {
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();
        let handle = Handle::open(child.id() as i32, None).unwrap();
        assert!(handle.alive().unwrap());
        handle.signal(libc::SIGTERM).unwrap();
        child.wait().unwrap();
        assert!(!handle.alive().unwrap());
        for _ in 0..32 {
            assert!(Command::new("true").status().unwrap().success());
        }
        assert_eq!(
            handle.signal(libc::SIGTERM).unwrap_err().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn recovery_refuses_a_different_start_identity() {
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();
        let handle = Handle::open(child.id() as i32, None).unwrap();
        let reopened = Handle::open(handle.pid, Some(handle.start_ticks));
        assert!(reopened.is_ok());
        assert!(Handle::open(handle.pid, Some(handle.start_ticks.wrapping_add(1))).is_err());
        handle.signal(libc::SIGTERM).unwrap();
        child.wait().unwrap();
    }
}
