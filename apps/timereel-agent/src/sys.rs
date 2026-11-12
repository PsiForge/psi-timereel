//! The few system calls the agent makes directly. This is the only module
//! that allows `unsafe`; each block is a single libc call with valid
//! arguments.
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

/// Offset of local time from UTC in minutes at the Unix time `unix_ms`
/// (positive east of Greenwich). Returns 0 if the local time is unknown.
pub fn local_offset_min(unix_ms: i64) -> i32 {
    let secs: libc::time_t = unix_ms.div_euclid(1000) as libc::time_t;
    // SAFETY: an all-zero `tm` is a valid value; `localtime_r` only writes to
    // the `tm` we pass and reads `secs`, and is thread-safe.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let res = unsafe { libc::localtime_r(&secs, &mut tm) };
    if res.is_null() {
        return 0;
    }
    (tm.tm_gmtoff / 60) as i32
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

    #[test]
    fn local_offset_is_a_sane_number_of_minutes() {
        let off = super::local_offset_min(1_747_215_000_000);
        assert!((-14 * 60..=14 * 60).contains(&off));
    }
}
