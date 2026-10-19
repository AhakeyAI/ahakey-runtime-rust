//! Per-client authentication, subscription and owned file-transfer state.
use super::MAX_MESSAGE_BYTES;
use super::{
    dispatch::dispatch,
    protocol::{Hello, Subscribe, decode, token_matches},
    resources,
};
use crate::{
    error::{Error, Result},
    oled::Transfer,
    runtime::{Event, Runtime},
    wire::METHODS,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;
#[derive(Default)]
pub(super) struct ClientSession {
    pub(super) authenticated: bool,
    pub(super) subscription: Option<(String, broadcast::Receiver<Event>)>,
    transfer: Option<Transfer>,
}
impl ClientSession {
    pub(super) async fn request(
        &mut self,
        runtime: &Runtime,
        token: &str,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        match method {
            "runtime.hello" => self.hello(runtime, token, params),
            _ if !self.authenticated => Err(Error("PERMISSION_DENIED")),
            "runtime.subscribe" => self.subscribe(runtime, params).await,
            "resource.begin" => resources::begin(&mut self.transfer, params),
            "resource.finish" => resources::finish(runtime, &mut self.transfer, params).await,
            "resource.abort" => resources::abort(&mut self.transfer, params),
            _ => dispatch(runtime, method, params).await,
        }
    }
    fn hello(&mut self, runtime: &Runtime, token: &str, params: Value) -> Result<Value> {
        match decode::<Hello>(params) {
            Ok(hello) if token_matches(token, &hello.token) => {
                if hello.protocol.major != 1 {
                    Err(Error("PROTOCOL_MISMATCH"))
                } else if hello.client.name.is_empty()
                    || hello.client.version.is_empty()
                    || hello.client.kind.is_empty()
                {
                    Err(Error("INVALID_PARAMS"))
                } else {
                    let _requested_minor = hello.protocol.minor;
                    self.authenticated = true;
                    Ok(
                        json!({"protocol":{"major":1,"minor":0}, "instanceId":runtime.instance_id,
                        "runtimeVersion":env!("CARGO_PKG_VERSION"), "methods":METHODS,
                        "limits":{"messageBytes":MAX_MESSAGE_BYTES,"resourceBytes":crate::oled::MAX_SOURCE_BYTES,
                            "resourceChunkBytes":crate::oled::MAX_CHUNK_BYTES}}),
                    )
                }
            }
            Ok(_) => {
                self.authenticated = false;
                self.subscription = None;
                self.transfer = None;
                Err(Error("PERMISSION_DENIED"))
            }
            Err(error) => Err(error),
        }
    }
    async fn subscribe(&mut self, runtime: &Runtime, params: Value) -> Result<Value> {
        match decode::<Subscribe>(params) {
            Ok(params) if params.topics == ["state"] && params.cursor.is_none() => {
                let (snapshot, receiver) = runtime.subscribe().await;
                let id = Uuid::new_v4().to_string();
                self.subscription = Some((id.clone(), receiver));
                Ok(json!({"subscriptionId":id,"snapshot":snapshot}))
            }
            _ => Err(Error("INVALID_PARAMS")),
        }
    }
    pub(super) fn binary(&mut self, bytes: &[u8]) -> Value {
        let result = if self.authenticated {
            self.transfer
                .as_mut()
                .ok_or(Error("RESOURCE_UPLOAD_NOT_FOUND"))
                .and_then(|upload| upload.append(bytes))
        } else {
            Err(Error("PERMISSION_DENIED"))
        };
        match result {
            Ok(received) => json!({"jsonrpc":"2.0", "method":"resource.progress", "params": {
                "uploadId":self.transfer.as_ref().unwrap().id, "receivedBytes":received}}),
            Err(error) => json!({"jsonrpc":"2.0", "method":"resource.error", "params": {
                "uploadId":self.transfer.as_ref().map(|upload| upload.id), "code":error.0}}),
        }
    }
}
