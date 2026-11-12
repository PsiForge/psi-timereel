//! The one system call the viewer makes directly. This is the only module
//! that allows `unsafe`.
#![allow(unsafe_code)]

/// Turns off core dumps and blocks `ptrace` by other non-root processes
/// (`prctl(PR_SET_DUMPABLE, 0)`, architecture section 6).
pub fn disable_dumping() -> std::io::Result<()> {
    // SAFETY: PR_SET_DUMPABLE takes one integer argument and touches no memory.
    let rc = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dumping_can_be_disabled() {
        super::disable_dumping().expect("prctl");
        // SAFETY: PR_GET_DUMPABLE takes no arguments and touches no memory.
        let v = unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) };
        assert_eq!(v, 0);
    }
}
