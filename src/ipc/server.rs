//! Loopback listener, client admission and native-client upgrade validation.
use super::session::session;
use crate::runtime::Runtime;
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as UpgradeRequest, Response as UpgradeResponse,
};
pub async fn serve(
    listener: TcpListener,
    runtime: Arc<Runtime>,
    token: String,
) -> std::io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::other("Runtime requires loopback"));
    }
    let slots = Arc::new(Semaphore::new(16));
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            connection = listener.accept() => {
                let (stream, peer) = connection?;
                if !peer.ip().is_loopback() { continue; }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let runtime = runtime.clone(); let token = token.clone();
                clients.spawn(async move {
                    let _permit = permit;
                    let _ = session(stream, runtime, token).await;
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}
// Tungstenite requires its concrete HTTP error response in this callback.
#[allow(clippy::result_large_err)]
pub(super) fn validate_upgrade(
    request: &UpgradeRequest,
    response: UpgradeResponse,
) -> std::result::Result<
    UpgradeResponse,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    if request.headers().contains_key("origin")
        || request.uri().path() != "/"
        || request.uri().query().is_some()
    {
        let mut denied =
            tokio_tungstenite::tungstenite::http::Response::new(Some("Native clients only".into()));
        *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
        Err(denied)
    } else {
        Ok(response)
    }
}
