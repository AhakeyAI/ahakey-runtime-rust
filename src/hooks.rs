//! Tool lifecycle and per-request approvals. No global IDE permission settings are changed.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub device_id: Option<String>,
    pub allow_automatic: bool,
    pub input_device_id: Option<String>,
    pub approve_usage: Option<u32>,
    pub reject_usage: Option<u32>,
}
impl Preferences {
    pub fn validate(&self) -> Result<()> {
        if self.enabled && self.device_id.is_none() {
            return Err(Error("HOOK_DEVICE_REQUIRED"));
        }
        for id in [&self.device_id, &self.input_device_id]
            .into_iter()
            .flatten()
        {
            if id.is_empty() || id.len() > 256 {
                return Err(Error("INVALID_PARAMS"));
            }
        }
        for usage in [self.approve_usage, self.reject_usage]
            .into_iter()
            .flatten()
        {
            if !(4..=115).contains(&usage) || usage == 109 {
                return Err(Error("INVALID_PARAMS"));
            }
        }
        if self.approve_usage.is_some() && self.approve_usage == self.reject_usage {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookEvent {
    pub event_id: String,
    pub tool: String,
    pub session_id: String,
    pub event: String,
    pub cwd: String,
    pub tool_name: String,
    pub summary: String,
}
impl HookEvent {
    pub fn approval(&self) -> bool {
        match self.tool.as_str() {
            "claude" | "codex" => self.event == "PermissionRequest",
            "cursor" => matches!(
                self.event.as_str(),
                "beforeShellExecution" | "beforeMCPExecution"
            ),
            _ => false,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !["claude", "codex", "cursor"].contains(&self.tool.as_str())
            || self.event_id.is_empty()
            || self.event_id.len() > 128
            || self.session_id.is_empty()
            || self.session_id.len() > 256
            || self.cwd.len() > 4096
            || self.summary.len() > 16_384
            || self.tool_name.len() > 256
            || self.event.len() > 64
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub tool: String,
    pub session_id: String,
    pub cwd: String,
    pub event: String,
    pub status: String,
    pub updated_at_ms: u64,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub request_id: String,
    pub context: HookEvent,
    pub decision: String,
    pub expires_at_ms: u64,
    #[serde(skip)]
    pub deadline: std::time::Instant,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub revision: u64,
    pub preferences: Preferences,
    pub sessions: VecDeque<Session>,
    pub approvals: VecDeque<Approval>,
    pub selected_request_id: Option<String>,
    pub learning: Option<String>,
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
impl Controller {
    pub fn expire(&mut self) -> bool {
        let mut changed = false;
        for request in &mut self.approvals {
            if request.decision == "pending" && request.deadline <= std::time::Instant::now() {
                request.decision = "defer".into();
                changed = true;
            }
        }
        changed
    }
    pub fn receive(&mut self, context: HookEvent) -> Result<Value> {
        context.validate()?;
        self.expire();
        if !self.preferences.enabled {
            return Ok(json!({"decision":"defer"}));
        }
        if let Some(existing) = self
            .approvals
            .iter()
            .find(|a| a.context.event_id == context.event_id)
        {
            if serde_json::to_value(&existing.context).unwrap()
                != serde_json::to_value(&context).unwrap()
            {
                return Err(Error("HOOK_EVENT_CONFLICT"));
            }
            return Ok(json!({"requestId":existing.request_id,"decision":existing.decision}));
        }
        let state = match context.event.to_lowercase().as_str() {
            "permissionrequest" | "beforeshellexecution" | "beforemcpexecution" => "waiting",
            "stop" | "taskcompleted" => "done",
            "sessionend" | "interrupt" => "idle",
            "posttoolusefailure" => "error",
            _ => "working",
        };
        self.sessions
            .retain(|s| s.tool != context.tool || s.session_id != context.session_id);
        self.sessions.push_front(Session {
            tool: context.tool.clone(),
            session_id: context.session_id.clone(),
            cwd: context.cwd.clone(),
            event: context.event.clone(),
            status: state.into(),
            updated_at_ms: now_ms(),
        });
        self.sessions.truncate(32);
        if matches!(
            context.event.to_lowercase().as_str(),
            "stop" | "interrupt" | "sessionend"
        ) {
            for request in &mut self.approvals {
                if request.context.tool == context.tool
                    && request.context.session_id == context.session_id
                    && request.decision == "pending"
                {
                    request.decision = "defer".into();
                }
            }
        }
        if !context.approval() {
            return Ok(json!({"decision":"defer"}));
        }
        if self
            .approvals
            .iter()
            .filter(|a| a.decision == "pending")
            .count()
            >= 16
        {
            return Err(Error("HOOK_QUEUE_FULL"));
        }
        if self.approvals.len() >= 64
            && let Some(index) = self.approvals.iter().rposition(|a| a.decision != "pending")
        {
            self.approvals.remove(index);
        }
        let id = Uuid::new_v4().to_string();
        self.approvals.push_front(Approval {
            request_id: id.clone(),
            context,
            decision: "pending".into(),
            expires_at_ms: now_ms() + 60_000,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(60),
        });
        Ok(json!({"requestId":id,"decision":"pending"}))
    }
    pub fn decide(&mut self, id: &str, decision: &str) -> Result<Value> {
        if !["allow", "deny", "defer"].contains(&decision) {
            return Err(Error("INVALID_PARAMS"));
        }
        self.expire();
        let request = self
            .approvals
            .iter_mut()
            .find(|r| r.request_id == id)
            .ok_or(Error("HOOK_REQUEST_NOT_FOUND"))?;
        if request.decision != "pending" {
            return Err(Error("HOOK_REQUEST_FINISHED"));
        }
        request.decision = decision.into();
        Ok(json!({"requestId":id,"decision":decision}))
    }
    pub fn key(&mut self, event: &crate::input::KeyEvent) -> bool {
        if !self.preferences.enabled
            || !event.pressed
            || self.preferences.input_device_id.as_deref() != Some(&event.input_device_id)
        {
            return false;
        }
        if let Some(learning) = self.learning.clone() {
            if !(4..=115).contains(&event.usage) || event.usage == 109 {
                return false;
            }
            if learning == "approve" && Some(event.usage) != self.preferences.reject_usage {
                self.preferences.approve_usage = Some(event.usage);
                self.learning = None;
            }
            if learning == "reject" && Some(event.usage) != self.preferences.approve_usage {
                self.preferences.reject_usage = Some(event.usage);
                self.learning = None;
            }
            return true;
        }
        let decision = if Some(event.usage) == self.preferences.approve_usage {
            "allow"
        } else if Some(event.usage) == self.preferences.reject_usage {
            "deny"
        } else {
            return false;
        };
        self.expire();
        let pending: Vec<_> = self
            .approvals
            .iter()
            .filter(|r| r.decision == "pending")
            .map(|r| r.request_id.clone())
            .collect();
        let selected = self
            .selected_request_id
            .as_ref()
            .filter(|id| pending.contains(id))
            .cloned()
            .or_else(|| (pending.len() == 1).then(|| pending[0].clone()));
        if let Some(id) = selected {
            let _ = self.decide(&id, decision);
            return true;
        }
        false
    }
    pub fn deactivate(&mut self) {
        self.revision += 1;
        self.preferences.enabled = false;
        self.preferences.allow_automatic = false;
        self.learning = None;
        self.preferences.approve_usage = None;
        self.preferences.reject_usage = None;
        for request in &mut self.approvals {
            if request.decision == "pending" {
                request.decision = "defer".into();
            }
        }
    }
}

/// Only documented per-event decisions. An unavailable Runtime returns control to the host.
pub fn hook_output(tool: &str, event: &str, decision: &str) -> Value {
    if tool == "cursor" && matches!(event, "beforeShellExecution" | "beforeMCPExecution") {
        return json!({"permission":match decision { "allow"=>"allow", "deny"=>"deny", _=>"ask" }});
    }
    if matches!(tool, "claude" | "codex")
        && event == "PermissionRequest"
        && matches!(decision, "allow" | "deny")
    {
        return json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest", "decision":{"behavior":decision}}});
    }
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(tool: &str, event: &str) -> HookEvent {
        HookEvent {
            event_id: Uuid::new_v4().to_string(),
            tool: tool.into(),
            session_id: "session".into(),
            event: event.into(),
            cwd: "/work".into(),
            tool_name: "Bash".into(),
            summary: "echo hello".into(),
        }
    }
    #[test]
    fn only_permission_events_wait_and_decisions_are_once_only() {
        let mut c = Controller::default();
        c.preferences.enabled = true;
        assert_eq!(
            c.receive(event("codex", "PreToolUse")).unwrap()["decision"],
            "defer"
        );
        let e = event("codex", "PermissionRequest");
        let result = c.receive(e.clone()).unwrap();
        assert_eq!(c.receive(e).unwrap(), result);
        let id = result["requestId"].as_str().unwrap();
        c.decide(id, "deny").unwrap();
        assert_eq!(
            c.decide(id, "allow").unwrap_err().0,
            "HOOK_REQUEST_FINISHED"
        );
    }
    #[test]
    fn timeout_and_disable_never_allow_pending_requests() {
        let mut c = Controller::default();
        c.preferences.enabled = true;
        c.receive(event("claude", "PermissionRequest")).unwrap();
        c.approvals[0].deadline = std::time::Instant::now();
        c.expire();
        assert_eq!(c.approvals[0].decision, "defer");
        c.receive(event("cursor", "beforeShellExecution")).unwrap();
        c.deactivate();
        assert!(c.approvals.iter().all(|a| a.decision == "defer"));
    }
    #[test]
    fn protocol_outputs_do_not_invent_ask_for_permission_request() {
        assert_eq!(
            hook_output("claude", "PermissionRequest", "defer"),
            json!({})
        );
        assert_eq!(
            hook_output("codex", "PermissionRequest", "allow")["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        assert_eq!(
            hook_output("cursor", "beforeShellExecution", "defer")["permission"],
            "ask"
        );
        assert_eq!(hook_output("codex", "PreToolUse", "allow"), json!({}));
    }
}
