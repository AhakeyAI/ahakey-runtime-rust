use crate::protocol::x1::valid_frame;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareState {
    pub work_mode: u8,
    pub light_mode: u8,
    pub battery: Option<u8>,
    pub lever: Option<&'static str>,
    pub brightness: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct X1State(pub HardwareState);
impl X1State {
    /// Old status frames have no brightness byte; a reserved zero is not support.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if !valid_frame(data) || data.len() < 12 || data[2] != 0 || data[7] > 3 {
            return None;
        }
        Some(Self(HardwareState {
            battery: (data[3] <= 100).then_some(data[3]),
            work_mode: data[7],
            light_mode: data[8],
            lever: match data[9] {
                0 => Some("automatic"),
                1 => Some("manual"),
                _ => None,
            },
            brightness: if data.len() >= 13 && (1..=100).contains(&data[10]) {
                Some(data[10])
            } else {
                None
            },
        }))
    }
}
