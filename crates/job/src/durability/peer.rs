use std::cell::Cell;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub pid: i32,
}

thread_local! {
    static CURRENT: Cell<Option<Peer>> = const { Cell::new(None) };
}

pub fn of(stream: &UnixStream) -> Option<Peer> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    (result == 0).then_some(Peer {
        uid: credentials.uid,
        pid: credentials.pid,
    })
}

pub fn enter(peer: Option<Peer>) {
    CURRENT.with(|current| current.set(peer));
}

pub fn current() -> Option<Peer> {
    CURRENT.with(Cell::get)
}

pub fn actor_uid() -> u32 {
    current().map_or_else(|| unsafe { libc::geteuid() }, |peer| peer.uid)
}
