//! X1 adapter: BLE session, protocol encoding, notifications and firmware resources.
mod adapter;
pub mod commands;
mod connection;
mod notifications;
pub mod oled;
mod readback;
pub mod state;
mod transport;
use btleplug::{
    api::Characteristic,
    platform::{Adapter, Peripheral},
};
use notifications::Notifications;
/// X1 alone owns its GATT session and the notification stream used to confirm commands.
pub struct ModelX1 {
    // Peripheral uses a weak adapter reference on macOS. Keep its owner alive.
    _adapter: Adapter,
    peripheral: Peripheral,
    command_char: Option<Characteristic>,
    notifications: Option<Notifications>,
    skip_readback_once: bool,
}
impl ModelX1 {
    pub fn new(peripheral: Peripheral, adapter: Adapter) -> Self {
        Self {
            _adapter: adapter,
            peripheral,
            command_char: None,
            notifications: None,
            skip_readback_once: false,
        }
    }
}
