use std::ffi::CString;
use std::io;

pub struct NoNetwork {
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    setgroups: CString,
    uid_path: CString,
    gid_path: CString,
}

impl NoNetwork {
    pub fn prepare() -> NoNetwork {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let path = |name: &str| CString::new(format!("/proc/self/{name}")).unwrap_or_default();
        NoNetwork {
            uid_map: format!("{uid} {uid} 1").into_bytes(),
            gid_map: format!("{gid} {gid} 1").into_bytes(),
            setgroups: path("setgroups"),
            uid_path: path("uid_map"),
            gid_path: path("gid_map"),
        }
    }

    pub fn enter(&self) -> io::Result<()> {
        if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) } != 0 {
            return Err(io::Error::last_os_error());
        }
        write_file(&self.setgroups, b"deny")?;
        write_file(&self.uid_path, &self.uid_map)?;
        write_file(&self.gid_path, &self.gid_map)?;
        loopback_up()
    }
}

fn write_file(path: &CString, bytes: &[u8]) -> io::Result<()> {
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    let error = io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if written == bytes.len() as isize {
        Ok(())
    } else {
        Err(error)
    }
}

fn loopback_up() -> io::Result<()> {
    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if socket < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (slot, byte) in request.ifr_name.iter_mut().zip(b"lo\0") {
        *slot = *byte as libc::c_char;
    }
    let result = unsafe {
        if libc::ioctl(socket, libc::SIOCGIFFLAGS, &mut request) != 0 {
            -1
        } else {
            request.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short;
            libc::ioctl(socket, libc::SIOCSIFFLAGS, &request)
        }
    };
    let error = io::Error::last_os_error();
    unsafe { libc::close(socket) };
    if result == 0 { Ok(()) } else { Err(error) }
}

pub struct LinkedNetwork {
    user: std::fs::File,
    net: std::fs::File,
    resolver: CString,
    target: CString,
    root: CString,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    setgroups: CString,
    uid_path: CString,
    gid_path: CString,
}

impl LinkedNetwork {
    pub fn prepare(holder: i32, resolver: &std::path::Path) -> io::Result<LinkedNetwork> {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let text = |t: &str| CString::new(t).map_err(io::Error::other);
        Ok(LinkedNetwork {
            user: std::fs::File::open(format!("/proc/{holder}/ns/user"))?,
            net: std::fs::File::open(format!("/proc/{holder}/ns/net"))?,
            resolver: CString::new(resolver.as_os_str().as_encoded_bytes())
                .map_err(io::Error::other)?,
            target: text("/etc/resolv.conf")?,
            root: text("/")?,
            uid_map: format!("{uid} 0 1").into_bytes(),
            gid_map: format!("{gid} 0 1").into_bytes(),
            setgroups: text("/proc/self/setgroups")?,
            uid_path: text("/proc/self/uid_map")?,
            gid_path: text("/proc/self/gid_map")?,
        })
    }

    pub fn enter(&self) -> io::Result<()> {
        use std::os::unix::io::AsRawFd;
        let check = |result: libc::c_int| {
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        };
        check(unsafe { libc::setns(self.user.as_raw_fd(), libc::CLONE_NEWUSER) })?;
        check(unsafe { libc::setns(self.net.as_raw_fd(), libc::CLONE_NEWNET) })?;
        check(unsafe { libc::unshare(libc::CLONE_NEWNS) })?;
        check(unsafe {
            libc::mount(
                std::ptr::null(),
                self.root.as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            )
        })?;
        check(unsafe {
            libc::mount(
                self.resolver.as_ptr(),
                self.target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        })?;
        check(unsafe { libc::unshare(libc::CLONE_NEWUSER) })?;
        write_file(&self.setgroups, b"deny")?;
        write_file(&self.uid_path, &self.uid_map)?;
        write_file(&self.gid_path, &self.gid_map)
    }
}
