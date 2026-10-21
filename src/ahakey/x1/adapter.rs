use super::{ModelX1, commands::frame};
use crate::{
    ahakey::{
        hardware::Hardware,
        state::{HardwareState, X1State},
    },
    error::{Error, Result},
};
use async_trait::async_trait;
use btleplug::api::Peripheral as _;
use std::time::Duration;
#[async_trait]
impl Hardware for ModelX1 {
    async fn oled_state(&mut self) -> Result<crate::oled::State> {
        super::oled::state(self).await
    }
    async fn upload_oled(
        &mut self,
        target: &crate::oled::Target,
        asset: &crate::oled::Prepared,
        pause: tokio::sync::watch::Receiver<bool>,
        progress: tokio::sync::mpsc::Sender<f64>,
    ) -> Result<()> {
        super::oled::upload(self, target, asset, pause, progress).await
    }
    fn configuration_commands(
        &self,
        configuration: &crate::configuration::Configuration,
    ) -> Result<Vec<Vec<u8>>> {
        crate::ahakey::x1::commands::configuration_commands(configuration)
    }
    fn keymap_commands(
        &self,
        request: &crate::configuration::keymap::KeymapApply,
    ) -> Result<Vec<Vec<u8>>> {
        crate::ahakey::x1::commands::keymap_commands(request)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        crate::debug::event(
            "ble.connect",
            || serde_json::json!({"deviceId":format!("x1-{}", self.peripheral.id())}),
        );
        let result = match tokio::time::timeout(Duration::from_secs(15), self.open()).await {
            Ok(result) => result,
            Err(_) => {
                eprintln!("X1 connection setup exceeded 15 seconds");
                Err(Error("DEVICE_TIMEOUT"))
            }
        };
        if result.is_err() {
            let _ = self.disconnect().await;
        }
        crate::debug::event("ble.connectResult", || {
            serde_json::json!({
                "deviceId":format!("x1-{}", self.peripheral.id()),
                "error":result.as_ref().err().map(|error| error.0),
            })
        });
        result
    }
    async fn disconnect(&mut self) -> Result<()> {
        crate::debug::event(
            "ble.disconnect",
            || serde_json::json!({"deviceId":format!("x1-{}", self.peripheral.id())}),
        );
        self.command_char = None;
        self.notifications = None;
        tokio::time::timeout(Duration::from_secs(3), self.peripheral.disconnect())
            .await
            .map_err(|_| Error("DEVICE_TIMEOUT"))?
            .map_err(|_| Error("DEVICE_DISCONNECTED"))
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        let bytes = self.exchange(&frame(0, &[])).await?;
        X1State::from_bytes(&bytes)
            .map(|s| s.0)
            .ok_or(Error("DEVICE_INVALID_RESPONSE"))
    }
    async fn next_event(&mut self) -> Result<HardwareState> {
        self.notifications
            .as_mut()
            .ok_or(Error("DEVICE_NOT_READY"))?
            .next_state()
            .await
    }
    async fn read_configuration(
        &mut self,
    ) -> Result<Option<crate::configuration::ConfigurationReadback>> {
        if std::mem::take(&mut self.skip_readback_once) {
            return Ok(None);
        }
        let result = super::readback::read_bounded(self, Duration::from_secs(45)).await;
        match &result {
            Err(error)
                if matches!(
                    error.0,
                    "UNSUPPORTED_CONFIGURATION" | "CONFIGURATION_CHANGED"
                ) => {}
            Err(_) => {
                let _ = self.disconnect().await;
            }
            _ => {}
        }
        result
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        self.exchange(command).await?;
        if command[2] == 0x85 && self.refresh().await?.brightness != Some(command[3]) {
            return Err(Error("UNSUPPORTED_CAPABILITY"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    }
}
