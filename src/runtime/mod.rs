//! Runtime ownership, startup, registration and shutdown.
mod configuration;
mod device;
mod oled;
mod operations;
mod state;
mod worker;
use crate::{
    ahakey::device::AhaKey,
    configuration::Configuration,
    error::{Error, Result},
    storage::Storage,
    wire::{ConfigurationDocument, Device},
};
use serde_json::json;
pub use state::Event;
use state::{Entry, Job, State};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

pub struct Runtime {
    pub instance_id: String,
    state: Mutex<State>,
    storage: Storage,
    workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub resources: crate::oled::Resources,
}
impl Runtime {
    pub async fn open(path: std::path::PathBuf) -> Result<Arc<Self>> {
        let resources = crate::oled::Resources::new(path.with_extension("resources"))?;
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
            resources,
        }))
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
                    readback: None,
                },
                sender,
                busy: false,
                desired_connected: true,
                lighting_supported: false,
                upload_pause: None,
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
    pub async fn shutdown(&self) {
        let senders = {
            let mut state = self.state.lock().await;
            state.stopping = true;
            for entry in state.devices.values() {
                if let Some((_, pause)) = &entry.upload_pause {
                    pause.send_replace(true);
                }
            }
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
