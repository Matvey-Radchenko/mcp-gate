//! Read-only process inventory for proving browser ownership and cleanup.
use std::collections::BTreeSet;

pub struct TrackedProcess {
    pid: u32,
    #[cfg(windows)]
    created: Option<u64>,
}
impl TrackedProcess {
    pub fn capture(pid: u32) -> Self {
        Self {
            pid,
            #[cfg(windows)]
            created: process_state(pid).map(|(created, _)| created),
        }
    }
    pub fn running(&self) -> bool {
        #[cfg(windows)]
        {
            // Windows can reuse the PID after a browser renderer exits. An
            // unrelated replacement is not an orphan belonging to the test.
            process_state(self.pid)
                .is_some_and(|(created, running)| Some(created) == self.created && running)
        }
        #[cfg(not(windows))]
        super::alive(self.pid)
    }
}

pub fn track(pids: &BTreeSet<u32>) -> Vec<TrackedProcess> {
    pids.iter()
        .map(|pid| TrackedProcess::capture(*pid))
        .collect()
}

#[cfg(windows)]
fn process_state(pid: u32) -> Option<(u64, bool)> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    // SAFETY: the handle is read-only and owned; all four FILETIME outputs are
    // valid. Close it immediately so the probe does not retain job references.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32),
                "Cannot inspect tracked process {pid}: {error}"
            );
            return None;
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let mut exit_code = 0;
        let result = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user)
            != 0
            && GetExitCodeProcess(handle, &mut exit_code) != 0;
        let error = std::io::Error::last_os_error();
        CloseHandle(handle);
        assert!(
            result,
            "Cannot inspect process identity and exit status: {error}"
        );
        Some((
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
            exit_code == 259,
        ))
    }
}

pub fn descendants(root: u32) -> BTreeSet<u32> {
    let rows = inventory();
    let mut found = BTreeSet::from([root]);
    loop {
        let next: Vec<_> = rows
            .iter()
            .filter(|(pid, parent)| found.contains(parent) && !found.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if next.is_empty() {
            break;
        }
        found.extend(next);
    }
    found.remove(&root);
    found
}

#[cfg(unix)]
fn inventory() -> Vec<(u32, u32)> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .collect()
}

#[cfg(windows)]
fn inventory() -> Vec<(u32, u32)> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    // SAFETY: snapshot is read-only; the initialized size-tagged entry is valid
    // for each API call. Its owned handle is closed before returning or asserting.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        assert_ne!(snapshot, INVALID_HANDLE_VALUE);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut rows = Vec::new();
        let mut found = Process32FirstW(snapshot, &mut entry);
        while found != 0 {
            rows.push((entry.th32ProcessID, entry.th32ParentProcessID));
            found = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        assert!(!rows.is_empty(), "Cannot inspect the native process tree");
        rows
    }
}
