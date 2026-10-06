use super::{Deny, message};

pub const NAMES: &[&str] = &[
    "getppid",
    "getuid",
    "getgid",
    "uname",
    "getcwd",
    "ptrace",
    "process_vm_readv",
    "process_vm_writev",
    "kcmp",
    "bpf",
    "userfaultfd",
    "perf_event_open",
    "keyctl",
    "add_key",
    "request_key",
    "mount",
    "umount2",
    "pivot_root",
    "chroot",
    "unshare",
    "setns",
    "clone",
    "clone3",
    "socket",
    "socketpair",
    "connect",
    "accept",
    "accept4",
    "bind",
    "listen",
    "sendto",
    "recvfrom",
    "io_uring_setup",
    "io_uring_enter",
    "io_uring_register",
    "reboot",
    "kexec_load",
    "init_module",
    "finit_module",
    "delete_module",
    "open_by_handle_at",
    "name_to_handle_at",
    "sethostname",
    "setdomainname",
    "syslog",
    "acct",
    "swapon",
    "swapoff",
    "fsopen",
    "fsconfig",
    "fsmount",
    "fspick",
    "move_mount",
    "open_tree",
    "mount_setattr",
    "kexec_file_load",
    "sendmsg",
    "recvmsg",
    "sendmmsg",
    "recvmmsg",
];

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn number(name: &str) -> u32 {
    (match name {
        "getppid" => libc::SYS_getppid,
        "getuid" => libc::SYS_getuid,
        "getgid" => libc::SYS_getgid,
        "uname" => libc::SYS_uname,
        "getcwd" => libc::SYS_getcwd,
        "ptrace" => libc::SYS_ptrace,
        "process_vm_readv" => libc::SYS_process_vm_readv,
        "process_vm_writev" => libc::SYS_process_vm_writev,
        "kcmp" => libc::SYS_kcmp,
        "bpf" => libc::SYS_bpf,
        "userfaultfd" => libc::SYS_userfaultfd,
        "perf_event_open" => libc::SYS_perf_event_open,
        "keyctl" => libc::SYS_keyctl,
        "add_key" => libc::SYS_add_key,
        "request_key" => libc::SYS_request_key,
        "mount" => libc::SYS_mount,
        "umount2" => libc::SYS_umount2,
        "pivot_root" => libc::SYS_pivot_root,
        "chroot" => libc::SYS_chroot,
        "unshare" => libc::SYS_unshare,
        "setns" => libc::SYS_setns,
        "clone" => libc::SYS_clone,
        "clone3" => libc::SYS_clone3,
        "socket" => libc::SYS_socket,
        "socketpair" => libc::SYS_socketpair,
        "connect" => libc::SYS_connect,
        "accept" => libc::SYS_accept,
        "accept4" => libc::SYS_accept4,
        "bind" => libc::SYS_bind,
        "listen" => libc::SYS_listen,
        "sendto" => libc::SYS_sendto,
        "recvfrom" => libc::SYS_recvfrom,
        "io_uring_setup" => libc::SYS_io_uring_setup,
        "io_uring_enter" => libc::SYS_io_uring_enter,
        "io_uring_register" => libc::SYS_io_uring_register,
        "reboot" => libc::SYS_reboot,
        "kexec_load" => libc::SYS_kexec_load,
        "init_module" => libc::SYS_init_module,
        "finit_module" => libc::SYS_finit_module,
        "delete_module" => libc::SYS_delete_module,
        "open_by_handle_at" => libc::SYS_open_by_handle_at,
        "name_to_handle_at" => libc::SYS_name_to_handle_at,
        "sethostname" => libc::SYS_sethostname,
        "setdomainname" => libc::SYS_setdomainname,
        "syslog" => libc::SYS_syslog,
        "acct" => libc::SYS_acct,
        "swapon" => libc::SYS_swapon,
        "swapoff" => libc::SYS_swapoff,
        "fsopen" => libc::SYS_fsopen,
        "fsconfig" => libc::SYS_fsconfig,
        "fsmount" => libc::SYS_fsmount,
        "fspick" => libc::SYS_fspick,
        "move_mount" => libc::SYS_move_mount,
        "open_tree" => libc::SYS_open_tree,
        "mount_setattr" => libc::SYS_mount_setattr,
        "kexec_file_load" => libc::SYS_kexec_file_load,
        "sendmsg" => libc::SYS_sendmsg,
        "recvmsg" => libc::SYS_recvmsg,
        "sendmmsg" => libc::SYS_sendmmsg,
        "recvmmsg" => libc::SYS_recvmmsg,
        _ => unreachable!(),
    }) as u32
}

pub fn architecture() -> Option<u32> {
    #[cfg(target_arch = "x86_64")]
    {
        Some(0xc000003e)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Some(0xc00000b7)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

pub fn compile(policy: &Deny) -> Result<Vec<libc::sock_filter>, String> {
    let arch =
        architecture().ok_or_else(|| message("seccomp native architecture is unsupported"))?;
    let stmt = |code, k| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |code, k, jt, jf| libc::sock_filter { code, jt, jf, k };
    let ld = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let jeq = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let ret = libc::BPF_RET as u16;
    let mut program = vec![
        stmt(ld, std::mem::offset_of!(libc::seccomp_data, arch) as u32),
        jump(jeq, arch, 1, 0),
        stmt(ret, libc::SECCOMP_RET_KILL_PROCESS),
        stmt(ld, std::mem::offset_of!(libc::seccomp_data, nr) as u32),
    ];
    #[cfg(target_arch = "x86_64")]
    program.extend([
        jump(
            (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16,
            0x40000000,
            0,
            1,
        ),
        stmt(ret, libc::SECCOMP_RET_KILL_PROCESS),
    ]);
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    for name in policy.names() {
        program.push(jump(jeq, number(name), 0, 1));
        let error = if name == "clone3" {
            libc::ENOSYS
        } else {
            libc::EPERM
        };
        program.push(stmt(ret, libc::SECCOMP_RET_ERRNO | error as u32));
    }
    program.push(stmt(ret, libc::SECCOMP_RET_ALLOW));
    Ok(program)
}
