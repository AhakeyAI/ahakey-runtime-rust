//! Volatile voice sessions. Audio stays in the native process; transcripts never enter the journal.
pub mod native;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub input_device_id: Option<String>,
    pub trigger: Trigger,
    pub locale: String,
    pub auto_insert: bool,
    pub on_device_only: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Trigger {
    Hold,
    Toggle,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device_id: None,
            trigger: Trigger::Hold,
            locale: "zh-CN".into(),
            auto_insert: false,
            on_device_only: false,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<()> {
        if self.locale.is_empty()
            || self.locale.len() > 48
            || !self
                .locale
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(Error("INVALID_PARAMS"));
        }
        if self
            .input_device_id
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128)
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub available: bool,
    pub status: String,
    pub preferences: Preferences,
    pub session_id: Option<String>,
    pub text: String,
    pub error: Option<String>,
    pub microphone: String,
    pub speech: String,
    pub accessibility: bool,
    pub output: Option<String>,
}
pub struct Controller {
    pub view: View,
    pub sender: Option<mpsc::Sender<Value>>,
    pub session_auto_insert: bool,
}
impl Controller {
    pub fn new(preferences: Preferences) -> Self {
        Self {
            session_auto_insert: false,
            sender: None,
            view: View {
                available: false,
                status: "unavailable".into(),
                preferences,
                session_id: None,
                text: String::new(),
                error: None,
                microphone: "unknown".into(),
                speech: "unknown".into(),
                accessibility: false,
                output: None,
            },
        }
    }
    pub fn active(&self) -> bool {
        matches!(
            self.view.status.as_str(),
            "recording" | "finalizing" | "inserting"
        )
    }
    pub fn send(&self, command: Value) -> Result<()> {
        self.sender
            .as_ref()
            .ok_or(Error("VOICE_UNAVAILABLE"))?
            .try_send(command)
            .map_err(|_| Error("VOICE_UNAVAILABLE"))
    }
    pub fn start(&mut self) -> Result<()> {
        if self.active() {
            return Err(Error("VOICE_BUSY"));
        }
        if self.view.microphone != "granted" || self.view.speech != "granted" {
            return Err(Error("VOICE_PERMISSION_REQUIRED"));
        }
        let id = Uuid::new_v4().to_string();
        self.send(
            json!({"command":"start", "sessionId":id, "locale":self.view.preferences.locale,
            "onDeviceOnly":self.view.preferences.on_device_only}),
        )?;
        self.view.session_id = Some(id);
        self.session_auto_insert = self.view.preferences.auto_insert;
        self.view.status = "recording".into();
        self.view.text.clear();
        self.view.error = None;
        self.view.output = None;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.view.status != "recording" {
            return Err(Error("VOICE_NOT_RECORDING"));
        }
        self.send(json!({"command":"stop", "sessionId":self.view.session_id}))?;
        self.view.status = "finalizing".into();
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<()> {
        let result = self.send(json!({"command":"cancel", "sessionId":self.view.session_id}));
        self.view.session_id = None;
        self.view.status = if self.view.available {
            "idle"
        } else {
            "unavailable"
        }
        .into();
        self.view.text.clear();
        self.view.error = None;
        self.view.output = None;
        result
    }
    pub fn key(&mut self, event: &crate::input::KeyEvent) -> bool {
        if !self.view.preferences.enabled
            || event.usage != 109
            || self.view.preferences.input_device_id.as_deref() != Some(&event.input_device_id)
        {
            return false;
        }
        let result = match (self.view.preferences.trigger, event.pressed) {
            (Trigger::Hold, true) => self.start(),
            (Trigger::Hold, false) if self.view.status == "recording" => self.stop(),
            (Trigger::Toggle, true) if self.view.status == "recording" => self.stop(),
            (Trigger::Toggle, true) => self.start(),
            _ => return false,
        };
        if let Err(error) = result {
            self.view.error = Some(error.0.into());
        }
        true
    }
    pub fn native_event(&mut self, event: Value) {
        if event["type"] == "permissions" {
            self.view.available = true;
            if self.view.status == "unavailable" {
                self.view.status = "idle".into();
            }
            self.view.microphone = event["microphone"].as_str().unwrap_or("unknown").into();
            self.view.speech = event["speech"].as_str().unwrap_or("unknown").into();
            self.view.accessibility = event["accessibility"].as_bool().unwrap_or(false);
            if self.active() && (self.view.microphone != "granted" || self.view.speech != "granted")
            {
                let _ = self.cancel();
                self.view.error = Some("VOICE_PERMISSION_REQUIRED".into());
            }
            return;
        }
        if self.view.session_id.is_none()
            || event["sessionId"].as_str() != self.view.session_id.as_deref()
        {
            return;
        }
        match event["type"].as_str() {
            Some("finalizing") if self.view.status == "recording" => {
                self.view.status = "finalizing".into();
            }
            Some("partial") if matches!(self.view.status.as_str(), "recording" | "finalizing") => {
                self.view.text = event["text"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(16_384)
                    .collect();
            }
            Some("final") if matches!(self.view.status.as_str(), "recording" | "finalizing") => {
                self.view.text = event["text"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(16_384)
                    .collect();
                self.view.status = "completed".into();
                if self.session_auto_insert && !self.view.text.is_empty() {
                    match self.send(json!({"command":"insert", "sessionId":self.view.session_id})) {
                        Ok(()) => self.view.status = "inserting".into(),
                        Err(error) => self.view.error = Some(error.0.into()),
                    }
                }
            }
            Some("output") if matches!(self.view.status.as_str(), "inserting" | "completed") => {
                self.view.status = "completed".into();
                self.view.output = event["result"].as_str().map(str::to_owned);
            }
            Some("cancelled") => {
                self.view.session_id = None;
                self.view.status = "idle".into();
                self.view.text.clear();
                self.view.error = None;
                self.view.output = None;
            }
            Some("error") => {
                self.view.status = "failed".into();
                self.view.error = Some(event["code"].as_str().unwrap_or("VOICE_ERROR").into());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller() -> (Controller, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(32);
        let mut c = Controller::new(Preferences {
            enabled: true,
            input_device_id: Some("hid-1".into()),
            ..Preferences::default()
        });
        c.sender = Some(tx);
        c.native_event(json!({"type":"permissions", "microphone":"granted", "speech":"granted", "accessibility":true}));
        (c, rx)
    }
    fn key(id: &str, pressed: bool) -> crate::input::KeyEvent {
        crate::input::KeyEvent {
            input_device_id: id.into(),
            name: "AhaKey".into(),
            usage: 109,
            pressed,
            timestamp_ms: "0".into(),
            held_ms: None,
        }
    }
    #[test]
    fn hold_records_only_selected_source_and_finishes_on_release() {
        let (mut c, mut rx) = controller();
        assert!(!c.key(&key("other", true)));
        assert!(rx.try_recv().is_err());
        c.key(&key("hid-1", true));
        let start = rx.try_recv().unwrap();
        assert_eq!(start["command"], "start");
        c.key(&key("hid-1", false));
        assert_eq!(rx.try_recv().unwrap()["command"], "stop");
        c.native_event(json!({"type":"final", "sessionId":start["sessionId"], "text":"你好"}));
        assert_eq!(c.view.status, "completed");
        assert_eq!(c.view.text, "你好");
    }
    #[test]
    fn cancel_discards_late_results_and_never_inserts_them() {
        let (mut c, mut rx) = controller();
        c.view.preferences.auto_insert = true;
        c.start().unwrap();
        let id = c.view.session_id.clone();
        rx.try_recv().unwrap();
        c.cancel().unwrap();
        rx.try_recv().unwrap();
        c.native_event(json!({"type":"final", "sessionId":id, "text":"late"}));
        assert!(c.view.text.is_empty());
        assert!(rx.try_recv().is_err());
        c.start().unwrap();
        rx.try_recv().unwrap();
        c.native_event(json!({"type":"partial", "sessionId":id, "text":"old"}));
        assert!(c.view.text.is_empty());
    }
    #[test]
    fn toggle_ignores_release_and_finishes_on_next_press() {
        let (mut c, mut rx) = controller();
        c.view.preferences.trigger = Trigger::Toggle;
        c.key(&key("hid-1", true));
        rx.try_recv().unwrap();
        assert!(!c.key(&key("hid-1", false)));
        assert!(rx.try_recv().is_err());
        c.key(&key("hid-1", true));
        assert_eq!(rx.try_recv().unwrap()["command"], "stop");
    }
    #[test]
    fn duplicate_final_cannot_repeat_output_and_permissions_gate_start() {
        let (mut c, mut rx) = controller();
        c.view.preferences.auto_insert = true;
        c.start().unwrap();
        rx.try_recv().unwrap();
        let final_event = json!({"type":"final", "sessionId":c.view.session_id, "text":"hello"});
        c.native_event(final_event.clone());
        assert_eq!(rx.try_recv().unwrap()["command"], "insert");
        c.native_event(final_event);
        assert!(rx.try_recv().is_err());
        c.cancel().unwrap();
        c.view.microphone = "denied".into();
        assert_eq!(c.start().unwrap_err().0, "VOICE_PERMISSION_REQUIRED");
    }
    #[test]
    fn native_deadline_and_revoked_permission_update_the_current_session() {
        let (mut c, mut rx) = controller();
        c.view.preferences.auto_insert = true;
        c.start().unwrap();
        let start = rx.try_recv().unwrap();
        c.native_event(json!({"type":"finalizing", "sessionId":start["sessionId"]}));
        assert_eq!(c.view.status, "finalizing");
        c.native_event(json!({"type":"permissions", "microphone":"denied", "speech":"granted", "accessibility":true}));
        assert_eq!(rx.try_recv().unwrap()["command"], "cancel");
        assert_eq!(c.view.error.as_deref(), Some("VOICE_PERMISSION_REQUIRED"));
        assert!(c.view.session_id.is_none());
        c.native_event(json!({"type":"final", "sessionId":start["sessionId"], "text":"late"}));
        assert!(c.view.text.is_empty());
        assert!(rx.try_recv().is_err());
    }
}
