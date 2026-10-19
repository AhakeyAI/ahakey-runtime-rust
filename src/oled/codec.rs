use super::{FRAME_BYTES, HEIGHT, Prepared, Resource, WIDTH, crc32, sha256};
use crate::error::{Error, Result};
use image::{AnimationDecoder, ImageDecoder, ImageFormat};
use std::io::Cursor;

pub(super) fn prepared(bytes: Vec<u8>) -> Result<Prepared> {
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
pub(super) fn decode(source: &[u8]) -> Result<Prepared> {
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
