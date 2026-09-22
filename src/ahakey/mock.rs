use super::{hardware::Hardware, state::HardwareState};
use crate::error::{Error, Result};
use async_trait::async_trait;

/// Explicit simulator for frontend integration; never selected by BLE discovery.
pub struct MockX1 {
    connected: bool,
    brightness: u8,
}
impl Default for MockX1 {
    fn default() -> Self {
        Self {
            connected: false,
            brightness: 35,
        }
    }
}
#[async_trait]
impl Hardware for MockX1 {
    fn configuration_commands(
        &self,
        configuration: &crate::configuration::Configuration,
    ) -> Result<Vec<Vec<u8>>> {
        crate::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.connected = true;
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        if !self.connected {
            return Err(Error("DEVICE_NOT_READY"));
        }
        Ok(HardwareState {
            battery: Some(82),
            work_mode: 0,
            light_mode: 0,
            lever: Some("manual"),
            brightness: Some(self.brightness),
        })
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        self.refresh().await?;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        if command[2] == 0x85 {
            self.brightness = command[3];
        }
        Ok(())
    }
}
