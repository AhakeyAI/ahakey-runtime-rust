use super::Runtime;
use crate::{
    error::{Error, Result},
    voice::{Preferences, View},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

impl Runtime {
    pub async fn voice(&self) -> View {
        self.state.lock().await.voice.view.clone()
    }
    pub async fn attach_voice(&self, sender: mpsc::Sender<Value>) {
        self.state.lock().await.voice.sender = Some(sender);
    }
    pub async fn detach_voice(&self) {
        let mut state = self.state.lock().await;
        state.voice.sender = None;
        state.voice.view.available = false;
        let _ = state.voice.cancel();
        state.voice.view.error = Some("VOICE_UNAVAILABLE".into());
        let view = json!(state.voice.view);
        state.emit("voice.changed", view);
    }
    pub async fn voice_event(&self, event: Value) {
        let mut state = self.state.lock().await;
        if state.stopping {
            return;
        }
        state.voice.native_event(event);
        state.restore_features();
        let view = json!(state.voice.view);
        state.emit("voice.changed", view);
    }
    pub async fn configure_voice(&self, preferences: Preferences) -> Result<View> {
        preferences.validate()?;
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if state.voice.active() {
            return Err(Error("VOICE_BUSY"));
        }
        if preferences.enabled {
            let id = preferences
                .input_device_id
                .as_ref()
                .ok_or(Error("VOICE_INPUT_REQUIRED"))?;
            let input = state
                .input
                .devices
                .iter()
                .find(|device| &device.input_device_id == id)
                .ok_or(Error("VOICE_INPUT_REQUIRED"))?;
            if state.input.status != "listening" || !input.pressed_usages.is_empty() {
                return Err(Error("VOICE_INPUT_NOT_READY"));
            }
        }
        // Keep legacy preferences readable; restoration uses the stable HID identity separately.
        let mut saved = preferences.clone();
        saved.enabled = false;
        saved.input_device_id = None;
        crate::storage::atomic_write(
            &self.voice_preferences_path,
            &serde_json::to_vec(&saved).map_err(|_| Error("STORAGE_ERROR"))?,
        )?;
        state.remember_voice(&preferences)?;
        state.voice.view.preferences = preferences;
        let view = state.voice.view.clone();
        state.emit("voice.changed", json!(view));
        Ok(view)
    }
    pub async fn control_voice(&self, action: &str, session_id: Option<&str>) -> Result<View> {
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if !matches!(action, "start" | "permissions" | "refresh")
            && (session_id.is_none() || session_id != state.voice.view.session_id.as_deref())
        {
            return Err(Error("VOICE_SESSION_CHANGED"));
        }
        match action {
            "start" => {
                state.voice.start()?;
                state.voice.session_auto_insert = false;
            }
            "stop" => state.voice.stop()?,
            "cancel" | "clear" => state.voice.cancel()?,
            "permissions" | "refresh" => state.voice.send(json!({"command":action}))?,
            "copy" if state.voice.view.status == "completed" => state
                .voice
                .send(json!({"command":"copy", "sessionId":session_id}))?,
            _ => return Err(Error("INVALID_PARAMS")),
        }
        let view = state.voice.view.clone();
        state.emit("voice.changed", json!(view));
        Ok(view)
    }
}
