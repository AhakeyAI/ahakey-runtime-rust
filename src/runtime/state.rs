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
    ReadConfiguration,
    ReadOled(oneshot::Sender<Result<crate::oled::State>>),
    RefreshHardware(oneshot::Sender<Result<crate::ahakey::state::HardwareState>>),
    ResumeConfiguration(Mutation),
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
    pub(super) voice: crate::voice::Controller,
    pub(super) hooks: crate::hooks::Controller,
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
        if self.hooks.preferences.enabled && self.hooks.preferences.device_id.as_deref() == Some(id)
        {
            self.hooks.deactivate();
            let view = json!(self.hooks);
            self.emit("hooks.changed", view);
        }
        if let Some(entry) = self.devices.get_mut(id) {
            if entry
                .document
                .read
                .as_ref()
                .is_some_and(|read| read.status == "completed")
            {
                entry.document.read = None;
            }
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
            "input": state.input, "voice": state.voice.view, "hooks": state.hooks});
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
                if state.hooks.preferences.input_device_id.is_some()
                    && (input.status != "listening"
                        || !input.devices.iter().any(|d| {
                            Some(&d.input_device_id)
                                == state.hooks.preferences.input_device_id.as_ref()
                        }))
                {
                    state.hooks.preferences.input_device_id = None;
                    state.hooks.preferences.approve_usage = None;
                    state.hooks.preferences.reject_usage = None;
                    state.hooks.learning = None;
                    let view = json!(state.hooks);
                    state.emit("hooks.changed", view);
                }
                if state.voice.view.preferences.enabled
                    && (input.status != "listening"
                        || !input.devices.iter().any(|device| {
                            Some(&device.input_device_id)
                                == state.voice.view.preferences.input_device_id.as_ref()
                        }))
                {
                    let _ = state.voice.cancel();
                    state.voice.view.preferences.enabled = false;
                    state.voice.view.preferences.input_device_id = None;
                    state.voice.view.error = Some("VOICE_INPUT_LOST".into());
                    let view = json!(state.voice.view);
                    state.emit("voice.changed", view);
                }
            }
            InputMessage::Key(event) => {
                if state.input.apply_key(&event) {
                    state.emit("input.key", json!(event));
                    let single_key = state.input.devices.iter().any(|d| {
                        d.input_device_id == event.input_device_id
                            && d.pressed_usages == [event.usage]
                    });
                    if single_key && state.hooks.key(&event) {
                        let view = json!(state.hooks);
                        state.emit("hooks.changed", view);
                    }
                    if state.voice.key(&event) {
                        let view = json!(state.voice.view);
                        state.emit("voice.changed", view);
                    }
                }
            }
        }
    }
}
