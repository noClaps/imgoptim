use std::{
    fs::{create_dir_all, read, write},
    io::{self, Cursor, Write},
    num::{NonZeroU32, NonZeroU64},
    path::Path,
};

use image::{
    ColorType, DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageReader, Rgb,
    RgbImage, codecs::png::PngEncoder, imageops, imageops::FilterType,
};

use crate::{Format, avif, detect, jpeg, png};

const SVG_UNSUPPORTED: &str = "SVGs can only be optimised, not converted";

pub struct Settings {
    pub width: Option<NonZeroU32>,
    pub height: Option<NonZeroU32>,
    pub quality: u8,
    pub zopfli_iterations: NonZeroU64,
}

/// The output size for an image of `original` size. A missing dimension is scaled to keep the
/// original aspect ratio, and with neither given the original size is kept.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, which is the right behaviour for a pixel size"
)]
fn target_size(
    (original_width, original_height): (f64, f64),
    width: Option<NonZeroU32>,
    height: Option<NonZeroU32>,
) -> (u32, u32) {
    let pixels = |value: f64| (value.round() as u32).max(1);
    match (width, height) {
        (Some(width), Some(height)) => (width.get(), height.get()),
        (Some(width), None) => (
            width.get(),
            pixels(original_height * f64::from(width.get()) / original_width),
        ),
        (None, Some(height)) => (
            pixels(original_width * f64::from(height.get()) / original_height),
            height.get(),
        ),
        (None, None) => (pixels(original_width), pixels(original_height)),
    }
}

/// Decoded pixels along with the colour profile they're in.
struct Raster {
    image: DynamicImage,
    icc_profile: Option<Vec<u8>>,
}

fn from_decoder(mut decoder: impl ImageDecoder) -> Result<Raster, String> {
    let error = |e: image::ImageError| e.to_string();
    let orientation = decoder.orientation().map_err(error)?;
    let icc_profile = decoder.icc_profile().map_err(error)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    Ok(Raster { image, icc_profile })
}

fn decode(data: &[u8], format: Format) -> Result<Raster, String> {
    if format == Format::Avif {
        let decoded = avif::decode(data)?;
        return Ok(Raster {
            image: decoded.image,
            icc_profile: decoded.icc_profile,
        });
    }
    let decoder = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    from_decoder(decoder)
}

fn resize(raster: Raster, settings: &Settings) -> Raster {
    let (width, height) = target_size(
        (
            f64::from(raster.image.width()),
            f64::from(raster.image.height()),
        ),
        settings.width,
        settings.height,
    );
    if (width, height) == raster.image.dimensions() {
        return raster;
    }
    Raster {
        image: resample(&raster.image, width, height),
        ..raster
    }
}

/// The sRGB transfer curve, which Display P3 shares.
fn to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055f32.mul_add(value.powf(1.0 / 2.4), -0.055)
    }
}

/// Resample with Lanczos3 in linear light with premultiplied alpha, so fine detail keeps its
/// brightness and transparent pixels don't bleed colour into their neighbours. The result has the
/// source's bit depth and channels.
fn resample(image: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let mut linear = image.to_rgba32f();
    for pixel in linear.pixels_mut() {
        let alpha = pixel[3];
        for channel in &mut pixel.0[..3] {
            *channel = to_linear(*channel) * alpha;
        }
    }
    let mut resized = imageops::resize(&linear, width, height, FilterType::Lanczos3);
    for pixel in resized.pixels_mut() {
        // Lanczos overshoots slightly at sharp edges.
        let alpha = pixel[3].clamp(0.0, 1.0);
        for channel in &mut pixel.0[..3] {
            *channel = if alpha > 0.0 {
                from_linear((*channel / alpha).clamp(0.0, 1.0))
            } else {
                0.0
            };
        }
        pixel[3] = alpha;
    }

    let resized = DynamicImage::ImageRgba32F(resized);
    match image.color() {
        ColorType::L8 => resized.to_luma8().into(),
        ColorType::La8 => resized.to_luma_alpha8().into(),
        ColorType::Rgb8 => resized.to_rgb8().into(),
        ColorType::Rgba8 => resized.to_rgba8().into(),
        ColorType::L16 => resized.to_luma16().into(),
        ColorType::La16 => resized.to_luma_alpha16().into(),
        ColorType::Rgb16 => resized.to_rgb16().into(),
        _ => resized.to_rgba16().into(),
    }
}

fn encode_png(raster: &Raster, settings: &Settings) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let mut encoder = PngEncoder::new(&mut data);
    if let Some(icc_profile) = &raster.icc_profile {
        encoder
            .set_icc_profile(icc_profile.clone())
            .map_err(|e| e.to_string())?;
    }
    raster
        .image
        .write_with_encoder(encoder)
        .map_err(|e| e.to_string())?;
    png::optimise(&data, settings.zopfli_iterations)
}

/// JPEG has no transparency, so translucent pixels are blended onto white.
fn encode_jpeg(raster: &Raster, settings: &Settings) -> Result<Vec<u8>, String> {
    let rgba = raster.image.to_rgba8();
    let rgb = RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend = |c: u8| {
            let blended = (u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255;
            u8::try_from(blended).expect("a blend of two bytes fits in a byte")
        };
        Rgb([blend(r), blend(g), blend(b)])
    });
    jpeg::encode(
        &rgb,
        rgb.width(),
        rgb.height(),
        settings.quality,
        raster.icc_profile.as_deref(),
    )
}

fn encode(raster: &Raster, to: Format, settings: &Settings) -> Result<Vec<u8>, String> {
    match to {
        Format::Png => encode_png(raster, settings),
        Format::Svg => Err(SVG_UNSUPPORTED.to_owned()),
        Format::Jpeg => encode_jpeg(raster, settings),
        Format::Avif => avif::encode(
            &raster.image,
            raster.icc_profile.as_deref(),
            settings.quality,
        ),
    }
}

/// Convert `input` to the format given by `output`'s extension, resizing it and compressing the
/// result as far as the output format allows.
pub fn convert(input: &Path, output: &Path, settings: &Settings) -> Result<(), String> {
    let data = read(input).map_err(|e| e.to_string())?;
    let from = detect(input, &data).ok_or("not a supported image format")?;
    let to = Format::from_extension(output)
        .ok_or("the output path must end in .png, .jpg, .jpeg or .avif")?;
    if from == Format::Svg {
        return Err(SVG_UNSUPPORTED.to_owned());
    }
    let result = encode(&resize(decode(&data, from)?, settings), to, settings)?;

    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write(output, &result).map_err(|e| e.to_string())?;
    let _ = writeln!(
        io::stdout(),
        "{} → {}: {} bytes",
        input.display(),
        output.display(),
        result.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_aspect_ratio() {
        let size = |width, height| {
            target_size(
                (400.0, 300.0),
                NonZeroU32::new(width),
                NonZeroU32::new(height),
            )
        };
        assert_eq!(size(0, 0), (400, 300));
        assert_eq!(size(200, 0), (200, 150));
        assert_eq!(size(0, 600), (800, 600));
        assert_eq!(size(10, 10), (10, 10));
        assert_eq!(size(1, 0), (1, 1));
    }
}
