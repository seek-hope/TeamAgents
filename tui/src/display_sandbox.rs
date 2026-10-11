//! Linux display-only process limits. Installed before accepting untrusted input.
//! No file opens, sockets, new processes, exec, or cross-process access after sealing.
#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn limits() -> Result<(), String> {
    for (resource, value) in [
        (libc::RLIMIT_AS, 768 * 1024 * 1024),
        (libc::RLIMIT_CPU, 2),
        (libc::RLIMIT_STACK, 8 * 1024 * 1024),
        (libc::RLIMIT_NOFILE, 32),
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_FSIZE, 0),
    ] {
        let limit = libc::rlimit { rlim_cur: value, rlim_max: value };
        // SAFETY: pointer names a valid rlimit; this runs only in the standalone helper.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(format!("display resource limit: {}", std::io::Error::last_os_error()));
        }
    }
    // SAFETY: close only inherited descriptors in the standalone helper, retaining its three pipes.
    if unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } != 0 {
        return Err(format!("display descriptor isolation: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn seal() -> Result<(), String> {
    fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }
    // seccomp_data has nr at 0 and arch at 4. Reject other ABIs (including x32 syscall numbers).
    #[cfg(target_arch = "x86_64")]
    let arch = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    let arch = 0xc00000b7;
    let mut filter = vec![
        instruction(0x20, 0, 0, 4),
        instruction(0x15, 1, 0, arch),
        instruction(0x06, 0, 0, 0x80000000), // SECCOMP_RET_KILL_PROCESS
        instruction(0x20, 0, 0, 0),
    ];
    for syscall in [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_brk,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_sched_yield,
        libc::SYS_getrandom,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        filter.push(instruction(0x15, 0, 1, syscall as u32));
        filter.push(instruction(0x06, 0, 0, 0x7fff0000)); // SECCOMP_RET_ALLOW
    }
    filter.push(instruction(0x06, 0, 0, 0x00050000 | libc::EPERM as u32));
    let program = libc::sock_fprog { len: filter.len() as u16, filter: filter.as_mut_ptr() };
    // SAFETY: filter and program stay alive for prctl; installation copies the validated program.
    // No pre_exec closure or shared process is involved; failure prevents any input parsing.
    let result = unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            -1
        } else {
            libc::prctl(libc::PR_SET_SECCOMP, 2, &program)
        }
    };
    if result != 0 {
        return Err(format!("display syscall isolation: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
pub(crate) fn limits() -> Result<(), String> {
    Err("display process isolation is unavailable on this platform; source remains visible".into())
}
#[cfg(not(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
pub(crate) fn seal() -> Result<(), String> {
    limits()
}
