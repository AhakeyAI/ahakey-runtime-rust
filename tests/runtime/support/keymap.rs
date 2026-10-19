use ahakey_runtime_rust::{
    ahakey::{hardware::Hardware, state::HardwareState},
    configuration::Configuration,
    error::{Error, Result},
};
use async_trait::async_trait;
use serde_json::json;
use std::{sync::Arc, time::Duration};
pub(crate) struct KeyOnlyHardware {
    pub(crate) writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    pub(crate) fail_at: Option<usize>,
}
#[async_trait]
impl Hardware for KeyOnlyHardware {
    fn configuration_commands(&self, _: &Configuration) -> Result<Vec<Vec<u8>>> {
        Err(Error("UNSUPPORTED_CAPABILITY"))
    }
    fn keymap_commands(
        &self,
        request: &ahakey_runtime_rust::keymap::KeymapApply,
    ) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::keymap_commands(request)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 2,
            light_mode: 0,
            battery: Some(50),
            lever: Some("manual"),
            brightness: None,
        })
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.connect().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut writes = self.writes.lock().unwrap();
        writes.push(command.to_vec());
        if self.fail_at == Some(writes.len()) {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}
pub(crate) fn keys_request(token: &str, id: &str) -> ahakey_runtime_rust::keymap::KeymapApply {
    serde_json::from_value(json!({"operationId":id,"deviceId":"test-x1","baseToken":token,"mode":2,
        "keys":([7,9,13,14].iter().enumerate().map(|(index,code)| json!({"index":index,"action":{"type":"shortcut","modifiers":[],"keyCode":code}})).collect::<Vec<_>>())})).unwrap()
}
