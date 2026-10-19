use super::state::HardwareState;
use crate::configuration::{Configuration, ConfigurationReadback};
use crate::error::Result;
use async_trait::async_trait;

/// Called exclusively by one device worker: there is never more than one ACK in flight.
#[async_trait]
pub trait Hardware: Send {
    async fn oled_state(&mut self) -> Result<crate::oled::State> {
        Err(crate::error::Error("UNSUPPORTED_CAPABILITY"))
    }
    async fn upload_oled(
        &mut self,
        _target: &crate::oled::Target,
        _asset: &crate::oled::Prepared,
        _pause: tokio::sync::watch::Receiver<bool>,
        _progress: tokio::sync::mpsc::Sender<f64>,
    ) -> Result<()> {
        Err(crate::error::Error("UNSUPPORTED_CAPABILITY"))
    }
    /// Encoding belongs to the concrete model, never to the IPC or runtime scheduler.
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>>;
    fn keymap_commands(
        &self,
        _request: &crate::configuration::keymap::KeymapApply,
    ) -> Result<Vec<Vec<u8>>> {
        Err(crate::error::Error("UNSUPPORTED_CAPABILITY"))
    }
    async fn connect(&mut self) -> Result<HardwareState>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn refresh(&mut self) -> Result<HardwareState>;
    /// Read all observable configuration fields. None means unsupported.
    /// The worker serializes this with writes; missing fields remain unknown.
    async fn read_configuration(&mut self) -> Result<Option<ConfigurationReadback>> {
        Ok(None)
    }
    /// Wait for a device notification; must be cancellation-safe when a control job arrives.
    /// Static simulators may remain pending. Connected devices must also report link loss.
    async fn next_event(&mut self) -> Result<HardwareState> {
        std::future::pending().await
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()>;
}
