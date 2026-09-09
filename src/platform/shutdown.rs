//! Register OS handlers before serving any readiness response. An async function
//! would register only when first polled, after the HTTP task could already run.
use std::io;

pub struct ShutdownSignal {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
}
impl ShutdownSignal {
    pub fn new() -> io::Result<Self> {
        #[cfg(unix)]
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            #[cfg(unix)]
            terminate: signal(SignalKind::terminate())?,
            #[cfg(unix)]
            interrupt: signal(SignalKind::interrupt())?,
            #[cfg(windows)]
            interrupt: tokio::signal::windows::ctrl_c()?,
        })
    }
    pub async fn wait(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        let event = tokio::select! {
            event = self.terminate.recv() => event,
            event = self.interrupt.recv() => event,
        };
        #[cfg(windows)]
        let event = self.interrupt.recv().await;
        event.ok_or_else(|| io::Error::other("OS shutdown signal stream closed"))
    }
}

#[cfg(all(test, unix))]
#[tokio::test]
async fn signals_are_registered_before_wait_is_polled() {
    let mut shutdown = ShutdownSignal::new().unwrap();
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: target only this test process. new() must already have installed
        // the handlers; a lazy registration regression terminates this test binary.
        assert_eq!(unsafe { libc::raise(signal) }, 0);
        tokio::time::timeout(std::time::Duration::from_secs(2), shutdown.wait())
            .await
            .expect("Signal sent before polling was lost")
            .unwrap();
    }
}
