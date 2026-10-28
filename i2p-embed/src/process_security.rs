//! Process-wide dump mitigations shared by GUI, agent, bots and relay.
//! Windows SetErrorMode controls error dialogs; it does not disable all dumps.

pub fn harden_process() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn hardening_is_applied_in_an_isolated_child() {
        const CHILD: &str = "GIPNY_HARDEN_PROCESS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            super::harden_process().unwrap();
            let mut limit = libc::rlimit { rlim_cur: 1, rlim_max: 1 };
            assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) }, 0);
            assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
            #[cfg(any(target_os = "linux", target_os = "android"))]
            assert_eq!(unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) }, 0);
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact").arg("process_security::tests::hardening_is_applied_in_an_isolated_child")
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
    }
}
