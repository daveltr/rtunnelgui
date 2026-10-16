use std::process::{Child, Command};

pub fn configure(_c: &mut Command) {
    // Linux: ssh gets SIGTERM if we die, even by SIGKILL. macOS has no equivalent; Ctrl-C/SIGTERM
    // are handled in main via ctrlc, a hard kill of the app can orphan ssh there.
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        _c.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
            Ok(())
        });
    }
}

pub fn adopt(_child: &Child) {}
