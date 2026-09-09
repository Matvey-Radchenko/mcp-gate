//! Read-only process inventory for proving browser ownership and cleanup.
use std::collections::BTreeSet;

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
