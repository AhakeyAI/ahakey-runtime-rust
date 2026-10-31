//! Parent-stdin and operating-system shutdown signals.
/// Registration normally finishes discovery; a panic must reach the supervisor
/// instead of leaving a healthy IPC listener with no device discovery running.
pub(super) async fn discovery_failed(
    task: &mut Option<tokio::task::JoinHandle<()>>,
) -> tokio::task::JoinError {
    if let Some(handle) = task.as_mut() {
        let result = handle.await;
        *task = None;
        if let Err(error) = result {
            return error;
        }
    }
    std::future::pending().await
}

pub(super) async fn parent_closed() {
    // A dedicated thread avoids keeping Tokio's blocking pool alive on SIGTERM
    // while the parent still owns the pipe's write end.
    let (closed, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut input = std::io::stdin().lock();
        let mut buffer = [0; 32];
        while let Ok(count) = input.read(&mut buffer) {
            if count == 0 {
                break;
            }
        }
        let _ = closed.send(());
    });
    let _ = receiver.await;
}
pub(super) async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

#[cfg(test)]
mod tests {
    use super::discovery_failed;
    use std::time::Duration;

    #[tokio::test]
    async fn discovery_panic_is_reported_for_process_recovery() {
        let mut task = Some(tokio::spawn(async { panic!("discovery failed") }));
        let error = tokio::time::timeout(Duration::from_secs(1), discovery_failed(&mut task))
            .await
            .expect("discovery panic must not be swallowed");
        assert!(error.is_panic());
        assert!(task.is_none());
    }

    #[tokio::test]
    async fn successful_registration_or_mock_mode_does_not_stop_runtime() {
        let mut task = Some(tokio::spawn(async {}));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), discovery_failed(&mut task))
                .await
                .is_err()
        );
        assert!(task.is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), discovery_failed(&mut task))
                .await
                .is_err()
        );
    }
}
