use std::{
    fs::{create_dir_all, read, write},
    io::{self, Cursor, Write},
    num::{NonZeroU32, NonZeroU64},
    path::Path,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageReader, Rgb, RgbImage,
    RgbaImage, codecs::png::PngEncoder, imageops::FilterType,
};
use resvg::{tiny_skia, usvg};

use heifer::{
    EncodeOptions,
    image_crate::{HeifDecoder, HeifEncoder},
};

use crate::{Format, avif, detect, jpeg, png, svg};

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
pub fn target_size(
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
    match format {
        Format::Heif => {
            from_decoder(HeifDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?)
        }
        Format::Avif => Ok(Raster {
            image: avif::decode(data)?,
            icc_profile: None,
        }),
        _ => from_decoder(
            ImageReader::new(Cursor::new(data))
                .with_guessed_format()
                .map_err(|e| e.to_string())?
                .into_decoder()
                .map_err(|e| e.to_string())?,
        ),
    }
}

fn rasterise(data: &[u8], path: &Path, settings: &Settings) -> Result<Raster, String> {
    let mut options = usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;

    let size = tree.size();
    let (width, height) = target_size(
        (f64::from(size.width()), f64::from(size.height())),
        settings.width,
        settings.height,
    );
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("output size is too large")?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "f32 holds every whole number up to 2^24, far beyond any renderable size"
    )]
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    let image = RgbaImage::from_raw(width, height, pixels).expect("pixmap matches its size");
    Ok(Raster {
        image: DynamicImage::ImageRgba8(image),
        icc_profile: None,
    })
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

/// Wrap a raster image in an SVG. JPEG sources without transparency stay JPEG inside, everything
/// else is embedded as PNG.
fn encode_svg(raster: &Raster, from: Format, settings: &Settings) -> Result<Vec<u8>, String> {
    let (mime, data) = if from == Format::Jpeg && !raster.image.color().has_alpha() {
        ("image/jpeg", encode_jpeg(raster, settings)?)
    } else {
        ("image/png", encode_png(raster, settings)?)
    };
    let (width, height) = raster.image.dimensions();
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><image width="{width}" height="{height}" href="data:{mime};base64,{}"/></svg>"#,
        STANDARD.encode(data)
    )
    .into_bytes())
}

fn encode(
    raster: &Raster,
    from: Format,
    to: Format,
    settings: &Settings,
) -> Result<Vec<u8>, String> {
    match to {
        Format::Png => encode_png(raster, settings),
        Format::Svg => encode_svg(raster, from, settings),
        Format::Jpeg => encode_jpeg(raster, settings),
        Format::Avif => avif::encode(&raster.image, settings.quality),
        Format::Heif => encode_heif(raster, settings),
    }
}

/// Convert `input` to the format given by `output`'s extension, resizing it and compressing the
/// result as far as the output format allows.
pub fn convert(input: &Path, output: &Path, settings: &Settings) -> Result<(), String> {
    let data = read(input).map_err(|e| e.to_string())?;
    let from = detect(input, &data).ok_or("not a supported image format")?;
    let to = Format::from_extension(output)
        .ok_or("the output path must end in .png, .jpg, .jpeg, .svg, .avif, .heic or .heif")?;

    let result = match (from, to) {
        (Format::Svg, Format::Svg) => {
            svg::optimise(&svg::resize(&data, settings.width, settings.height)?)?
        }
        (Format::Svg, _) => encode(&rasterise(&data, input, settings)?, from, to, settings)?,
        _ => encode(&resize(decode(&data, from)?, settings), from, to, settings)?,
    };

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
