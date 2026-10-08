use super::{message, text};
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::OnceLock;

const INIT: i32 = 1;
const COMMAND: i32 = 2;
const EXITED: i32 = 3;
const RECORD: usize = 2 * std::mem::size_of::<i32>();
const FAULTS: [libc::c_int; 6] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGILL,
    libc::SIGTRAP,
    libc::SIGSYS,
];
const FAILED: libc::c_int = 70;
const CLOSE_FALLBACK: RawFd = 65536;
const PROBE_STEPS: [&str; 6] = [
    "create the user namespace",
    "write the user namespace identity maps",
    "create the requested namespaces",
    "start the init of the pid namespace",
    "make mounts private",
    "mount a fresh /proc",
];

fn send(fd: RawFd, tag: i32, value: i32) {
    let record = [tag, value];
    unsafe { libc::send(fd, record.as_ptr().cast(), RECORD, libc::MSG_NOSIGNAL) };
}

fn receive(fd: RawFd) -> Option<(i32, i32)> {
    let mut record = [0i32; 2];
    let mut filled = 0;
    while filled < RECORD {
        let read = unsafe {
            libc::read(
                fd,
                record.as_mut_ptr().cast::<u8>().add(filled).cast(),
                RECORD - filled,
            )
        };
        if read > 0 {
            filled += read as usize;
        } else if read == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
    Some((record[0], record[1]))
}

pub(super) fn make_private(root: &CStr) -> io::Result<()> {
    if unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn mount_proc(target: &CStr) -> io::Result<()> {
    if unsafe {
        libc::mount(
            c"proc".as_ptr(),
            target.as_ptr(),
            c"proc".as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            std::ptr::null(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn alone(proc: &CStr) -> io::Result<bool> {
    if unsafe { libc::getpid() } != 1 {
        return Ok(false);
    }
    let fd = unsafe {
        libc::open(
            proc.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = [0u64; 512];
    let bytes = std::mem::size_of_val(&buffer);
    let mut alone = true;
    loop {
        let read = unsafe { libc::syscall(libc::SYS_getdents64, fd, buffer.as_mut_ptr(), bytes) };
        if read < 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error);
        }
        if read == 0 {
            break;
        }
        let start = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        while offset + 19 < read as usize {
            let entry = unsafe { start.add(offset) };
            let length = unsafe { entry.add(16).cast::<u16>().read_unaligned() } as usize;
            let first = unsafe { entry.add(19).read() };
            let second = unsafe { entry.add(20).read() };
            if first.is_ascii_digit() && !(first == b'1' && second == 0) {
                alone = false;
            }
            if length == 0 {
                break;
            }
            offset += length;
        }
    }
    unsafe { libc::close(fd) };
    Ok(alone)
}

fn close_others(keep: RawFd) {
    let range = |first: RawFd, last: libc::c_uint| unsafe {
        libc::syscall(libc::SYS_close_range, first as libc::c_uint, last, 0u32)
    };
    let below = if keep > 0 {
        range(0, keep as libc::c_uint - 1)
    } else {
        0
    };
    if below == 0 && range(keep + 1, libc::c_uint::MAX) == 0 {
        return;
    }
    for fd in (0..CLOSE_FALLBACK).filter(|fd| *fd != keep) {
        unsafe { libc::close(fd) };
    }
}

unsafe fn serve(wire: RawFd, command: libc::pid_t, mask: &libc::sigset_t) -> ! {
    unsafe {
        libc::prctl(libc::PR_SET_NAME, c"job-init".as_ptr(), 0, 0, 0);
        close_others(wire);
        send(wire, COMMAND, command);
        let signals = libc::signalfd(-1, mask, libc::SFD_CLOEXEC);
        if signals < 0 {
            libc::_exit(FAILED);
        }
        let mut done = false;
        loop {
            let mut watched = [
                libc::pollfd {
                    fd: signals,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if done { wire } else { -1 },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            if libc::poll(watched.as_mut_ptr(), 2, -1) < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                libc::_exit(FAILED);
            }
            if watched[1].revents != 0 {
                libc::_exit(0);
            }
            if watched[0].revents & libc::POLLIN != 0 {
                let mut info: libc::signalfd_siginfo = std::mem::zeroed();
                let size = std::mem::size_of::<libc::signalfd_siginfo>();
                let read = libc::read(signals, (&raw mut info).cast(), size);
                let signal = info.ssi_signo as libc::c_int;
                if read == size as isize
                    && signal != libc::SIGCHLD
                    && !done
                    && info.ssi_code <= 0
                    && info.ssi_pid != 0
                    && libc::kill(-command, signal) != 0
                {
                    libc::kill(command, signal);
                }
            }
            loop {
                let mut status = 0;
                let pid = libc::waitpid(-1, &mut status, libc::WNOHANG);
                if pid <= 0 {
                    break;
                }
                if pid == command && !done {
                    done = true;
                    send(wire, EXITED, status);
                }
            }
        }
    }
}

pub(super) fn reap(pid: i32) -> io::Result<ExitStatus> {
    loop {
        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
            return Ok(ExitStatus::from_raw(status));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Wire {
    far: RawFd,
}
impl Wire {
    pub(super) fn detach(self) -> io::Result<()> {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid > 0 {
            send(self.far, INIT, pid);
            unsafe { libc::_exit(0) };
        }
        unsafe { libc::setsid() };
        Ok(())
    }

    pub(super) fn supervise(self) -> io::Result<()> {
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            let mut before: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            for signal in FAULTS {
                libc::sigdelset(&mut all, signal);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &all, &mut before);
            let dumpable = libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) == 1;
            libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
            let pid = libc::fork();
            if pid > 0 {
                serve(self.far, pid, &all);
            }
            let error = io::Error::last_os_error();
            if dumpable {
                libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &before, std::ptr::null_mut());
            if pid < 0 { Err(error) } else { Ok(()) }
        }
    }
}

pub struct Init {
    near: OwnedFd,
    far: Option<OwnedFd>,
    pid: Option<i32>,
    process: Option<crate::process::Handle>,
    pub command: Option<i32>,
    exited: Option<i32>,
}
impl Init {
    pub(super) fn pair() -> io::Result<(Wire, Self)> {
        let mut fds = [0; 2];
        if unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let (near, far) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok((
            Wire {
                far: far.as_raw_fd(),
            },
            Self {
                near,
                far: Some(far),
                pid: None,
                process: None,
                command: None,
                exited: None,
            },
        ))
    }

    pub fn pid(&self) -> Option<i32> {
        self.pid
    }

    pub(super) fn adopt(&mut self) {
        drop(self.far.take());
        while self.pid.is_none() {
            match receive(self.near.as_raw_fd()) {
                Some((INIT, pid)) => {
                    self.pid = Some(pid);
                    self.process = crate::process::Handle::open(pid, None).ok();
                }
                Some((COMMAND, pid)) => self.command = Some(pid),
                Some((EXITED, status)) => self.exited = Some(status),
                Some(_) => {}
                None => break,
            }
        }
    }

    fn reap(&mut self) -> io::Result<ExitStatus> {
        let pid = self
            .pid
            .take()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ECHILD))?;
        self.process = None;
        reap(pid)
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.exited.take() {
            return Ok(ExitStatus::from_raw(status));
        }
        loop {
            match receive(self.near.as_raw_fd()) {
                Some((COMMAND, pid)) => self.command = Some(pid),
                Some((EXITED, status)) => return Ok(ExitStatus::from_raw(status)),
                Some(_) => {}
                None => return self.reap(),
            }
        }
    }

    pub(super) fn release(&mut self) {
        match (&self.process, self.pid) {
            (Some(process), _) => {
                let _ = process.signal(libc::SIGKILL);
            }
            (None, Some(pid)) => unsafe {
                libc::kill(pid, libc::SIGKILL);
            },
            (None, None) => {}
        }
        let _ = self.reap();
    }
}

fn attempt(privileged: bool) -> io::Result<Option<(i32, i32)>> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let uid_map = format!("{uid} {uid} 1").into_bytes();
    let gid_map = format!("{gid} {gid} 1").into_bytes();
    let mut pipe = [0; 2];
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (reader, writer) =
        unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    let report = writer.as_raw_fd();
    let fail = |step: i32| -> ! {
        send_pipe(
            report,
            step,
            io::Error::last_os_error().raw_os_error().unwrap_or(0),
        );
        unsafe { libc::_exit(1) }
    };
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        unsafe {
            if !privileged {
                if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                    fail(1);
                }
                if super::kernel::write_file(c"/proc/self/setgroups", b"deny").is_err()
                    || super::kernel::write_file(c"/proc/self/uid_map", &uid_map).is_err()
                    || super::kernel::write_file(c"/proc/self/gid_map", &gid_map).is_err()
                {
                    fail(2);
                }
            }
            if libc::unshare(libc::CLONE_NEWPID | libc::CLONE_NEWNS) != 0 {
                fail(3);
            }
            let inner = libc::fork();
            if inner < 0 {
                fail(4);
            }
            if inner == 0 {
                if make_private(c"/").is_err() {
                    fail(5);
                }
                if mount_proc(c"/proc").is_err() {
                    fail(6);
                }
                libc::_exit(0);
            }
            let mut status = 0;
            libc::waitpid(inner, &mut status, 0);
            libc::_exit(0);
        }
    }
    drop(writer);
    let found = receive(reader.as_raw_fd());
    let mut status = 0;
    unsafe { libc::waitpid(child, &mut status, 0) };
    Ok(found)
}

fn send_pipe(fd: RawFd, tag: i32, value: i32) {
    let record = [tag, value];
    unsafe { libc::write(fd, record.as_ptr().cast(), RECORD) };
}

pub fn probe() -> Option<String> {
    static FOUND: OnceLock<Option<String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let privileged = unsafe { libc::geteuid() } == 0;
            if !privileged && !crate::host::user_namespaces() {
                return Some(message(
                    "this host does not let a user create namespaces, so it cannot run a job with --namespaces user",
                ));
            }
            let explain = |step: String, error: String| {
                text(
                    "isolation could not {step}: {error}",
                    &[("step", step), ("error", error)],
                )
            };
            match attempt(privileged) {
                Ok(None) => None,
                Ok(Some((step, errno))) => Some(explain(
                    message(
                        PROBE_STEPS
                            .get((step as usize).wrapping_sub(1))
                            .copied()
                            .unwrap_or(PROBE_STEPS[2]),
                    ),
                    io::Error::from_raw_os_error(errno).to_string(),
                )),
                Err(error) => Some(explain(message(PROBE_STEPS[2]), error.to_string())),
            }
        })
        .clone()
}

pub(super) fn proc_path() -> CString {
    c"/proc".to_owned()
}
