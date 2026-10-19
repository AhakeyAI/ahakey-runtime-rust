use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub(super) trait CommandTransport: Send {
    async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>>;
}
