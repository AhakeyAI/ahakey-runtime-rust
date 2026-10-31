use super::{Runtime, state::Job};
use crate::{
    error::{Error, Result},
    hooks::{HookEvent, Preferences},
};
use serde_json::{Value, json};
use tokio::sync::oneshot;

impl Runtime {
    pub async fn expire_hooks(&self) {
        let mut state = self.state.lock().await;
        if state.hooks.expire() {
            let view = json!(state.hooks);
            state.emit("hooks.changed", view);
        }
    }
    pub async fn hooks(&self) -> Value {
        let mut state = self.state.lock().await;
        if state.hooks.expire() {
            let view = json!(state.hooks);
            state.emit("hooks.changed", view);
        }
        json!(state.hooks)
    }
    pub async fn configure_hooks(&self, preferences: Preferences) -> Result<Value> {
        preferences.validate()?;
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if preferences.enabled {
            let device = state
                .devices
                .get(preferences.device_id.as_deref().unwrap())
                .ok_or(Error("DEVICE_NOT_FOUND"))?;
            if device.device.connection_state != "ready" {
                return Err(Error("DEVICE_NOT_READY"));
            }
        }
        if let Some(id) = &preferences.input_device_id
            && (!state
                .input
                .devices
                .iter()
                .any(|d| &d.input_device_id == id && d.pressed_usages.is_empty())
                || state.input.status != "listening")
        {
            return Err(Error("HOOK_INPUT_NOT_READY"));
        }
        state.remember_hooks(&preferences)?;
        state.hooks.deactivate();
        state.hooks.preferences = preferences;
        let view = json!(state.hooks);
        state.emit("hooks.changed", view.clone());
        Ok(view)
    }
    pub async fn hook_event(&self, event: HookEvent) -> Result<Value> {
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        let result = state.hooks.receive(event)?;
        let view = json!(state.hooks);
        state.emit("hooks.changed", view);
        Ok(result)
    }
    pub async fn hook_decide(&self, id: &str, decision: &str) -> Result<Value> {
        let mut state = self.state.lock().await;
        let result = state.hooks.decide(id, decision)?;
        let view = json!(state.hooks);
        state.emit("hooks.changed", view);
        Ok(result)
    }
    pub async fn hook_select(&self, id: Option<String>, learning: Option<String>) -> Result<Value> {
        let mut state = self.state.lock().await;
        if let Some(kind) = &learning
            && (!["approve", "reject"].contains(&kind.as_str())
                || state.hooks.preferences.input_device_id.is_none())
        {
            return Err(Error("INVALID_PARAMS"));
        }
        if let Some(id) = &id
            && !state
                .hooks
                .approvals
                .iter()
                .any(|r| &r.request_id == id && r.decision == "pending")
        {
            return Err(Error("HOOK_REQUEST_NOT_FOUND"));
        }
        state.hooks.selected_request_id = id;
        state.hooks.learning = learning;
        let view = json!(state.hooks);
        state.emit("hooks.changed", view.clone());
        Ok(view)
    }
    pub async fn hook_poll(&self, id: &str) -> Result<Value> {
        let refresh = {
            let mut state = self.state.lock().await;
            if state.hooks.expire() {
                let view = json!(state.hooks);
                state.emit("hooks.changed", view);
            }
            let request = state
                .hooks
                .approvals
                .iter()
                .find(|r| r.request_id == id)
                .ok_or(Error("HOOK_REQUEST_NOT_FOUND"))?;
            if request.decision != "pending" {
                return Ok(json!({"requestId":id,"decision":request.decision}));
            }
            let preferences = &state.hooks.preferences;
            let device_id = preferences.device_id.clone();
            let revision = state.hooks.revision;
            if preferences.enabled && preferences.allow_automatic {
                device_id.and_then(|device_id| {
                    let entry = state.devices.get_mut(&device_id)?;
                    if entry.busy || entry.device.connection_state != "ready" {
                        return None;
                    }
                    let (sender, receiver) = oneshot::channel();
                    entry.sender.try_send(Job::RefreshHardware(sender)).ok()?;
                    entry.busy = true;
                    Some((device_id, revision, receiver))
                })
            } else {
                None
            }
        };
        if let Some((device_id, revision, receiver)) = refresh
            && let Ok(Ok(Ok(hardware))) =
                tokio::time::timeout(std::time::Duration::from_secs(3), receiver).await
        {
            let mut state = self.state.lock().await;
            if hardware.lever == Some("automatic")
                && state.hooks.preferences.enabled
                && state.hooks.preferences.allow_automatic
                && state.hooks.revision == revision
                && state.devices.get(&device_id).is_some_and(|e| {
                    e.device.connection_state == "ready"
                        && !e.busy
                        && e.device.lever.as_deref() == Some("automatic")
                })
            {
                let _ = state.hooks.decide(id, "allow");
                let view = json!(state.hooks);
                state.emit("hooks.changed", view);
            }
        }
        let mut state = self.state.lock().await;
        if state.hooks.expire() {
            let view = json!(state.hooks);
            state.emit("hooks.changed", view);
        }
        let request = state
            .hooks
            .approvals
            .iter()
            .find(|r| r.request_id == id)
            .ok_or(Error("HOOK_REQUEST_NOT_FOUND"))?;
        Ok(json!({"requestId":id,"decision":request.decision}))
    }
}
