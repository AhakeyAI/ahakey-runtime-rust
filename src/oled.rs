//! Application-facing OLED targets and immutable, locally staged image resources.
use crate::error::{Error, Result};
use image::{AnimationDecoder, ImageDecoder, ImageFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::PathBuf};
use uuid::Uuid;

pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 80;
pub const FRAME_BYTES: usize = 28_672;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScreenState {
    Working,
    Waiting,
    Done,
}
impl ScreenState {
    fn index(&self) -> u8 {
        match self {
            Self::Working => 1,
            Self::Waiting => 2,
            Self::Done => 3,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Target {
    ModeDefault { mode: u8 },
    ModeState { mode: u8, state: ScreenState },
    MultiState { state: ScreenState },
}
impl Target {
    pub fn entry(&self) -> Result<u8> {
        match self {
            Self::ModeDefault { mode } if *mode < 4 => Ok(mode * 4),
            Self::ModeState { mode, state } if *mode < 4 => Ok(mode * 4 + state.index()),
            Self::MultiState { state } => Ok(15 + state.index()),
            _ => Err(Error("INVALID_PARAMS")),
        }
    }
    pub fn max_frames(&self) -> u16 {
        if matches!(self, Self::ModeDefault { .. }) {
            8
        } else {
            12
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Upload {
    pub operation_id: String,
    pub device_id: String,
    pub base_token: String,
    pub resource_id: String,
    pub target: Target,
}
impl Upload {
    pub fn validate(&self) -> Result<()> {
        self.target.entry()?;
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.base_token.is_empty()
            || self.base_token.len() > 128
            || !valid_id(&self.resource_id)
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub resource_id: String,
    pub width: u32,
    pub height: u32,
    pub frame_count: u16,
    pub interval_ms: u16,
    pub byte_length: usize,
    pub crc32: u32,
}
pub struct Prepared {
    pub info: Resource,
    pub bytes: Vec<u8>,
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn prepared(bytes: Vec<u8>) -> Result<Prepared> {
    if bytes.is_empty()
        || !bytes.len().is_multiple_of(FRAME_BYTES)
        || bytes.len() > 12 * FRAME_BYTES
    {
        return Err(Error("RESOURCE_CORRUPT"));
    }
    let info = Resource {
        resource_id: sha256(&bytes),
        width: WIDTH,
        height: HEIGHT,
        frame_count: (bytes.len() / FRAME_BYTES) as u16,
        interval_ms: 100,
        byte_length: bytes.len(),
        crc32: crc32(&bytes),
    };
    Ok(Prepared { info, bytes })
}
fn encode_frame(image: image::RgbaImage, bytes: &mut Vec<u8>) -> Result<()> {
    if image.dimensions() != (WIDTH, HEIGHT) {
        return Err(Error("IMAGE_DIMENSIONS"));
    }
    if bytes.len() >= 12 * FRAME_BYTES {
        return Err(Error("TOO_MANY_FRAMES"));
    }
    let start = bytes.len();
    for pixel in image.pixels() {
        // Composite transparency onto black, then encode top-to-bottom RGB565 BE.
        let channel = |c: u8| c as u16 * pixel[3] as u16 / 255;
        let rgb = ((channel(pixel[0]) >> 3) << 11)
            | ((channel(pixel[1]) >> 2) << 5)
            | (channel(pixel[2]) >> 3);
        bytes.extend_from_slice(&rgb.to_be_bytes());
    }
    // CP2 CRC and offsets cover the complete 7-sector frame slot, including padding.
    bytes.resize(start + FRAME_BYTES, 0);
    Ok(())
}
fn decode(source: &[u8]) -> Result<Prepared> {
    let format = image::guess_format(source).map_err(|_| Error("INVALID_IMAGE"))?;
    let mut bytes = Vec::new();
    if format == ImageFormat::Gif {
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(source))
            .map_err(|_| Error("INVALID_IMAGE"))?;
        if decoder.dimensions() != (WIDTH, HEIGHT) {
            return Err(Error("IMAGE_DIMENSIONS"));
        }
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(8 * 1024 * 1024);
        decoder
            .set_limits(limits)
            .map_err(|_| Error("INVALID_IMAGE"))?;
        let mut delays = Vec::new();
        let finite_loop = matches!(decoder.loop_count(), image::metadata::LoopCount::Finite(_));
        for frame in decoder.into_frames() {
            let frame = frame.map_err(|_| Error("INVALID_IMAGE"))?;
            delays.push(frame.delay().numer_denom_ms());
            encode_frame(frame.into_buffer(), &mut bytes)?;
        }
        if delays.len() > 1 && delays.iter().any(|(n, d)| *n != 100 * *d) {
            return Err(Error("INCOMPATIBLE_FRAME_INTERVAL"));
        }
        if delays.len() > 1 && finite_loop {
            return Err(Error("INCOMPATIBLE_LOOP_COUNT"));
        }
    } else if matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(8 * 1024 * 1024);
        let image = if format == ImageFormat::Png {
            let decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(source), limits)
                .map_err(|_| Error("INVALID_IMAGE"))?;
            if decoder.dimensions() != (WIDTH, HEIGHT) {
                return Err(Error("IMAGE_DIMENSIONS"));
            }
            if decoder.is_apng().map_err(|_| Error("INVALID_IMAGE"))? {
                return Err(Error("UNSUPPORTED_IMAGE_FORMAT"));
            }
            image::DynamicImage::from_decoder(decoder)
        } else {
            let mut reader = image::ImageReader::with_format(Cursor::new(source), format);
            reader.limits(limits);
            let decoder = reader.into_decoder().map_err(|_| Error("INVALID_IMAGE"))?;
            if decoder.dimensions() != (WIDTH, HEIGHT) {
                return Err(Error("IMAGE_DIMENSIONS"));
            }
            image::DynamicImage::from_decoder(decoder)
        }
        .map_err(|_| Error("INVALID_IMAGE"))?;
        encode_frame(image.to_rgba8(), &mut bytes)?;
    } else {
        return Err(Error("INVALID_IMAGE"));
    }
    prepared(bytes)
}

#[derive(Clone)]
pub struct Resources {
    directory: PathBuf,
    writing: std::sync::Arc<tokio::sync::Mutex<()>>,
}
impl Resources {
    pub fn new(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            directory,
            writing: Default::default(),
        })
    }
    pub async fn finish(&self, source: Vec<u8>, expected_sha256: String) -> Result<Resource> {
        if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
            return Err(Error("INVALID_PARAMS"));
        }
        let _writing = self.writing.lock().await;
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            if sha256(&source) != expected_sha256 {
                return Err(Error("RESOURCE_HASH_MISMATCH"));
            }
            let asset = decode(&source)?;
            let path = directory.join(format!("{}.rgb565", asset.info.resource_id));
            if !path.exists() {
                let total = std::fs::read_dir(&directory)?.try_fold(
                    0u64,
                    |sum, entry| -> std::io::Result<_> {
                        Ok(sum.saturating_add(entry?.metadata()?.len()))
                    },
                )?;
                if total + asset.bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(Error("RESOURCE_STORE_FULL"));
                }
                crate::storage::atomic_write(&path, &asset.bytes)?;
            }
            Ok(asset.info)
        })
        .await
        .map_err(|_| Error("RESOURCE_ERROR"))?
    }
    pub async fn load(&self, id: &str) -> Result<Prepared> {
        if !valid_id(id) {
            return Err(Error("INVALID_PARAMS"));
        }
        let path = self.directory.join(format!("{id}.rgb565"));
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let metadata = std::fs::metadata(&path).map_err(|_| Error("RESOURCE_NOT_FOUND"))?;
            if metadata.len() > (12 * FRAME_BYTES) as u64 {
                return Err(Error("RESOURCE_CORRUPT"));
            }
            let asset = prepared(std::fs::read(path)?)?;
            if asset.info.resource_id != id {
                return Err(Error("RESOURCE_CORRUPT"));
            }
            Ok(asset)
        })
        .await
        .map_err(|_| Error("RESOURCE_ERROR"))?
    }
}

/// One bounded, connection-owned transfer. No hardware changes until oled.upload.
pub struct Transfer {
    pub id: Uuid,
    pub total: usize,
    pub hash: String,
    bytes: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Begin {
    pub byte_length: usize,
    pub sha256: String,
}
impl Transfer {
    pub fn begin(params: Begin) -> Result<Self> {
        if params.byte_length == 0
            || params.byte_length > MAX_SOURCE_BYTES
            || !valid_id(&params.sha256)
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            total: params.byte_length,
            hash: params.sha256,
            bytes: Vec::new(),
        })
    }
    pub fn append(&mut self, message: &[u8]) -> Result<usize> {
        if message.len() <= 20
            || message.len() > 20 + MAX_CHUNK_BYTES
            || message[..16] != self.id.as_bytes()[..]
        {
            return Err(Error("INVALID_RESOURCE_CHUNK"));
        }
        let offset = u32::from_le_bytes(message[16..20].try_into().unwrap()) as usize;
        let payload = &message[20..];
        if offset != self.bytes.len() || payload.len() > self.total - self.bytes.len() {
            return Err(Error("INVALID_RESOURCE_CHUNK"));
        }
        self.bytes.extend_from_slice(payload);
        Ok(self.bytes.len())
    }
    pub fn complete(self, id: Uuid) -> Result<(Vec<u8>, String)> {
        if id != self.id || self.bytes.len() != self.total {
            return Err(Error("RESOURCE_INCOMPLETE"));
        }
        Ok((self.bytes, self.hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_conversion_and_animation_limits_preserve_source_intent() {
        let rgba = image::RgbaImage::from_pixel(WIDTH, HEIGHT, image::Rgba([255, 0, 0, 255]));
        let mut png = Cursor::new(Vec::new());
        rgba.write_to(&mut png, ImageFormat::Png).unwrap();
        let asset = decode(png.get_ref()).unwrap();
        assert_eq!(asset.bytes.len(), FRAME_BYTES);
        assert!(
            asset.bytes[..25_600]
                .as_chunks::<2>()
                .0
                .iter()
                .all(|p| p == &[0xf8, 0])
        );
        assert!(asset.bytes[25_600..].iter().all(|b| *b == 0));
        for (count, delay, expected) in [
            (2, 100, None),
            (2, 50, Some("INCOMPATIBLE_FRAME_INTERVAL")),
            (13, 100, Some("TOO_MANY_FRAMES")),
        ] {
            let mut gif = Vec::new();
            {
                let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
                encoder
                    .encode_frames((0..count).map(|_| {
                        image::Frame::from_parts(
                            rgba.clone(),
                            0,
                            0,
                            image::Delay::from_numer_denom_ms(delay, 1),
                        )
                    }))
                    .unwrap();
            }
            match (decode(&gif), expected) {
                (Ok(asset), None) => assert_eq!(asset.info.frame_count, count),
                (Err(error), Some(code)) => assert_eq!(error.0, code),
                _ => panic!("unexpected GIF acceptance"),
            }
        }
    }
}
