use crate::{
    ahakey::{device::AhaKey, hardware::Hardware, state::HardwareState},
    configuration::{Apply, Configuration, Scope},
    error::{Error, Result},
    storage::{Journal, Record, Storage},
    wire::{ConfigurationDocument, Device, Operation},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

const MAX_OPERATIONS: usize = 128;
#[derive(Clone, Debug)]
pub struct Event {
    pub sequence: u64,
    pub kind: &'static str,
    pub data: Value,
}
enum Job {
    Connect,
    Disconnect,
    Apply(Apply),
    Shutdown,
}
struct Entry {
    device: Device,
    document: ConfigurationDocument,
    sender: mpsc::Sender<Job>,
    busy: bool,
    desired_connected: bool,
    lighting_supported: bool,
}
struct State {
    input: crate::input::InputState,
    devices: BTreeMap<String, Entry>,
    journal: Journal,
    sequence: u64,
    events: broadcast::Sender<Event>,
    storage_healthy: bool,
    stopping: bool,
}
impl State {
    fn emit(&mut self, kind: &'static str, data: Value) {
        self.sequence += 1;
        let _ = self.events.send(Event {
            sequence: self.sequence,
            kind,
            data,
        });
    }
    fn invalidate(&mut self, id: &str) {
        if let Some(entry) = self.devices.get_mut(id) {
            entry.document.configuration = None;
            entry.document.base_token = Uuid::new_v4().to_string();
        }
        self.emit("configuration.invalidated", json!({"deviceId": id}));
    }
}

pub struct Runtime {
    pub instance_id: String,
    state: Mutex<State>,
    storage: Storage,
    workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl Runtime {
    pub async fn open(path: std::path::PathBuf) -> Result<Arc<Self>> {
        let (storage, journal) = Storage::open(path).await?;
        let (events, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            instance_id: Uuid::new_v4().to_string(),
            storage,
            state: Mutex::new(State {
                input: crate::input::InputState::default(),
                devices: BTreeMap::new(),
                journal,
                sequence: 0,
                events,
                storage_healthy: true,
                stopping: false,
            }),
            workers: Mutex::new(vec![]),
        }))
    }
    pub async fn subscribe(&self) -> (Value, broadcast::Receiver<Event>) {
        let state = self.state.lock().await;
        // Receiver registration and the snapshot share the exact same mutation boundary.
        let receiver = state.events.subscribe();
        let snapshot = json!({"instanceId": self.instance_id, "sequence": state.sequence.to_string(),
            "devices": state.devices.values().map(|e| &e.device).collect::<Vec<_>>(),
            "operations": state.journal.operations.values().map(|r| &r.operation).collect::<Vec<_>>(),
            "input": state.input});
        (snapshot, receiver)
    }
    pub async fn input_message(&self, message: crate::input::InputMessage) {
        use crate::input::InputMessage;
        let mut state = self.state.lock().await;
        if state.stopping {
            return;
        }
        match message {
            InputMessage::State(input) => {
                if state.input == input {
                    return;
                }
                state.input = input.clone();
                state.emit("input.changed", json!(input));
            }
            InputMessage::Key(event) => {
                if state.input.apply_key(&event) {
                    state.emit("input.key", json!(event));
                }
            }
        }
    }
    pub async fn configuration(&self, id: &str) -> Result<ConfigurationDocument> {
        let state = self.state.lock().await;
        let entry = state.devices.get(id).ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.busy && entry.document.configuration.is_some() {
            return Err(Error("BUSY"));
        }
        Ok(entry.document.clone())
    }
    pub async fn operation(&self, id: &str) -> Result<Operation> {
        self.state
            .lock()
            .await
            .journal
            .operations
            .get(id)
            .map(|r| r.operation.clone())
            .ok_or(Error("OPERATION_NOT_FOUND"))
    }
    pub async fn register(
        self: &Arc<Self>,
        device: AhaKey,
        initialize: Option<Configuration>,
        simulated: bool,
    ) -> Result<()> {
        let id = device.device_id.clone();
        let (sender, receiver) = mpsc::channel(1);
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if state.devices.contains_key(&id) {
            return Err(Error("DEVICE_ALREADY_REGISTERED"));
        }
        let projection = Device::disconnected(id.clone(), device.name);
        state.devices.insert(
            id.clone(),
            Entry {
                device: projection.clone(),
                document: ConfigurationDocument {
                    device_id: id.clone(),
                    base_token: Uuid::new_v4().to_string(),
                    configuration: None,
                },
                sender,
                busy: false,
                desired_connected: true,
                lighting_supported: false,
            },
        );
        state.emit("device.changed", json!(projection));
        let runtime = self.clone();
        let handle = tokio::spawn(async move {
            runtime
                .worker(id, device.hardware, receiver, initialize, simulated)
                .await;
        });
        self.workers.lock().await.push(handle);
        Ok(())
    }
    pub async fn connect(&self, id: &str, connected: bool) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        let entry = state.devices.get_mut(id).ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.busy {
            return Err(Error("BUSY"));
        }
        entry
            .sender
            .try_send(if connected {
                Job::Connect
            } else {
                Job::Disconnect
            })
            .map_err(|_| Error("BUSY"))?;
        entry.busy = true;
        entry.desired_connected = connected;
        Ok(())
    }
    pub async fn apply(&self, request: Apply) -> Result<Operation> {
        self.accept(request, false).await
    }
    async fn accept(&self, mut request: Apply, initialize: bool) -> Result<Operation> {
        request.validate()?;
        request.canonicalize();
        let mut state = self.state.lock().await;
        // Deduplication must precede readiness, busy, and baseline checks.
        if let Some(record) = state.journal.operations.get(&request.operation_id) {
            return if record.request == request {
                Ok(record.operation.clone())
            } else {
                Err(Error("OPERATION_ID_CONFLICT"))
            };
        }
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if !state.storage_healthy {
            return Err(Error("STORAGE_ERROR"));
        }
        // No silent eviction: every accepted ID remains recoverable until explicit maintenance.
        if state.journal.operations.len() >= MAX_OPERATIONS {
            return Err(Error("BUSY"));
        }
        let entry = state
            .devices
            .get_mut(&request.device_id)
            .ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.device.connection_state != "ready" {
            return Err(Error("DEVICE_NOT_READY"));
        }
        if entry.busy {
            return Err(Error("BUSY"));
        }
        if entry.document.base_token != request.base_token {
            return Err(Error("BASE_CONFLICT"));
        }
        if !entry.lighting_supported {
            return Err(Error("UNSUPPORTED_CAPABILITY"));
        }
        if entry.document.configuration.is_none()
            && !(initialize && request.scope.modes == [0, 1, 2, 3])
        {
            return Err(Error("INCOMPLETE_WRITE_GROUP"));
        }
        let sender = entry.sender.clone();
        entry.busy = true;
        let operation = Operation {
            operation_id: request.operation_id.clone(),
            device_id: request.device_id.clone(),
            status: "accepted".into(),
            progress: 0.0,
            effect: "none".into(),
            error_code: None,
        };
        state.journal.operations.insert(
            request.operation_id.clone(),
            Record {
                request: request.clone(),
                operation: operation.clone(),
            },
        );
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            state.devices.get_mut(&request.device_id).unwrap().busy = false;
            let failed = &mut state
                .journal
                .operations
                .get_mut(&request.operation_id)
                .unwrap()
                .operation;
            failed.status = "failed".into();
            failed.error_code = Some("STORAGE_ERROR".into());
            let _ = self.storage.save(&state.journal).await;
            return Err(Error("STORAGE_ERROR"));
        }
        if sender.try_send(Job::Apply(request.clone())).is_err() {
            state.devices.get_mut(&request.device_id).unwrap().busy = false;
            let failed = &mut state
                .journal
                .operations
                .get_mut(&request.operation_id)
                .unwrap()
                .operation;
            failed.status = "failed".into();
            failed.error_code = Some("DEVICE_NOT_READY".into());
            self.storage.save(&state.journal).await?;
            return Err(Error("DEVICE_NOT_READY"));
        }
        state.emit("operation.changed", json!(operation));
        Ok(operation)
    }
    async fn publish_hardware(&self, id: &str, hardware: &HardwareState) {
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        let old = entry.device.clone();
        entry.device.connection_state = "ready".into();
        entry.device.battery_percent = hardware.battery;
        entry.device.work_mode = Some(hardware.work_mode);
        entry.device.lever = hardware.lever.map(str::to_owned);
        entry.lighting_supported = hardware.brightness.is_some();
        let device = entry.device.clone();
        let external_change =
            entry.document.configuration.as_ref().is_some_and(|config| {
                Some(config.device.brightness_percent) != hardware.brightness
            });
        if external_change {
            state.invalidate(id);
        }
        if old != device {
            state.emit("device.changed", json!(device));
        }
    }
    async fn disconnected(&self, id: &str) {
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        let changed = entry.device.connection_state != "disconnected"
            || entry.document.configuration.is_some();
        entry.device.clear("disconnected");
        entry.lighting_supported = false;
        let device = entry.device.clone();
        if changed {
            state.invalidate(id);
            state.emit("device.changed", json!(device));
        }
    }
    async fn set_connecting(&self, id: &str) {
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        entry.device.clear("connecting");
        let device = entry.device.clone();
        state.emit("device.changed", json!(device));
    }
    async fn execute_apply(&self, hardware: &mut dyn Hardware, request: &Apply) {
        let id = &request.device_id;
        let operation_id = &request.operation_id;
        let plan = hardware.configuration_commands(&request.changes);
        {
            let mut state = self.state.lock().await;
            // A notification received after admission can invalidate the session or baseline.
            let entry = &state.devices[id];
            let preflight = if let Err(error) = &plan {
                Some(error.0)
            } else if entry.device.connection_state != "ready" {
                Some("DEVICE_NOT_READY")
            } else if entry.document.base_token != request.base_token {
                Some("BASE_CONFLICT")
            } else {
                None
            };
            if let Some(code) = preflight {
                let operation = &mut state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .operation;
                operation.status = "failed".into();
                operation.error_code = Some(code.into());
                operation.effect = "none".into();
                let event = json!(operation);
                state.devices.get_mut(id).unwrap().busy = false;
                if self.storage.save(&state.journal).await.is_err() {
                    state.storage_healthy = false;
                }
                state.emit("operation.changed", event);
                return;
            }
            let record = state.journal.operations.get_mut(operation_id).unwrap();
            record.operation.status = "running".into();
            // An in-flight command can have effects before its ACK is observed.
            record.operation.effect = "unknown".into();
            let operation = record.operation.clone();
            state.emit("operation.changed", json!(operation));
        }
        let commands = plan.expect("model plan checked before execution");
        let mut failure = None;
        for (index, command) in commands.iter().enumerate() {
            if let Err(error) = hardware.write_confirmed(command).await {
                failure = Some(error);
                break;
            }
            let mut state = self.state.lock().await;
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.progress = (index + 1) as f64 / commands.len() as f64;
            operation.effect = "partial".into();
            let event = json!(operation);
            state.emit("operation.changed", event);
        }
        if failure.is_some() {
            let _ = hardware.disconnect().await;
            self.disconnected(id).await;
        }
        let mut state = self.state.lock().await;
        if let Some(error) = failure {
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "failed".into();
            operation.error_code = Some(error.0.into());
            // The failing command might have executed even when no ACK arrived.
            operation.effect = "unknown".into();
            state.invalidate(id);
        } else {
            let entry = state.devices.get_mut(id).unwrap();
            if let Some(config) = &mut entry.document.configuration {
                config.merge(&request.changes);
            } else {
                entry.document.configuration = Some(request.changes.clone());
            }
            entry.document.base_token = Uuid::new_v4().to_string();
            let config = entry.document.configuration.clone().unwrap();
            state.journal.configurations.insert(id.clone(), config);
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "completed".into();
            operation.effect = "complete".into();
            operation.progress = 1.0;
        }
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "failed".into();
            operation.effect = "unknown".into();
            operation.error_code = Some("STORAGE_ERROR".into());
            state.invalidate(id);
            let _ = self.storage.save(&state.journal).await;
        }
        state.devices.get_mut(id).unwrap().busy = false;
        let operation = state
            .journal
            .operations
            .get(operation_id)
            .unwrap()
            .operation
            .clone();
        if operation.status == "completed" {
            state.emit("configuration.changed", json!({"deviceId": id}));
        }
        state.emit("operation.changed", json!(operation));
    }
    async fn worker(
        self: Arc<Self>,
        id: String,
        mut hardware: Box<dyn Hardware>,
        mut receiver: mpsc::Receiver<Job>,
        mut initialize: Option<Configuration>,
        simulated: bool,
    ) {
        // Retry connection failures only. Connected devices wait on notifications.
        let mut retry = tokio::time::interval(Duration::from_secs(3));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut connected = false;
        let mut seed_mock = simulated;
        loop {
            let job = tokio::select! {
                biased;
                event = hardware.next_event(), if connected => {
                    match event {
                        Ok(status) => self.publish_hardware(&id, &status).await,
                        Err(error) => {
                            eprintln!("Device {id}: {error}");
                            let _ = hardware.disconnect().await;
                            connected = false;
                            self.disconnected(&id).await;
                        }
                    }
                    continue;
                },
                job = receiver.recv() => match job { Some(job) => Some(job), None => break },
                _ = retry.tick(), if !connected => None,
            };
            match job {
                Some(Job::Shutdown) => break,
                Some(Job::Apply(request)) => {
                    self.execute_apply(hardware.as_mut(), &request).await;
                    connected =
                        self.state.lock().await.devices[&id].device.connection_state == "ready";
                    continue;
                }
                Some(Job::Disconnect) => {
                    let _ = hardware.disconnect().await;
                    connected = false;
                    self.disconnected(&id).await;
                }
                Some(Job::Connect) | None => {
                    let wanted = self.state.lock().await.devices[&id].desired_connected;
                    if !wanted || connected {
                        if job.is_some() {
                            self.state.lock().await.devices.get_mut(&id).unwrap().busy = false;
                        }
                        continue;
                    }
                    self.set_connecting(&id).await;
                    let result = hardware.connect().await;
                    match result {
                        Ok(status) => {
                            connected = true;
                            self.publish_hardware(&id, &status).await;
                            if seed_mock {
                                let mut state = self.state.lock().await;
                                state.devices.get_mut(&id).unwrap().document.configuration =
                                    Some(Configuration::mock_defaults());
                                state.emit("configuration.changed", json!({"deviceId": id}));
                                seed_mock = false;
                            }
                        }
                        Err(error) => {
                            eprintln!("Device {id}: {error}");
                            let _ = hardware.disconnect().await;
                            connected = false;
                            self.disconnected(&id).await;
                        }
                    }
                }
            }
            // Clear only a reservation for this control job, never one admitted while connecting.
            if job.is_some() {
                self.state.lock().await.devices.get_mut(&id).unwrap().busy = false;
            }
            if connected && let Some(changes) = initialize.take() {
                let base_token = self.configuration(&id).await.unwrap().base_token;
                let request = Apply {
                    operation_id: Uuid::new_v4().to_string(),
                    device_id: id.clone(),
                    base_token,
                    scope: Scope {
                        device_fields: vec!["brightnessPercent".into()],
                        modes: vec![0, 1, 2, 3],
                    },
                    changes,
                };
                match self.accept(request, true).await {
                    Ok(operation) => {
                        eprintln!("Initialization accepted: {}", operation.operation_id)
                    }
                    Err(error) => eprintln!("Initialization failed: {error}"),
                }
            }
        }
        let _ = hardware.disconnect().await;
        self.disconnected(&id).await;
    }
    pub async fn shutdown(&self) {
        let senders = {
            let mut state = self.state.lock().await;
            state.stopping = true;
            state
                .devices
                .values()
                .map(|e| e.sender.clone())
                .collect::<Vec<_>>()
        };
        for sender in senders {
            let _ = sender.send(Job::Shutdown).await;
        }
        for worker in self.workers.lock().await.drain(..) {
            let _ = worker.await;
        }
    }
}
