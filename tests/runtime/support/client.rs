use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
pub(crate) struct Client {
    pub(crate) socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub(crate) serial: usize,
    pub(crate) events: Vec<Value>,
}
impl Client {
    pub(crate) async fn open(endpoint: &str) -> Self {
        Self {
            socket: connect_async(endpoint).await.unwrap().0,
            serial: 0,
            events: vec![],
        }
    }
    pub(crate) async fn receive(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), self.socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }
    pub(crate) async fn send(&mut self, method: &str, params: Value) -> String {
        self.serial += 1;
        let id = self.serial.to_string();
        self.socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        id
    }
    pub(crate) async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        loop {
            let value = self.receive().await;
            if value["id"] == id {
                return value;
            }
            self.events.push(value);
        }
    }
    pub(crate) async fn hello(&mut self) -> Value {
        self.call("runtime.hello",json!({"protocol":{"major":1,"minor":0},"client":{"name":"test","version":"1","kind":"studio"},"token":"test-token"})).await
    }
    pub(crate) async fn subscribe(&mut self) -> Value {
        self.call(
            "runtime.subscribe",
            json!({"topics":["state"],"cursor":null}),
        )
        .await
    }
}
