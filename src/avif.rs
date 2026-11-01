use avif_decode::{Decoder, Image};
use avif_parse::AvifData;
use image::{DynamicImage, ImageBuffer, RgbaImage};
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, Layout, MatrixCoefficients,
    TransferCharacteristics, TransformOptions,
};
use ravif::{AlphaColorMode, BitDepth, Encoder, Img, RGBA8};

use crate::ChromaSubsampling;

pub struct Decoded {
    pub image: DynamicImage,
    pub icc_profile: Option<Vec<u8>>,
    pub chroma_subsampling: Option<ChromaSubsampling>,
}

/// The colour space from the first `colr` box of an AVIF or HEIF file, as an ICC profile, or `None`
/// for sRGB. AVIFs usually describe their colour space with CICP codes rather than a profile, so
/// those are turned into an equivalent profile.
pub fn colour_profile(data: &[u8]) -> Option<Vec<u8>> {
    let start = data.windows(4).position(|w| w == b"colr")?.checked_sub(4)?;
    let size = u32::from_be_bytes(data.get(start..start + 4)?.try_into().ok()?);
    let payload = data.get(start + 8..start + usize::try_from(size).ok()?)?;
    match payload.get(..4)? {
        b"prof" | b"rICC" => Some(payload[4..].to_vec()),
        b"nclx" => {
            let code = |offset: usize| {
                let value = u16::from_be_bytes(payload.get(offset..offset + 2)?.try_into().ok()?);
                u8::try_from(value).ok()
            };
            let (primaries, transfer, matrix) = (code(4)?, code(6)?, code(8)?);
            // BT.709 primaries with the sRGB curve, or both unspecified, is plain sRGB.
            if matches!((primaries, transfer), (1, 13) | (2, 2)) {
                return None;
            }
            let cicp = CicpProfile {
                color_primaries: CicpColorPrimaries::try_from(primaries).ok()?,
                transfer_characteristics: TransferCharacteristics::try_from(transfer).ok()?,
                matrix_coefficients: MatrixCoefficients::try_from(matrix).ok()?,
                full_range: payload.get(10).is_some_and(|flags| flags & 0x80 != 0),
            };
            ColorProfile::new_from_cicp(cicp).encode().ok()
        }
        _ => None,
    }
}

pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    let image = Decoder::from_avif(data)
        .and_then(Decoder::to_image)
        .map_err(|e| e.to_string())?;

    macro_rules! convert {
        ($image:expr, $variant:ident, |$pixel:ident| $channels:expr) => {{
            let size = |pixels: usize| {
                u32::try_from(pixels).expect("AV1 frames are under 65536 pixels a side")
            };
            let (width, height) = (size($image.width()), size($image.height()));
            let samples = $image.pixels().flat_map(|$pixel| $channels).collect();
            DynamicImage::$variant(
                ImageBuffer::from_raw(width, height, samples).expect("buffer matches its size"),
            )
        }};
    }
    let image = match image {
        Image::Rgb8(image) => convert!(image, ImageRgb8, |p| [p.r, p.g, p.b]),
        Image::Rgb16(image) => convert!(image, ImageRgb16, |p| [p.r, p.g, p.b]),
        Image::Rgba8(image) => convert!(image, ImageRgba8, |p| [p.r, p.g, p.b, p.a]),
        Image::Rgba16(image) => convert!(image, ImageRgba16, |p| [p.r, p.g, p.b, p.a]),
        Image::Gray8(image) => convert!(image, ImageLuma8, |p| [p.value()]),
        Image::Gray16(image) => convert!(image, ImageLuma16, |p| [p.value()]),
    };

    let chroma_subsampling = AvifData::from_reader(&mut &data[..])
        .and_then(|avif| avif.primary_item_metadata())
        .ok()
        .filter(|metadata| !metadata.monochrome)
        .map(|metadata| {
            let (x, y) = metadata.chroma_subsampling;
            (1 + u8::from(x), 1 + u8::from(y))
        });
    Ok(Decoded {
        image,
        icc_profile: colour_profile(data),
        chroma_subsampling,
    })
}

/// Convert pixels in the colour space of `icc_profile` to sRGB.
fn to_srgb(rgba: &mut RgbaImage, icc_profile: &[u8]) -> Result<(), String> {
    let source = ColorProfile::new_from_slice(icc_profile).map_err(|e| e.to_string())?;
    let transform = source
        .create_transform_8bit(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgba,
            TransformOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut converted = vec![0; rgba.len()];
    transform
        .transform(rgba.as_raw(), &mut converted)
        .map_err(|e| e.to_string())?;
    rgba.copy_from_slice(&converted);
    Ok(())
}

/// Encode with rav1e at its slowest, most thorough speed, in 10 bits, which compresses better
/// than 8 even for 8-bit sources. Fully transparent pixels have their colour cleared. ravif always
/// stores 4:4:4 chroma and can only label images as sRGB, so other colour spaces are converted to
/// it.
pub fn encode(
    image: &DynamicImage,
    icc_profile: Option<&[u8]>,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let mut rgba = image.to_rgba8();
    if let Some(icc_profile) = icc_profile {
        to_srgb(&mut rgba, icc_profile)?;
    }
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
    use super::*;

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the channel values are all at most 255"
    )]
    fn round_trip() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_fn(40, 24, |x, y| {
            image::Rgba([
                (x * 6) as u8,
                (y * 10) as u8,
                128,
                if x < 20 { 255 } else { 128 },
            ])
        }));
        let decoded = decode(&encode(&image, None, 80).unwrap()).unwrap();
        assert_eq!((decoded.image.width(), decoded.image.height()), (40, 24));
        assert!(decoded.image.color().has_alpha());
        // ravif writes 4:4:4 and labels it sRGB.
        assert_eq!(decoded.chroma_subsampling, Some((1, 1)));
        assert_eq!(decoded.icc_profile, None);
    }

    #[test]
    fn converts_wide_gamut_to_srgb() {
        // A Display P3 orange is more saturated than the same numbers in sRGB, so its red rises
        // and its blue falls once converted.
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([200, 100, 50, 255]),
        ));
        let p3 = ColorProfile::new_display_p3().encode().unwrap();
        let decoded = decode(&encode(&image, Some(&p3), 100).unwrap()).unwrap();
        let [r, _, b, _] = decoded.image.to_rgba8().get_pixel(8, 8).0;
        assert!(r > 205 && b < 45, "{r} {b}");
    }

    #[test]
    fn reads_cicp_colour_spaces() {
        let nclx = |primaries: u8, transfer: u8| {
            [
                &[0, 0, 0, 19][..],
                b"colrnclx",
                &[0, primaries, 0, transfer, 0, 1, 0x80],
            ]
            .concat()
        };
        assert_eq!(colour_profile(&nclx(1, 13)), None);
        let p3 = ColorProfile::new_from_slice(&colour_profile(&nclx(12, 13)).unwrap()).unwrap();
        let reference = ColorProfile::new_display_p3();
        assert!((p3.red_colorant.x - reference.red_colorant.x).abs() < 0.001);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"\0\0\0\x18ftypavifnope").is_err());
    }
}
