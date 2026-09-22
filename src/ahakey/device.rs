use super::hardware::Hardware;

/// Model-independent owner. Each concrete model implements Hardware on its own struct.
pub struct AhaKey {
    pub device_id: String,
    pub name: String,
    pub hardware: Box<dyn Hardware>,
}
