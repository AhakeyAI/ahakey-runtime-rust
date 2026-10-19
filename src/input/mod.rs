//! Host HID observations, independent of BLE and physical key configuration.
mod model;
mod monitor;
pub use model::*;
pub use monitor::monitor;
