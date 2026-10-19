//! IPC client for the running Runtime. Never opens a second Bluetooth connection.
use ahakey_runtime_rust::{discovery, keymap::KeymapApply};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{error::Error, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
async fn call(socket: &mut Socket, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
    let id = Uuid::new_v4().to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await?;
        while let Some(frame) = socket.next().await {
            if let Message::Text(text) = frame? {
                let value: Value = serde_json::from_str(&text)?;
                if value["id"] == id {
                    if let Some(error) = value.get("error") {
                        return Err(format!("RPC {method}: {}", error["data"]["code"]).into());
                    }
                    return Ok(value["result"].clone());
                }
            }
        }
        Err("Runtime disconnected; query the original operation ID before retrying".into())
    })
    .await?
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut device_id = None;
    let mut mode = None;
    let mut keys = None;
    let mut apply = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--device-id" => device_id = Some(args.next().ok_or("missing device ID")?),
            "--mode" => {
                mode = Some(
                    args.next()
                        .ok_or("missing Studio mode 1..4")?
                        .parse::<u8>()?,
                )
            }
            "--keys" => keys = Some(args.next().ok_or("missing four lowercase letters")?),
            "--apply" => apply = true,
            "--help" => {
                println!(
                    "runtime-keymap --device-id x1-UUID --mode 1..4 --keys dfjk [--apply]\nMode numbers match Studio. Default is read-only preview; --apply writes via the running Runtime."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let device_id = device_id.ok_or("--device-id required")?;
    let mode = mode
        .filter(|m| (1..=4).contains(m))
        .ok_or("--mode must be 1..4 (Studio numbering)")?;
    let keys = keys
        .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_lowercase()))
        .ok_or("--keys must contain four lowercase letters, e.g. dfjk")?;
    let endpoint: Value = serde_json::from_slice(&std::fs::read(discovery::default_path()?)?)?;
    let url = endpoint["endpoint"].as_str().ok_or("missing endpoint")?;
    // This client only sends the private token to the loopback Runtime.
    let address = url
        .strip_prefix("ws://127.0.0.1:")
        .and_then(|s| s.strip_suffix('/'))
        .ok_or("non-loopback endpoint rejected")?;
    address.parse::<u16>()?;
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(10), connect_async(url)).await??;
    let hello = call(&mut socket,"runtime.hello",json!({"protocol":{"major":1,"minor":0},"client":{"name":"runtime-keymap","version":"1","kind":"cli"},"token":endpoint["token"]})).await?;
    if !hello["methods"]
        .as_array()
        .is_some_and(|m| m.iter().any(|m| m == "keymap.apply"))
    {
        return Err(
            "running Runtime does not support keymap.apply; restart with the updated build".into(),
        );
    }
    let snapshot = call(
        &mut socket,
        "runtime.subscribe",
        json!({"topics":["state"],"cursor":null}),
    )
    .await?;
    let device = snapshot["snapshot"]["devices"]
        .as_array()
        .and_then(|d| d.iter().find(|d| d["deviceId"] == device_id))
        .ok_or("selected device not found")?;
    if device["connectionState"] != "ready" {
        return Err("selected device is not ready".into());
    }
    let baseline = call(
        &mut socket,
        "configuration.get",
        json!({"deviceId":device_id}),
    )
    .await?;
    let request: KeymapApply = serde_json::from_value(
        json!({"operationId":Uuid::new_v4().to_string(),"deviceId":device_id,"baseToken":baseline["baseToken"],"mode":mode-1,
        "keys":keys.bytes().enumerate().map(|(index,key)|json!({"index":index,"action":{"type":"shortcut","modifiers":[],"keyCode":key-b'a'+4}})).collect::<Vec<_>>()}),
    )?;
    request.validate()?;
    println!(
        "Device: {}; Studio mode {mode} (firmware {}); keys: {keys}",
        device["name"],
        mode - 1
    );
    if apply {
        println!("Operation: {}", request.operation_id);
        call(&mut socket, "keymap.apply", json!(request)).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut previous = Value::Null;
        loop {
            let op = call(
                &mut socket,
                "operation.get",
                json!({"operationId":request.operation_id}),
            )
            .await?;
            if op != previous {
                println!(
                    "{}: {}%",
                    op["status"],
                    op["progress"].as_f64().unwrap_or(0.0) * 100.0
                );
                previous = op.clone();
            }
            match op["status"].as_str() {
                Some("completed") => {
                    println!(
                        "PASS: Runtime confirmed every key command and save ACK. Verify physical output separately."
                    );
                    break;
                }
                Some("failed" | "cancelled") => {
                    return Err(format!(
                        "operation failed: {}; effect={}",
                        op["errorCode"], op["effect"]
                    )
                    .into());
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("operation still pending; query the original operation ID; do not blindly resubmit".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    } else {
        println!("Read-only preview. Add --apply to write these four bindings through Runtime.");
    }
    socket.close(None).await?;
    Ok(())
}
