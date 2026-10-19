#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareState {
    pub work_mode: u8,
    pub light_mode: u8,
    pub battery: Option<u8>,
    pub lever: Option<&'static str>,
    pub brightness: Option<u8>,
}

// Existing Rust clients can still use ahakey::state::X1State.
pub use super::x1::state::X1State;
