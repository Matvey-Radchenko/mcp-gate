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
    {
        // Keep the leader unreaped through both signals so its PID/process-group
        // ID cannot be reused by an unrelated process between TERM and KILL.
        signal_group(pid, libc::SIGTERM);
        tokio::time::sleep(Duration::from_secs(2)).await;
        signal_group(pid, libc::SIGKILL);
        let _ = child.wait().await;
    }
    #[cfg(windows)]
    {
        if tokio::time::timeout(Duration::from_secs(6), child.wait())
            .await
            .is_err()
        {
            if let Some(job) = &job {
                job.terminate();
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        // Even a normally exited parent may have surviving descendants.
        drop(job);
    }
    live.fetch_sub(1, Ordering::SeqCst);
    tracing::info!(pid, "worker process stopped");
}
#[cfg(unix)]
fn signal_group(pid: Option<u32>, signal: i32) {
    if let Some(pid) = pid {
        // SAFETY: only called before our owned child has been reaped. Its process
        // group was created at spawn; its ID cannot have been reused. No pointers.
        unsafe {
            libc::kill(-(pid as i32), signal);
        }
    }
}
