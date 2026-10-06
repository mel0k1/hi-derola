//! Windows Job Object wrapper for foreground bash runs.
//!
//! Killing cmd.exe alone leaves its grandchildren running (servers, watchers
//! spawned by the command). Every foreground bash run is therefore placed into
//! a private job object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: a timeout
//! calls TerminateJobObject to kill the whole tree immediately, and dropping
//! the handle is a final safety net that terminates any stragglers.

use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Owns the job handle for one bash run; terminating on drop would be
/// redundant with KILL_ON_JOB_CLOSE but explicit close keeps the lifetime
/// obvious.
pub struct Job(HANDLE);

// HANDLE is a raw *mut c_void and therefore !Send, but a kernel object handle
// is process-wide, not thread-affine: TerminateJobObject / CloseHandle are
// thread-safe. Without this, every future that touches an MCP client or a
// bash run (both keep a Job inside a Send state) stops being Send on windows.
unsafe impl Send for Job {}

impl Job {
    /// Places the process behind `proc` (a spawned Child's raw handle) into a
    /// fresh job object. Returns None when the job API fails; the caller then
    /// falls back to kill-on-drop of the direct child only.
    pub fn attach(proc: std::os::windows::io::RawHandle) -> Option<Job> {
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_BASIC_LIMIT_INFORMATION = std::mem::zeroed();
            info.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectBasicLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, proc) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }

    /// Kills every process currently in the job (the whole bash tree).
    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
