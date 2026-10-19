pub(crate) fn oled_png() -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(160, 80, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
pub(crate) fn oled_gif() -> Vec<u8> {
    let mut output = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut output, 30);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        let mut seed = 1u32;
        for _ in 0..6 {
            let pixels = image::RgbaImage::from_fn(160, 80, |_, _| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                image::Rgba([
                    (seed >> 24) as u8,
                    (seed >> 16) as u8,
                    (seed >> 8) as u8,
                    255,
                ])
            });
            encoder
                .encode_frame(image::Frame::from_parts(
                    pixels,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
        }
    }
    assert!(output.len() > ahakey_runtime_rust::oled::MAX_CHUNK_BYTES);
    output
}
