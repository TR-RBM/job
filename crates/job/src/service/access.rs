use std::cell::Cell;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;

use super::message;

thread_local! {
    static ACTOR: Cell<Option<u32>> = const { Cell::new(None) };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub uid: u32,
    pub group: Option<(String, u32)>,
}

pub fn group_id(name: &str) -> io::Result<Option<u32>> {
    if let Ok(number) = name.parse::<u32>() {
        return Ok(Some(number));
    }
    let Ok(name) = std::ffi::CString::new(name) else {
        return Ok(None);
    };
    let mut buffer = vec![0u8; 4096];
    loop {
        let mut group: libc::group = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::group = std::ptr::null_mut();
        let result = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                &mut group,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if result == libc::ERANGE && buffer.len() < (1 << 24) {
            buffer.resize(buffer.len() * 4, 0);
            continue;
        }
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        return Ok((!found.is_null()).then_some(group.gr_gid));
    }
}

fn credentials(stream: &UnixStream) -> Option<libc::ucred> {
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
    (result == 0).then_some(credentials)
}

fn supplementary(stream: &UnixStream) -> Option<Vec<u32>> {
    let mut groups = vec![0 as libc::gid_t; 64];
    loop {
        let mut length = std::mem::size_of_val(groups.as_slice()) as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERGROUPS,
                groups.as_mut_ptr().cast(),
                &mut length,
            )
        };
        let count = length as usize / std::mem::size_of::<libc::gid_t>();
        if result == 0 {
            groups.truncate(count);
            return Some(groups);
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) || count <= groups.len()
        {
            return None;
        }
        groups.resize(count, 0);
    }
}

impl Policy {
    pub fn private() -> Self {
        Self {
            uid: unsafe { libc::getuid() },
            group: None,
        }
    }

    pub fn resolve(settings: &super::Socket) -> io::Result<Self> {
        let mut policy = Self::private();
        if let Some(name) = &settings.group {
            let id = group_id(name)?.ok_or_else(|| {
                io::Error::other(message(
                    "the socket group {group} does not exist; create it or correct [socket] group in the configuration",
                    &[("group", name.clone())],
                ))
            })?;
            policy.group = Some((name.clone(), id));
        }
        Ok(policy)
    }

    pub fn current() -> Self {
        crate::config::Config::load()
            .ok()
            .and_then(|(config, _)| Self::resolve(&config.socket).ok())
            .unwrap_or_else(Self::private)
    }

    pub fn admits(&self, stream: &UnixStream) -> Option<u32> {
        let peer = credentials(stream)?;
        if peer.uid == self.uid {
            return Some(peer.uid);
        }
        let (_, group) = self.group.as_ref()?;
        (peer.gid == *group || supplementary(stream).is_some_and(|all| all.contains(group)))
            .then_some(peer.uid)
    }

    pub fn share_socket(&self, path: &Path) -> io::Result<()> {
        self.share(path, if self.group.is_some() { 0o660 } else { 0o600 })
    }

    pub fn refusal(&self) -> String {
        match &self.group {
            None => message("refused: the caller runs as a different user", &[]),
            Some((group, _)) => message(
                "refused: the caller is neither the service user nor a member of group {group}",
                &[("group", group.clone())],
            ),
        }
    }

    pub fn share(&self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some((name, group)) = &self.group {
            let text = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            if unsafe { libc::chown(text.as_ptr(), libc::uid_t::MAX, *group) } != 0 {
                let error = io::Error::last_os_error();
                return Err(io::Error::other(message(
                    "cannot give {path} to the socket group {group}: {error}; the service user must be a member of that group",
                    &[
                        ("path", path.display().to_string()),
                        ("group", name.clone()),
                        ("error", error.to_string()),
                    ],
                )));
            }
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
}

static SHARED: std::sync::OnceLock<Policy> = std::sync::OnceLock::new();

pub fn install(policy: Policy) {
    let _ = SHARED.set(policy);
}

const SUPERVISOR_OPTION: &str = "--socket-group";

pub fn supervisor_arguments() -> Vec<String> {
    match &shared().group {
        Some((name, id)) => vec![SUPERVISOR_OPTION.to_owned(), format!("{id}:{name}")],
        None => Vec::new(),
    }
}

pub fn adopt(arguments: &[String]) {
    let mut policy = Policy::private();
    policy.group = arguments
        .windows(2)
        .find(|pair| pair[0] == SUPERVISOR_OPTION)
        .and_then(|pair| pair[1].split_once(':'))
        .and_then(|(id, name)| Some((name.to_owned(), id.parse().ok()?)));
    install(policy);
}

pub fn shared() -> &'static Policy {
    SHARED.get_or_init(Policy::current)
}

pub fn enter(uid: u32) {
    ACTOR.with(|actor| actor.set(Some(uid)));
}

pub fn actor_uid() -> u32 {
    ACTOR
        .with(Cell::get)
        .unwrap_or_else(|| unsafe { libc::geteuid() })
}
