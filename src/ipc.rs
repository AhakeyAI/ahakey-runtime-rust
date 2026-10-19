use crate::{
    error::{Error, Result},
    runtime::{Event, Runtime},
    wire::METHODS,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, broadcast},
    task::JoinSet,
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{Request as UpgradeRequest, Response as UpgradeResponse},
        protocol::WebSocketConfig,
    },
};
use uuid::Uuid;

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    jsonrpc: String,
    id: String,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: Version,
    client: Client,
    token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    major: u32,
    minor: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Client {
    name: String,
    version: String,
    kind: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceParams {
    device_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationParams {
    operation_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FinishResource {
    upload_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe {
    topics: Vec<String>,
    cursor: Option<Value>,
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| Error("INVALID_PARAMS"))
}
fn failure(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message, "data": {"code": message}}})
}
fn token_matches(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

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
async fn session(
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
    let mut authenticated = false;
    let mut transfer: Option<crate::oled::Transfer> = None;
    let mut subscription: Option<(String, broadcast::Receiver<Event>)> = None;
    let auth_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let outgoing = tokio::select! {
            _ = tokio::time::sleep_until(auth_deadline), if !authenticated => break,
            event = async {
                match &mut subscription {
                    Some((_, receiver)) => receiver.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // A slow reader reconnects with a new snapshot; never silently drop events.
                let Ok(event) = event else { break; };
                json!({"jsonrpc": "2.0", "method": "runtime.event", "params": {
                    "subscriptionId": subscription.as_ref().unwrap().0, "instanceId": runtime.instance_id,
                    "sequence": event.sequence.to_string(), "type": event.kind, "data": event.data }})
            }
            message = socket.next() => {
                let Some(message) = message else { break; };
                let text = match message? {
                    Message::Text(text) => text,
                    Message::Binary(bytes) => {
                        let result = if authenticated {
                            transfer.as_mut().ok_or(Error("RESOURCE_UPLOAD_NOT_FOUND")).and_then(|upload| upload.append(&bytes))
                        } else { Err(Error("PERMISSION_DENIED")) };
                        let reply = match result {
                            Ok(received) => json!({"jsonrpc":"2.0", "method":"resource.progress", "params": {
                                "uploadId":transfer.as_ref().unwrap().id, "receivedBytes":received}}),
                            Err(error) => json!({"jsonrpc":"2.0", "method":"resource.error", "params": {
                                "uploadId":transfer.as_ref().map(|upload| upload.id), "code":error.0}}),
                        };
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
                let outcome = if request.method == "runtime.hello" {
                    match decode::<Hello>(request.params) {
                        Ok(hello) if token_matches(&token, &hello.token) => {
                            if hello.protocol.major != 1 { Err(Error("PROTOCOL_MISMATCH")) }
                            else if hello.client.name.is_empty() || hello.client.version.is_empty() || hello.client.kind.is_empty() {
                                Err(Error("INVALID_PARAMS"))
                            } else {
                                let _requested_minor = hello.protocol.minor;
                                authenticated = true;
                                Ok(json!({"protocol":{"major":1,"minor":0}, "instanceId":runtime.instance_id,
                                    "runtimeVersion":env!("CARGO_PKG_VERSION"), "methods":METHODS,
                                    "limits":{"messageBytes":MAX_MESSAGE_BYTES,"resourceBytes":crate::oled::MAX_SOURCE_BYTES,
                                        "resourceChunkBytes":crate::oled::MAX_CHUNK_BYTES}}))
                            }
                        }
                        Ok(_) => { authenticated = false; subscription = None; transfer = None; Err(Error("PERMISSION_DENIED")) },
                        Err(error) => Err(error),
                    }
                } else if !authenticated { Err(Error("PERMISSION_DENIED")) }
                else if request.method == "runtime.subscribe" {
                    match decode::<Subscribe>(request.params) {
                        Ok(params) if params.topics == ["state"] && params.cursor.is_none() => {
                            let (snapshot, receiver) = runtime.subscribe().await;
                            let id = Uuid::new_v4().to_string();
                            subscription = Some((id.clone(), receiver));
                            Ok(json!({"subscriptionId":id,"snapshot":snapshot}))
                        }
                        _ => Err(Error("INVALID_PARAMS")),
                    }
                } else if request.method == "resource.begin" {
                    if transfer.is_some() { Err(Error("BUSY")) }
                    else {
                        match decode(request.params).and_then(crate::oled::Transfer::begin) {
                            Ok(upload) => {
                                let response = json!({"uploadId":upload.id,"chunkBytes":crate::oled::MAX_CHUNK_BYTES});
                                transfer = Some(upload);
                                Ok(response)
                            },
                            Err(error) => Err(error),
                        }
                    }
                } else if request.method == "resource.finish" {
                    match decode::<FinishResource>(request.params) {
                        Ok(params) => {
                            if transfer.as_ref().is_some_and(|t| t.id == params.upload_id && t.total > 0) {
                                match transfer.take().unwrap().complete(params.upload_id) {
                                    Ok((bytes, hash)) => runtime.resources.finish(bytes, hash).await.map(|resource| json!(resource)),
                                    Err(error) => Err(error),
                                }
                            } else { Err(Error("RESOURCE_UPLOAD_NOT_FOUND")) }
                        },
                        Err(error) => Err(error),
                    }
                } else if request.method == "resource.abort" {
                    match decode::<FinishResource>(request.params) {
                        Ok(params) if transfer.as_ref().is_some_and(|t| t.id == params.upload_id) => {
                            transfer = None;
                            Ok(json!({}))
                        },
                        Ok(_) => Err(Error("RESOURCE_UPLOAD_NOT_FOUND")),
                        Err(error) => Err(error),
                    }
                } else { dispatch(&runtime, &request.method, request.params).await };
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
        // One writer per connection, always send subscription response before queued events.
        tokio::time::timeout(
            Duration::from_secs(5),
            socket.send(Message::Text(outgoing.to_string().into())),
        )
        .await??;
    }
    Ok(())
}
fn request_valid(request: &Request) -> bool {
    request.jsonrpc == "2.0" && request.id.len() <= 128 && !request.id.is_empty()
}
async fn dispatch(runtime: &Runtime, method: &str, params: Value) -> Result<Value> {
    match method {
        "configuration.get" => {
            let params: DeviceParams = decode(params)?;
            Ok(json!(runtime.configuration(&params.device_id).await?))
        }
        "oled.get" => {
            let params: DeviceParams = decode(params)?;
            Ok(json!(runtime.oled_state(&params.device_id).await?))
        }
        "oled.upload" => {
            let operation = runtime.upload_oled(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "operation.pause" | "operation.resume" => {
            let params: OperationParams = decode(params)?;
            let operation = if method == "operation.pause" {
                runtime.pause_operation(&params.operation_id).await?
            } else {
                runtime.resume_operation(&params.operation_id).await?
            };
            Ok(json!(operation))
        }
        "operation.get" => {
            let params: OperationParams = decode(params)?;
            Ok(json!(runtime.operation(&params.operation_id).await?))
        }
        "keymap.apply" => {
            let operation = runtime.apply_keymap(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "configuration.apply" => {
            let operation = runtime.apply(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "device.connect" | "device.disconnect" => {
            let params: DeviceParams = decode(params)?;
            runtime
                .connect(&params.device_id, method == "device.connect")
                .await?;
            Ok(json!({}))
        }
        _ => Err(Error("UNSUPPORTED_CAPABILITY")),
    }
}

// Tungstenite requires its concrete HTTP error response in this callback.
#[allow(clippy::result_large_err)]
fn validate_upgrade(
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
