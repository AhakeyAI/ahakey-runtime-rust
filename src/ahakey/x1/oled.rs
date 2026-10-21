//! CP2 resource protocol, verified against remote Fireware-harness 70ca334f.
use super::transport::CommandTransport;
use crate::{
    ahakey::x1::commands::{frame, valid_frame},
    error::{Error, Result},
    oled::{Prepared, Target},
};
use tokio::sync::{mpsc, watch};

pub(super) const CATALOG: &[u8] = b"x1-c582-hw1-p1-1.0.0-r001";
pub const PROFILE: &str = "x1-cp2-resource-v1";

pub use crate::oled::{Entry, State};

pub(super) async fn identify(io: &mut impl CommandTransport) -> Result<()> {
    let response = io.query(&frame(0x9f, &[])).await?;
    if response != frame(0x9f, &[&[0][..], CATALOG].concat()) {
        return Err(Error("UNSUPPORTED_CAPABILITY"));
    }
    // Catalog is shared by older builds. Actual typed read replies are required;
    // a generic success ACK is never accepted as resource capability evidence.
    metadata(io, Target::ModeDefault { mode: 0 }).await?;
    Ok(())
}
pub(super) async fn metadata(io: &mut impl CommandTransport, target: Target) -> Result<Entry> {
    let entry = target.entry()?;
    let basic = io.query(&frame(0x88, &[2, entry])).await?;
    if !valid_frame(&basic)
        || basic.len() != 10
        || basic[2..6] != [0x88, 0, 2, entry]
        || basic[6] > 1
        || basic[7] > 3
        || (basic[6] == 1) != (basic[7] == 2)
    {
        return Err(Error("UNSUPPORTED_CAPABILITY"));
    }
    let integrity = io.query(&frame(0x88, &[3, entry])).await?;
    if !valid_frame(&integrity) || integrity.len() != 18 || integrity[2..6] != [0x88, 0, 3, entry] {
        return Err(Error("DEVICE_INVALID_RESPONSE"));
    }
    let frame_count = u16::from_le_bytes(integrity[6..8].try_into().unwrap());
    let byte_length = u32::from_le_bytes(integrity[8..12].try_into().unwrap());
    let crc32 = u32::from_le_bytes(integrity[12..16].try_into().unwrap());
    let valid = basic[6] == 1;
    if (valid
        && (frame_count == 0
            || frame_count > target.max_frames()
            || byte_length != frame_count as u32 * crate::oled::FRAME_BYTES as u32))
        || (!valid && (frame_count != 0 || byte_length != 0 || crc32 != 0))
    {
        return Err(Error("DEVICE_INVALID_RESPONSE"));
    }
    Ok(Entry {
        max_frames: target.max_frames(),
        target,
        valid,
        write_state: ["empty", "writing", "valid", "invalid"][basic[7] as usize].into(),
        frame_count,
        byte_length,
        crc32,
    })
}
pub(super) async fn state(io: &mut impl CommandTransport) -> Result<State> {
    use crate::oled::ScreenState;
    identify(io).await?;
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
    let mut entries = Vec::new();
    for target in targets {
        entries.push(metadata(io, target).await?);
    }
    Ok(State {
        profile: PROFILE.into(),
        width: crate::oled::WIDTH,
        height: crate::oled::HEIGHT,
        interval_ms: 100,
        entries,
    })
}
pub(super) async fn upload(
    io: &mut impl CommandTransport,
    target: &Target,
    asset: &Prepared,
    pause: watch::Receiver<bool>,
    progress: mpsc::Sender<f64>,
) -> Result<()> {
    identify(io).await?;
    let current = metadata(io, target.clone()).await?;
    if asset.info.frame_count > target.max_frames() {
        return Err(Error("TOO_MANY_FRAMES"));
    }
    // Resume/uncertain commit: inspect the device before overwriting any bytes.
    if current.valid
        && current.frame_count == asset.info.frame_count
        && current.byte_length as usize == asset.info.byte_length
        && current.crc32 == asset.info.crc32
    {
        return Ok(());
    }
    if *pause.borrow() {
        return Err(Error("UPLOAD_PAUSED"));
    }
    let entry = target.entry()?;
    let mut begin = vec![entry];
    begin.extend_from_slice(&(asset.bytes.len() as u32).to_le_bytes());
    begin.extend_from_slice(&asset.info.frame_count.to_le_bytes());
    begin.extend_from_slice(&asset.info.crc32.to_le_bytes());
    ack(io, 0x8a, &begin).await?;
    let mut reported = 0;
    for (index, bytes) in asset.bytes.chunks(48).enumerate() {
        if *pause.borrow() {
            return Err(Error("UPLOAD_PAUSED"));
        }
        let offset = index * 48;
        let mut chunk = vec![entry];
        chunk.extend_from_slice(&(offset as u32).to_le_bytes());
        chunk.push(bytes.len() as u8);
        chunk.extend_from_slice(bytes);
        ack(io, 0x8b, &chunk).await?;
        let completed = offset + bytes.len();
        if completed - reported >= 4096 || completed == asset.bytes.len() {
            // Reserve the last 1% for commit and readback, never report complete early.
            let _ = progress
                .send(completed as f64 / asset.bytes.len() as f64 * 0.99)
                .await;
            reported = completed;
        }
    }
    if *pause.borrow() {
        return Err(Error("UPLOAD_PAUSED"));
    }
    ack(io, 0x8c, &[entry]).await?;
    let actual = metadata(io, target.clone()).await?;
    if !actual.valid
        || actual.frame_count != asset.info.frame_count
        || actual.byte_length as usize != asset.bytes.len()
        || actual.crc32 != asset.info.crc32
    {
        return Err(Error("RESOURCE_VERIFY_FAILED"));
    }
    Ok(())
}
async fn ack(io: &mut impl CommandTransport, opcode: u8, payload: &[u8]) -> Result<()> {
    let response = io.query(&frame(opcode, payload)).await?;
    if response != frame(opcode, &[0]) {
        return Err(Error("DEVICE_INVALID_RESPONSE"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oled::{FRAME_BYTES, Resource};
    use std::collections::VecDeque;

    struct Script(VecDeque<(Vec<u8>, Vec<u8>)>);
    #[async_trait::async_trait]
    impl CommandTransport for Script {
        async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>> {
            let (expected, reply) = self.0.pop_front().expect("unexpected device command");
            assert_eq!(command, expected);
            Ok(reply)
        }
    }
    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect()
    }
    fn metadata_script(entry: u8, valid: bool, crc: u32) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut integrity = vec![0, 3, entry];
        integrity.extend_from_slice(&(if valid { 1u16 } else { 0 }).to_le_bytes());
        integrity.extend_from_slice(&(if valid { FRAME_BYTES as u32 } else { 0 }).to_le_bytes());
        integrity.extend_from_slice(&(if valid { crc } else { 0 }).to_le_bytes());
        vec![
            (
                frame(0x88, &[2, entry]),
                frame(
                    0x88,
                    &[0, 2, entry, u8::from(valid), if valid { 2 } else { 0 }],
                ),
            ),
            (frame(0x88, &[3, entry]), frame(0x88, &integrity)),
        ]
    }
    fn probe() -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut steps = vec![(frame(0x9f, &[]), frame(0x9f, &[&[0][..], CATALOG].concat()))];
        steps.extend(metadata_script(0, false, 0));
        steps
    }
    #[tokio::test]
    async fn remote_cp2_capture_upload_and_resume_after_uncertain_commit() {
        let capture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/x1-oled-cp2.json")).unwrap();
        let crc = capture["crc32"].as_u64().unwrap() as u32;
        let bytes = vec![0; FRAME_BYTES];
        assert_eq!(crate::oled::crc32(&bytes), crc);
        let asset = Prepared {
            info: Resource {
                resource_id: crate::oled::sha256(&bytes),
                width: 160,
                height: 80,
                frame_count: 1,
                interval_ms: 100,
                byte_length: FRAME_BYTES,
                crc32: crc,
            },
            bytes,
        };
        let mut steps = probe();
        steps.extend(metadata_script(8, false, 0));
        steps.push((hex(capture["begin"].as_str().unwrap()), frame(0x8a, &[0])));
        for offset in (0..FRAME_BYTES).step_by(48) {
            let mut request = hex(capture[if offset + 48 > FRAME_BYTES {
                "lastChunk"
            } else {
                "firstChunk"
            }]
            .as_str()
            .unwrap());
            request[4..8].copy_from_slice(&(offset as u32).to_le_bytes());
            steps.push((request, frame(0x8b, &[0])));
        }
        steps.push((hex(capture["commit"].as_str().unwrap()), frame(0x8c, &[0])));
        steps.extend(metadata_script(8, true, crc));
        let mut io = Script(steps.into());
        let (_, pause) = watch::channel(false);
        let (progress, mut updates) = mpsc::channel(128);
        upload(
            &mut io,
            &Target::ModeDefault { mode: 2 },
            &asset,
            pause,
            progress,
        )
        .await
        .unwrap();
        assert!(io.0.is_empty());
        let mut last = 0.0;
        while let Ok(value) = updates.try_recv() {
            assert!(value > last && value < 1.0);
            last = value;
        }
        assert_eq!(last, 0.99);
        // Device already committed, but the client lost the ACK: only reads, no rewrite.
        let mut steps = probe();
        steps.extend(metadata_script(8, true, crc));
        let mut io = Script(steps.into());
        let (_, pause) = watch::channel(false);
        let (progress, _) = mpsc::channel(128);
        upload(
            &mut io,
            &Target::ModeDefault { mode: 2 },
            &asset,
            pause,
            progress,
        )
        .await
        .unwrap();
        assert!(io.0.is_empty());
    }
    #[tokio::test]
    async fn shared_catalog_with_generic_ack_does_not_enable_resource_writes() {
        let mut steps = probe();
        steps.truncate(2);
        steps[1].1 = frame(0x88, &[0]);
        let mut io = Script(steps.into());
        assert_eq!(
            identify(&mut io).await.unwrap_err().0,
            "UNSUPPORTED_CAPABILITY"
        );
        assert!(io.0.is_empty());
    }
}
