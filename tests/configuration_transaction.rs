use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, state::HardwareState},
    configuration::{
        Action, Configuration, ConfigurationReadback, ReadbackDevice, ReadbackKey, ReadbackMode,
        keymap::{KeyBinding, KeymapApply},
        patch::{ConfigurationPatch, DescriptionPatch},
        transaction::Plan,
    },
    error::{Error, Result},
    operation::{Mutation, Operation},
    runtime::Runtime,
};
use async_trait::async_trait;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct Device {
    config: Arc<Mutex<Configuration>>,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
    block_at: Arc<AtomicUsize>,
    fail_at: Arc<AtomicUsize>,
    gate: Arc<Semaphore>,
    reads: Arc<AtomicUsize>,
}
impl Device {
    fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(Configuration::mock_defaults())),
            writes: Default::default(),
            block_at: Arc::new(AtomicUsize::new(1)),
            fail_at: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Semaphore::new(0)),
            reads: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn device(&self) -> AhaKey {
        AhaKey {
            device_id: "transaction-x1".into(),
            name: "X1".into(),
            hardware: Box::new(self.clone()),
        }
    }
}
#[async_trait]
impl Hardware for Device {
    fn configuration_plan(&self, request: &Mutation) -> Result<Option<Plan>> {
        ahakey_runtime_rust::ahakey::x1::commands::transaction_plan(request).map(Some)
    }
    fn configuration_commands(&self, request: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(request)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            brightness: Some(self.config.lock().unwrap().device.brightness_percent),
            work_mode: 0,
            light_mode: 0,
            battery: Some(80),
            lever: Some("manual"),
        })
    }
    async fn read_configuration(&mut self) -> Result<Option<ConfigurationReadback>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let config = self.config.lock().unwrap().clone();
        Ok(Some(ConfigurationReadback {
            device: ReadbackDevice {
                brightness_percent: config.device.brightness_percent,
                task_light_mode: None,
            },
            modes: config
                .modes
                .into_iter()
                .map(|m| ReadbackMode {
                    mode: m.mode,
                    lights: m.lights,
                    keys: m
                        .keys
                        .into_iter()
                        .map(|k| ReadbackKey {
                            role: k.role,
                            action: Some(k.action),
                            description: Some(k.description),
                            raw_binding: vec![],
                            raw_description: None,
                            read_status: Some(0),
                        })
                        .collect(),
                })
                .collect(),
        }))
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        let count = {
            let mut writes = self.writes.lock().unwrap();
            writes.push(command.to_vec());
            writes.len()
        };
        if count == self.block_at.load(Ordering::SeqCst) {
            self.gate.acquire().await.unwrap().forget();
        }
        {
            let mut config = self.config.lock().unwrap();
            match command[2] {
                0x85 => config.device.brightness_percent = command[3],
                0x73 => {
                    let key = &mut config.modes[command[4] as usize].keys[command[5] as usize];
                    let payload = &command[6..command.len() - 2];
                    match command[3] {
                        0x75 => key.description = String::from_utf8(payload.to_vec()).unwrap(),
                        0x73 if !payload.is_empty() => {
                            key.action = Action::Shortcut {
                                modifiers: vec![],
                                key_code: payload[0],
                            }
                        }
                        0x73 | 0x74 => key.action = Action::Disabled {},
                        _ => unreachable!(),
                    }
                }
                0x84 => {
                    for (index, light) in config.modes[command[3] as usize]
                        .lights
                        .iter_mut()
                        .enumerate()
                    {
                        light.effect = ahakey_runtime_rust::configuration::EFFECTS
                            [command[index + 4] as usize]
                            .into();
                    }
                }
                0x04 => {}
                _ => unreachable!(),
            }
        }
        if count == self.fail_at.load(Ordering::SeqCst) {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}
async fn ready(runtime: &Runtime) {
    tokio::time::timeout(Duration::from_secs(8), async {
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
async fn stopped(runtime: &Runtime, id: &str) -> Operation {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let op = runtime.operation(id).await.unwrap();
            if op.terminal() || op.status == "paused" {
                break op;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn first_write(device: &Device) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !device.writes.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn open(path: std::path::PathBuf, device: &Device) -> Arc<Runtime> {
    let runtime = Runtime::open(path).await.unwrap();
    runtime
        .register(device.device(), None, false)
        .await
        .unwrap();
    ready(&runtime).await;
    runtime
}
async fn patch(runtime: &Runtime, id: &str) {
    let base = runtime
        .configuration("transaction-x1")
        .await
        .unwrap()
        .base_token;
    runtime
        .patch_configuration(ConfigurationPatch {
            operation_id: id.into(),
            device_id: "transaction-x1".into(),
            base_token: base,
            brightness_percent: Some(62),
            descriptions: vec![DescriptionPatch {
                mode: 0,
                index: 1,
                description: "Continue".into(),
            }],
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn pause_survives_restart_and_resume_skips_verified_field_then_saves() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let device = Device::new();
    let runtime = open(path.clone(), &device).await;
    let original = device.config.lock().unwrap().clone();
    patch(&runtime, "resume-fields").await;
    first_write(&device).await;
    runtime.pause_operation("resume-fields").await.unwrap();
    device.gate.add_permits(1);
    let paused = stopped(&runtime, "resume-fields").await;
    assert_eq!(paused.status, "paused");
    assert_eq!(
        paused.configuration_progress.unwrap().fields_acknowledged,
        1
    );
    assert_eq!(device.writes.lock().unwrap().len(), 1);
    runtime.shutdown().await;
    drop(runtime);
    let runtime = open(path, &device).await;
    assert_eq!(
        runtime.operation("resume-fields").await.unwrap().status,
        "paused"
    );
    assert_eq!(
        device.writes.lock().unwrap().len(),
        1,
        "no replay on startup"
    );
    runtime.resume_operation("resume-fields").await.unwrap();
    let complete = stopped(&runtime, "resume-fields").await;
    assert_eq!(complete.status, "completed");
    let progress = complete.configuration_progress.unwrap();
    assert_eq!(progress.fields_verified, 2);
    assert!(progress.save_acknowledged);
    let writes = device.writes.lock().unwrap().clone();
    assert_eq!(
        writes.iter().map(|c| c[2]).collect::<Vec<_>>(),
        [0x85, 0x73, 0x04]
    );
    let mut expected = original;
    expected.device.brightness_percent = 62;
    expected.modes[0].keys[1].description = "Continue".into();
    assert_eq!(*device.config.lock().unwrap(), expected);
    assert!(device.reads.load(Ordering::SeqCst) >= 4);
    runtime.shutdown().await;
}

#[tokio::test]
async fn pause_finishes_clear_set_pair_and_does_not_start_next_key() {
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    let runtime = open(directory.path().join("state.json"), &device).await;
    let base = runtime
        .configuration("transaction-x1")
        .await
        .unwrap()
        .base_token;
    runtime
        .apply_keymap(KeymapApply {
            operation_id: "keys".into(),
            device_id: "transaction-x1".into(),
            base_token: base,
            mode: 0,
            keys: vec![
                KeyBinding {
                    index: 0,
                    action: Action::Shortcut {
                        modifiers: vec![],
                        key_code: 9,
                    },
                },
                KeyBinding {
                    index: 1,
                    action: Action::Shortcut {
                        modifiers: vec![],
                        key_code: 10,
                    },
                },
            ],
        })
        .await
        .unwrap();
    first_write(&device).await;
    runtime.pause_operation("keys").await.unwrap();
    device.gate.add_permits(1);
    assert_eq!(stopped(&runtime, "keys").await.status, "paused");
    assert_eq!(device.writes.lock().unwrap().len(), 2);
    runtime.resume_operation("keys").await.unwrap();
    assert_eq!(stopped(&runtime, "keys").await.status, "completed");
    assert_eq!(
        device.writes.lock().unwrap().len(),
        5,
        "two frames per key plus one save; first key not replayed"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn external_change_pauses_before_any_resume_write_and_preserves_newer_value() {
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    let runtime = open(directory.path().join("state.json"), &device).await;
    patch(&runtime, "conflict").await;
    first_write(&device).await;
    runtime.pause_operation("conflict").await.unwrap();
    device.gate.add_permits(1);
    stopped(&runtime, "conflict").await;
    device.config.lock().unwrap().modes[0].keys[1].description = "External edit".into();
    runtime.resume_operation("conflict").await.unwrap();
    let conflict = stopped(&runtime, "conflict").await;
    assert_eq!(conflict.status, "paused");
    assert_eq!(
        conflict.error_code.as_deref(),
        Some("CONFIGURATION_CONFLICT")
    );
    assert_eq!(device.writes.lock().unwrap().len(), 1);
    assert_eq!(
        device.config.lock().unwrap().modes[0].keys[1].description,
        "External edit"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn lost_ack_is_reconciled_from_device_without_replaying_successful_field() {
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    device.block_at.store(0, Ordering::SeqCst);
    device.fail_at.store(1, Ordering::SeqCst);
    let runtime = open(directory.path().join("state.json"), &device).await;
    patch(&runtime, "lost-ack").await;
    let paused = stopped(&runtime, "lost-ack").await;
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.effect, "unknown");
    ready(&runtime).await;
    runtime.resume_operation("lost-ack").await.unwrap();
    assert_eq!(stopped(&runtime, "lost-ack").await.status, "completed");
    assert_eq!(
        device
            .writes
            .lock()
            .unwrap()
            .iter()
            .map(|c| c[2])
            .collect::<Vec<_>>(),
        [0x85, 0x73, 0x04]
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn in_flight_journal_recovers_as_paused_and_never_replays_without_resume() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let device = Device::new();
    let runtime = open(path.clone(), &device).await;
    patch(&runtime, "interrupted").await;
    first_write(&device).await;
    // This is the durable file at the crash boundary, before a field's ACK arrives.
    let crash_bytes = std::fs::read(&path).unwrap();
    let crash: serde_json::Value = serde_json::from_slice(&crash_bytes).unwrap();
    assert_eq!(
        crash["operations"]["interrupted"]["configuration_transaction"]["in_flight"],
        0
    );
    runtime.pause_operation("interrupted").await.unwrap();
    device.gate.add_permits(1);
    stopped(&runtime, "interrupted").await;
    runtime.shutdown().await;
    drop(runtime);
    std::fs::write(&path, crash_bytes).unwrap();
    let runtime = open(path, &device).await;
    let recovered = runtime.operation("interrupted").await.unwrap();
    assert_eq!(recovered.status, "paused");
    assert_eq!(recovered.effect, "unknown");
    assert_eq!(recovered.error_code.as_deref(), Some("RUNTIME_RESTARTED"));
    assert_eq!(device.writes.lock().unwrap().len(), 1);
    runtime.resume_operation("interrupted").await.unwrap();
    assert_eq!(stopped(&runtime, "interrupted").await.status, "completed");
    assert_eq!(
        device
            .writes
            .lock()
            .unwrap()
            .iter()
            .map(|c| c[2])
            .collect::<Vec<_>>(),
        [0x85, 0x73, 0x04]
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn lost_save_ack_retries_save_only_after_readback() {
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    device.block_at.store(0, Ordering::SeqCst);
    device.fail_at.store(3, Ordering::SeqCst);
    let runtime = open(directory.path().join("state.json"), &device).await;
    patch(&runtime, "save-ack").await;
    let paused = stopped(&runtime, "save-ack").await;
    assert_eq!(paused.status, "paused");
    assert!(!paused.configuration_progress.unwrap().save_acknowledged);
    ready(&runtime).await;
    runtime.resume_operation("save-ack").await.unwrap();
    let complete = stopped(&runtime, "save-ack").await;
    assert_eq!(complete.status, "completed");
    assert!(complete.configuration_progress.unwrap().save_acknowledged);
    assert_eq!(
        device
            .writes
            .lock()
            .unwrap()
            .iter()
            .map(|c| c[2])
            .collect::<Vec<_>>(),
        [0x85, 0x73, 0x04, 0x04]
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn websocket_can_pause_and_resume_configuration_under_the_original_id() {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    let runtime = open(directory.path().join("state.json"), &device).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ahakey_runtime_rust::ipc::serve(
        listener,
        runtime.clone(),
        "test-token".into(),
    ));
    let (mut socket, _) = connect_async(endpoint).await.unwrap();
    patch(&runtime, "wire-control").await;
    first_write(&device).await;
    for (method, params) in [
        (
            "runtime.hello",
            json!({"protocol":{"major":1,"minor":0},"client":{"name":"transaction-test","version":"1","kind":"studio"},"token":"test-token"}),
        ),
        ("operation.pause", json!({"operationId":"wire-control"})),
        ("operation.resume", json!({"operationId":"wire-control"})),
    ] {
        socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":method,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let Message::Text(text) = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("response")
        };
        let reply: Value = serde_json::from_str(&text).unwrap();
        assert!(reply.get("error").is_none(), "{reply}");
        if method == "operation.pause" {
            assert_eq!(reply["result"]["configurationProgress"]["fieldsTotal"], 2);
            device.gate.add_permits(1);
            assert_eq!(stopped(&runtime, "wire-control").await.status, "paused");
        }
    }
    assert_eq!(stopped(&runtime, "wire-control").await.status, "completed");
    assert_eq!(device.writes.lock().unwrap().len(), 3);
    socket.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn scoped_configuration_verifies_all_groups_and_keeps_other_modes_unchanged() {
    use ahakey_runtime_rust::configuration::{Apply, Scope};
    let directory = tempfile::tempdir().unwrap();
    let device = Device::new();
    device.block_at.store(0, Ordering::SeqCst);
    let runtime = open(directory.path().join("state.json"), &device).await;
    let mut expected = device.config.lock().unwrap().clone();
    expected.device.brightness_percent = 44;
    for (index, key) in expected.modes[2].keys.iter_mut().enumerate() {
        key.action = Action::Shortcut {
            modifiers: vec![],
            key_code: 10 + index as u8,
        };
        key.description = format!("key{index}");
    }
    expected.modes[2].lights[3].effect = "rainbowWave".into();
    let base = runtime
        .configuration("transaction-x1")
        .await
        .unwrap()
        .base_token;
    runtime
        .apply(Apply {
            operation_id: "whole-mode".into(),
            device_id: "transaction-x1".into(),
            base_token: base,
            scope: Scope {
                device_fields: vec!["brightnessPercent".into()],
                modes: vec![2],
            },
            changes: Configuration {
                device: expected.device.clone(),
                modes: vec![expected.modes[2].clone()],
            },
        })
        .await
        .unwrap();
    let complete = stopped(&runtime, "whole-mode").await;
    assert_eq!(complete.status, "completed");
    let progress = complete.configuration_progress.unwrap();
    assert_eq!(progress.fields_total, 10);
    assert_eq!(progress.fields_verified, 10);
    assert_eq!(*device.config.lock().unwrap(), expected);
    assert_eq!(
        device
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c[2] == 0x04)
            .count(),
        1
    );
    runtime.shutdown().await;
}
