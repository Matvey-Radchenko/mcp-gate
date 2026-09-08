//! Assign the suspended child to a kill-on-close job before any backend code runs.
use std::{io, mem::size_of, ptr};
use tokio::process::Child;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

pub struct Job(HANDLE);
// SAFETY: the owned handle has no thread affinity; it is closed exactly once.
unsafe impl Send for Job {}
// SAFETY: Windows synchronizes operations on the job handle.
unsafe impl Sync for Job {}
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: this value exclusively owns a valid job handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Job {
    pub fn assign(child: &Child) -> io::Result<Self> {
        // SAFETY: all pointers are valid for their calls, and sizes match the ABI.
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let process = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("Missing child handle"))?;
            if AssignProcessToJobObject(handle, process) == 0 {
                return Err(io::Error::last_os_error());
            }
            resume(
                child
                    .id()
                    .ok_or_else(|| io::Error::other("Missing child PID"))?,
            )?;
            Ok(job)
        }
    }
    pub fn terminate(&self) {
        // SAFETY: the job owns only processes launched by this worker.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: the process is still suspended and owned; snapshot entries are size-tagged.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut found = Thread32First(snapshot, &mut entry);
        let mut result = Err(io::Error::other("Suspended backend thread not found"));
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    result = Err(io::Error::last_os_error());
                } else {
                    result = if ResumeThread(thread) == u32::MAX {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    };
                    CloseHandle(thread);
                }
                break;
            }
            found = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        result
    }
}
