use std::{
    ffi::{CStr, c_int, c_uint},
    marker::PhantomData,
    mem,
    panic::{self, AssertUnwindSafe},
    ptr::{self, addr_of_mut},
    slice,
};

use mozjpeg_sys::{
    J_BOOLEAN_PARAM, J_COLOR_SPACE, JCOEF, boolean, jpeg_c_set_bool_param, jpeg_common_struct,
    jpeg_compress_struct, jpeg_copy_critical_parameters, jpeg_create_compress,
    jpeg_create_decompress, jpeg_decompress_struct, jpeg_destination_mgr, jpeg_destroy_compress,
    jpeg_destroy_decompress, jpeg_error_mgr, jpeg_finish_compress, jpeg_mem_src,
    jpeg_read_coefficients, jpeg_read_header, jpeg_save_markers, jpeg_simple_progression,
    jpeg_std_error, jpeg_write_coefficients, jpeg_write_marker, jvirt_barray_control,
};

const APP1: c_int = 0xE1;
const APP2: c_int = 0xE2;
const EXIF_SIGNATURE: &[u8] = b"Exif\0\0";
const ICC_SIGNATURE: &[u8] = b"ICC_PROFILE\0";
const ORIENTATION_TAG: u16 = 0x0112;

/// libjpeg's `JMSG_LENGTH_MAX`: the most it writes when formatting an error message.
const MESSAGE_LENGTH: usize = 200;

/// The C signature of libjpeg's `format_message`. mozjpeg-sys declares its buffer as `&[u8; 80]`,
/// but libjpeg writes up to [`MESSAGE_LENGTH`] bytes into it, which needs a larger, mutable buffer.
type FormatMessage =
    unsafe extern "C-unwind" fn(&mut jpeg_common_struct, &mut [u8; MESSAGE_LENGTH]);

/// libjpeg's error handler. It formats the message and unwinds with it, to be caught by
/// [`catch`], instead of exiting the process.
///
/// # Safety
///
/// `cinfo.err` must point to a valid error manager, which libjpeg guarantees when calling it.
unsafe extern "C-unwind" fn error_exit(cinfo: &mut jpeg_common_struct) {
    let mut buffer = [0u8; MESSAGE_LENGTH];
    // SAFETY: libjpeg only calls this with `cinfo.err` set to the error manager it's installed in.
    let format_message = unsafe { (*cinfo.err).format_message };
    if let Some(format_message) = format_message {
        // SAFETY: Both are function pointers to the same C function with the C-unwind ABI, and
        // differ only in the buffer's reference type. References of any mutability and pointee
        // size are passed as the same pointer, so the signatures are ABI-compatible.
        let format_message: FormatMessage = unsafe { mem::transmute(format_message) };
        // SAFETY: `cinfo` is the struct libjpeg is reporting an error for, and the buffer holds
        // the most libjpeg writes.
        unsafe { format_message(cinfo, &mut buffer) };
    }
    let message = CStr::from_bytes_until_nul(&buffer).map_or_else(
        |_| "unknown JPEG error".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    panic::resume_unwind(Box::new(message));
}

unsafe extern "C-unwind" fn silence(_: &mut jpeg_common_struct) {}

/// An error manager that unwinds instead of exiting the process, for use with [`catch`].
fn error_manager() -> Box<jpeg_error_mgr> {
    // SAFETY: Every field of `jpeg_error_mgr` is an integer, raw pointer, array, or `Option` of a
    // function pointer, for all of which zero is a valid value.
    let mut err: Box<jpeg_error_mgr> = Box::new(unsafe { mem::zeroed() });
    // SAFETY: `err` is a valid, exclusively borrowed error manager for libjpeg to fill in.
    unsafe { jpeg_std_error(&mut err) };
    err.error_exit = Some(error_exit);
    err.output_message = Some(silence);
    err
}

/// Run libjpeg calls, turning errors raised through [`error_manager`] into `Err`.
///
/// Unwinding out of libjpeg is defined because mozjpeg-sys compiles it with `-fexceptions` (its
/// `unwinding` feature) and declares its functions with the C-unwind ABI. After an error, the
/// libjpeg structs may only be destroyed, which [`Reader`] and [`Writer`] do when dropped.
fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        e.downcast::<String>()
            .map_or_else(|_| "unknown JPEG error".to_owned(), |message| *message)
    })
}

const CHUNK_SIZE: usize = 16 * 1024;

/// A libjpeg destination that collects the output in a `Vec`. libjpeg's own memory destination
/// can't be cleaned up after an error: it frees and replaces its buffer as it grows, without
/// telling the caller.
#[repr(C)]
struct VecDestination {
    /// Must be first, since libjpeg is given a pointer to it and the callbacks cast that pointer
    /// back to the whole struct.
    manager: jpeg_destination_mgr,
    chunk: [u8; CHUNK_SIZE],
    output: Vec<u8>,
}

impl VecDestination {
    fn new() -> Box<Self> {
        Box::new(Self {
            manager: jpeg_destination_mgr {
                next_output_byte: ptr::null_mut(),
                free_in_buffer: 0,
                init_destination: Some(init_destination),
                empty_output_buffer: Some(empty_output_buffer),
                term_destination: Some(term_destination),
            },
            chunk: [0; CHUNK_SIZE],
            output: Vec::new(),
        })
    }
}

/// Point libjpeg at the start of the empty chunk.
///
/// # Safety
///
/// `destination` must point to a live `VecDestination`.
unsafe fn reset_chunk(destination: *mut VecDestination) {
    // SAFETY: The caller guarantees `destination` is live. Taking the chunk's address with
    // `addr_of_mut!` gives libjpeg a pointer with provenance over the whole chunk.
    unsafe {
        (*destination).manager.next_output_byte = addr_of_mut!((*destination).chunk).cast();
        (*destination).manager.free_in_buffer = CHUNK_SIZE;
    }
}

/// Append the first `used` bytes of the chunk to the output.
///
/// # Safety
///
/// `destination` must point to a live `VecDestination`, and `used` must be at most
/// [`CHUNK_SIZE`].
unsafe fn flush_chunk(destination: *mut VecDestination, used: usize) {
    // SAFETY: The caller guarantees `destination` is live and `used` is within the chunk. The
    // chunk and output are separate fields, so borrowing both at once doesn't alias.
    unsafe {
        let chunk = &(&(*destination).chunk)[..used];
        (*destination).output.extend_from_slice(chunk);
    }
}

/// # Safety
///
/// `cinfo.dest` must point to the `manager` field of a live `VecDestination`.
unsafe extern "C-unwind" fn init_destination(cinfo: &mut jpeg_compress_struct) {
    // SAFETY: `manager` is the first field of the `#[repr(C)]` struct, so a pointer to it is a
    // pointer to the struct, which the caller guarantees is live.
    unsafe { reset_chunk(cinfo.dest.cast()) };
}

/// Called when the chunk is full. libjpeg requires the whole chunk to be written out, whatever
/// `free_in_buffer` says.
///
/// # Safety
///
/// `cinfo.dest` must point to the `manager` field of a live `VecDestination`.
unsafe extern "C-unwind" fn empty_output_buffer(cinfo: &mut jpeg_compress_struct) -> boolean {
    let destination = cinfo.dest.cast::<VecDestination>();
    // SAFETY: As for `init_destination`, and the whole chunk is within the chunk.
    unsafe {
        flush_chunk(destination, CHUNK_SIZE);
        reset_chunk(destination);
    }
    1
}

/// # Safety
///
/// `cinfo.dest` must point to the `manager` field of a live `VecDestination`.
unsafe extern "C-unwind" fn term_destination(cinfo: &mut jpeg_compress_struct) {
    let destination = cinfo.dest.cast::<VecDestination>();
    // SAFETY: As for `init_destination`. libjpeg only lowers `free_in_buffer` from the
    // `CHUNK_SIZE` it was set to, so the subtraction can't underflow.
    unsafe {
        let used = CHUNK_SIZE - (*destination).manager.free_in_buffer;
        flush_chunk(destination, used);
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

/// A libjpeg decompressor that has read a JPEG's DCT coefficients. It's destroyed when dropped,
/// including when an error unwinds out of libjpeg, and borrows the data it reads from.
struct Reader<'data> {
    src: Box<jpeg_decompress_struct>,
    err: Box<jpeg_error_mgr>,
    coefficients: *mut *mut jvirt_barray_control,
    data: PhantomData<&'data [u8]>,
}

impl<'data> Reader<'data> {
    /// Read a JPEG's header, saved markers and DCT coefficients. Must run inside [`catch`].
    /// libjpeg only warns about truncated or corrupt data, patching over it, so that's refused:
    /// re-encoding it would turn a damaged image into a different, complete one.
    fn new(data: &'data [u8]) -> Result<Self, String> {
        let mut reader = Self {
            // SAFETY: Every field of `jpeg_decompress_struct` is an integer, raw pointer, array,
            // `Option` of a function pointer, or an enum with a zero variant, so all zeroes is a
            // valid value. It's boxed so the pointers libjpeg keeps to it stay valid.
            src: Box::new(unsafe { mem::zeroed() }),
            err: error_manager(),
            coefficients: ptr::null_mut(),
            data: PhantomData,
        };
        reader.src.common.err = &raw mut *reader.err;
        // SAFETY: The struct is zeroed apart from its error manager, which is what creating it
        // expects. `data` outlives `src`'s use of it, which ends when `src` is destroyed on drop.
        // Markers are saved before the header is read.
        unsafe {
            jpeg_create_decompress(&raw mut *reader.src);
            jpeg_mem_src(&mut reader.src, data.as_ptr(), data.len() as _);
            jpeg_save_markers(&mut reader.src, APP1, 0xFFFF);
            jpeg_save_markers(&mut reader.src, APP2, 0xFFFF);
            jpeg_read_header(&mut reader.src, 1);
            reader.coefficients = jpeg_read_coefficients(&mut reader.src);
        }
        // A memory source never suspends, so this only happens if libjpeg changes.
        assert!(!reader.coefficients.is_null(), "libjpeg suspended reading");
        if reader.err.num_warnings > 0 {
            return Err("the JPEG is truncated or corrupt".to_owned());
        }
        Ok(reader)
    }

    /// Keep only the markers that change how the image is displayed: the ICC profile, and the
    /// EXIF orientation when it isn't the default.
    fn display_markers(&self) -> Vec<(c_int, Vec<u8>)> {
        let mut markers = Vec::new();
        let mut marker = self.src.marker_list;
        // SAFETY: The header was read with markers saved, so the list is valid until `src` is
        // destroyed: each node is null or a live marker.
        while let Some(m) = unsafe { marker.as_ref() } {
            // SAFETY: libjpeg allocates each saved marker's data right after its node, so `data`
            // is never null, and `data_length` bytes of it are initialised.
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
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        // SAFETY: Destroying is valid in any state, including after an error unwound out of
        // libjpeg, and nothing uses `src` afterwards.
        unsafe { jpeg_destroy_decompress(&mut self.src) };
    }
}

/// A libjpeg compressor writing to its own destination, destroyed when dropped, including when an
/// error unwinds out of libjpeg.
struct Writer {
    dst: Box<jpeg_compress_struct>,
    err: Box<jpeg_error_mgr>,
    destination: Box<VecDestination>,
}

impl Writer {
    /// Must run inside [`catch`].
    fn new() -> Self {
        let mut writer = Self {
            // SAFETY: As for `jpeg_decompress_struct` in `Reader::new`.
            dst: Box::new(unsafe { mem::zeroed() }),
            err: error_manager(),
            destination: VecDestination::new(),
        };
        writer.dst.common.err = &raw mut *writer.err;
        // SAFETY: The struct is zeroed apart from its error manager, which is what creating it
        // expects.
        unsafe { jpeg_create_compress(&raw mut *writer.dst) };
        // Creating clears the destination, so it's set afterwards.
        writer.dst.dest = &raw mut writer.destination.manager;
        writer
    }

    /// Write `reader`'s coefficients out as a new JPEG with `markers`, with progressive or
    /// baseline scans.
    fn transcode(
        &mut self,
        reader: &Reader,
        markers: &[(c_int, Vec<u8>)],
        progressive: bool,
    ) -> Vec<u8> {
        let dst = &mut *self.dst;
        // SAFETY: `reader` holds its coefficients until it's dropped, and `dst` is idle: newly
        // created, or finished by the previous call. That's what copying the parameters, setting
        // scan options and writing coefficients need. The destination is boxed alongside `dst`,
        // so it lives as long as `dst` does.
        unsafe {
            jpeg_copy_critical_parameters(&reader.src, dst);
            dst.optimize_coding = 1;
            if progressive {
                jpeg_c_set_bool_param(dst, J_BOOLEAN_PARAM::JBOOLEAN_OPTIMIZE_SCANS, 1);
                jpeg_simple_progression(dst);
            } else {
                jpeg_c_set_bool_param(dst, J_BOOLEAN_PARAM::JBOOLEAN_OPTIMIZE_SCANS, 0);
                dst.num_scans = 0;
                dst.scan_info = ptr::null();
            }
            jpeg_write_coefficients(dst, reader.coefficients);
        }
        for (code, data) in markers {
            let length = c_uint::try_from(data.len()).expect("JPEG markers are under 64 KiB");
            // SAFETY: `dst` is writing, after the coefficients and before finishing, when markers
            // may be added, and `data` is valid for `length` bytes.
            unsafe { jpeg_write_marker(dst, *code, data.as_ptr(), length) };
        }
        // SAFETY: `dst` is writing, so it can be finished.
        unsafe { jpeg_finish_compress(dst) };
        mem::take(&mut self.destination.output)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // SAFETY: As for `Reader`'s drop.
        unsafe { jpeg_destroy_compress(&mut self.dst) };
    }
}

/// Losslessly re-encode a JPEG's DCT coefficients with optimised Huffman tables, trying both
/// progressive and baseline scans and keeping whichever is smaller. Equivalent to mozjpeg's
/// `jpegtran -copy none -optimize`, keeping the colour profile and orientation.
pub fn optimise(data: &[u8]) -> Result<Vec<u8>, String> {
    catch(|| {
        let reader = Reader::new(data)?;
        let markers = reader.display_markers();
        let mut writer = Writer::new();
        let [progressive, baseline] =
            [true, false].map(|progressive| writer.transcode(&reader, &markers, progressive));
        Ok(if baseline.len() < progressive.len() {
            baseline
        } else {
            progressive
        })
    })?
}

/// Everything that determines how a JPEG is displayed: its layout, the DCT coefficients and
/// quantisation tables that fully determine its pixels, and the colour profile and orientation.
#[derive(Debug, PartialEq)]
struct Decoded {
    width: u32,
    height: u32,
    colour_space: J_COLOR_SPACE,
    components: Vec<Component>,
    markers: Vec<(c_int, Vec<u8>)>,
}

#[derive(Debug, PartialEq)]
struct Component {
    horizontal_sampling: c_int,
    vertical_sampling: c_int,
    quantisation: Vec<u16>,
    coefficients: Vec<JCOEF>,
}

fn decode(data: &[u8]) -> Result<Decoded, String> {
    catch(|| {
        let mut reader = Reader::new(data)?;
        let markers = reader.display_markers();
        let src = &mut *reader.src;
        // SAFETY: After `jpeg_read_coefficients`, `common.mem` is libjpeg's memory manager.
        let access = unsafe { (*src.common.mem).access_virt_barray }
            .expect("libjpeg's memory manager can access coefficient arrays");

        let mut components = Vec::new();
        for c in 0..usize::try_from(src.num_components).expect("a positive component count") {
            // SAFETY: `comp_info` has `num_components` entries, and `coefficients` has one array
            // per component. Each array row holds `width_in_blocks` blocks (rounded up), valid
            // until `src` is destroyed, and each component's quantisation table is set once its
            // coefficients are read.
            let component = unsafe {
                let info = &*src.comp_info.add(c);
                let mut coefficients = Vec::new();
                for row in 0..info.height_in_blocks {
                    let blocks = *access(&mut src.common, *reader.coefficients.add(c), row, 1, 0);
                    for block in slice::from_raw_parts(blocks, info.width_in_blocks as usize) {
                        coefficients.extend_from_slice(block);
                    }
                }
                Component {
                    horizontal_sampling: info.h_samp_factor,
                    vertical_sampling: info.v_samp_factor,
                    quantisation: (*info.quant_table).quantval.to_vec(),
                    coefficients,
                }
            };
            components.push(component);
        }

        Ok(Decoded {
            width: src.image_width,
            height: src.image_height,
            colour_space: src.jpeg_color_space,
            components,
            markers,
        })
    })?
}

/// Whether two JPEGs display identically: the same coefficients, quantisation tables, layout,
/// colour profile and orientation.
pub fn same_image(original: &[u8], optimised: &[u8]) -> Result<bool, String> {
    Ok(decode(original)? == decode(optimised)?)
}

#[cfg(test)]
mod tests {
    use mozjpeg::{ColorSpace, Compress, Marker};

    use super::*;

    /// Encode a synthetic RGB image the way plain libjpeg would: baseline, default Huffman tables.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the channel values are all at most 255"
    )]
    fn encode_baseline(width: u32, height: u32, markers: &[(Marker, &[u8])]) -> Vec<u8> {
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

        let mut compress = Compress::new(ColorSpace::JCS_RGB);
        compress.set_fastest_defaults();
        compress.set_size(width as usize, height as usize);
        compress.set_quality(90.0);
        let mut started = compress.start_compress(Vec::new()).unwrap();
        for (marker, data) in markers {
            started.write_marker(*marker, data);
        }
        started.write_scanlines(&pixels).unwrap();
        started.finish().unwrap()
    }

    #[test]
    fn lossless_and_smaller() {
        let original = encode_baseline(257, 123, &[]);
        let optimised = optimise(&original).unwrap();
        assert!(optimised.len() < original.len());
        assert!(same_image(&original, &optimised).unwrap());
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
                (Marker::APP(1), &exif),
                (Marker::APP(1), xmp),
                (Marker::APP(2), &icc),
                (Marker::COM, b"a comment"),
            ],
        );

        let optimised = optimise(&original).unwrap();
        let contains = |needle: &[u8]| optimised.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&icc));
        assert!(contains(&minimal_exif(6)));
        assert!(!contains(b"camera serial number"));
        assert!(!contains(b"xmpmeta"));
        assert!(!contains(b"a comment"));
        assert!(same_image(&original, &optimised).unwrap());
    }

    #[test]
    fn detects_different_images() {
        let image = encode_baseline(64, 48, &[]);
        assert!(!same_image(&image, &encode_baseline(64, 49, &[])).unwrap());
        let rotated = encode_baseline(64, 48, &[(Marker::APP(1), &minimal_exif(6))]);
        assert!(!same_image(&image, &rotated).unwrap());
    }

    #[test]
    fn drops_default_orientation() {
        let original = encode_baseline(32, 32, &[(Marker::APP(1), &minimal_exif(1))]);
        let optimised = optimise(&original).unwrap();
        assert!(!optimised.windows(4).any(|w| w == b"Exif"));
    }

    #[test]
    fn reads_little_endian_orientation() {
        let exif = b"Exif\0\0II\x2A\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x08\0\0\0\0\0\0\0";
        assert_eq!(exif_orientation(exif), Some(8));
    }

    /// Feed deterministically corrupted baseline and progressive JPEGs through the optimiser,
    /// which must return either a result or an error for every one, without crashing.
    #[test]
    fn survives_corrupt_input() {
        let baseline = encode_baseline(
            96,
            64,
            &[(
                Marker::APP(2),
                &[ICC_SIGNATURE, b"\x01\x01profile"].concat(),
            )],
        );
        let progressive = optimise(&baseline).unwrap();
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state >> 32).unwrap()
        };
        let mut outcomes = [0; 2];
        for original in [&baseline, &progressive] {
            for _ in 0..300 {
                let mut corrupt = original.clone();
                match random() % 3 {
                    0 => corrupt.truncate(random() % corrupt.len()),
                    1 => {
                        for _ in 0..=random() % 8 {
                            let at = random() % corrupt.len();
                            corrupt[at] ^= 1 << (random() % 8);
                        }
                    }
                    _ => {
                        let at = random() % corrupt.len();
                        corrupt[at] = 0xFF;
                    }
                }
                outcomes[usize::from(optimise(&corrupt).is_ok())] += 1;
            }
        }
        // Both paths should have been exercised.
        assert!(outcomes[0] > 0 && outcomes[1] > 0, "{outcomes:?}");
    }

    #[test]
    fn reports_libjpeg_errors() {
        let error = optimise(b"\xFF\xD8\xFF\xC0\x00\x02\xFF\xD9").unwrap_err();
        assert!(
            !error.is_empty() && error != "unknown JPEG error",
            "{error}"
        );
    }

    #[test]
    fn leaves_truncated_files_alone() {
        let original = optimise(&encode_baseline(256, 256, &[])).unwrap();
        let error = optimise(&original[..original.len() * 3 / 4]).unwrap_err();
        assert_eq!(error, "the JPEG is truncated or corrupt");
    }

    #[test]
    fn rejects_corrupt_input() {
        let original = encode_baseline(32, 32, &[]);
        assert!(optimise(&original[..original.len() / 4]).is_err());
        assert!(optimise(b"\xFF\xD8\xFFnot a jpeg").is_err());
    }
}
