use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, state::HardwareState},
    configuration::{
        Action, Configuration, ConfigurationReadback, ReadbackDevice, ReadbackKey, ReadbackMode,
    },
    error::Result,
    keymap::{KeyBinding, KeymapApply},
    runtime::Runtime,
};
use async_trait::async_trait;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

struct ReadableDevice {
    config: Arc<Mutex<Configuration>>,
    writes: Arc<AtomicUsize>,
    gate: Arc<Semaphore>,
    reads: Arc<AtomicUsize>,
    failures: Arc<Mutex<std::collections::VecDeque<&'static str>>>,
}
#[async_trait]
impl Hardware for ReadableDevice {
    fn configuration_commands(&self, config: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(config)
    }
    fn keymap_commands(&self, request: &KeymapApply) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::keymap_commands(request)
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
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(self.config.lock().unwrap().device.brightness_percent),
        })
    }
    async fn read_configuration(&mut self) -> Result<Option<ConfigurationReadback>> {
        let _permit = self.gate.acquire().await.unwrap();
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(code) = self.failures.lock().unwrap().pop_front() {
            return Err(ahakey_runtime_rust::error::Error(code));
        }
        let config = self.config.lock().unwrap().clone();
        Ok(Some(ConfigurationReadback {
            device: ReadbackDevice {
                brightness_percent: config.device.brightness_percent,
                task_light_mode: None,
            },
            modes: config
                .modes
                .into_iter()
                .map(|mode| ReadbackMode {
                    mode: mode.mode,
                    lights: mode.lights,
                    keys: mode
                        .keys
                        .into_iter()
                        .map(|key| ReadbackKey {
                            role: key.role,
                            action: Some(key.action),
                            description: None,
                            raw_binding: vec![],
                            raw_description: None,
                            read_status: Some(0),
                        })
                        .collect(),
                })
                .collect(),
        }))
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl ReadableDevice {
    fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(Configuration::mock_defaults())),
            writes: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Semaphore::new(0)),
            reads: Arc::new(AtomicUsize::new(0)),
            failures: Default::default(),
        }
    }
    fn clone_device(&self) -> AhaKey {
        AhaKey {
            device_id: "readable-x1".into(),
            name: "Readback X1".into(),
            hardware: Box::new(Self {
                config: self.config.clone(),
                writes: self.writes.clone(),
                gate: self.gate.clone(),
                reads: self.reads.clone(),
                failures: self.failures.clone(),
            }),
        }
    }
}
async fn state(runtime: &Runtime, wanted: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if runtime.subscribe().await.0["devices"][0]["connectionState"] == wanted {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn readback_is_atomic_and_refreshes_after_reconnect_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let device = ReadableDevice::new();
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    runtime
        .register(device.clone_device(), None, false)
        .await
        .unwrap();
    state(&runtime, "connecting").await;
    let document = runtime.configuration("readable-x1").await.unwrap();
    assert!(document.readback.is_none());
    let error = runtime
        .apply_keymap(KeymapApply {
            operation_id: "during-readback".into(),
            device_id: "readable-x1".into(),
            base_token: document.base_token.clone(),
            mode: 0,
            keys: vec![KeyBinding {
                index: 0,
                action: Action::Disabled {},
            }],
        })
        .await
        .unwrap_err();
    assert_eq!(error.0, "DEVICE_NOT_READY");
    device.gate.add_permits(1);
    state(&runtime, "ready").await;
    assert_ne!(
        runtime
            .configuration("readable-x1")
            .await
            .unwrap()
            .base_token,
        document.base_token
    );
    let initial = runtime.configuration("readable-x1").await.unwrap();
    assert!(initial.configuration.is_none());
    let wire = serde_json::to_value(&initial).unwrap();
    assert!(wire["configuration"].is_null());
    assert_eq!(
        wire["readback"]["modes"][0]["keys"][0].get("description"),
        Some(&serde_json::Value::Null)
    );

    runtime.connect("readable-x1", false).await.unwrap();
    state(&runtime, "disconnected").await;
    assert!(
        runtime
            .configuration("readable-x1")
            .await
            .unwrap()
            .readback
            .is_none()
    );
    device.config.lock().unwrap().device.brightness_percent = 61;
    runtime.connect("readable-x1", true).await.unwrap();
    state(&runtime, "ready").await;
    let current = runtime.configuration("readable-x1").await.unwrap();
    assert!(current.configuration.is_none());
    assert_eq!(
        current.readback.as_ref().unwrap().device.brightness_percent,
        61
    );
    assert_ne!(current.base_token, initial.base_token);

    let mut full_save: ahakey_runtime_rust::configuration::Apply =
        serde_json::from_str(include_str!("../contracts/runtime-v1/fixtures/apply.json")).unwrap();
    full_save.operation_id = "unknown-descriptions".into();
    full_save.device_id = "readable-x1".into();
    full_save.base_token = current.base_token.clone();
    assert_eq!(
        runtime.apply(full_save).await.unwrap_err().0,
        "INCOMPLETE_WRITE_GROUP"
    );
    assert_eq!(device.writes.load(Ordering::SeqCst), 0);

    // A write invalidates the last observed raw snapshot; it is not passed off
    // as device readback of the newly written bytes.
    runtime
        .apply_keymap(KeymapApply {
            operation_id: "after-readback".into(),
            device_id: "readable-x1".into(),
            base_token: current.base_token,
            mode: 0,
            keys: vec![KeyBinding {
                index: 0,
                action: Action::Disabled {},
            }],
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let operation = runtime.operation("after-readback").await.unwrap();
            if operation.terminal() {
                assert_eq!(operation.status, "completed");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let after_write = runtime.configuration("readable-x1").await.unwrap();
    assert!(after_write.readback.is_none());
    assert!(after_write.configuration.is_none());
    runtime.shutdown().await;

    device.config.lock().unwrap().device.brightness_percent = 72;
    let restarted = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    restarted
        .register(device.clone_device(), None, false)
        .await
        .unwrap();
    state(&restarted, "ready").await;
    let restored = restarted.configuration("readable-x1").await.unwrap();
    assert!(restored.configuration.is_none());
    assert_eq!(restored.readback.unwrap().device.brightness_percent, 72);
    assert_ne!(restored.base_token, after_write.base_token);
    restarted.shutdown().await;
}

async fn read_finished(runtime: &Runtime) -> ahakey_runtime_rust::wire::ConfigurationDocument {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let document = runtime.configuration("readable-x1").await.unwrap();
            if document
                .read
                .as_ref()
                .is_some_and(|read| read.status != "reading")
            {
                break document;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn explicit_read_reserves_device_keeps_drafts_out_of_runtime_and_replaces_only_after_success()
{
    let directory = tempfile::tempdir().unwrap();
    let device = ReadableDevice::new();
    device.gate.add_permits(1);
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    runtime
        .register(device.clone_device(), None, false)
        .await
        .unwrap();
    state(&runtime, "ready").await;
    let initial = runtime.configuration("readable-x1").await.unwrap();
    device.gate.forget_permits(1);
    device.config.lock().unwrap().device.brightness_percent = 63;
    let accepted = runtime.read_configuration("readable-x1").await.unwrap();
    assert_eq!(accepted.status, "reading");
    let reading = runtime.configuration("readable-x1").await.unwrap();
    assert!(reading.readback.is_none());
    assert!(reading.configuration.is_none());
    assert_ne!(reading.base_token, initial.base_token);
    assert_eq!(
        runtime
            .read_configuration("readable-x1")
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        runtime.connect("readable-x1", false).await.unwrap_err().0,
        "BUSY"
    );
    assert_eq!(
        runtime
            .apply_keymap(KeymapApply {
                operation_id: "during-explicit-read".into(),
                device_id: "readable-x1".into(),
                base_token: reading.base_token,
                mode: 0,
                keys: vec![KeyBinding {
                    index: 0,
                    action: Action::Disabled {}
                }],
            })
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    device.gate.add_permits(1);
    let complete = read_finished(&runtime).await;
    assert_eq!(complete.read.as_ref().unwrap().status, "completed");
    assert_eq!(complete.read.as_ref().unwrap().attempt, 1);
    assert_eq!(complete.readback.unwrap().device.brightness_percent, 63);
    assert!(
        complete.configuration.is_none(),
        "unknown descriptions cannot become a full baseline"
    );
    assert_eq!(device.reads.load(Ordering::SeqCst), 2);
    assert_eq!(device.writes.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
}

#[tokio::test]
async fn explicit_read_retries_at_most_three_and_keeps_failures_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let device = ReadableDevice::new();
    device.gate.add_permits(1);
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    runtime
        .register(device.clone_device(), None, false)
        .await
        .unwrap();
    state(&runtime, "ready").await;
    device
        .failures
        .lock()
        .unwrap()
        .extend(["CONFIGURATION_CHANGED"; 3]);
    runtime.read_configuration("readable-x1").await.unwrap();
    let failed = read_finished(&runtime).await;
    assert_eq!(failed.read.as_ref().unwrap().status, "failed");
    assert_eq!(failed.read.unwrap().attempt, 3);
    assert!(failed.readback.is_none());
    assert_eq!(device.reads.load(Ordering::SeqCst), 4);
    // Explicit retry after failure; a transport error causes reconnection before another read.
    device.failures.lock().unwrap().push_back("DEVICE_TIMEOUT");
    runtime.read_configuration("readable-x1").await.unwrap();
    let succeeded = read_finished(&runtime).await;
    assert_eq!(succeeded.read.as_ref().unwrap().status, "completed");
    assert_eq!(succeeded.read.unwrap().attempt, 2);
    device
        .failures
        .lock()
        .unwrap()
        .push_back("UNSUPPORTED_CONFIGURATION");
    runtime.read_configuration("readable-x1").await.unwrap();
    let unsupported = read_finished(&runtime).await;
    assert_eq!(unsupported.read.as_ref().unwrap().status, "failed");
    assert_eq!(unsupported.read.unwrap().attempt, 1);
    assert_eq!(device.writes.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
}
