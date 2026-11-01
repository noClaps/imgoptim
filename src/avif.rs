use avif_decode::{Decoder, Image};
use avif_serialize::{Aviffy, constants};
use image::{DynamicImage, ImageBuffer, Rgba};
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, Layout, MatrixCoefficients,
    TransferCharacteristics, TransformOptions,
};
use rav1e::prelude::{
    ChromaSampling, ColorDescription, ColorPrimaries, Config, Context, EncoderConfig,
    EncoderStatus, MatrixCoefficients as Rav1eMatrixCoefficients, PixelRange, SpeedSettings,
    TransferCharacteristics as Rav1eTransferCharacteristics,
};

pub struct Decoded {
    pub image: DynamicImage,
    pub icc_profile: Option<Vec<u8>>,
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

    Ok(Decoded {
        image,
        icc_profile: colour_profile(data),
    })
}

/// A colour space AVIF can describe with CICP codes, which is the only way it's widely supported.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ColourSpace {
    Srgb,
    DisplayP3,
}

impl ColourSpace {
    fn profile(self) -> ColorProfile {
        match self {
            Self::Srgb => ColorProfile::new_srgb(),
            Self::DisplayP3 => ColorProfile::new_display_p3(),
        }
    }

    fn rav1e_primaries(self) -> ColorPrimaries {
        match self {
            Self::Srgb => ColorPrimaries::BT709,
            Self::DisplayP3 => ColorPrimaries::SMPTE432,
        }
    }

    fn avif_primaries(self) -> constants::ColorPrimaries {
        match self {
            Self::Srgb => constants::ColorPrimaries::Bt709,
            Self::DisplayP3 => constants::ColorPrimaries::DisplayP3,
        }
    }
}

/// Whether converting from `source` to `target` leaves colours unchanged, i.e. they're the same
/// colour space even if their profiles differ byte for byte.
fn same_colour_space(source: &ColorProfile, target: &ColorProfile) -> bool {
    let Ok(transform) = source.create_transform_8bit(
        Layout::Rgb,
        target,
        Layout::Rgb,
        TransformOptions::default(),
    ) else {
        return false;
    };
    let samples: Vec<u8> = (0..=255u8)
        .step_by(51)
        .flat_map(|r| {
            (0..=255u8)
                .step_by(51)
                .flat_map(move |g| (0..=255u8).step_by(51).flat_map(move |b| [r, g, b]))
        })
        .collect();
    let mut converted = vec![0; samples.len()];
    transform.transform(&samples, &mut converted).is_ok()
        && samples
            .iter()
            .zip(&converted)
            .all(|(a, b)| a.abs_diff(*b) <= 1)
}

/// Keep the source's colour space when AVIF can describe it, otherwise convert to Display P3.
fn colour_space(
    rgba: &mut ImageBuffer<Rgba<u16>, Vec<u16>>,
    icc_profile: Option<&[u8]>,
) -> Result<ColourSpace, String> {
    let Some(icc_profile) = icc_profile else {
        return Ok(ColourSpace::Srgb);
    };
    let source = ColorProfile::new_from_slice(icc_profile).map_err(|e| e.to_string())?;
    for space in [ColourSpace::Srgb, ColourSpace::DisplayP3] {
        if same_colour_space(&source, &space.profile()) {
            return Ok(space);
        }
    }
    let transform = source
        .create_transform_16bit(
            Layout::Rgba,
            &ColourSpace::DisplayP3.profile(),
            Layout::Rgba,
            TransformOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut converted = vec![0; rgba.len()];
    transform
        .transform(rgba.as_raw(), &mut converted)
        .map_err(|e| e.to_string())?;
    rgba.copy_from_slice(&converted);
    Ok(ColourSpace::DisplayP3)
}

/// rav1e's slowest, most thorough speed preset.
const SPEED: u8 = 0;
const BIT_DEPTH: usize = 10;
const MAX_SAMPLE: f32 = 1023.0;
/// The chroma value for no colour, half way through the 10-bit range.
const NEUTRAL_CHROMA: f32 = 512.0;

/// ravif's mapping from a 1–100 quality to a quantiser, where 0 is lossless.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the result is between 0 and 255"
)]
fn quantizer(quality: u8) -> usize {
    let q = f32::from(quality) / 100.0;
    let x = if q >= 0.82 {
        (1.0 - q) * 2.6
    } else if q > 0.25 {
        q.mul_add(-0.5, 1.0 - 0.125)
    } else {
        1.0 - q
    };
    (x * 255.0).round() as usize
}

/// Encode planes of 10-bit samples as a single AV1 key frame.
fn encode_av1(
    width: usize,
    height: usize,
    planes: &[Vec<u16>],
    chroma_sampling: ChromaSampling,
    color_description: Option<ColorDescription>,
    quantizer: usize,
) -> Result<Vec<u8>, String> {
    let config = Config::new().with_encoder_config(EncoderConfig {
        width,
        height,
        bit_depth: BIT_DEPTH,
        chroma_sampling,
        pixel_range: PixelRange::Full,
        color_description,
        still_picture: true,
        quantizer,
        min_quantizer: u8::try_from(quantizer).expect("quantisers are at most 255"),
        speed_settings: SpeedSettings::from_preset(SPEED),
        ..EncoderConfig::default()
    });
    let mut context: Context<u16> = config.new_context().map_err(|e| e.to_string())?;
    let mut frame = context.new_frame();
    for (plane, samples) in frame.planes.iter_mut().zip(planes) {
        #[expect(
            clippy::default_trait_access,
            reason = "rav1e doesn't export the plane offset type"
        )]
        let mut slice = plane.mut_slice(Default::default());
        for (row, samples) in slice
            .rows_iter_mut()
            .zip(samples.chunks_exact(width))
            .take(height)
        {
            row[..width].copy_from_slice(samples);
        }
    }
    context.send_frame(frame).map_err(|e| e.to_string())?;
    context.flush();

    let mut data = Vec::new();
    loop {
        match context.receive_packet() {
            Ok(mut packet) => data.append(&mut packet.data),
            Err(EncoderStatus::Encoded) => {}
            Err(EncoderStatus::LimitReached) => return Ok(data),
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Encode with rav1e at its slowest speed, as 10-bit 4:4:4 BT.601 YCbCr, which keeps the most
/// colour detail. Transparency is stored losslessly. The colour space is kept when AVIF can
/// describe it, and converted to Display P3 otherwise.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "samples are clamped to the 10-bit range before conversion"
)]
pub fn encode(
    image: &DynamicImage,
    icc_profile: Option<&[u8]>,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let mut rgba = image.to_rgba16();
    let colour_space = colour_space(&mut rgba, icc_profile)?;
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);

    let sample = |value: f32| value.round().clamp(0.0, MAX_SAMPLE) as u16;
    let ten_bit = |value: u16| f32::from(value) * MAX_SAMPLE / f32::from(u16::MAX);
    let pixels = width * height;
    let (mut luma, mut blue_chroma, mut red_chroma, mut alpha) = (
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
    );
    for pixel in rgba.pixels() {
        let [red, green, blue, opacity] = pixel.0.map(ten_bit);
        let y = 0.299f32.mul_add(red, 0.587f32.mul_add(green, 0.114 * blue));
        luma.push(sample(y));
        blue_chroma.push(sample((blue - y) / 1.772 + NEUTRAL_CHROMA));
        red_chroma.push(sample((red - y) / 1.402 + NEUTRAL_CHROMA));
        alpha.push(sample(opacity));
    }
    let planes = [luma, blue_chroma, red_chroma];

    let colour = encode_av1(
        width,
        height,
        &planes,
        ChromaSampling::Cs444,
        Some(ColorDescription {
            color_primaries: colour_space.rav1e_primaries(),
            transfer_characteristics: Rav1eTransferCharacteristics::SRGB,
            matrix_coefficients: Rav1eMatrixCoefficients::BT601,
        }),
        quantizer(quality),
    )?;
    let alpha = if alpha.iter().all(|&a| a == sample(MAX_SAMPLE)) {
        None
    } else {
        Some(encode_av1(
            width,
            height,
            &[alpha],
            ChromaSampling::Cs400,
            None,
            0,
        )?)
    };

    let size =
        |pixels: usize| u32::try_from(pixels).expect("AV1 frames are under 65536 pixels a side");
    Ok(Aviffy::new()
        .set_color_primaries(colour_space.avif_primaries())
        .set_transfer_characteristics(constants::TransferCharacteristics::Srgb)
        .set_matrix_coefficients(constants::MatrixCoefficients::Bt601)
        .set_full_color_range(true)
        .to_vec(&colour, alpha.as_deref(), size(width), size(height), 10))
}

#[cfg(test)]
mod tests {
    use image::RgbaImage;

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
        // Untagged sources are stored as sRGB.
        assert_eq!(decoded.icc_profile, None);
    }

    fn orange() -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([200, 100, 50, 255]),
        ))
    }

    #[test]
    fn keeps_display_p3() {
        let p3 = ColorProfile::new_display_p3().encode().unwrap();
        let decoded = decode(&encode(&orange(), Some(&p3), 100).unwrap()).unwrap();
        let profile = ColorProfile::new_from_slice(&decoded.icc_profile.unwrap()).unwrap();
        assert!(same_colour_space(&profile, &ColorProfile::new_display_p3()));
        let [r, g, b, _] = decoded.image.to_rgba8().get_pixel(8, 8).0;
        assert!(r.abs_diff(200) <= 2 && g.abs_diff(100) <= 2 && b.abs_diff(50) <= 2);
    }

    #[test]
    fn converts_other_colour_spaces_to_display_p3() {
        // BT.2020's gamut is wider than Display P3's, so the same numbers become a more
        // saturated Display P3 colour.
        let bt2020 = ColorProfile::new_bt2020().encode().unwrap();
        let decoded = decode(&encode(&orange(), Some(&bt2020), 100).unwrap()).unwrap();
        let profile = ColorProfile::new_from_slice(&decoded.icc_profile.unwrap()).unwrap();
        assert!(same_colour_space(&profile, &ColorProfile::new_display_p3()));
        let [r, ..] = decoded.image.to_rgba8().get_pixel(8, 8).0;
        assert!(r > 215, "{r}");
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
