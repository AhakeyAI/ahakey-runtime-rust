//! Runtime ownership, startup, registration and shutdown.
mod configuration;
mod configuration_transaction;
mod device;
mod hooks;
mod oled;
mod operations;
mod readback;
mod recovery;
mod state;
mod voice;
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
    voice_preferences_path: std::path::PathBuf,
}
impl Runtime {
    pub async fn open(path: std::path::PathBuf) -> Result<Arc<Self>> {
        let resources = crate::oled::Resources::new(path.with_extension("resources"))?;
        let voice_preferences_path = path.with_extension("voice.json");
        let mut voice_preferences: crate::voice::Preferences =
            match std::fs::read(&voice_preferences_path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|_| Error("INVALID_VOICE_PREFERENCES"))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
                Err(_) => return Err(Error("STORAGE_ERROR")),
            };
        voice_preferences.validate()?;
        voice_preferences.enabled = false;
        voice_preferences.input_device_id = None;
        let recovery_path = path.with_extension("recovery.json");
        let recovery = recovery::Recovery::load(&recovery_path)?;
        let (storage, journal) = Storage::open(path).await?;
        let (events, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            instance_id: Uuid::new_v4().to_string(),
            storage,
            state: Mutex::new(State {
                recovery,
                recovery_path,
                resume_voice: true,
                resume_hooks: true,
                input: crate::input::InputState::default(),
                voice: crate::voice::Controller::new(voice_preferences),
                hooks: crate::hooks::Controller::default(),
                devices: BTreeMap::new(),
                journal,
                sequence: 0,
                events,
                storage_healthy: true,
                stopping: false,
            }),
            workers: Mutex::new(vec![]),
            resources,
            voice_preferences_path,
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
                    read: None,
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
            let _ = state.voice.cancel();
            state.hooks.deactivate();
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
