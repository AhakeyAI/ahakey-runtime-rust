use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, mock::MockX1, state::HardwareState},
    configuration::{Apply, Configuration},
    error::{Error, Result},
    ipc,
    runtime::Runtime,
    storage::{Journal, Record, Storage},
    wire::Operation,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Client {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    serial: usize,
    events: Vec<Value>,
}
impl Client {
    async fn open(endpoint: &str) -> Self {
        Self {
            socket: connect_async(endpoint).await.unwrap().0,
            serial: 0,
            events: vec![],
        }
    }
    async fn receive(&mut self) -> Value {
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
    async fn send(&mut self, method: &str, params: Value) -> String {
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
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        loop {
            let value = self.receive().await;
            if value["id"] == id {
                return value;
            }
            self.events.push(value);
        }
    }
    async fn hello(&mut self) -> Value {
        self.call("runtime.hello",json!({"protocol":{"major":1,"minor":0},"client":{"name":"test","version":"1","kind":"studio"},"token":"test-token"})).await
    }
    async fn subscribe(&mut self) -> Value {
        self.call(
            "runtime.subscribe",
            json!({"topics":["state"],"cursor":null}),
        )
        .await
    }
}
async fn ready(runtime: &Runtime) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if runtime.subscribe().await.0["devices"][0]["connectionState"] == "ready" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn terminal(runtime: &Runtime, id: &str) -> Operation {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(operation) = runtime.operation(id).await
                && operation.terminal()
            {
                break operation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn setup(
    hardware: Box<dyn Hardware>,
    simulated: bool,
    initial: Option<Configuration>,
) -> (TempDir, Arc<Runtime>) {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    runtime
        .register(
            AhaKey {
                device_id: "test-x1".into(),
                name: "Test X1".into(),
                hardware,
            },
            initial,
            simulated,
        )
        .await
        .unwrap();
    ready(&runtime).await;
    (directory, runtime)
}
fn apply(token: &str, id: &str) -> Apply {
    let mut request: Apply =
        serde_json::from_str(include_str!("../contracts/runtime-v1/fixtures/apply.json")).unwrap();
    request.device_id = "test-x1".into();
    request.operation_id = id.into();
    request.base_token = token.into();
    request
}

#[tokio::test]
async fn websocket_auth_save_disconnect_recovery_and_event_order() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut writer = Client::open(&endpoint).await;
    assert_eq!(
        writer.subscribe().await["error"]["data"]["code"],
        "PERMISSION_DENIED"
    );
    let hello = writer.hello().await;
    assert_eq!(hello["result"]["protocol"]["major"], 1);
    let mut observer = Client::open(&endpoint).await;
    observer.hello().await;
    let subscription = observer.subscribe().await;
    let snapshot_sequence = subscription["result"]["snapshot"]["sequence"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let document = writer
        .call("configuration.get", json!({"deviceId":"test-x1"}))
        .await;
    let original = document["result"]["configuration"].clone();
    let request = apply(
        document["result"]["baseToken"].as_str().unwrap(),
        "lost-acceptance",
    );
    writer.send("configuration.apply", json!(request)).await;
    // Close before reading the acceptance response. The operation must still finish.
    writer.socket.close(None).await.unwrap();
    drop(writer);
    let completed = terminal(&runtime, "lost-acceptance").await;
    assert_eq!(completed.status, "completed");
    let mut config_index = None;
    let mut completed_index = None;
    let mut last = snapshot_sequence;
    for index in 0..100 {
        let event = observer.receive().await;
        assert_eq!(
            event["params"]["subscriptionId"],
            subscription["result"]["subscriptionId"]
        );
        let sequence = event["params"]["sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(sequence > last);
        last = sequence;
        if event["params"]["type"] == "configuration.changed" {
            config_index = Some(index);
        }
        if event["params"]["type"] == "operation.changed"
            && event["params"]["data"]["status"] == "completed"
        {
            completed_index = Some(index);
            break;
        }
    }
    assert!(config_index.unwrap() < completed_index.unwrap());
    let after = observer
        .call("configuration.get", json!({"deviceId":"test-x1"}))
        .await;
    assert_eq!(
        after["result"]["configuration"]["device"]["brightnessPercent"],
        60
    );
    assert_ne!(
        after["result"]["baseToken"],
        document["result"]["baseToken"]
    );
    let mut expected: Configuration = serde_json::from_value(original).unwrap();
    expected.merge(&request.changes);
    assert_eq!(after["result"]["configuration"], json!(expected));
    let recovered = observer
        .call("operation.get", json!({"operationId":"lost-acceptance"}))
        .await;
    assert_eq!(recovered["result"]["status"], "completed");
    assert_eq!(
        observer.call("configuration.apply", json!(request)).await["result"]["status"],
        "completed"
    );
    let mut conflict = request.clone();
    conflict.changes.device.brightness_percent = 61;
    assert_eq!(
        observer.call("configuration.apply", json!(conflict)).await["error"]["data"]["code"],
        "OPERATION_ID_CONFLICT"
    );
    let mut stale = request.clone();
    stale.operation_id = "stale".into();
    assert_eq!(
        observer.call("configuration.apply", json!(stale)).await["error"]["data"]["code"],
        "BASE_CONFLICT"
    );
    observer.socket.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    runtime.shutdown().await;
    drop(runtime);
    let restarted = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        restarted.operation("lost-acceptance").await.unwrap().status,
        "completed"
    );
    assert_eq!(restarted.apply(request).await.unwrap().status, "completed");
    restarted.shutdown().await;
}

#[tokio::test]
async fn concurrent_admission_is_atomic_and_canonical_retries_are_idempotent() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let one = apply(&token, "one");
    let two = apply(&token, "two");
    let (a, b) = tokio::join!(runtime.apply(one.clone()), runtime.apply(two));
    assert!(a.is_ok());
    assert_eq!(b.unwrap_err().0, "BUSY");
    let mut reordered = one.clone();
    reordered.changes.modes[0].keys.reverse();
    reordered.changes.modes[0].lights.reverse();
    assert!(runtime.apply(reordered).await.is_ok());
    assert_eq!(
        runtime.connect("test-x1", false).await.unwrap_err().0,
        "BUSY"
    );
    assert_eq!(terminal(&runtime, "one").await.status, "completed");
    runtime.connect("test-x1", false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.subscribe().await.0["devices"][0]["connectionState"] != "disconnected" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let disconnected = runtime.configuration("test-x1").await.unwrap();
    assert!(disconnected.configuration.is_none());
    assert_ne!(disconnected.base_token, token);
    runtime.connect("test-x1", true).await.unwrap();
    ready(&runtime).await;
    assert!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .is_none()
    );
    runtime.shutdown().await;
}

struct FailingHardware {
    writes: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for FailingHardware {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(50),
            lever: Some("manual"),
            brightness: Some(35),
        })
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        if self.writes.fetch_add(1, Ordering::SeqCst) == 1 {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn partial_write_failure_invalidates_baseline_and_does_not_retry() {
    let writes = Arc::new(AtomicUsize::new(0));
    let (_directory, runtime) = setup(
        Box::new(FailingHardware {
            writes: writes.clone(),
        }),
        true,
        None,
    )
    .await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = apply(&token, "fail");
    runtime.apply(request.clone()).await.unwrap();
    let result = terminal(&runtime, "fail").await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.error_code.as_deref(), Some("DEVICE_TIMEOUT"));
    assert_eq!(result.effect, "unknown");
    assert!(result.progress > 0.0);
    assert!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .is_none()
    );
    assert_eq!(runtime.apply(request).await.unwrap().status, "failed");
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    runtime.shutdown().await;
}
#[tokio::test]
async fn real_device_baseline_requires_explicit_complete_initialization() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), false, None).await;
    let document = runtime.configuration("test-x1").await.unwrap();
    assert!(document.configuration.is_none());
    assert_eq!(
        runtime
            .apply(apply(&document.base_token, "unknown"))
            .await
            .unwrap_err()
            .0,
        "INCOMPLETE_WRITE_GROUP"
    );
    runtime.shutdown().await;
    let (_directory, runtime) = setup(
        Box::<MockX1>::default(),
        false,
        Some(Configuration::mock_defaults()),
    )
    .await;
    let operation = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshot = runtime.subscribe().await.0;
            if let Some(id) = snapshot["operations"][0]["operationId"].as_str() {
                break id.to_owned();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal(&runtime, &operation).await.status, "completed");
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .unwrap()
            .modes
            .len(),
        4
    );
    runtime.shutdown().await;
}
#[tokio::test]
async fn restart_marks_unfinished_operations_unknown_and_corrupt_storage_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let (storage, _) = Storage::open(path.clone()).await.unwrap();
    let mut journal = Journal::default();
    journal.operations.insert(
        "pending".into(),
        Record {
            request: apply("base", "pending").into(),
            operation: Operation {
                operation_id: "pending".into(),
                device_id: "test-x1".into(),
                status: "running".into(),
                progress: 0.4,
                effect: "partial".into(),
                error_code: None,
            },
        },
    );
    storage.save(&journal).await.unwrap();
    let runtime = Runtime::open(path.clone()).await.unwrap();
    let recovered = runtime.operation("pending").await.unwrap();
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.effect, "unknown");
    assert_eq!(recovered.error_code.as_deref(), Some("RUNTIME_RESTARTED"));
    runtime.shutdown().await;
    std::fs::write(path, b"broken").unwrap();
    assert!(
        Runtime::open(directory.path().join("journal.json"))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn rejects_browser_origin_wrong_token_and_unknown_mutation_fields() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut request = endpoint.clone().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://localhost".parse().unwrap());
    assert!(connect_async(request).await.is_err());
    let mut client = Client::open(&endpoint).await;
    let bad=client.call("runtime.hello",json!({"protocol":{"major":1,"minor":0},"client":{"name":"test","version":"1","kind":"studio"},"token":"bad"})).await;
    assert_eq!(bad["error"]["data"]["code"], "PERMISSION_DENIED");
    assert!(client.subscribe().await.get("result").is_none());
    client.hello().await;
    let response = client
        .call("device.connect", json!({"deviceId":"test-x1","extra":true}))
        .await;
    assert_eq!(response["error"]["code"], -32602);
    let response = client.call("made.up", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
    client
        .socket
        .send(Message::Text("[]".into()))
        .await
        .unwrap();
    assert_eq!(client.receive().await["error"]["code"], -32600);
    client.socket.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    runtime.shutdown().await;
}

struct DelayedNotification {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    writes: Arc<AtomicUsize>,
    fail: bool,
}
#[async_trait]
impl Hardware for DelayedNotification {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(35),
        })
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.connect().await
    }
    async fn next_event(&mut self) -> Result<HardwareState> {
        self.started.notify_one();
        self.release.notified().await;
        if self.fail {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            let mut state = self.connect().await?;
            state.brightness = Some(99);
            Ok(state)
        }
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn ready_notification_rechecks_baseline_before_queued_apply_can_write() {
    for fail in [true, false] {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let writes = Arc::new(AtomicUsize::new(0));
        let (_directory, runtime) = setup(
            Box::new(DelayedNotification {
                started: started.clone(),
                release: release.clone(),
                writes: writes.clone(),
                fail,
            }),
            true,
            None,
        )
        .await;
        let document = runtime.configuration("test-x1").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        // Make the notification ready before admission yields to the worker.
        // It must be projected before executing the newly queued write.
        release.notify_one();
        runtime
            .apply(apply(&document.base_token, "racing-notification"))
            .await
            .unwrap();
        let operation = terminal(&runtime, "racing-notification").await;
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.effect, "none");
        assert_eq!(
            operation.error_code.as_deref(),
            Some(if fail {
                "DEVICE_NOT_READY"
            } else {
                "BASE_CONFLICT"
            })
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert!(
            runtime
                .configuration("test-x1")
                .await
                .unwrap()
                .configuration
                .is_none()
        );
        runtime.shutdown().await;
    }
}

struct NotifyingHardware {
    events: tokio::sync::mpsc::Receiver<Result<HardwareState>>,
    refreshes: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for NotifyingHardware {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(35),
        })
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.connect().await
    }
    async fn next_event(&mut self) -> Result<HardwareState> {
        self.events
            .recv()
            .await
            .unwrap_or(Err(Error("DEVICE_DISCONNECTED")))
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn hardware_notifications_reach_websocket_without_polling() {
    let (sender, events) = tokio::sync::mpsc::channel(8);
    let refreshes = Arc::new(AtomicUsize::new(0));
    let (_directory, runtime) = setup(
        Box::new(NotifyingHardware {
            events,
            refreshes: refreshes.clone(),
        }),
        false,
        None,
    )
    .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    client.hello().await;
    client.subscribe().await;
    sender
        .send(Ok(HardwareState {
            work_mode: 3,
            light_mode: 0,
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(35),
        }))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_millis(500), client.receive())
        .await
        .expect("mode notification must reach Studio without waiting for a poll");
    assert_eq!(event["params"]["type"], "device.changed");
    assert_eq!(event["params"]["data"]["workMode"], 3);
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        0,
        "connected devices must not be polled"
    );
    sender
        .send(Err(Error("DEVICE_DISCONNECTED")))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let event = client.receive().await;
            if event["params"]["type"] == "device.changed" {
                assert_eq!(event["params"]["data"]["connectionState"], "disconnected");
                break;
            }
        }
    })
    .await
    .expect("link loss must be reported without polling");
    client.socket.close(None).await.unwrap();
    server.abort();
    runtime.shutdown().await;
}

#[tokio::test]
async fn keymap_ipc_works_without_full_configuration_and_survives_restart() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), false, None).await;
    let document = runtime.configuration("test-x1").await.unwrap();
    assert!(document.configuration.is_none());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    client.hello().await;
    let request = json!({"operationId":"keys-only", "deviceId":"test-x1",
        "baseToken":document.base_token, "mode":2,
        "keys":[{"index":0,"action":{"type":"shortcut","modifiers":[],"keyCode":7}}]});
    let accepted = client.call("keymap.apply", request.clone()).await;
    assert_eq!(accepted["result"]["status"], "accepted", "{accepted}");
    assert_eq!(terminal(&runtime, "keys-only").await.status, "completed");
    let after = runtime.configuration("test-x1").await.unwrap();
    assert!(
        after.configuration.is_none(),
        "a key write must not invent lights/brightness"
    );
    assert_ne!(after.base_token, document.base_token);
    assert_eq!(
        client.call("keymap.apply", request.clone()).await["result"]["status"],
        "completed"
    );
    let mut conflicting = request.clone();
    conflicting["keys"][0]["action"]["keyCode"] = json!(9);
    assert_eq!(
        client.call("keymap.apply", conflicting).await["error"]["data"]["code"],
        "OPERATION_ID_CONFLICT"
    );
    let mut stale = request.clone();
    stale["operationId"] = json!("stale-keys");
    assert_eq!(
        client.call("keymap.apply", stale).await["error"]["data"]["code"],
        "BASE_CONFLICT"
    );
    client.socket.close(None).await.unwrap();
    server.abort();
    runtime.shutdown().await;
    let restarted = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        restarted.operation("keys-only").await.unwrap().status,
        "completed"
    );
    restarted.shutdown().await;
}

struct KeyOnlyHardware {
    writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    fail_at: Option<usize>,
}
#[async_trait]
impl Hardware for KeyOnlyHardware {
    fn configuration_commands(&self, _: &Configuration) -> Result<Vec<Vec<u8>>> {
        Err(Error("UNSUPPORTED_CAPABILITY"))
    }
    fn keymap_commands(
        &self,
        request: &ahakey_runtime_rust::keymap::KeymapApply,
    ) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::keymap_commands(request)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 2,
            light_mode: 0,
            battery: Some(50),
            lever: Some("manual"),
            brightness: None,
        })
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.connect().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut writes = self.writes.lock().unwrap();
        writes.push(command.to_vec());
        if self.fail_at == Some(writes.len()) {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}
fn keys_request(token: &str, id: &str) -> ahakey_runtime_rust::keymap::KeymapApply {
    serde_json::from_value(json!({"operationId":id,"deviceId":"test-x1","baseToken":token,"mode":2,
        "keys":([7,9,13,14].iter().enumerate().map(|(index,code)| json!({"index":index,"action":{"type":"shortcut","modifiers":[],"keyCode":code}})).collect::<Vec<_>>())})).unwrap()
}

#[tokio::test]
async fn keymap_without_lighting_writes_only_requested_keys_then_save_and_reserves_worker() {
    let writes = Arc::new(std::sync::Mutex::new(vec![]));
    let (_directory, runtime) = setup(
        Box::new(KeyOnlyHardware {
            writes: writes.clone(),
            fail_at: None,
        }),
        false,
        None,
    )
    .await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = keys_request(&token, "dfjk");
    runtime.apply_keymap(request.clone()).await.unwrap();
    assert_eq!(
        runtime
            .apply_keymap(keys_request(&token, "concurrent"))
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        runtime
            .apply(apply(&token, "config-concurrent"))
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        runtime.connect("test-x1", false).await.unwrap_err().0,
        "BUSY"
    );
    assert_eq!(terminal(&runtime, "dfjk").await.status, "completed");
    let mut expected = vec![];
    for (index, code) in [7, 9, 13, 14].into_iter().enumerate() {
        expected.push(vec![0xaa, 0xbb, 0x73, 0x74, 2, index as u8, 0xcc, 0xdd]);
        expected.push(vec![
            0xaa,
            0xbb,
            0x73,
            0x73,
            2,
            index as u8,
            code,
            0xcc,
            0xdd,
        ]);
    }
    expected.push(vec![0xaa, 0xbb, 4, 0xcc, 0xdd]);
    assert_eq!(*writes.lock().unwrap(), expected);
    let mut reordered = request;
    reordered.keys.reverse();
    assert_eq!(
        runtime.apply_keymap(reordered).await.unwrap().status,
        "completed"
    );
    assert_eq!(writes.lock().unwrap().len(), 9);
    runtime.shutdown().await;
}

#[tokio::test]
async fn keymap_failure_is_durable_unknown_and_never_replayed() {
    let writes = Arc::new(std::sync::Mutex::new(vec![]));
    let (directory, runtime) = setup(
        Box::new(KeyOnlyHardware {
            writes: writes.clone(),
            fail_at: Some(2),
        }),
        false,
        None,
    )
    .await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = keys_request(&token, "key-failure");
    runtime.apply_keymap(request.clone()).await.unwrap();
    let result = terminal(&runtime, "key-failure").await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.effect, "unknown");
    assert_eq!(result.error_code.as_deref(), Some("DEVICE_TIMEOUT"));
    assert!(result.progress > 0.0);
    runtime.shutdown().await;
    let recovered = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        recovered.apply_keymap(request).await.unwrap().status,
        "failed"
    );
    assert_eq!(writes.lock().unwrap().len(), 2);
    recovered.shutdown().await;
}

#[tokio::test]
async fn keymap_preserves_known_unrelated_configuration() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let before = runtime.configuration("test-x1").await.unwrap();
    let mut request = keys_request(&before.base_token, "one-key");
    request.keys.truncate(1);
    let mut expected = before.configuration.unwrap();
    let mode = expected.modes.iter_mut().find(|m| m.mode == 2).unwrap();
    mode.keys
        .iter_mut()
        .find(|k| k.role == "voice")
        .unwrap()
        .action = request.keys[0].action.clone();
    runtime.apply_keymap(request).await.unwrap();
    assert_eq!(terminal(&runtime, "one-key").await.status, "completed");
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration,
        Some(expected)
    );
    runtime.shutdown().await;
}

#[test]
fn keymap_rejects_invalid_scope_actions_and_extra_fields() {
    let request = keys_request("base", "id");
    for bad in [
        json!({"mode":4}),
        json!({"keys":[]}),
        json!({"operationId":""}),
        json!({"baseToken":""}),
        json!({"keys":[{"index":4,"action":{"type":"disabled"}}]}),
        json!({"keys":[{"index":0,"action":{"type":"shortcut","modifiers":["bad"],"keyCode":7}}]}),
        json!({"keys":[{"index":0,"action":{"type":"macro","steps":[{"type":"delay","delayMs":1}]}}]}),
    ] {
        let mut value = json!(request);
        for (k, v) in bad.as_object().unwrap() {
            value[k] = v.clone();
        }
        let parsed: ahakey_runtime_rust::keymap::KeymapApply =
            serde_json::from_value(value).unwrap();
        assert_eq!(parsed.validate().unwrap_err().0, "INVALID_PARAMS");
    }
    let mut duplicate = request.clone();
    duplicate.keys[1].index = 0;
    assert!(duplicate.validate().is_err());
    let mut value = json!(request);
    value["brightnessPercent"] = json!(50);
    assert!(serde_json::from_value::<ahakey_runtime_rust::keymap::KeymapApply>(value).is_err());
}

#[test]
fn journal_still_reads_legacy_configuration_records() {
    let value = json!({"version":1,"operations":{"old":{"request":apply("base","old"),
        "operation":{"operationId":"old","deviceId":"test-x1","status":"completed","progress":1.0,"effect":"complete","errorCode":null}}},"configurations":{}});
    let journal: Journal = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(journal).unwrap(), value);
}
