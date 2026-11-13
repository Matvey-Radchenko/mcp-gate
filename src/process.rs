//! RAII ownership starts immediately after spawn, including failed initialization.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::OwnedSemaphorePermit,
};

pub(crate) struct ProcessOwner {
    child: Option<Child>,
    container: Option<crate::platform::docker::Container>,
    permit: Option<OwnedSemaphorePermit>,
    live: Arc<AtomicUsize>,
    #[cfg(windows)]
    job: Option<crate::platform::windows_job::Job>,
}
impl ProcessOwner {
    pub fn spawn(
        command: &mut Command,
        permit: Option<OwnedSemaphorePermit>,
        live: Arc<AtomicUsize>,
        container: Option<crate::platform::docker::Container>,
    ) -> std::io::Result<Self> {
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x00000004 | 0x08000000); // suspended, no console
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match crate::platform::windows_job::Job::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                return Err(error);
            }
        };
        // Unix also keeps a mutable child here so the Windows failure path can reap it.
        let _ = &mut child;
        live.fetch_add(1, Ordering::SeqCst);
        tracing::info!(pid = child.id(), "worker process started");
        Ok(Self {
            child: Some(child),
            container,
            permit,
            live,
            #[cfg(windows)]
            job: Some(job),
        })
    }
    pub fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned child until cleanup")
    }
    pub async fn close(mut self) {
        if let Some(child) = self.child.take() {
            cleanup(
                child,
                self.permit.take(),
                self.live.clone(),
                self.container.take(),
                #[cfg(windows)]
                self.job.take(),
            )
            .await;
        }
    }
}
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            tokio::spawn(cleanup(
                child,
                self.permit.take(),
                self.live.clone(),
                self.container.take(),
                #[cfg(windows)]
                self.job.take(),
            ));
        }
    }
}
async fn cleanup(
    mut child: Child,
    _permit: Option<OwnedSemaphorePermit>,
    live: Arc<AtomicUsize>,
    container: Option<crate::platform::docker::Container>,
    #[cfg(windows)] job: Option<crate::platform::windows_job::Job>,
) {
    let pid = child.id();
    if let Some(container) = container {
        container.stop().await;
    }
    #[cfg(unix)]
    cleanup_unix(&mut child, pid).await;
    #[cfg(windows)]
    cleanup_windows(&mut child, job.expect("Windows worker owns its job")).await;
    live.fetch_sub(1, Ordering::SeqCst);
    tracing::info!(pid, "worker process stopped");
}
#[cfg(windows)]
async fn cleanup_windows(child: &mut Child, job: crate::platform::windows_job::Job) {
    let _ = tokio::time::timeout(Duration::from_secs(6), child.wait()).await;
    loop {
        // A normally exited parent can still have descendants. Do not publish
        // an idle worker count or release its permit before the whole job exits.
        let result = async {
            job.terminate()?;
            tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Owned parent is still stopping",
                    )
                })??;
            job.wait_empty().await
        }
        .await;
        match result {
            Ok(()) => break,
            Err(error) => {
                tracing::error!(%error, "Owned job cleanup incomplete; worker remains busy");
                // Retain ownership and the maintenance barrier after a bounded
                // attempt fails. Only cleanup is retried, never backend actions.
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}
#[cfg(unix)]
fn signal_group(pid: Option<u32>, signal: i32) -> std::io::Result<()> {
    if let Some(pid) = pid {
        // SAFETY: only called before our owned child has been reaped. Its process
        // group was created at spawn; its ID cannot have been reused. No pointers.
        let result = unsafe { libc::kill(-(pid as i32), signal) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn cleanup_unix(child: &mut Child, pid: Option<u32>) {
    terminate_group(pid).await;
    reap_owned_child(child, pid).await;
    // SIGKILL delivery is asynchronous for descendants. Probe only after reap;
    // never send another signal, because the leader's PID could now be reused.
    while group_exists(pid) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(unix)]
async fn terminate_group(pid: Option<u32>) {
    // Keep the leader unreaped through both signals to prevent PID/group reuse.
    if let Err(error) = signal_group(pid, libc::SIGTERM) {
        tracing::warn!(
            code = error.raw_os_error(),
            "Owned process group TERM failed"
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Err(error) = signal_group(pid, libc::SIGKILL) {
        // Darwin can report EPERM when the group contains only an exited,
        // unreaped leader. Reap it and verify the group before releasing capacity.
        tracing::warn!(
            code = error.raw_os_error(),
            "Owned group KILL not delivered; verifying exit"
        );
    }
}

#[cfg(unix)]
async fn reap_owned_child(child: &mut Child, pid: Option<u32>) {
    loop {
        match child.wait().await {
            Ok(status) => {
                tracing::info!(pid, %status, "Owned backend reaped");
                break;
            }
            Err(error) => tracing::error!(
                code = error.raw_os_error(),
                "Owned backend reap failed; capacity retained"
            ),
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

#[cfg(unix)]
fn group_exists(pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        let mut member: libc::pid_t = 0;
        // SAFETY: PROC_PGRP_ONLY (2, sys/proc_info.h) reads at most one pid into
        // a valid, correctly sized buffer. A truncated nonempty list suffices.
        let bytes = unsafe {
            libc::proc_listpids(
                2,
                pid,
                (&mut member as *mut libc::pid_t).cast(),
                std::mem::size_of_val(&member) as i32,
            )
        };
        bytes < 0 || member != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: signal 0 only queries existence and dereferences no pointers.
        let result = unsafe { libc::kill(-(pid as i32), 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
