use super::{Controls, KINDS, message, pidns, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MOUNT_ATTR_RDONLY: u64 = 1;
const AT_RECURSIVE: libc::c_int = 0x8000;
const MISMATCH: libc::c_int = libc::ENOTRECOVERABLE;
const TMP_TARGETS: [&str; 2] = ["/tmp", "/var/tmp"];
const DEEPEST: usize = 128;
const SPARE_NAMES: u32 = 8;
const LEFTOVERS: &str = "private-tmp-leftovers";
const STEPS: [&str; 17] = [
    "create the user namespace",
    "write the user namespace identity maps",
    "create the requested namespaces",
    "start the init of the pid namespace",
    "make mounts private",
    "mount a fresh /proc",
    "bind a writable path",
    "bind the private tmp",
    "make the root read-only",
    "make a writable path or the private tmp writable again",
    "return to the working directory",
    "read back the namespaces",
    "read back the pid namespace",
    "read back the read-only root",
    "read back a writable path",
    "read back the private tmp",
    "start the command under the init",
];

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

fn check(result: libc::c_long) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn mismatch() -> io::Error {
    io::Error::from_raw_os_error(MISMATCH)
}
fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())
}
fn file_of(kind: &str) -> &str {
    if kind == "mount" { "mnt" } else { kind }
}
fn flag_of(kind: &str) -> libc::c_int {
    match kind {
        "user" => libc::CLONE_NEWUSER,
        "mount" => libc::CLONE_NEWNS,
        "ipc" => libc::CLONE_NEWIPC,
        "uts" => libc::CLONE_NEWUTS,
        "pid" => libc::CLONE_NEWPID,
        _ => libc::CLONE_NEWCGROUP,
    }
}
fn set_attr(path: &CString, flags: libc::c_int, set: u64, clear: u64) -> io::Result<()> {
    let attr = MountAttr {
        attr_set: set,
        attr_clr: clear,
        propagation: 0,
        userns_fd: 0,
    };
    check(unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            libc::AT_FDCWD,
            path.as_ptr(),
            flags,
            &attr,
            std::mem::size_of::<MountAttr>(),
        )
    })
}
fn bind(source: &CString, target: &CString, flags: libc::c_ulong) -> io::Result<()> {
    check(unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND | flags,
            std::ptr::null(),
        )
    } as libc::c_long)
}
pub(super) fn write_file(path: &std::ffi::CStr, bytes: &[u8]) -> io::Result<()> {
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
fn read_only(path: &CString) -> io::Result<bool> {
    let mut found: libc::statvfs = unsafe { std::mem::zeroed() };
    check(unsafe { libc::statvfs(path.as_ptr(), &mut found) } as libc::c_long)?;
    Ok(found.f_flag & libc::ST_RDONLY != 0)
}
fn identity(path: &CString) -> io::Result<(u64, u64)> {
    let mut found: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::stat(path.as_ptr(), &mut found) } as libc::c_long)?;
    Ok((found.st_dev, found.st_ino))
}
fn mount_setattr_probe() -> Option<String> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            -1,
            c"".as_ptr(),
            0,
            std::ptr::null::<MountAttr>(),
            0usize,
        )
    };
    let error = io::Error::last_os_error();
    (result < 0 && error.raw_os_error() != Some(libc::EINVAL)).then(|| error.to_string())
}

fn open_at(parent: RawFd, name: &std::ffi::CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn remove_entry(parent: RawFd, name: &std::ffi::CStr, depth: usize) -> io::Result<()> {
    if unsafe { libc::unlinkat(parent, name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOENT) => return Ok(()),
        Some(libc::EISDIR) | Some(libc::EPERM) => {}
        _ => return Err(error),
    }
    if depth == 0 {
        return Err(io::Error::other(message(
            "it is nested deeper than 128 directories",
        )));
    }
    let handle = open_at(
        parent,
        name,
        libc::O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY,
    )?;
    if let Ok(link) = CString::new(format!("/proc/self/fd/{}", handle.as_raw_fd())) {
        unsafe { libc::chmod(link.as_ptr(), 0o700) };
    }
    let listing = open_at(
        handle.as_raw_fd(),
        c".",
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )?;
    let stream = unsafe { libc::fdopendir(listing.as_raw_fd()) };
    if stream.is_null() {
        return Err(io::Error::last_os_error());
    }
    std::mem::forget(listing);
    let mut outcome = Ok(());
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let child = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_owned();
        if child.as_bytes() == b"." || child.as_bytes() == b".." {
            continue;
        }
        if let Err(error) = remove_entry(handle.as_raw_fd(), &child, depth - 1) {
            outcome = Err(error);
            break;
        }
    }
    unsafe { libc::closedir(stream) };
    outcome?;
    check(unsafe { libc::unlinkat(parent, name.as_ptr(), libc::AT_REMOVEDIR) } as libc::c_long)
}

fn remove_tree(path: &Path) -> io::Result<()> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    };
    let parent = std::fs::File::open(parent)?;
    let name = CString::new(name.as_bytes()).map_err(io::Error::other)?;
    remove_entry(parent.as_raw_fd(), &name, DEEPEST)
}

fn set_aside(path: &Path) -> Option<PathBuf> {
    let job_dir = path.parent()?;
    let leftovers = job_dir.parent()?.parent()?.join(LEFTOVERS);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&leftovers)
        .ok()?;
    if path.symlink_metadata().ok()?.is_dir() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    let aside = leftovers.join(format!(
        "{}-{}-{}",
        job_dir.file_name()?.to_string_lossy(),
        path.file_name()?.to_string_lossy(),
        crate::shim::now_ms()
    ));
    std::fs::rename(path, &aside).ok()?;
    Some(aside)
}

fn fresh_tmp(job_dir: &Path, attempt: u64) -> io::Result<PathBuf> {
    let mut last = io::Error::from_raw_os_error(libc::EEXIST);
    for spare in 0..SPARE_NAMES {
        let path = job_dir.join(if spare == 0 {
            format!("tmp-{attempt}")
        } else {
            format!("tmp-{attempt}-{spare}")
        });
        match path.symlink_metadata() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                last = error;
                continue;
            }
            Ok(found) => {
                if found.is_dir()
                    && found.mode() & 0o777 == 0o700
                    && std::fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none())
                {
                    return Ok(path);
                }
                if let Err(error) = remove_tree(&path) {
                    last = error;
                    continue;
                }
            }
        }
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        return Ok(path);
    }
    Err(last)
}

fn under_tmp(path: &Path) -> bool {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    TMP_TARGETS.iter().any(|hidden| {
        resolved.starts_with(hidden)
            || std::fs::canonicalize(hidden).is_ok_and(|target| resolved.starts_with(target))
    })
}

fn drops_sys_admin(security: &crate::security::Controls) -> bool {
    security.cap_drop.as_ref().is_some_and(|drop| {
        let names = String::from(drop.clone());
        names == "all" || names.split(',').any(|name| name == "sys_admin")
    })
}

fn lockable(
    controls: &Controls,
    security: &crate::security::Controls,
    privileged: bool,
) -> Result<(), String> {
    if privileged && (controls.read_only() || controls.private()) && !drops_sys_admin(security) {
        return Err(message(
            "this service runs as root, so a job keeps the capability to undo --root read-only and --private-tmp; add --cap-drop sys_admin or --cap-drop all",
        ));
    }
    Ok(())
}

fn placed(controls: &Controls, cwd: &Path) -> Result<(), String> {
    if controls.private() && under_tmp(cwd) {
        return Err(text(
            "--private-tmp yes hides the working directory {path}; choose a working directory outside /tmp and /var/tmp",
            &[("path", cwd.display().to_string())],
        ));
    }
    Ok(())
}

pub fn admissible(declared: &crate::model::Declared, cwd: &Path) -> Result<(), String> {
    available(&declared.isolation)?;
    if declared.isolation.has("pid")
        && let Some(error) = pidns::probe()
    {
        return Err(text(
            "this host cannot start a job in a pid namespace: {error}",
            &[("error", error)],
        ));
    }
    placed(&declared.isolation, cwd)?;
    lockable(
        &declared.isolation,
        &declared.security,
        unsafe { libc::geteuid() } == 0,
    )
}

pub fn confined_places(controls: &Controls, job_dir: &Path, attempt: u64) -> Vec<PathBuf> {
    controls
        .private()
        .then(|| fresh_tmp(job_dir, attempt).ok())
        .flatten()
        .into_iter()
        .chain(
            controls
                .paths()
                .filter_map(|path| std::fs::canonicalize(path).ok()),
        )
        .collect()
}

pub fn available(controls: &Controls) -> Result<(), String> {
    controls.validate()?;
    for kind in controls.kinds() {
        if !Path::new("/proc/self/ns").join(file_of(kind)).exists() {
            return Err(text(
                "this host has no {kind} namespaces, so it cannot run a job with --namespaces {kind}",
                &[("kind", kind.to_owned())],
            ));
        }
    }
    if controls.has("user") && !crate::host::user_namespaces() {
        return Err(message(
            "this host does not let a user create namespaces, so it cannot run a job with --namespaces user",
        ));
    }
    if controls.active() && !controls.has("user") && unsafe { libc::geteuid() } != 0 {
        return Err(message(
            "this service is not privileged, so --namespaces needs user in its list; add user",
        ));
    }
    if controls.mounts() && mount_setattr_probe().is_some() {
        return Err(message(
            "this host has no mount_setattr, so it cannot run a job with --root, --private-tmp or --writable",
        ));
    }
    for path in controls.paths() {
        let resolved = std::fs::canonicalize(path).map_err(|_| {
            text(
                "the writable path {path} does not exist",
                &[("path", path.to_owned())],
            )
        })?;
        if controls.private() && under_tmp(&resolved) {
            return Err(text(
                "--private-tmp yes hides {path}; choose a writable path outside /tmp and /var/tmp",
                &[("path", path.to_owned())],
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidNamespace {
    pub proc: String,
    pub init_pid: i32,
    #[serde(default)]
    pub command_pid: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub user_namespace_from_network: bool,
    #[serde(default)]
    pub root_read_only: bool,
    #[serde(default)]
    pub private_tmp: Vec<String>,
    #[serde(default)]
    pub writable: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_namespace: Option<PidNamespace>,
}

#[derive(Clone, Copy)]
struct Step(*mut u32);
unsafe impl Send for Step {}
unsafe impl Sync for Step {}
impl Step {
    fn new() -> Result<Self, String> {
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                std::mem::size_of::<u32>(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if page == libc::MAP_FAILED {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(Self(page.cast()))
    }
    fn set(self, step: u32) {
        unsafe { self.0.write_volatile(step) }
    }
    fn get(self) -> u32 {
        unsafe { self.0.read_volatile() }
    }
}

struct Maps {
    setgroups: CString,
    uid_path: CString,
    gid_path: CString,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
}

struct PrivateTmp {
    source: CString,
    targets: Vec<CString>,
    identity: (u64, u64),
}

pub struct Handle {
    step: Option<Step>,
    tmp: Option<PathBuf>,
    init: Option<pidns::Init>,
    pub applied: Option<Applied>,
}
impl Handle {
    pub fn adopt(&mut self, child: &std::process::Child) {
        if let Some(init) = &mut self.init {
            init.adopt();
            let _ = pidns::reap(child.id() as i32);
        }
    }
    pub fn abandon(&mut self) {
        if let Some(init) = &mut self.init {
            init.adopt();
            init.release();
        }
    }
    pub fn init(&self) -> Option<i32> {
        self.init.as_ref().and_then(pidns::Init::pid)
    }
    pub fn wait(
        &mut self,
        child: &mut std::process::Child,
    ) -> io::Result<std::process::ExitStatus> {
        let Some(init) = &mut self.init else {
            return child.wait();
        };
        let status = init.wait();
        if let Some(space) = self
            .applied
            .as_mut()
            .and_then(|applied| applied.pid_namespace.as_mut())
        {
            space.command_pid = init.command;
        }
        status
    }
    pub fn release(&mut self) {
        if let Some(init) = &mut self.init {
            init.release();
        }
    }
    pub fn explain(&self, error: &io::Error) -> Option<String> {
        let step = self.step?.get() as usize;
        let name = STEPS.get(step.checked_sub(1)?)?;
        Some(text(
            "isolation could not {step}: {error}",
            &[
                ("step", message(name)),
                (
                    "error",
                    if error.raw_os_error() == Some(MISMATCH) {
                        message("the kernel state does not match the request")
                    } else {
                        error.to_string()
                    },
                ),
            ],
        ))
    }
    pub fn cleanup(&self) -> Option<String> {
        let path = self.tmp.as_ref()?;
        let error = remove_tree(path).err()?;
        let kept = set_aside(path).unwrap_or_else(|| path.clone());
        Some(text(
            "the private tmp could not be removed: {error}; it is kept in {path}",
            &[
                ("error", error.to_string()),
                ("path", kept.display().to_string()),
            ],
        ))
    }
}

pub struct Prepared {
    maps: Option<Maps>,
    rest: libc::c_int,
    checks: Vec<(CString, u64)>,
    mounts: bool,
    root: CString,
    read_only: bool,
    writable: Vec<(CString, (u64, u64))>,
    tmp: Option<PrivateTmp>,
    cwd: Option<CString>,
    step: Option<Step>,
    tmp_dir: Option<PathBuf>,
    applied: Option<Applied>,
    wire: Option<pidns::Wire>,
    proc: CString,
    init: std::sync::Mutex<Option<pidns::Init>>,
}
impl Prepared {
    pub fn new(
        controls: &Controls,
        security: &crate::security::Controls,
        in_user_namespace: bool,
        job_dir: &Path,
        attempt: u64,
        cwd: &Path,
    ) -> Result<Self, String> {
        let mut prepared = Self {
            maps: None,
            rest: 0,
            checks: Vec::new(),
            mounts: controls.mounts(),
            root: c"/".to_owned(),
            read_only: controls.read_only(),
            writable: Vec::new(),
            tmp: None,
            cwd: None,
            step: None,
            tmp_dir: None,
            applied: None,
            wire: None,
            proc: pidns::proc_path(),
            init: std::sync::Mutex::new(None),
        };
        if !controls.active() {
            return Ok(prepared);
        }
        available(controls)?;
        placed(controls, cwd)?;
        lockable(controls, security, unsafe { libc::geteuid() } == 0)?;
        let context = |e: io::Error| {
            format!(
                "{}: {e}",
                message("requested isolation control is unavailable")
            )
        };
        for kind in controls.kinds() {
            let path = Path::new("/proc/self/ns").join(file_of(kind));
            let inode = std::fs::metadata(&path).map_err(context)?.ino();
            prepared.checks.push((c_path(&path)?, inode));
            if kind != "user" {
                prepared.rest |= flag_of(kind);
            }
        }
        if controls.has("user") && !in_user_namespace {
            let uid = unsafe { libc::getuid() };
            let gid = unsafe { libc::getgid() };
            prepared.maps = Some(Maps {
                setgroups: c"/proc/self/setgroups".to_owned(),
                uid_path: c"/proc/self/uid_map".to_owned(),
                gid_path: c"/proc/self/gid_map".to_owned(),
                uid_map: format!("{uid} {uid} 1").into_bytes(),
                gid_map: format!("{gid} {gid} 1").into_bytes(),
            });
        }
        let mut resolved = std::collections::BTreeSet::new();
        for path in controls.paths() {
            resolved.insert(std::fs::canonicalize(path).map_err(context)?);
        }
        let mut writable = Vec::new();
        for path in resolved {
            let found = std::fs::metadata(&path).map_err(context)?;
            prepared
                .writable
                .push((c_path(&path)?, (found.dev(), found.ino())));
            writable.push(path.to_string_lossy().into_owned());
        }
        if prepared.mounts {
            prepared.cwd = Some(c_path(cwd)?);
        }
        let mut private_tmp = Vec::new();
        if controls.private() {
            let source = fresh_tmp(job_dir, attempt).map_err(context)?;
            let found = std::fs::metadata(&source).map_err(context)?;
            let mut targets = Vec::new();
            for target in TMP_TARGETS {
                if let Ok(resolved) = std::fs::canonicalize(target)
                    && resolved.is_dir()
                    && !private_tmp.contains(&resolved.to_string_lossy().into_owned())
                {
                    targets.push(c_path(&resolved)?);
                    private_tmp.push(resolved.to_string_lossy().into_owned());
                }
            }
            if targets.is_empty() {
                return Err(message("this host has no /tmp to make private"));
            }
            prepared.tmp = Some(PrivateTmp {
                source: c_path(&source)?,
                targets,
                identity: (found.dev(), found.ino()),
            });
            prepared.tmp_dir = Some(source);
        }
        if controls.has("pid") {
            let (wire, init) = pidns::Init::pair().map_err(context)?;
            prepared.wire = Some(wire);
            prepared.init = std::sync::Mutex::new(Some(init));
        }
        prepared.step = Some(Step::new()?);
        prepared.applied = Some(Applied {
            pid_namespace: prepared.wire.map(|_| PidNamespace {
                proc: prepared.proc.to_string_lossy().into_owned(),
                init_pid: 1,
                command_pid: None,
            }),
            namespaces: controls.kinds().map(str::to_owned).collect(),
            user_namespace_from_network: controls.has("user") && in_user_namespace,
            root_read_only: prepared.read_only,
            private_tmp,
            writable,
        });
        Ok(prepared)
    }

    pub fn handle(&self) -> Handle {
        Handle {
            step: self.step,
            tmp: self.tmp_dir.clone(),
            init: self.init.lock().ok().and_then(|mut init| init.take()),
            applied: self.applied.clone(),
        }
    }

    pub fn apply(&self) -> io::Result<()> {
        let Some(step) = self.step else {
            return Ok(());
        };
        if let Some(maps) = &self.maps {
            step.set(1);
            check(unsafe { libc::unshare(libc::CLONE_NEWUSER) } as libc::c_long)?;
            step.set(2);
            write_file(&maps.setgroups, b"deny")?;
            write_file(&maps.uid_path, &maps.uid_map)?;
            write_file(&maps.gid_path, &maps.gid_map)?;
        }
        if self.rest != 0 {
            step.set(3);
            check(unsafe { libc::unshare(self.rest) } as libc::c_long)?;
        }
        if let Some(wire) = self.wire {
            step.set(4);
            wire.detach()?;
        }
        if self.mounts || self.wire.is_some() {
            step.set(5);
            pidns::make_private(&self.root)?;
        }
        if self.wire.is_some() {
            step.set(6);
            pidns::mount_proc(&self.proc)?;
        }
        if self.mounts {
            step.set(7);
            for (path, _) in &self.writable {
                bind(path, path, libc::MS_REC)?;
            }
            if let Some(tmp) = &self.tmp {
                step.set(8);
                bind(&tmp.source, &tmp.targets[0], 0)?;
                for target in &tmp.targets[1..] {
                    bind(&tmp.targets[0], target, 0)?;
                }
            }
            if self.read_only {
                step.set(9);
                set_attr(&self.root, AT_RECURSIVE, MOUNT_ATTR_RDONLY, 0)?;
                step.set(10);
                for (path, _) in &self.writable {
                    set_attr(path, AT_RECURSIVE, 0, MOUNT_ATTR_RDONLY)?;
                }
                for target in self.tmp.iter().flat_map(|tmp| &tmp.targets) {
                    set_attr(target, 0, 0, MOUNT_ATTR_RDONLY)?;
                }
            }
            if let Some(cwd) = &self.cwd {
                step.set(11);
                check(unsafe { libc::chdir(cwd.as_ptr()) } as libc::c_long)?;
            }
        }
        step.set(12);
        for (path, parent) in &self.checks {
            if identity(path)?.1 == *parent {
                return Err(mismatch());
            }
        }
        if self.wire.is_some() {
            step.set(13);
            if !pidns::alone(&self.proc)? {
                return Err(mismatch());
            }
        }
        if self.read_only {
            step.set(14);
            if !read_only(&self.root)? {
                return Err(mismatch());
            }
        }
        step.set(15);
        for (path, expected) in &self.writable {
            if identity(path)? != *expected || read_only(path)? {
                return Err(mismatch());
            }
        }
        if let Some(tmp) = &self.tmp {
            step.set(16);
            for target in &tmp.targets {
                if identity(target)? != tmp.identity || (self.read_only && read_only(target)?) {
                    return Err(mismatch());
                }
            }
        }
        if let Some(wire) = self.wire {
            step.set(17);
            wire.supervise()?;
        }
        step.set(0);
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkMode {
    pub available: bool,
    pub namespace: String,
    pub link: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub namespace_kinds: BTreeMap<String, bool>,
    pub requestable_namespaces: Vec<String>,
    pub pid_namespace_requestable: bool,
    #[serde(default)]
    pub pid_namespace_error: Option<String>,
    pub unprivileged_user_namespaces: bool,
    pub mount_setattr: bool,
    pub mount_setattr_error: Option<String>,
    pub landlock_abi: Option<i64>,
    pub missing_link_tools: Vec<String>,
    pub ipv6_in_linked_network: bool,
    pub network_modes: BTreeMap<String, NetworkMode>,
}

pub fn capabilities() -> Capabilities {
    let exists = |kind: &str| Path::new("/proc/self/ns").join(file_of(kind)).exists();
    let user = crate::host::user_namespaces();
    let privileged = unsafe { libc::geteuid() } == 0;
    let probe = mount_setattr_probe();
    let abi = crate::confine::abi_version();
    let missing = crate::link::missing_tools();
    let linked = user && missing.is_empty();
    let pid_error = if exists("pid") {
        pidns::probe()
    } else {
        Some(text(
            "this host has no {kind} namespaces, so it cannot run a job with --namespaces {kind}",
            &[("kind", "pid".to_owned())],
        ))
    };
    let mode = |available: bool, namespace: &str, link: &str| NetworkMode {
        available,
        namespace: namespace.to_owned(),
        link: link.to_owned(),
    };
    Capabilities {
        namespace_kinds: KINDS
            .iter()
            .copied()
            .chain(["net", "pid"])
            .map(|kind| (kind.to_owned(), exists(kind)))
            .collect(),
        requestable_namespaces: KINDS
            .iter()
            .filter(|kind| {
                exists(kind) && (privileged || user) && (**kind != "pid" || pid_error.is_none())
            })
            .map(|kind| (*kind).to_owned())
            .collect(),
        pid_namespace_requestable: pid_error.is_none(),
        pid_namespace_error: pid_error,
        unprivileged_user_namespaces: user,
        mount_setattr: probe.is_none(),
        mount_setattr_error: probe,
        landlock_abi: (abi > 0).then_some(abi),
        missing_link_tools: missing.iter().map(|tool| (*tool).to_owned()).collect(),
        ipv6_in_linked_network: false,
        network_modes: [
            ("host", mode(true, "host", "none")),
            ("none", mode(user, "per_job", "none")),
            ("proxy", mode(linked, "per_job", "per_queue_or_per_job")),
            ("wireguard", mode(linked, "per_job", "per_queue_or_per_job")),
            ("bandwidth", mode(linked, "per_job", "per_queue_or_per_job")),
            ("openvpn", mode(false, "per_job", "per_queue_or_per_job")),
        ]
        .into_iter()
        .map(|(name, mode)| (name.to_owned(), mode))
        .collect(),
    }
}
