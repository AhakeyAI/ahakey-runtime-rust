//! Parent-stdin and operating-system shutdown signals.
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
