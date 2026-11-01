use std::{borrow::Cow, io::Cursor};

use image::{DynamicImage, ImageDecoder};
use jxl_encoder::{
    ImageMetadata, LosslessConfig, LossyConfig, PixelLayout,
    jpeg::{encode_jpeg_to_jxl_container, read_jpeg},
    quality_to_distance,
};
use jxl_image_rs_integration::JxlDecoder;
use jxl_oxide::{JxlImage, image::BitDepth};

/// The encoder's two highest efforts, which add optimal LZ77 parsing and the most thorough
/// searches. Neither always wins, so both are tried.
const EFFORTS: [u8; 2] = [9, 10];

const CONTAINER_SIGNATURE: &[u8] = b"\0\0\0\x0cJXL \r\n\x87\n";

/// Metadata box types that don't affect how the image is displayed. Orientation and colour live
/// in the codestream, not in these.
const METADATA_BOXES: [&[u8; 4]; 3] = [b"Exif", b"xml ", b"jumb"];

/// A top-level box of the JPEG XL container, including its header.
struct JxlBox<'a> {
    kind: [u8; 4],
    raw: &'a [u8],
    payload: &'a [u8],
}

/// Split a JPEG XL container into its boxes, or `None` for a bare codestream.
fn boxes(data: &[u8]) -> Option<Vec<JxlBox<'_>>> {
    if !data.starts_with(CONTAINER_SIGNATURE) {
        return None;
    }
    let mut boxes = Vec::new();
    let mut position = 0;
    while position < data.len() {
        let size = u32::from_be_bytes(data.get(position..position + 4)?.try_into().ok()?);
        let kind = data.get(position + 4..position + 8)?.try_into().ok()?;
        let (size, header) = match size {
            0 => (data.len() - position, 8),
            1 => {
                let size =
                    u64::from_be_bytes(data.get(position + 8..position + 16)?.try_into().ok()?);
                (usize::try_from(size).ok()?, 16)
            }
            size => (size as usize, 8),
        };
        if size < header {
            return None;
        }
        let raw = data.get(position..position + size)?;
        boxes.push(JxlBox {
            kind,
            raw,
            payload: &raw[header..],
        });
        position += size;
    }
    Some(boxes)
}

fn is_metadata(jxl_box: &JxlBox) -> bool {
    let kind = match &jxl_box.kind {
        // Brotli-compressed boxes start with the type of the box they hold.
        b"brob" => jxl_box.payload.get(..4).unwrap_or_default(),
        kind => kind,
    };
    METADATA_BOXES
        .iter()
        .any(|metadata| metadata.as_slice() == kind)
}

/// Decode the first frame, with orientation applied, along with its ICC profile. This uses
/// jxl-rs, the decoder in Chrome and Firefox, since jxl-oxide can't read some valid lossless
/// files.
pub fn decode(data: &[u8]) -> Result<(DynamicImage, Option<Vec<u8>>), String> {
    let mut decoder = JxlDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let icc_profile = decoder.icc_profile().map_err(|e| e.to_string())?;
    let image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    Ok((image, icc_profile))
}

/// Encode at maximum effort, losslessly without a quality.
pub fn encode(
    image: &DynamicImage,
    icc_profile: Option<&[u8]>,
    quality: Option<u8>,
) -> Result<Vec<u8>, String> {
    let native = |samples: &[u16]| -> Cow<[u8]> {
        Cow::Owned(
            samples
                .iter()
                .flat_map(|sample| sample.to_ne_bytes())
                .collect(),
        )
    };
    let (layout, pixels): (PixelLayout, Cow<[u8]>) = match image {
        DynamicImage::ImageLuma8(image) => (PixelLayout::Gray8, Cow::Borrowed(image.as_raw())),
        DynamicImage::ImageLumaA8(image) => {
            (PixelLayout::GrayAlpha8, Cow::Borrowed(image.as_raw()))
        }
        DynamicImage::ImageRgb8(image) => (PixelLayout::Rgb8, Cow::Borrowed(image.as_raw())),
        DynamicImage::ImageRgba8(image) => (PixelLayout::Rgba8, Cow::Borrowed(image.as_raw())),
        DynamicImage::ImageLuma16(image) => (PixelLayout::Gray16, native(image.as_raw())),
        DynamicImage::ImageLumaA16(image) => (PixelLayout::GrayAlpha16, native(image.as_raw())),
        DynamicImage::ImageRgb16(image) => (PixelLayout::Rgb16, native(image.as_raw())),
        DynamicImage::ImageRgba16(image) => (PixelLayout::Rgba16, native(image.as_raw())),
        image => (PixelLayout::Rgba8, Cow::Owned(image.to_rgba8().into_raw())),
    };

    let metadata = match icc_profile {
        Some(icc_profile) => ImageMetadata::new().with_icc_profile(icc_profile),
        None => ImageMetadata::new(),
    };
    let (width, height) = (image.width(), image.height());
    let mut smallest: Option<Vec<u8>> = None;
    for effort in EFFORTS {
        let encoded = match quality {
            None => LosslessConfig::new()
                .with_effort(effort)
                .encode_request(width, height, layout)
                .with_metadata(&metadata)
                .encode(&pixels),
            Some(quality) => LossyConfig::new(quality_to_distance(f32::from(quality)).max(0.1))
                .with_effort(effort)
                .encode_request(width, height, layout)
                .with_metadata(&metadata)
                .encode(&pixels),
        }
        .map_err(|e| e.to_string())?;
        if smallest
            .as_ref()
            .is_none_or(|smallest| encoded.len() < smallest.len())
        {
            smallest = Some(encoded);
        }
    }
    Ok(smallest.expect("at least one effort"))
}

/// Losslessly recompress a JPEG's DCT coefficients into JPEG XL, keeping what's needed to rebuild
/// the original JPEG byte for byte. Progressive JPEGs aren't supported by the transcoder, so
/// they're first rewritten losslessly as baseline JPEGs; that's the JPEG they then rebuild into,
/// with identical pixels.
pub fn transcode_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    let transcode = |data: &[u8]| {
        let jpeg = read_jpeg(data).map_err(|e| e.to_string())?;
        encode_jpeg_to_jxl_container(&jpeg).map_err(|e| e.to_string())
    };
    transcode(data).or_else(|_| transcode(&crate::jpeg::baseline(data)?))
}

/// Strip metadata boxes, and re-encode losslessly stored images at maximum effort, keeping the
/// re-encode only if it's smaller and decodes to identical pixels. Files that can be turned back
/// into the original JPEG are left alone, since that needs their metadata and exact codestream.
pub fn optimise(data: &[u8]) -> Result<Vec<u8>, String> {
    let header = JxlImage::builder()
        .read(Cursor::new(data))
        .map_err(|e| e.to_string())?;
    let boxes = boxes(data);
    if boxes
        .iter()
        .flatten()
        .any(|jxl_box| &jxl_box.kind == b"jbrd")
    {
        return Ok(data.to_vec());
    }

    let mut best = match &boxes {
        Some(boxes) => boxes
            .iter()
            .filter(|jxl_box| !is_metadata(jxl_box))
            .flat_map(|jxl_box| jxl_box.raw)
            .copied()
            .collect(),
        None => data.to_vec(),
    };

    let metadata = &header.image_header().metadata;
    let stored_losslessly = !metadata.xyb_encoded
        && metadata.animation.is_none()
        && metadata.ec_info.iter().all(|channel| channel.is_alpha())
        && metadata.ec_info.len() <= 1
        && matches!(metadata.bit_depth, BitDepth::IntegerSample { bits_per_sample } if bits_per_sample <= 16);
    if stored_losslessly {
        let (image, icc_profile) = decode(data)?;
        let encoded = encode(&image, icc_profile.as_deref(), None)?;
        if encoded.len() < best.len()
            && decode(&encoded).is_ok_and(|(decoded, _)| {
                decoded.color() == image.color() && decoded.as_bytes() == image.as_bytes()
            })
        {
            best = encoded;
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use image::RgbImage;

    use super::*;

    fn gradient() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 7) as u8, (x ^ y) as u8])
        }))
    }

    #[test]
    fn lossless_round_trip() {
        let image = gradient();
        let encoded = encode(&image, None, None).unwrap();
        let (decoded, _) = decode(&encoded).unwrap();
        assert_eq!(decoded.to_rgb8().as_raw(), image.to_rgb8().as_raw());
    }

    /// Images over 256 pixels span several groups, which some decoders mishandle.
    #[test]
    fn large_lossless_round_trip() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(300, 280, |x, y| {
            image::Rgb([(x * 7 + y) as u8, (y * 3) as u8, ((x * y) >> 4) as u8])
        }));
        let (decoded, _) = decode(&encode(&image, None, None).unwrap()).unwrap();
        assert_eq!(decoded.to_rgb8().as_raw(), image.to_rgb8().as_raw());
    }

    #[test]
    fn lossy_encodes() {
        let encoded = encode(&gradient(), None, Some(80)).unwrap();
        let (decoded, _) = decode(&encoded).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (48, 32));
    }

    #[test]
    fn optimise_strips_metadata() {
        let encoded = encode(&gradient(), None, None).unwrap();
        let exif_box = [&[0, 0, 0, 24][..], b"Exif", b"camera serial", &[0, 0, 0]].concat();
        let original = if encoded.starts_with(CONTAINER_SIGNATURE) {
            [encoded.as_slice(), &exif_box].concat()
        } else {
            // Wrap the bare codestream in a container.
            let codestream = [
                &((encoded.len() + 8) as u32).to_be_bytes()[..],
                b"jxlc",
                &encoded,
            ]
            .concat();
            let ftyp = [&[0, 0, 0, 20][..], b"ftypjxl \0\0\0\0jxl "].concat();
            [CONTAINER_SIGNATURE, &ftyp, &exif_box, &codestream].concat()
        };
        let optimised = optimise(&original).unwrap();
        assert!(optimised.len() < original.len());
        assert!(!optimised.windows(13).any(|w| w == b"camera serial"));
        assert_eq!(
            decode(&optimised).unwrap().0.to_rgb8().as_raw(),
            gradient().to_rgb8().as_raw()
        );
    }

    /// Rebuild the JPEG stored in a JPEG XL file.
    fn reconstruct_jpeg(data: &[u8]) -> Vec<u8> {
        let image = JxlImage::builder().read(Cursor::new(data)).unwrap();
        let mut jpeg = Vec::new();
        image.reconstruct_jpeg(&mut jpeg).unwrap();
        jpeg
    }

    #[test]
    fn jpeg_transcode_is_reversible() {
        let rgb = gradient().to_rgb8();
        let original =
            crate::jpeg::encode(rgb.as_raw(), rgb.width(), rgb.height(), 90, None).unwrap();
        // mozjpeg writes progressive JPEGs, which are rebuilt as the equivalent baseline JPEG.
        let transcoded = transcode_jpeg(&original).unwrap();
        let baseline = crate::jpeg::baseline(&original).unwrap();
        assert_eq!(reconstruct_jpeg(&transcoded), baseline);
        let baseline_transcoded = transcode_jpeg(&baseline).unwrap();
        assert_eq!(reconstruct_jpeg(&baseline_transcoded), baseline);
        // Optimising must not break reconstruction.
        assert_eq!(optimise(&transcoded).unwrap(), transcoded);
    }

    #[test]
    fn rejects_garbage() {
        assert!(optimise(b"\xFF\x0Anope").is_err());
    }
}
