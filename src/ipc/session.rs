//! One socket reader/writer, timeouts, decoding and ordered event delivery.
use super::{
    MAX_MESSAGE_BYTES,
    client::ClientSession,
    protocol::{Request, failure, request_valid},
    server::validate_upgrade,
};
use crate::{runtime::Runtime, wire::METHODS};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
pub(super) async fn session(
    stream: TcpStream,
    runtime: Arc<Runtime>,
    token: String,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_MESSAGE_BYTES * 2);
    let mut socket = tokio::time::timeout(
        Duration::from_secs(10),
        accept_hdr_async_with_config(stream, validate_upgrade, Some(config)),
    )
    .await??;
    let mut client = ClientSession::default();
    let auth_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let outgoing = tokio::select! {
            _ = tokio::time::sleep_until(auth_deadline), if !client.authenticated => break,
            event = async {
                match &mut client.subscription {
                    Some((_, receiver)) => receiver.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // A slow reader reconnects with a new snapshot; never silently drop events.
                let Ok(event) = event else { break; };
                json!({"jsonrpc": "2.0", "method": "runtime.event", "params": {
                    "subscriptionId": client.subscription.as_ref().unwrap().0, "instanceId": runtime.instance_id,
                    "sequence": event.sequence.to_string(), "type": event.kind, "data": event.data }})
            }
            message = socket.next() => {
                let Some(message) = message else { break; };
                let text = match message? {
                    Message::Text(text) => text,
                    Message::Binary(bytes) => {
                        let reply = client.binary(&bytes);
                        tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Text(reply.to_string().into()))).await??;
                        continue;
                    },
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => { socket.flush().await?; continue; },
                    _ => break,
                };
                let value: Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(_) => {
                        tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Text(failure(Value::Null, -32700, "PARSE_ERROR").to_string().into()))).await??;
                        continue;
                    }
                };
                // v1 only accepts requests; valid notifications receive no response or side effects.
                if value.is_object() && value.get("id").is_none() && value["jsonrpc"] == "2.0" && value["method"].is_string() { continue; }
                let request: Request = match serde_json::from_value(value) {
                    Ok(request) if request_valid(&request) => request,
                    _ => {
                        tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Text(failure(Value::Null, -32600, "INVALID_REQUEST").to_string().into()))).await??;
                        continue;
                    }
                };
                let started = std::time::Instant::now();
                let outcome = client.request(&runtime,&token,&request.method,request.params).await;
                crate::debug::event("ipc.request", || json!({
                    "method":if request.method == "runtime.hello" || METHODS.contains(&request.method.as_str()) { request.method.as_str() } else { "unknown" },
                    "durationMs":started.elapsed().as_secs_f64() * 1000.0,
                    "error":outcome.as_ref().err().map(|error| error.0),
                }));
                match outcome {
                    Ok(result) => json!({"jsonrpc":"2.0", "id":request.id, "result":result}),
                    Err(error) => failure(json!(request.id), match error.0 { "INVALID_PARAMS" => -32602, "UNSUPPORTED_CAPABILITY" => -32601, _ => -32000 }, error.0),
                }
            }
        };
        // One writer per connection, always send client.subscription response before queued events.
        tokio::time::timeout(
            Duration::from_secs(5),
            socket.send(Message::Text(outgoing.to_string().into())),
        )
        .await??;
    }
    Ok(())
}
