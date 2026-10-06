use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

const NS_GET_NSTYPE: u32 = 0xb703;
const SELF_NET: &[u8] = b"/proc/self/ns/net\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Network,
    User,
}

impl Kind {
    fn flag(self) -> libc::c_int {
        match self {
            Kind::Network => libc::CLONE_NEWNET,
            Kind::User => libc::CLONE_NEWUSER,
        }
    }
}

#[derive(Debug)]
pub enum Fault {
    Unreadable(io::Error),
    NotNamespace,
    OtherKind,
}

pub fn open(path: &Path, kind: Kind) -> Result<File, Fault> {
    let file = File::open(path).map_err(Fault::Unreadable)?;
    let found = unsafe { libc::ioctl(file.as_raw_fd(), NS_GET_NSTYPE as _) };
    if found < 0 {
        return Err(Fault::NotNamespace);
    }
    if found != kind.flag() {
        return Err(Fault::OtherKind);
    }
    Ok(file)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    User = 1,
    Network = 2,
    Verify = 3,
    Resolver = 4,
    Identity = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub step: Step,
    pub errno: i32,
}

pub struct Joined {
    net: File,
    inode: u64,
    device: u64,
    user: Option<File>,
    resolver: Option<CString>,
    target: CString,
    root: CString,
    setgroups: CString,
    uid_path: CString,
    gid_path: CString,
    uid: u32,
    gid: u32,
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn digits(buffer: &mut [u8; 32], mut length: usize, mut number: u32) -> usize {
    let mut digits = [0u8; 10];
    let mut count = 0;
    loop {
        digits[count] = b'0' + (number % 10) as u8;
        number /= 10;
        count += 1;
        if number == 0 {
            break;
        }
    }
    while count > 0 {
        count -= 1;
        buffer[length] = digits[count];
        length += 1;
    }
    length
}

fn mapping(buffer: &mut [u8; 32], inside: u32, outside: u32) -> usize {
    let mut length = digits(buffer, 0, inside);
    buffer[length] = b' ';
    length = digits(buffer, length + 1, outside);
    buffer[length] = b' ';
    buffer[length + 1] = b'1';
    length + 2
}

fn write_file(path: &CString, bytes: &[u8]) -> bool {
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return false;
    }
    let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    let kept = errno();
    unsafe {
        libc::close(fd);
        *libc::__errno_location() = kept;
    }
    written == bytes.len() as isize
}

impl Joined {
    pub fn prepare(
        net: &Path,
        user: Option<&Path>,
        resolver: Option<&Path>,
    ) -> Result<Joined, Fault> {
        let file = open(net, Kind::Network)?;
        let meta = file.metadata().map_err(Fault::Unreadable)?;
        let text = |t: &str| CString::new(t).unwrap_or_default();
        Ok(Joined {
            inode: meta.ino(),
            device: meta.dev(),
            net: file,
            user: match user {
                Some(path) => Some(open(path, Kind::User)?),
                None => None,
            },
            resolver: match resolver {
                Some(path) => Some(
                    CString::new(path.as_os_str().as_encoded_bytes())
                        .map_err(|e| Fault::Unreadable(io::Error::other(e)))?,
                ),
                None => None,
            },
            target: text("/etc/resolv.conf"),
            root: text("/"),
            setgroups: text("/proc/self/setgroups"),
            uid_path: text("/proc/self/uid_map"),
            gid_path: text("/proc/self/gid_map"),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        })
    }

    pub fn enters_user_namespace(&self) -> bool {
        self.user.is_some()
    }

    fn run(&self) -> Result<(), Refusal> {
        let refused = |step: Step| Refusal {
            step,
            errno: errno(),
        };
        if let Some(user) = &self.user
            && unsafe { libc::setns(user.as_raw_fd(), libc::CLONE_NEWUSER) } != 0
        {
            return Err(refused(Step::User));
        }
        if unsafe { libc::setns(self.net.as_raw_fd(), libc::CLONE_NEWNET) } != 0 {
            return Err(refused(Step::Network));
        }
        let mut seen: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(SELF_NET.as_ptr().cast(), &mut seen) } != 0 {
            return Err(refused(Step::Verify));
        }
        if seen.st_ino != self.inode || seen.st_dev != self.device {
            return Err(Refusal {
                step: Step::Verify,
                errno: libc::ESTALE,
            });
        }
        if let Some(resolver) = &self.resolver {
            let mounted = unsafe {
                libc::unshare(libc::CLONE_NEWNS) == 0
                    && libc::mount(
                        std::ptr::null(),
                        self.root.as_ptr(),
                        std::ptr::null(),
                        libc::MS_REC | libc::MS_PRIVATE,
                        std::ptr::null(),
                    ) == 0
                    && libc::mount(
                        resolver.as_ptr(),
                        self.target.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) == 0
            };
            if !mounted {
                return Err(refused(Step::Resolver));
            }
        }
        if self.user.is_some() {
            let inner_uid = unsafe { libc::getuid() };
            let inner_gid = unsafe { libc::getgid() };
            let mut uid_map = [0u8; 32];
            let mut gid_map = [0u8; 32];
            let uid_length = mapping(&mut uid_map, self.uid, inner_uid);
            let gid_length = mapping(&mut gid_map, self.gid, inner_gid);
            let mapped = unsafe { libc::unshare(libc::CLONE_NEWUSER) } == 0
                && write_file(&self.setgroups, b"deny")
                && write_file(&self.uid_path, &uid_map[..uid_length])
                && write_file(&self.gid_path, &gid_map[..gid_length]);
            if !mapped {
                return Err(refused(Step::Identity));
            }
        }
        Ok(())
    }

    pub fn enter(&self) -> io::Result<()> {
        self.run()
            .map_err(|refusal| io::Error::from_raw_os_error(refusal.errno))
    }

    pub fn probe(&self) -> Result<(), Refusal> {
        let mut ends = [0 as libc::c_int; 2];
        if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(Refusal {
                step: Step::Verify,
                errno: errno(),
            });
        }
        let child = unsafe { libc::fork() };
        if child < 0 {
            let failed = errno();
            unsafe {
                libc::close(ends[0]);
                libc::close(ends[1]);
            }
            return Err(Refusal {
                step: Step::Verify,
                errno: failed,
            });
        }
        if child == 0 {
            let report = match self.run() {
                Ok(()) => [0u8, 0u8],
                Err(refusal) => [refusal.step as u8, refusal.errno.clamp(1, 255) as u8],
            };
            unsafe {
                libc::write(ends[1], report.as_ptr().cast(), 2);
                libc::_exit(0);
            }
        }
        let mut report = [0u8; 2];
        let read = unsafe {
            libc::close(ends[1]);
            let read = libc::read(ends[0], report.as_mut_ptr().cast(), 2);
            libc::close(ends[0]);
            let mut status = 0;
            libc::waitpid(child, &mut status, 0);
            read
        };
        if read != 2 {
            return Err(Refusal {
                step: Step::Verify,
                errno: libc::ECHILD,
            });
        }
        let step = match report[0] {
            0 => return Ok(()),
            1 => Step::User,
            2 => Step::Network,
            3 => Step::Verify,
            4 => Step::Resolver,
            _ => Step::Identity,
        };
        Err(Refusal {
            step,
            errno: report[1] as i32,
        })
    }
}
