//! Authenticated native WebSocket transport and JSON-RPC method dispatch.
mod client;
mod dispatch;
mod protocol;
mod resources;
mod server;
mod session;
pub use server::serve;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
