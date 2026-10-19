use super::{hardware::Hardware, state::HardwareState};
use crate::error::{Error, Result};
use async_trait::async_trait;

/// Explicit simulator for frontend integration; never selected by BLE discovery.
pub struct MockX1 {
    connected: bool,
    brightness: u8,
    oled: std::collections::BTreeMap<u8, crate::oled::Resource>,
}
impl Default for MockX1 {
    fn default() -> Self {
        Self {
            connected: false,
            brightness: 35,
            oled: Default::default(),
        }
    }
}
#[async_trait]
impl Hardware for MockX1 {
    async fn oled_state(&mut self) -> Result<super::oled::State> {
        self.refresh().await?;
        use crate::oled::{ScreenState, Target};
        let mut targets = Vec::new();
        for mode in 0..4 {
            targets.push(Target::ModeDefault { mode });
            for state in [
                ScreenState::Working,
                ScreenState::Waiting,
                ScreenState::Done,
            ] {
                targets.push(Target::ModeState { mode, state });
            }
        }
        for state in [
            ScreenState::Working,
            ScreenState::Waiting,
            ScreenState::Done,
        ] {
            targets.push(Target::MultiState { state });
        }
        Ok(super::oled::State {
            profile: super::oled::PROFILE.into(),
            width: crate::oled::WIDTH,
            height: crate::oled::HEIGHT,
            interval_ms: 100,
            entries: targets
                .into_iter()
                .map(|target| {
                    let asset = self.oled.get(&target.entry().unwrap());
                    super::oled::Entry {
                        max_frames: target.max_frames(),
                        target,
                        valid: asset.is_some(),
                        write_state: if asset.is_some() { "valid" } else { "empty" }.into(),
                        frame_count: asset.map_or(0, |a| a.frame_count),
                        byte_length: asset.map_or(0, |a| a.byte_length as u32),
                        crc32: asset.map_or(0, |a| a.crc32),
                    }
                })
                .collect(),
        })
    }
    async fn upload_oled(
        &mut self,
        target: &crate::oled::Target,
        asset: &crate::oled::Prepared,
        pause: tokio::sync::watch::Receiver<bool>,
        progress: tokio::sync::mpsc::Sender<f64>,
    ) -> Result<()> {
        self.refresh().await?;
        let entry = target.entry()?;
        if self.oled.get(&entry) == Some(&asset.info) {
            return Ok(());
        }
        self.oled.remove(&entry);
        for completed in (0..asset.bytes.len()).step_by(4096) {
            if *pause.borrow() {
                return Err(Error("UPLOAD_PAUSED"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(8)).await;
            let _ = progress
                .send(
                    (completed + 4096).min(asset.bytes.len()) as f64 / asset.bytes.len() as f64
                        * 0.99,
                )
                .await;
        }
        if *pause.borrow() {
            return Err(Error("UPLOAD_PAUSED"));
        }
        self.oled.insert(entry, asset.info.clone());
        Ok(())
    }
    fn configuration_commands(
        &self,
        configuration: &crate::configuration::Configuration,
    ) -> Result<Vec<Vec<u8>>> {
        crate::protocol::x1::configuration_commands(configuration)
    }
    fn keymap_commands(&self, request: &crate::keymap::KeymapApply) -> Result<Vec<Vec<u8>>> {
        crate::protocol::x1::keymap_commands(request)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.connected = true;
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        if !self.connected {
            return Err(Error("DEVICE_NOT_READY"));
        }
        Ok(HardwareState {
            battery: Some(82),
            work_mode: 0,
            light_mode: 0,
            lever: Some("manual"),
            brightness: Some(self.brightness),
        })
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        self.refresh().await?;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        if command[2] == 0x85 {
            self.brightness = command[3];
        }
        Ok(())
    }
}
