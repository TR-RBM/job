use std::ffi::CStr;
use std::io;

pub fn renameat2(source: &CStr, destination: &CStr, flags: libc::c_uint) -> io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            flags,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
