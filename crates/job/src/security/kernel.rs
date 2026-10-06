use super::{CapDrop, Controls, Deny, message, seccomp};
use serde::{Deserialize, Serialize};
use std::io;

#[repr(C)]
struct Header {
    version: u32,
    pid: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Caps {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const SETPCAP: u32 = 1 << 8;

fn control(option: libc::c_int, second: libc::c_ulong, third: libc::c_ulong) -> libc::c_long {
    (unsafe {
        libc::prctl(
            option,
            second,
            third,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    }) as libc::c_long
}
fn check(result: libc::c_long) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn read_caps(out: &mut [Caps; 2]) -> io::Result<()> {
    let mut header = Header {
        version: 0x20080522,
        pid: 0,
    };
    check(unsafe { libc::syscall(libc::SYS_capget, &mut header, out.as_mut_ptr()) })
}
fn cap_last() -> io::Result<u32> {
    std::fs::read_to_string("/proc/sys/kernel/cap_last_cap")?
        .trim()
        .parse()
        .map_err(io::Error::other)
}
fn filter_available() -> io::Result<()> {
    let mut action = libc::SECCOMP_RET_KILL_PROCESS;
    check(unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_GET_ACTION_AVAIL,
            0u32,
            &mut action,
        )
    })
}
fn context(e: impl std::fmt::Display) -> String {
    format!(
        "{}: {e}",
        message("requested security control is unavailable")
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub no_new_privs: bool,
    pub cap_drop: Option<CapDrop>,
    #[serde(default)]
    pub cap_bounding_reduced: bool,
    pub seccomp_deny: Option<Deny>,
    pub seccomp_arch: Option<u32>,
}

pub struct Prepared {
    nnp: bool,
    cap_mask: u64,
    bounding: bool,
    filter: Vec<libc::sock_filter>,
    pub applied: Option<Applied>,
}
impl Prepared {
    pub fn new(controls: &Controls, enters_user_namespace: bool) -> Result<Self, String> {
        controls.validate()?;
        let mut prepared = Self {
            nnp: controls.requires_nnp(),
            cap_mask: 0,
            bounding: false,
            filter: Vec::new(),
            applied: None,
        };
        if !prepared.nnp {
            return Ok(prepared);
        }
        check(control(libc::PR_GET_NO_NEW_PRIVS, 0, 0)).map_err(context)?;
        let cap_drop = controls.cap_drop.clone().filter(CapDrop::active);
        if let Some(drop) = &cap_drop {
            prepared.cap_mask = drop.mask(cap_last().map_err(context)?)?;
            let mut caps = [Caps::default(); 2];
            read_caps(&mut caps).map_err(context)?;
            prepared.bounding = enters_user_namespace || caps[0].effective & SETPCAP != 0;
        }
        let seccomp_deny = controls.seccomp_deny.clone().filter(Deny::active);
        if let Some(deny) = &seccomp_deny {
            prepared.filter = seccomp::compile(deny)?;
            filter_available().map_err(context)?;
        }
        prepared.applied = Some(Applied {
            no_new_privs: true,
            cap_bounding_reduced: prepared.bounding,
            cap_drop,
            seccomp_arch: seccomp_deny.as_ref().and_then(|_| seccomp::architecture()),
            seccomp_deny,
        });
        Ok(prepared)
    }

    pub fn apply(&self) -> io::Result<()> {
        if self.nnp {
            check(control(libc::PR_SET_NO_NEW_PRIVS, 1, 0))?;
            if control(libc::PR_GET_NO_NEW_PRIVS, 0, 0) != 1 {
                return Err(io::Error::from_raw_os_error(libc::EACCES));
            }
        }
        if self.cap_mask != 0 {
            let mut caps = [Caps::default(); 2];
            read_caps(&mut caps)?;
            if self.bounding {
                if caps[0].effective & SETPCAP == 0 {
                    return Err(io::Error::from_raw_os_error(libc::EACCES));
                }
                for bit in 0..64 {
                    if self.cap_mask & (1u64 << bit) != 0 {
                        check(control(libc::PR_CAPBSET_DROP, bit, 0))?;
                        if control(libc::PR_CAPBSET_READ, bit, 0) != 0 {
                            return Err(io::Error::from_raw_os_error(libc::EACCES));
                        }
                    }
                }
            }
            for (index, entry) in caps.iter_mut().enumerate() {
                let keep = !((self.cap_mask >> (index * 32)) as u32);
                entry.effective &= keep;
                entry.permitted &= keep;
                entry.inheritable &= keep;
            }
            let header = Header {
                version: 0x20080522,
                pid: 0,
            };
            check(unsafe { libc::syscall(libc::SYS_capset, &header, caps.as_ptr()) })?;
            read_caps(&mut caps)?;
            for (index, entry) in caps.iter().enumerate() {
                let mask = (self.cap_mask >> (index * 32)) as u32;
                if (entry.effective | entry.permitted | entry.inheritable) & mask != 0 {
                    return Err(io::Error::from_raw_os_error(libc::EACCES));
                }
            }
            for bit in 0..64 {
                if self.cap_mask & (1u64 << bit) != 0 {
                    check(control(
                        libc::PR_CAP_AMBIENT,
                        libc::PR_CAP_AMBIENT_LOWER as libc::c_ulong,
                        bit,
                    ))?;
                    if control(
                        libc::PR_CAP_AMBIENT,
                        libc::PR_CAP_AMBIENT_IS_SET as libc::c_ulong,
                        bit,
                    ) != 0
                    {
                        return Err(io::Error::from_raw_os_error(libc::EACCES));
                    }
                }
            }
        }
        if !self.filter.is_empty() {
            let program = libc::sock_fprog {
                len: self.filter.len() as u16,
                filter: self.filter.as_ptr().cast_mut(),
            };
            check(unsafe {
                libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_SET_MODE_FILTER,
                    0u32,
                    &program,
                )
            })?;
            if control(libc::PR_GET_SECCOMP, 0, 0) != 2 {
                return Err(io::Error::from_raw_os_error(libc::EACCES));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub service_no_new_privs: Option<bool>,
    pub no_new_privs_error: Option<String>,
    pub service_seccomp_mode: Option<i32>,
    pub seccomp_arch: Option<u32>,
    pub seccomp_query_error: Option<String>,
    pub cap_last_cap: Option<u32>,
    pub cap_query_error: Option<String>,
    pub supported_cap_names: Vec<String>,
    pub supported_syscalls: Vec<String>,
}
pub fn capabilities() -> Capabilities {
    let mut out = Capabilities::default();
    let nnp = control(libc::PR_GET_NO_NEW_PRIVS, 0, 0);
    if nnp < 0 {
        out.no_new_privs_error = Some(io::Error::last_os_error().to_string());
    } else {
        out.service_no_new_privs = Some(nnp == 1);
    }
    let mode = control(libc::PR_GET_SECCOMP, 0, 0);
    if mode >= 0 {
        out.service_seccomp_mode = Some(mode as i32);
    }
    out.seccomp_arch = seccomp::architecture();
    out.seccomp_query_error = filter_available().err().map(|e| e.to_string());
    match cap_last() {
        Ok(last) => {
            out.cap_last_cap = Some(last);
            out.supported_cap_names = super::CAPABILITIES
                .iter()
                .take(last as usize + 1)
                .map(|v| (*v).to_owned())
                .collect();
            if last >= 64 {
                out.cap_query_error = Some(message("kernel capability range is unsupported"));
            }
        }
        Err(e) => out.cap_query_error = Some(e.to_string()),
    }
    if out.seccomp_arch.is_some() {
        out.supported_syscalls = seccomp::NAMES.iter().map(|v| (*v).to_owned()).collect();
    }
    out
}
