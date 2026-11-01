use avif_decode::{Decoder, Image};
use image::{DynamicImage, ImageBuffer};
use ravif::{AlphaColorMode, BitDepth, Encoder, Img, RGBA8};

pub fn decode(data: &[u8]) -> Result<DynamicImage, String> {
    let image = Decoder::from_avif(data)
        .and_then(Decoder::to_image)
        .map_err(|e| e.to_string())?;

    macro_rules! convert {
        ($image:expr, $variant:ident, |$pixel:ident| $channels:expr) => {{
            let (width, height) = ($image.width() as u32, $image.height() as u32);
            let samples = $image.pixels().flat_map(|$pixel| $channels).collect();
            DynamicImage::$variant(
                ImageBuffer::from_raw(width, height, samples).expect("buffer matches its size"),
            )
        }};
    }
    Ok(match image {
        Image::Rgb8(image) => convert!(image, ImageRgb8, |p| [p.r, p.g, p.b]),
        Image::Rgb16(image) => convert!(image, ImageRgb16, |p| [p.r, p.g, p.b]),
        Image::Rgba8(image) => convert!(image, ImageRgba8, |p| [p.r, p.g, p.b, p.a]),
        Image::Rgba16(image) => convert!(image, ImageRgba16, |p| [p.r, p.g, p.b, p.a]),
        Image::Gray8(image) => convert!(image, ImageLuma8, |p| [p.value()]),
        Image::Gray16(image) => convert!(image, ImageLuma16, |p| [p.value()]),
    })
}

/// Encode with rav1e at its slowest, most thorough speed, in 10 bits, which compresses better
/// than 8 even for 8-bit sources. Fully transparent pixels have their colour cleared.
pub fn encode(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgba = image.to_rgba8();
    let pixels: Vec<RGBA8> = rgba
        .pixels()
        .map(|pixel| RGBA8::new(pixel[0], pixel[1], pixel[2], pixel[3]))
        .collect();
    Encoder::new()
        .with_quality(f32::from(quality))
        .with_alpha_quality(f32::from(quality))
        .with_speed(1)
        .with_bit_depth(BitDepth::Ten)
        .with_alpha_color_mode(AlphaColorMode::UnassociatedClean)
        .encode_rgba(Img::new(
            &pixels,
            rgba.width() as usize,
            rgba.height() as usize,
        ))
        .map(|encoded| encoded.avif_file)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use image::RgbaImage;

    use super::*;

    #[test]
    fn round_trip() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_fn(40, 24, |x, y| {
            image::Rgba([
                (x * 6) as u8,
                (y * 10) as u8,
                128,
                if x < 20 { 255 } else { 128 },
            ])
        }));
        let decoded = decode(&encode(&image, 80).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (40, 24));
        assert!(decoded.color().has_alpha());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"\0\0\0\x18ftypavifnope").is_err());
    }
}
