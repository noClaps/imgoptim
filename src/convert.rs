use std::{
    fs::{create_dir_all, read, write},
    io::{self, Cursor, Write},
    num::{NonZeroU32, NonZeroU64},
    path::Path,
};

use heifer::{
    EncodeOptions,
    image_crate::{HeifDecoder, HeifEncoder},
};
use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageReader, Rgb, RgbImage,
    codecs::png::PngEncoder, imageops::FilterType, metadata::Orientation,
};

use crate::{ChromaSubsampling, Format, avif, detect, jpeg, png};

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

/// Decoded pixels along with the colour profile they're in and the chroma subsampling they were
/// stored with, if any.
struct Raster {
    image: DynamicImage,
    icc_profile: Option<Vec<u8>>,
    chroma_subsampling: Option<ChromaSubsampling>,
}

/// The chroma subsampling of a HEIF's HEVC stream, from the chroma format in its `hvcC` box.
fn hevc_chroma_subsampling(data: &[u8]) -> Option<ChromaSubsampling> {
    let config = data.windows(4).position(|w| w == b"hvcC")? + 4;
    match data.get(config + 16)? & 0b11 {
        1 => Some((2, 2)),
        2 => Some((2, 1)),
        3 => Some((1, 1)),
        _ => None,
    }
}

fn from_decoder(
    mut decoder: impl ImageDecoder,
    chroma_subsampling: Option<ChromaSubsampling>,
) -> Result<Raster, String> {
    let error = |e: image::ImageError| e.to_string();
    let orientation = decoder.orientation().map_err(error)?;
    let icc_profile = decoder.icc_profile().map_err(error)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    // Turning the image a quarter turn swaps its horizontal and vertical subsampling.
    let quarter_turn = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    Ok(Raster {
        image,
        icc_profile,
        chroma_subsampling: chroma_subsampling
            .map(|(h, v)| if quarter_turn { (v, h) } else { (h, v) }),
    })
}

fn decode(data: &[u8], format: Format) -> Result<Raster, String> {
    match format {
        Format::Heif => {
            let decoder = HeifDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
            let mut raster = from_decoder(decoder, hevc_chroma_subsampling(data))?;
            raster.icc_profile = raster.icc_profile.or_else(|| avif::colour_profile(data));
            Ok(raster)
        }
        Format::Avif => {
            let decoded = avif::decode(data)?;
            Ok(Raster {
                image: decoded.image,
                icc_profile: decoded.icc_profile,
                chroma_subsampling: decoded.chroma_subsampling,
            })
        }
        _ => {
            let chroma_subsampling = match format {
                Format::Jpeg => jpeg::chroma_subsampling(data),
                _ => None,
            };
            let decoder = ImageReader::new(Cursor::new(data))
                .with_guessed_format()
                .map_err(|e| e.to_string())?
                .into_decoder()
                .map_err(|e| e.to_string())?;
            from_decoder(decoder, chroma_subsampling)
        }
    }
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
        image: raster
            .image
            .resize_exact(width, height, FilterType::Lanczos3),
        ..raster
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

/// JPEG has no transparency, so translucent pixels are blended onto white. The source's chroma
/// subsampling is kept, or 4:4:4 used when it had none.
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
        raster.chroma_subsampling.unwrap_or((1, 1)),
    )
}

fn encode_heif(raster: &Raster, settings: &Settings) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let options = EncodeOptions {
        quality: settings.quality,
        ..EncodeOptions::default()
    };
    let mut encoder = HeifEncoder::with_options(&mut data, options);
    if let Some(icc_profile) = &raster.icc_profile {
        encoder
            .set_icc_profile(icc_profile.clone())
            .map_err(|e| e.to_string())?;
    }
    raster
        .image
        .write_with_encoder(encoder)
        .map_err(|e| e.to_string())?;
    Ok(data)
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
        Format::Heif => encode_heif(raster, settings),
    }
}

/// Convert `input` to the format given by `output`'s extension, resizing it and compressing the
/// result as far as the output format allows.
pub fn convert(input: &Path, output: &Path, settings: &Settings) -> Result<(), String> {
    let data = read(input).map_err(|e| e.to_string())?;
    let from = detect(input, &data).ok_or("not a supported image format")?;
    let to = Format::from_extension(output)
        .ok_or("the output path must end in .png, .jpg, .jpeg, .avif, .heic or .heif")?;
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
