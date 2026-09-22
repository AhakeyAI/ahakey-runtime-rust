use super::state::HardwareState;
use crate::configuration::Configuration;
use crate::error::Result;
use async_trait::async_trait;

/// Called exclusively by one device worker: there is never more than one ACK in flight.
#[async_trait]
pub trait Hardware: Send {
    /// Encoding belongs to the concrete model, never to the IPC or runtime scheduler.
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>>;
    async fn connect(&mut self) -> Result<HardwareState>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn refresh(&mut self) -> Result<HardwareState>;
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()>;
}
