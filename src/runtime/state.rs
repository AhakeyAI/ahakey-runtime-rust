use super::Runtime;
use crate::{
    error::Result,
    operation::Mutation,
    storage::Journal,
    wire::{ConfigurationDocument, Device},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Event {
    pub sequence: u64,
    pub kind: &'static str,
    pub data: Value,
}
pub(super) enum Job {
    Connect,
    Disconnect,
    Apply(Mutation),
    ReadOled(oneshot::Sender<Result<crate::oled::State>>),
    ResumeOled(crate::oled::Upload),
    Shutdown,
}
pub(super) struct Entry {
    pub(super) device: Device,
    pub(super) document: ConfigurationDocument,
    pub(super) sender: mpsc::Sender<Job>,
    pub(super) busy: bool,
    pub(super) desired_connected: bool,
    pub(super) lighting_supported: bool,
    pub(super) upload_pause: Option<(String, watch::Sender<bool>)>,
}
pub(super) struct State {
    pub(super) input: crate::input::InputState,
    pub(super) devices: BTreeMap<String, Entry>,
    pub(super) journal: Journal,
    pub(super) sequence: u64,
    pub(super) events: broadcast::Sender<Event>,
    pub(super) storage_healthy: bool,
    pub(super) stopping: bool,
}
impl State {
    pub(super) fn emit(&mut self, kind: &'static str, data: Value) {
        if matches!(
            kind,
            "device.changed" | "operation.changed" | "configuration.invalidated"
        ) {
            crate::debug::event(kind, || data.clone());
        }
        self.sequence += 1;
        let _ = self.events.send(Event {
            sequence: self.sequence,
            kind,
            data,
        });
    }
    pub(super) fn invalidate(&mut self, id: &str) {
        if let Some(entry) = self.devices.get_mut(id) {
            entry.document.configuration = None;
            entry.document.readback = None;
            entry.document.base_token = Uuid::new_v4().to_string();
        }
        self.emit("configuration.invalidated", json!({"deviceId": id}));
    }
}

impl Runtime {
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
}
