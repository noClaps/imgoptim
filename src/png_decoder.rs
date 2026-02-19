use zlib_rs::{InflateConfig, ReturnCode, crc32::crc32, decompress_slice};

pub const PNG_MAGIC_BYTES: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const BYTES_PER_PIXEL: u8 = 3;

/// Decodes a PNG file and returns `Some((pixels, width, height))` if successful.
///
/// Ported from https://pyokagan.name/blog/2019-10-14-png/
pub fn decode(file: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let (magic, mut file) = file.split_first_chunk::<8>()?;
    assert_eq!(
        magic, &PNG_MAGIC_BYTES,
        "Input file signature does not match PNG signature"
    );

    let mut width = 0;
    let mut height = 0;
    let mut img_data = vec![];
    while let Some((length, chunk_type, mut data)) = read_chunk(&file) {
        match chunk_type.as_str() {
            "IHDR" => (width, height) = read_header(&data)?,
            "IDAT" => img_data.append(&mut data),
            _ => {}
        };

        file = &file[4 + 4 + length as usize + 4..];
    }

    let mut buffer = vec![0; height as usize * (1 + width as usize * BYTES_PER_PIXEL as usize)];
    let (img_data, rc) = decompress_slice(&mut buffer, &img_data, InflateConfig::default());
    assert_eq!(rc, ReturnCode::Ok, "Error decompressing PNG");

    let img_data = reconstruct(img_data, width, height);
    Some((img_data, width, height))
}

fn read_chunk(bytes: &[u8]) -> Option<(u32, String, Vec<u8>)> {
    let (length, bytes) = bytes.split_first_chunk::<4>()?;
    let length = ((length[0] as u32) << 24)
        + ((length[1] as u32) << 16)
        + ((length[2] as u32) << 8)
        + length[3] as u32;

    let (chunk_type_bytes, bytes) = bytes.split_first_chunk::<4>()?;
    let chunk_type = String::from_utf8_lossy(chunk_type_bytes).to_string();

    let (data, bytes) = bytes.split_at_checked(length as usize)?;

    let checksum_bytes = [chunk_type_bytes, data].concat();
    let checksum = crc32(0, &checksum_bytes);

    let crc = bytes.first_chunk::<4>()?;
    let crc =
        ((crc[0] as u32) << 24) + ((crc[1] as u32) << 16) + ((crc[2] as u32) << 8) + crc[3] as u32;
    assert_eq!(
        crc, checksum,
        "CRC did not match checksum for {chunk_type} chunk"
    );

    Some((length, chunk_type, data.to_vec()))
}

fn read_header(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() != 13 {
        return None;
    }

    let (width, bytes) = bytes.split_first_chunk::<4>()?;
    let width = ((width[0] as u32) << 24)
        + ((width[1] as u32) << 16)
        + ((width[2] as u32) << 8)
        + width[3] as u32;
    assert_ne!(width, 0, "width cannot be zero");

    let (height, bytes) = bytes.split_first_chunk::<4>()?;
    let height = ((height[0] as u32) << 24)
        + ((height[1] as u32) << 16)
        + ((height[2] as u32) << 8)
        + height[3] as u32;
    assert_ne!(height, 0, "height cannot be zero");

    let (&bit_depth, bytes) = bytes.split_first()?;
    assert_eq!(bit_depth, 8, "this tool only supports 8-bit PNG images");

    let (&color_type, bytes) = bytes.split_first()?;
    assert_eq!(
        color_type, 2,
        "this tool only supports truecolor PNG images"
    );

    let (&compression_method, bytes) = bytes.split_first()?;
    assert_eq!(compression_method, 0, "compression method can only be zero");

    let (&filter_method, bytes) = bytes.split_first()?;
    assert_eq!(filter_method, 0, "filter method can only be zero");

    let &interlace_method = bytes.first()?;
    assert_eq!(
        interlace_method, 0,
        "this tool does not support interlaced PNG images"
    );

    return Some((width, height));
}

fn reconstruct(bytes: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut recon = vec![];
    let stride = (BYTES_PER_PIXEL as u32) * width;

    let recon_a = |recon: &[u8], r: usize, c: usize| -> u8 {
        if c >= BYTES_PER_PIXEL as usize {
            recon[r * stride as usize + c - BYTES_PER_PIXEL as usize]
        } else {
            0
        }
    };
    let recon_b = |recon: &[u8], r: usize, c: usize| -> u8 {
        if r > 0 {
            recon[(r - 1) * stride as usize + c]
        } else {
            0
        }
    };
    let recon_c = |recon: &[u8], r: usize, c: usize| -> u8 {
        if r > 0 && c >= BYTES_PER_PIXEL as usize {
            recon[(r - 1) * stride as usize + c - BYTES_PER_PIXEL as usize]
        } else {
            0
        }
    };

    let mut i = 0;
    for r in 0..height {
        let filter_type = bytes[i];
        i += 1;
        for c in 0..stride {
            let filt_x = bytes[i];
            i += 1;
            let recon_x = filt_x
                + match filter_type {
                    0 => 0,
                    1 => recon_a(&recon, r as usize, c as usize),
                    2 => recon_b(&recon, r as usize, c as usize),
                    3 => {
                        (recon_a(&recon, r as usize, c as usize)
                            + recon_b(&recon, r as usize, c as usize))
                            / 2
                    }
                    4 => paeth_prediction(
                        recon_a(&recon, r as usize, c as usize),
                        recon_b(&recon, r as usize, c as usize),
                        recon_c(&recon, r as usize, c as usize),
                    ),
                    _ => unreachable!("unknown filter type: {filter_type}"),
                };
            recon.push(recon_x & 0xff);
        }
    }

    recon
}

fn paeth_prediction(a: u8, b: u8, c: u8) -> u8 {
    let p = a as isize + b as isize - c as isize;
    let pa = (p - a as isize).abs();
    let pb = (p - b as isize).abs();
    let pc = (p - c as isize).abs();

    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
