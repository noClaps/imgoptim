use std::{ffi::c_void, mem, slice};

use libwebp_sys::*;

/// VP8X flag bits, as laid out in the first byte of its payload.
const ICC_FLAG: u8 = 0x20;
const ALPHA_FLAG: u8 = 0x10;
const EXIF_FLAG: u8 = 0x08;
const XMP_FLAG: u8 = 0x04;

struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

/// Split a WebP file into its RIFF chunks.
fn chunks(data: &[u8]) -> Option<Vec<Chunk<'_>>> {
    if data.get(..4)? != b"RIFF" || data.get(8..12)? != b"WEBP" {
        return None;
    }
    let riff_size = u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    let end = (8 + riff_size).min(data.len());

    let mut chunks = Vec::new();
    let mut position = 12;
    while position + 8 <= end {
        let id = data[position..position + 4].try_into().ok()?;
        let size = u32::from_le_bytes(data[position + 4..position + 8].try_into().ok()?) as usize;
        let data = data.get(position + 8..position + 8 + size)?;
        chunks.push(Chunk { id, data });
        position += 8 + size + (size & 1);
    }
    Some(chunks)
}

fn assemble(chunks: &[Chunk]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for chunk in chunks {
        body.extend_from_slice(&chunk.id);
        body.extend_from_slice(&(chunk.data.len() as u32).to_le_bytes());
        body.extend_from_slice(chunk.data);
        if chunk.data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend(body);
    file
}

/// Drop the EXIF and XMP chunks, which don't affect how the image is displayed. The extended
/// header is dropped too when nothing needs it any more.
fn strip_metadata(chunks: &[Chunk]) -> Vec<u8> {
    let mut vp8x = None;
    let mut kept: Vec<Chunk> = chunks
        .iter()
        .filter(|chunk| !matches!(&chunk.id, b"EXIF" | b"XMP "))
        .map(|chunk| Chunk {
            id: chunk.id,
            data: chunk.data,
        })
        .collect();
    if let Some(header) = kept.iter_mut().find(|chunk| &chunk.id == b"VP8X") {
        let mut data = header.data.to_vec();
        if let Some(flags) = data.first_mut() {
            *flags &= !(EXIF_FLAG | XMP_FLAG);
        }
        vp8x = Some(data);
    }
    match &vp8x {
        // A lone VP8 or VP8L bitstream doesn't need the extended header.
        Some(data) if data.first() == Some(&0) && kept.len() == 2 => {
            kept.retain(|chunk| &chunk.id != b"VP8X");
            assemble(&kept)
        }
        Some(data) => {
            for chunk in &mut kept {
                if &chunk.id == b"VP8X" {
                    chunk.data = data;
                }
            }
            assemble(&kept)
        }
        None => assemble(&kept),
    }
}

/// Add an ICC profile to a freshly encoded WebP file.
fn with_icc_profile(
    encoded: &[u8],
    icc_profile: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
) -> Vec<u8> {
    let chunks = chunks(encoded).expect("libwebp writes valid files");
    let mut vp8x = match chunks.iter().find(|chunk| &chunk.id == b"VP8X") {
        Some(header) => header.data.to_vec(),
        None => {
            let mut header = vec![if has_alpha { ALPHA_FLAG } else { 0 }, 0, 0, 0];
            header.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
            header.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
            header
        }
    };
    vp8x[0] |= ICC_FLAG;

    let mut assembled = vec![
        Chunk {
            id: *b"VP8X",
            data: &vp8x,
        },
        Chunk {
            id: *b"ICCP",
            data: icc_profile,
        },
    ];
    assembled.extend(chunks.into_iter().filter(|chunk| &chunk.id != b"VP8X"));
    assemble(&assembled)
}

/// Decode the first frame of a WebP to 8-bit RGBA. Animations aren't supported.
pub fn decode(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let (mut width, mut height) = (0, 0);
        let pixels = WebPDecodeRGBA(data.as_ptr(), data.len(), &mut width, &mut height);
        if pixels.is_null() {
            return None;
        }
        let rgba = slice::from_raw_parts(pixels, width as usize * height as usize * 4).to_vec();
        WebPFree(pixels.cast());
        Some((rgba, width as u32, height as u32))
    }
}

/// Encode 8-bit RGBA with libwebp at its slowest, smallest settings. Without a quality the image
/// is stored losslessly, though the colour of fully transparent pixels may change.
pub fn encode(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: Option<u8>,
    icc_profile: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let encoded = unsafe {
        let mut config = WebPConfig::new().map_err(|()| "libwebp version mismatch")?;
        match quality {
            None => {
                // Equivalent to `cwebp -lossless -z 9`.
                WebPConfigLosslessPreset(&mut config, 9);
                config.exact = 0;
            }
            Some(quality) => {
                config.quality = f32::from(quality);
                config.method = 6;
                config.pass = 10;
                config.segments = 4;
                config.autofilter = 1;
                config.use_sharp_yuv = 1;
                config.alpha_quality = 100;
                config.alpha_filtering = 2;
            }
        }
        config.thread_level = 1;

        let mut picture = WebPPicture::new().map_err(|()| "libwebp version mismatch")?;
        picture.use_argb = 1;
        picture.width = width as i32;
        picture.height = height as i32;
        if WebPPictureImportRGBA(&mut picture, rgba.as_ptr(), width as i32 * 4) == 0 {
            WebPPictureFree(&mut picture);
            return Err("out of memory".to_owned());
        }

        let mut writer: WebPMemoryWriter = mem::zeroed();
        WebPMemoryWriterInit(&mut writer);
        picture.writer = Some(WebPMemoryWrite);
        picture.custom_ptr = (&raw mut writer).cast::<c_void>();
        let succeeded = WebPEncode(&config, &mut picture) != 0;
        let error = picture.error_code;
        WebPPictureFree(&mut picture);

        let encoded = slice::from_raw_parts(writer.mem, writer.size).to_vec();
        WebPMemoryWriterClear(&mut writer);
        if !succeeded {
            return Err(format!("WebP encoding failed: {error:?}"));
        }
        encoded
    };

    Ok(match icc_profile {
        Some(icc_profile) => {
            let has_alpha = rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 255);
            with_icc_profile(&encoded, icc_profile, width, height, has_alpha)
        }
        None => encoded,
    })
}

/// Whether two RGBA buffers look the same: identical, except for the colour of fully transparent
/// pixels.
fn looks_identical(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .all(|(a, b)| a[3] == b[3] && (a[3] == 0 || a == b))
}

/// Strip metadata that doesn't affect display, and re-encode lossless WebPs at maximum effort,
/// keeping whichever is smallest. Lossy and animated WebPs only have their metadata stripped.
pub fn optimise(data: &[u8]) -> Result<Vec<u8>, String> {
    let chunks = chunks(data)
        .filter(|chunks| {
            chunks
                .iter()
                .any(|chunk| matches!(&chunk.id, b"VP8 " | b"VP8L" | b"ANIM"))
        })
        .ok_or("not a valid WebP")?;
    let mut best = strip_metadata(&chunks);

    let lossless = chunks.iter().any(|chunk| &chunk.id == b"VP8L");
    let animated = chunks.iter().any(|chunk| &chunk.id == b"ANIM");
    if lossless && !animated {
        let (rgba, width, height) = decode(data).ok_or("WebP could not be decoded")?;
        let icc_profile = chunks
            .iter()
            .find(|chunk| &chunk.id == b"ICCP")
            .map(|chunk| chunk.data);
        let encoded = encode(&rgba, width, height, None, icc_profile)?;
        if encoded.len() < best.len()
            && decode(&encoded).is_some_and(|(decoded, ..)| looks_identical(&rgba, &decoded))
        {
            best = encoded;
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| [x as u8, y as u8, (x ^ y) as u8, (x + y) as u8])
            })
            .collect()
    }

    #[test]
    fn lossless_round_trip_keeps_pixels_and_profile() {
        let rgba = gradient(64, 48);
        let icc = b"fake profile";
        let encoded = encode(&rgba, 64, 48, None, Some(icc)).unwrap();
        let (decoded, width, height) = decode(&encoded).unwrap();
        assert_eq!((width, height), (64, 48));
        assert!(looks_identical(&rgba, &decoded));

        let chunks = chunks(&encoded).unwrap();
        let ids: Vec<_> = chunks.iter().map(|chunk| chunk.id).collect();
        assert_eq!(ids, [*b"VP8X", *b"ICCP", *b"VP8L"]);
        assert_eq!(chunks[1].data, icc);
        assert_eq!(chunks[0].data[0], ICC_FLAG | ALPHA_FLAG);
    }

    #[test]
    fn optimise_strips_metadata_and_keeps_pixels() {
        let rgba = gradient(32, 32);
        let mut plain = encode(&rgba, 32, 32, None, None).unwrap();
        // Rebuild with a fast, poorly compressed bitstream plus EXIF and XMP chunks.
        let original = {
            let chunks = chunks(&plain).unwrap();
            let vp8x = [
                EXIF_FLAG | XMP_FLAG | ALPHA_FLAG,
                0,
                0,
                0,
                31,
                0,
                0,
                31,
                0,
                0,
            ];
            let mut all = vec![Chunk {
                id: *b"VP8X",
                data: &vp8x,
            }];
            all.extend(chunks);
            all.push(Chunk {
                id: *b"EXIF",
                data: b"camera serial",
            });
            all.push(Chunk {
                id: *b"XMP ",
                data: b"<x:xmpmeta/>",
            });
            assemble(&all)
        };
        plain = optimise(&original).unwrap();
        assert!(plain.len() < original.len());
        assert!(!plain.windows(13).any(|w| w == b"camera serial"));
        assert!(looks_identical(&rgba, &decode(&plain).unwrap().0));
    }

    #[test]
    fn lossy_encodes() {
        let rgba = gradient(40, 30);
        let encoded = encode(&rgba, 40, 30, Some(80), None).unwrap();
        assert_eq!(decode(&encoded).unwrap().1, 40);
    }

    #[test]
    fn rejects_garbage() {
        assert!(optimise(b"RIFF\0\0\0\0WEBPnope").is_err());
        assert!(optimise(b"not a webp").is_err());
    }
}
