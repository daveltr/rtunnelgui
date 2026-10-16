use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn configure(c: &mut Command) {
    c.creation_flags(CREATE_NO_WINDOW);
}

/// One process-wide job; children assigned to it die when this process exits (handle closes).
fn job() -> HANDLE {
    static JOB: OnceLock<isize> = OnceLock::new();
    *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h as isize
    }) as HANDLE
}

pub fn adopt(child: &Child) {
    let j = job();
    if !j.is_null() {
        unsafe { AssignProcessToJobObject(j, child.as_raw_handle() as HANDLE) };
    }
}
