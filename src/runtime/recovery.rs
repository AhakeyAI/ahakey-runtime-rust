//! Persist user choices, never sessions or outstanding approvals. Rebind only stable identities.
use super::state::State;
use crate::{
    error::{Error, Result},
    hooks, voice,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Recovery {
    pub voice: Option<SavedVoice>,
    pub hooks: Option<SavedHooks>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SavedVoice {
    pub preferences: voice::Preferences,
    pub identity: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SavedHooks {
    pub preferences: hooks::Preferences,
    pub identity: Option<String>,
    pub mode: Option<u8>,
}
impl Recovery {
    pub fn load(path: &Path) -> Result<Self> {
        let result: Self = match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|_| Error("INVALID_RECOVERY_PREFERENCES"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => return Err(Error("STORAGE_ERROR")),
        };
        if let Some(v) = &result.voice {
            v.preferences.validate()?;
        }
        if let Some(h) = &result.hooks {
            h.preferences.validate()?;
        }
        Ok(result)
    }
}
impl State {
    fn input_identity(&self, id: Option<&str>) -> Option<String> {
        self.input
            .devices
            .iter()
            .find(|d| Some(d.input_device_id.as_str()) == id)
            .and_then(|d| d.stable_id.clone())
            .filter(|s| !s.is_empty())
    }
    fn input_for_identity(&self, identity: Option<&str>) -> Option<String> {
        let identity = identity.filter(|id| !id.is_empty())?;
        if self.input.status != "listening" || self.input.permission != "granted" {
            return None;
        }
        let matches: Vec<_> = self
            .input
            .devices
            .iter()
            .filter(|d| d.stable_id.as_deref() == Some(identity))
            .collect();
        if matches.len() != 1 || !matches[0].pressed_usages.is_empty() {
            return None;
        }
        Some(matches[0].input_device_id.clone())
    }
    fn save_recovery(&mut self, value: Recovery) -> Result<()> {
        crate::storage::atomic_write(
            &self.recovery_path,
            &serde_json::to_vec(&value).map_err(|_| Error("STORAGE_ERROR"))?,
        )?;
        self.recovery = value;
        Ok(())
    }
    pub(super) fn remember_voice(&mut self, preferences: &voice::Preferences) -> Result<()> {
        let mut saved = preferences.clone();
        saved.input_device_id = None;
        let mut value = self.recovery.clone();
        value.voice = Some(SavedVoice {
            preferences: saved,
            identity: self.input_identity(preferences.input_device_id.as_deref()),
        });
        self.save_recovery(value)?;
        self.resume_voice = false;
        Ok(())
    }
    pub(super) fn remember_hooks(&mut self, preferences: &hooks::Preferences) -> Result<()> {
        let mut saved = preferences.clone();
        saved.input_device_id = None;
        saved.allow_automatic = false;
        let identity = self.input_identity(preferences.input_device_id.as_deref());
        if identity.is_none() {
            saved.approve_usage = None;
            saved.reject_usage = None;
        }
        let mode = preferences
            .device_id
            .as_ref()
            .and_then(|id| self.devices.get(id))
            .and_then(|d| d.device.work_mode);
        let mut value = self.recovery.clone();
        value.hooks = Some(SavedHooks {
            preferences: saved,
            identity,
            mode,
        });
        self.save_recovery(value)?;
        self.resume_hooks = false;
        Ok(())
    }
    pub(super) fn restore_features(&mut self) {
        if self.stopping {
            return;
        }
        if self.resume_voice
            && !self.voice.active()
            && self.voice.view.microphone == "granted"
            && self.voice.view.speech == "granted"
            && let Some(saved) = self.recovery.voice.clone()
            && saved.preferences.enabled
            && let Some(id) = self.input_for_identity(saved.identity.as_deref())
        {
            self.voice.view.preferences = saved.preferences;
            self.voice.view.preferences.input_device_id = Some(id);
            self.voice.view.error = None;
            self.resume_voice = false;
            self.emit("voice.changed", json!(self.voice.view));
        }
        if self.resume_hooks
            && let Some(saved) = self.recovery.hooks.clone()
            && saved.preferences.enabled
            && let Some(device) = saved
                .preferences
                .device_id
                .as_ref()
                .and_then(|id| self.devices.get(id))
            && device.device.connection_state == "ready"
            && device.device.work_mode == saved.mode
        {
            let input_id = self.input_for_identity(saved.identity.as_deref());
            if saved.identity.is_some() && input_id.is_none() {
                return;
            }
            self.hooks.preferences = saved.preferences;
            self.hooks.preferences.allow_automatic = false;
            self.hooks.preferences.input_device_id = input_id;
            self.resume_hooks = false;
            self.emit("hooks.changed", json!(self.hooks));
        }
    }
}
