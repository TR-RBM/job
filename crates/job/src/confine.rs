use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

const CREATE_RULESET_VERSION: u32 = 1;
const RULE_PATH_BENEATH: u32 = 1;

const WRITE_FILE: u64 = 1 << 1;
const REMOVE_DIR: u64 = 1 << 4;
const REMOVE_FILE: u64 = 1 << 5;
const MAKE_CHAR: u64 = 1 << 6;
const MAKE_DIR: u64 = 1 << 7;
const MAKE_REG: u64 = 1 << 8;
const MAKE_SOCK: u64 = 1 << 9;
const MAKE_FIFO: u64 = 1 << 10;
const MAKE_BLOCK: u64 = 1 << 11;
const MAKE_SYM: u64 = 1 << 12;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

pub fn write_access(abi: i64) -> u64 {
    let mut access = WRITE_FILE
        | REMOVE_DIR
        | REMOVE_FILE
        | MAKE_CHAR
        | MAKE_DIR
        | MAKE_REG
        | MAKE_SOCK
        | MAKE_FIFO
        | MAKE_BLOCK
        | MAKE_SYM;
    if abi >= 2 {
        access |= REFER;
    }
    if abi >= 3 {
        access |= TRUNCATE;
    }
    access
}

pub fn abi_version() -> i64 {
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    }
}

pub fn default_places(cwd: &Path, home: &Path, tree: &Path, repository: &Path) -> Vec<PathBuf> {
    let mut places = vec![
        tree.to_path_buf(),
        cwd.to_path_buf(),
        repository.to_path_buf(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/dev"),
        home.join(".cargo"),
        home.join(".cache"),
        home.join(".rustup"),
    ];
    places.sort();
    places.dedup();
    places
}

pub fn tree_of(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
}

pub struct Ruleset {
    fd: RawFd,
}

impl Ruleset {
    pub fn build(places: &[PathBuf]) -> io::Result<Ruleset> {
        let abi = abi_version();
        if abi < 1 {
            return Err(io::Error::other("Landlock is not available on this kernel"));
        }
        let access = write_access(abi);
        let attr = RulesetAttr {
            handled_access_fs: access,
        };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let ruleset = Ruleset { fd: fd as RawFd };
        for place in places {
            let Ok(path) = CString::new(place.as_os_str().as_encoded_bytes()) else {
                continue;
            };
            let parent = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if parent < 0 {
                continue;
            }
            let is_dir = std::fs::metadata(place).is_ok_and(|m| m.is_dir());
            let allowed = if is_dir {
                access
            } else {
                access & (WRITE_FILE | TRUNCATE)
            };
            let rule = PathBeneathAttr {
                allowed_access: allowed,
                parent_fd: parent,
            };
            let added = unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    ruleset.fd,
                    RULE_PATH_BENEATH,
                    &rule as *const PathBeneathAttr,
                    0u32,
                )
            };
            let error = io::Error::last_os_error();
            unsafe { libc::close(parent) };
            if added < 0 {
                return Err(error);
            }
        }
        Ok(ruleset)
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for Ruleset {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

pub fn restrict_self(ruleset_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0u32) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub const REFUSAL_WORDS: [&str; 3] = [
    "Permission denied",
    "Operation not permitted",
    "Read-only file system",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_abis_handle_more_kinds_of_write() {
        assert_eq!(write_access(1) & REFER, 0);
        assert_ne!(write_access(2) & REFER, 0);
        assert_ne!(write_access(3) & TRUNCATE, 0);
        assert_eq!(write_access(3) & (1 << 2), 0);
    }

    #[test]
    fn the_default_places_hold_the_tree_the_repository_and_the_caches() {
        let places = default_places(
            Path::new("/w/x/src"),
            Path::new("/home/u"),
            Path::new("/w/x"),
            Path::new("/r/.git"),
        );
        for expected in [
            "/w/x",
            "/w/x/src",
            "/r/.git",
            "/tmp",
            "/dev",
            "/home/u/.cargo",
            "/home/u/.cache",
        ] {
            assert!(places.contains(&PathBuf::from(expected)), "{expected}");
        }
        assert!(!places.contains(&PathBuf::from("/home/u")));
    }

    #[test]
    fn this_kernel_offers_landlock() {
        assert!(abi_version() >= 1, "{}", abi_version());
    }
}
