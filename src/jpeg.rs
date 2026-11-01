use std::{
    ffi::{CStr, c_int, c_ulong},
    mem,
    panic::{self, AssertUnwindSafe},
    ptr, slice,
};

use mozjpeg_sys::*;

const APP1: c_int = 0xE1;
const APP2: c_int = 0xE2;
const EXIF_SIGNATURE: &[u8] = b"Exif\0\0";
const ICC_SIGNATURE: &[u8] = b"ICC_PROFILE\0";
const ORIENTATION_TAG: u16 = 0x0112;

unsafe extern "C-unwind" fn error_exit(cinfo: &mut jpeg_common_struct) {
    let buffer = [0u8; 80];
    unsafe {
        if let Some(format_message) = (*cinfo.err).format_message {
            format_message(cinfo, &buffer);
        }
    }
    let message = CStr::from_bytes_until_nul(&buffer)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown JPEG error".to_owned());
    panic::resume_unwind(Box::new(message));
}

unsafe extern "C-unwind" fn silence(_: &mut jpeg_common_struct) {}

/// An error manager that unwinds instead of exiting the process, for use with [`catch`].
unsafe fn error_manager() -> Box<jpeg_error_mgr> {
    unsafe {
        let mut err: Box<jpeg_error_mgr> = Box::new(mem::zeroed());
        jpeg_std_error(&mut err);
        err.error_exit = Some(error_exit);
        err.output_message = Some(silence);
        err
    }
}

/// Run libjpeg calls, turning errors raised through [`error_manager`] into `Err`.
fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        e.downcast::<String>()
            .map(|message| *message)
            .unwrap_or_else(|_| "unknown JPEG error".to_owned())
    })
}

/// Copy out and free a buffer filled by `jpeg_mem_dest`.
unsafe fn take_output(out: &mut *mut u8, size: c_ulong) -> Vec<u8> {
    unsafe {
        let result = slice::from_raw_parts(*out, size as usize).to_vec();
        libc::free(out.cast());
        *out = ptr::null_mut();
        result
    }
}

/// Read the EXIF orientation (1–8) out of an APP1 payload, if there is one.
fn exif_orientation(exif: &[u8]) -> Option<u16> {
    let tiff = exif.strip_prefix(EXIF_SIGNATURE)?;
    let big_endian = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |offset: usize| {
        let bytes = tiff.get(offset..offset + 2)?.try_into().ok()?;
        Some(if big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    };
    let u32_at = |offset: usize| {
        let bytes = tiff.get(offset..offset + 4)?.try_into().ok()?;
        Some(if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    };

    let ifd = u32_at(4)? as usize;
    (0..u16_at(ifd)? as usize)
        .map(|i| ifd + 2 + i * 12)
        .find(|&entry| u16_at(entry) == Some(ORIENTATION_TAG))
        .and_then(|entry| u16_at(entry + 8))
        .filter(|orientation| (1..=8).contains(orientation))
}

/// A big-endian EXIF payload holding nothing but an orientation tag.
fn minimal_exif(orientation: u16) -> Vec<u8> {
    let mut exif = EXIF_SIGNATURE.to_vec();
    exif.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
    exif.extend_from_slice(&1u16.to_be_bytes());
    exif.extend_from_slice(&ORIENTATION_TAG.to_be_bytes());
    exif.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    exif.extend_from_slice(&1u32.to_be_bytes());
    exif.extend_from_slice(&orientation.to_be_bytes());
    exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // Value padding, then no next IFD
    exif
}

/// Keep only the markers that change how the image is displayed: the ICC profile, and the EXIF
/// orientation when it isn't the default.
unsafe fn display_markers(src: &jpeg_decompress_struct) -> Vec<(c_int, Vec<u8>)> {
    let mut markers = Vec::new();
    let mut marker = src.marker_list;
    while let Some(m) = unsafe { marker.as_ref() } {
        let data = unsafe { slice::from_raw_parts(m.data, m.data_length as usize) };
        match c_int::from(m.marker) {
            APP2 if data.starts_with(ICC_SIGNATURE) => markers.push((APP2, data.to_vec())),
            APP1 => {
                if let Some(orientation) = exif_orientation(data).filter(|&o| o != 1) {
                    markers.push((APP1, minimal_exif(orientation)));
                }
            }
            _ => {}
        }
        marker = m.next;
    }
    // The EXIF marker conventionally comes first.
    markers.sort_by_key(|&(code, _)| code);
    markers
}

unsafe fn transcode(
    src: &jpeg_decompress_struct,
    dst: &mut jpeg_compress_struct,
    coefficients: *mut *mut jvirt_barray_control,
    markers: &[(c_int, Vec<u8>)],
    progressive: bool,
    out: &mut *mut u8,
) -> Vec<u8> {
    unsafe {
        jpeg_copy_critical_parameters(src, dst);
        dst.optimize_coding = 1;
        if progressive {
            jpeg_c_set_bool_param(dst, J_BOOLEAN_PARAM::JBOOLEAN_OPTIMIZE_SCANS, 1);
            jpeg_simple_progression(dst);
        } else {
            jpeg_c_set_bool_param(dst, J_BOOLEAN_PARAM::JBOOLEAN_OPTIMIZE_SCANS, 0);
            dst.num_scans = 0;
            dst.scan_info = ptr::null();
        }

        let mut size: c_ulong = 0;
        jpeg_mem_dest(dst, out, &mut size);
        jpeg_write_coefficients(dst, coefficients);
        for (code, data) in markers {
            jpeg_write_marker(dst, *code, data.as_ptr(), data.len() as _);
        }
        jpeg_finish_compress(dst);
        take_output(out, size)
    }
}

/// Losslessly re-encode a JPEG's DCT coefficients with optimised Huffman tables, trying both
/// baseline and progressive scans and keeping whichever is smaller. Equivalent to mozjpeg's
/// `jpegtran -copy none -optimize`, keeping the colour profile and orientation.
pub fn optimise(data: &[u8]) -> Result<Vec<u8>, String> {
    recompress(data, &[true, false])
}

/// Losslessly re-encode a JPEG with baseline (sequential) scans, as `optimise` does.
pub fn baseline(data: &[u8]) -> Result<Vec<u8>, String> {
    recompress(data, &[false])
}

/// Re-encode the DCT coefficients once per scan mode (`true` for progressive), returning the
/// smallest result.
fn recompress(data: &[u8], progressive_modes: &[bool]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut err = error_manager();
        let mut src: Box<jpeg_decompress_struct> = Box::new(mem::zeroed());
        let mut dst: Box<jpeg_compress_struct> = Box::new(mem::zeroed());
        src.common.err = &mut *err;
        dst.common.err = &mut *err;
        let mut out: *mut u8 = ptr::null_mut();

        let result = catch(|| {
            jpeg_create_decompress(&mut *src);
            jpeg_create_compress(&mut *dst);

            jpeg_mem_src(&mut src, data.as_ptr(), data.len() as _);
            jpeg_save_markers(&mut src, APP1, 0xFFFF);
            jpeg_save_markers(&mut src, APP2, 0xFFFF);
            jpeg_read_header(&mut src, 1);
            let coefficients = jpeg_read_coefficients(&mut src);
            let markers = display_markers(&src);

            let smallest = progressive_modes
                .iter()
                .map(|&progressive| {
                    transcode(
                        &src,
                        &mut dst,
                        coefficients,
                        &markers,
                        progressive,
                        &mut out,
                    )
                })
                .min_by_key(Vec::len)
                .expect("at least one scan mode");
            jpeg_finish_decompress(&mut src);
            smallest
        });

        if !out.is_null() {
            libc::free(out.cast());
        }
        jpeg_destroy_compress(&mut dst);
        jpeg_destroy_decompress(&mut src);
        result
    }
}

/// The most ICC profile data that fits in one APP2 marker, after its signature and sequence bytes.
const ICC_CHUNK_SIZE: usize = 65519;

/// Encode 8-bit RGB pixels with mozjpeg's maximum compression profile: progressive scans with
/// optimised scan order, trellis quantisation and optimised Huffman tables.
pub fn encode(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    icc_profile: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    unsafe {
        let mut err = error_manager();
        let mut dst: Box<jpeg_compress_struct> = Box::new(mem::zeroed());
        dst.common.err = &mut *err;
        let mut out: *mut u8 = ptr::null_mut();

        let result = catch(|| {
            jpeg_create_compress(&mut *dst);
            dst.image_width = width;
            dst.image_height = height;
            dst.input_components = 3;
            dst.in_color_space = J_COLOR_SPACE::JCS_RGB;
            jpeg_set_defaults(&mut dst);
            jpeg_set_quality(&mut dst, c_int::from(quality), 0);
            dst.optimize_coding = 1;

            let mut size: c_ulong = 0;
            jpeg_mem_dest(&mut dst, &mut out, &mut size);
            jpeg_start_compress(&mut dst, 1);
            if let Some(icc) = icc_profile {
                let chunks = icc.chunks(ICC_CHUNK_SIZE);
                let count = chunks.len() as u8;
                for (i, chunk) in chunks.enumerate() {
                    let marker = [ICC_SIGNATURE, &[i as u8 + 1, count], chunk].concat();
                    jpeg_write_marker(&mut dst, APP2, marker.as_ptr(), marker.len() as _);
                }
            }
            for row in rgb.chunks_exact(width as usize * 3) {
                jpeg_write_scanlines(&mut dst, &row.as_ptr(), 1);
            }
            jpeg_finish_compress(&mut dst);
            take_output(&mut out, size)
        });

        if !out.is_null() {
            libc::free(out.cast());
        }
        jpeg_destroy_compress(&mut dst);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a synthetic RGB image the way plain libjpeg would: baseline, default Huffman tables.
    fn encode_baseline(width: u32, height: u32, markers: &[(c_int, &[u8])]) -> Vec<u8> {
        let pixels: Vec<u8> = (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| {
                    [
                        (x * 255 / width) as u8,
                        (y * 255 / height) as u8,
                        ((x ^ y) & 0xFF) as u8,
                    ]
                })
            })
            .collect();

        unsafe {
            let mut err: jpeg_error_mgr = mem::zeroed();
            jpeg_std_error(&mut err);
            err.error_exit = Some(error_exit);
            let mut dst: jpeg_compress_struct = mem::zeroed();
            dst.common.err = &mut err;
            jpeg_create_compress(&mut dst);

            dst.image_width = width;
            dst.image_height = height;
            dst.input_components = 3;
            dst.in_color_space = J_COLOR_SPACE::JCS_RGB;
            jpeg_c_set_int_param(
                &mut dst,
                J_INT_PARAM::JINT_COMPRESS_PROFILE,
                JINT_COMPRESS_PROFILE_VALUE::JCP_FASTEST as c_int,
            );
            jpeg_set_defaults(&mut dst);
            jpeg_set_quality(&mut dst, 90, 1);

            let mut out: *mut u8 = ptr::null_mut();
            let mut size: c_ulong = 0;
            jpeg_mem_dest(&mut dst, &mut out, &mut size);
            jpeg_start_compress(&mut dst, 1);
            for (code, data) in markers {
                jpeg_write_marker(&mut dst, *code, data.as_ptr(), data.len() as _);
            }
            for row in pixels.chunks_exact(width as usize * 3) {
                jpeg_write_scanlines(&mut dst, &row.as_ptr(), 1);
            }
            jpeg_finish_compress(&mut dst);
            jpeg_destroy_compress(&mut dst);

            let result = slice::from_raw_parts(out, size as usize).to_vec();
            libc::free(out.cast());
            result
        }
    }

    /// Every component's quantisation table and DCT coefficients, which fully determine the
    /// decoded pixels.
    fn coefficients(data: &[u8]) -> Vec<(Vec<u16>, Vec<JCOEF>)> {
        unsafe {
            let mut err: jpeg_error_mgr = mem::zeroed();
            jpeg_std_error(&mut err);
            err.error_exit = Some(error_exit);
            let mut src: jpeg_decompress_struct = mem::zeroed();
            src.common.err = &mut err;
            jpeg_create_decompress(&mut src);
            jpeg_mem_src(&mut src, data.as_ptr(), data.len() as _);
            jpeg_read_header(&mut src, 1);

            let arrays = jpeg_read_coefficients(&mut src);
            let access = (*src.common.mem).access_virt_barray.unwrap();
            let mut components = Vec::new();
            for c in 0..src.num_components as usize {
                let info = &*src.comp_info.add(c);
                let mut coefs = Vec::new();
                for row in 0..info.height_in_blocks {
                    let blocks = *access(&mut src.common, *arrays.add(c), row, 1, 0);
                    for block in slice::from_raw_parts(blocks, info.width_in_blocks as usize) {
                        coefs.extend_from_slice(block);
                    }
                }
                components.push(((*info.quant_table).quantval.to_vec(), coefs));
            }

            jpeg_finish_decompress(&mut src);
            jpeg_destroy_decompress(&mut src);
            components
        }
    }

    #[test]
    fn lossless_and_smaller() {
        let original = encode_baseline(257, 123, &[]);
        let optimised = optimise(&original).unwrap();
        assert!(optimised.len() < original.len());
        assert_eq!(coefficients(&original), coefficients(&optimised));
    }

    #[test]
    fn keeps_only_display_metadata() {
        let icc = [ICC_SIGNATURE, b"\x01\x01fake profile"].concat();
        let mut exif = minimal_exif(6);
        exif.extend_from_slice(b"camera serial number");
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>";
        let original = encode_baseline(
            64,
            64,
            &[
                (APP1, &exif),
                (APP1, xmp),
                (APP2, &icc),
                (jpeg_marker::COM as c_int, b"a comment"),
            ],
        );

        let optimised = optimise(&original).unwrap();
        let contains = |needle: &[u8]| optimised.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&icc));
        assert!(contains(&minimal_exif(6)));
        assert!(!contains(b"camera serial number"));
        assert!(!contains(b"xmpmeta"));
        assert!(!contains(b"a comment"));
        assert_eq!(coefficients(&original), coefficients(&optimised));
    }

    #[test]
    fn drops_default_orientation() {
        let original = encode_baseline(32, 32, &[(APP1, &minimal_exif(1))]);
        let optimised = optimise(&original).unwrap();
        assert!(!optimised.windows(4).any(|w| w == b"Exif"));
    }

    #[test]
    fn reads_little_endian_orientation() {
        let exif = b"Exif\0\0II\x2A\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x08\0\0\0\0\0\0\0";
        assert_eq!(exif_orientation(exif), Some(8));
    }

    #[test]
    fn rejects_corrupt_input() {
        let original = encode_baseline(32, 32, &[]);
        assert!(optimise(&original[..original.len() / 4]).is_err());
        assert!(optimise(b"\xFF\xD8\xFFnot a jpeg").is_err());
    }
}
