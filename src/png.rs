use std::{io::Cursor, num::NonZeroU64};

use oxipng::{Deflater, Options, StripChunks, ZopfliOptions, indexset};
use png::{BitDepth, ColorType, Decoder, Info, Reader, Transformations};

/// Equivalent to `oxipng -o max -Z --zi <iterations> -i 0`, keeping only the chunks that affect
/// how the image is displayed. Pixels are kept exactly, including the colour of fully transparent
/// ones.
pub fn optimise(data: &[u8], zopfli_iterations: NonZeroU64) -> Result<Vec<u8>, String> {
    let options = Options {
        deflater: Deflater::Zopfli(ZopfliOptions {
            iteration_count: zopfli_iterations,
            ..ZopfliOptions::default()
        }),
        // oxipng's own safe list, plus gamma, chromaticities and HDR metadata, which it strips
        // but which some viewers use to render colour. Palettes and transparency aren't
        // affected, since oxipng handles them as part of the image.
        strip: StripChunks::Keep(indexset! {
            *b"cICP", *b"iCCP", *b"sRGB", *b"gAMA", *b"cHRM", *b"mDCV", *b"cLLI",
            *b"pHYs", *b"acTL", *b"fcTL", *b"fdAT",
        }),
        // Interlacing always costs bytes.
        interlace: Some(false),
        ..Options::max_compression()
    };
    oxipng::optimize_from_memory(data, &options).map_err(|e| e.to_string())
}

fn reader(data: &[u8]) -> Result<Reader<Cursor<&[u8]>>, String> {
    let mut decoder = Decoder::new(Cursor::new(data));
    // Palettes, transparency chunks and bit depths under 8 become plain 8 or 16-bit channels.
    decoder.set_transformations(Transformations::EXPAND);
    decoder.read_info().map_err(|e| e.to_string())
}

/// Everything in a PNG's header that affects how it's displayed, apart from the pixels.
fn display_info(info: &Info) -> impl PartialEq {
    (
        (info.width, info.height),
        info.animation_control
            .map(|animation| (animation.num_frames, animation.num_plays)),
        // The decoder only fills in `source_gamma` and `source_chromaticities` when converting,
        // so compare the chunks themselves.
        info.gama_chunk,
        info.chrm_chunk,
        info.srgb,
        info.icc_profile.clone(),
        info.coding_independent_code_points,
        info.mastering_display_color_volume,
        info.content_light_level,
    )
}

/// Where and how an animation frame is drawn.
fn frame_info(info: &Info) -> impl PartialEq {
    info.frame_control.map(|frame| {
        (
            (frame.x_offset, frame.y_offset, frame.width, frame.height),
            (frame.delay_num, frame.delay_den),
            frame.dispose_op,
            frame.blend_op,
        )
    })
}

/// A row of expanded samples as 16-bit RGBA, so differently stored rows can be compared.
fn normalise(row: &[u8], (colour, depth): (ColorType, BitDepth)) -> Vec<u16> {
    let samples: Vec<u16> = match depth {
        BitDepth::Sixteen => row
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_be_bytes(pair))
            .collect(),
        _ => row.iter().map(|&sample| u16::from(sample) * 257).collect(),
    };
    match colour {
        ColorType::Grayscale => samples.iter().flat_map(|&v| [v, v, v, u16::MAX]).collect(),
        ColorType::GrayscaleAlpha => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[grey, alpha]| [grey, grey, grey, alpha])
            .collect(),
        ColorType::Rgb => samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[red, green, blue]| [red, green, blue, u16::MAX])
            .collect(),
        ColorType::Rgba | ColorType::Indexed => samples,
    }
}

/// Whether two PNGs display identically: the same pixels in every frame, the same animation
/// timing and layout, and the same colour information. How the pixels are stored (palette, bit
/// depth, colour type, filters, compression) may differ.
pub fn same_image(original: &[u8], optimised: &[u8]) -> Result<bool, String> {
    let (mut original, mut optimised) = (reader(original)?, reader(optimised)?);
    if display_info(original.info()) != display_info(optimised.info()) {
        return Ok(false);
    }
    let formats = (original.output_color_type(), optimised.output_color_type());

    loop {
        if frame_info(original.info()) != frame_info(optimised.info()) {
            return Ok(false);
        }
        // Compare a row at a time, so large images don't need decoding whole.
        loop {
            let original_row = original
                .next_row()
                .map_err(|e| e.to_string())?
                .map(|row| normalise(row.data(), formats.0));
            let optimised_row = optimised
                .next_row()
                .map_err(|e| e.to_string())?
                .map(|row| normalise(row.data(), formats.1));
            match (original_row, optimised_row) {
                (Some(a), Some(b)) if a == b => {}
                (None, None) => break,
                _ => return Ok(false),
            }
        }
        // Move on to the next animation frame, if there is one.
        match (
            original.next_frame_info().is_ok(),
            optimised.next_frame_info().is_ok(),
        ) {
            (true, true) => {}
            (false, false) => return Ok(true),
            _ => return Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use png::{Encoder, ScaledFloat};

    use super::*;

    fn encode(
        (width, height): (u32, u32),
        (colour, depth): (ColorType, BitDepth),
        gamma: Option<ScaledFloat>,
        frames: &[&[u8]],
    ) -> Vec<u8> {
        let mut data = Vec::new();
        let mut encoder = Encoder::new(&mut data, width, height);
        encoder.set_color(colour);
        encoder.set_depth(depth);
        if let Some(gamma) = gamma {
            encoder.set_source_gamma(gamma);
        }
        if frames.len() > 1 {
            let frame_count = u32::try_from(frames.len()).unwrap();
            encoder.set_animated(frame_count, 0).unwrap();
        }
        let mut writer = encoder.write_header().unwrap();
        for frame in frames {
            writer.write_image_data(frame).unwrap();
        }
        writer.finish().unwrap();
        data
    }

    fn iterations() -> NonZeroU64 {
        NonZeroU64::new(1).unwrap()
    }

    #[test]
    fn keeps_transparent_pixels_exactly() {
        // Four colours, so oxipng can use a palette. Two are fully transparent, with different
        // colours underneath.
        let colours = [
            [255, 0, 0, 255],
            [0, 255, 0, 0],
            [0, 0, 255, 0],
            [9, 9, 9, 128],
        ];
        let pixels: Vec<u8> = (0..32 * 32).flat_map(|i| colours[i % 4]).collect();
        let original = encode(
            (32, 32),
            (ColorType::Rgba, BitDepth::Eight),
            None,
            &[&pixels],
        );
        let optimised = optimise(&original, iterations()).unwrap();
        assert!(optimised.len() < original.len());
        assert!(same_image(&original, &optimised).unwrap());
    }

    #[test]
    fn matches_across_bit_depths() {
        // 16-bit grey whose values all fit in 8 bits, which oxipng stores as 8-bit.
        let pixels: Vec<u8> = (0..64u16 * 64)
            .flat_map(|i| ((i % 256) * 257).to_be_bytes())
            .collect();
        let original = encode(
            (64, 64),
            (ColorType::Grayscale, BitDepth::Sixteen),
            None,
            &[&pixels],
        );
        let optimised = optimise(&original, iterations()).unwrap();
        assert!(optimised.len() < original.len());
        assert!(same_image(&original, &optimised).unwrap());
    }

    #[test]
    fn detects_different_pixels_and_colour_information() {
        let format = (ColorType::Rgb, BitDepth::Eight);
        let pixels = vec![100; 16 * 16 * 3];
        let original = encode((16, 16), format, None, &[&pixels]);
        let mut changed = pixels.clone();
        changed[100] = 101;
        let different = encode((16, 16), format, None, &[&changed]);
        assert!(!same_image(&original, &different).unwrap());

        let gamma = Some(ScaledFloat::new(1.0 / 2.2));
        let tagged = encode((16, 16), format, gamma, &[&pixels]);
        assert!(!same_image(&original, &tagged).unwrap());
    }

    #[test]
    fn keeps_gamma() {
        let pixels: Vec<u8> = (0..32 * 32 * 3)
            .map(|i| u8::try_from(i % 7).unwrap())
            .collect();
        let gamma = Some(ScaledFloat::new(1.0 / 2.2));
        let original = encode(
            (32, 32),
            (ColorType::Rgb, BitDepth::Eight),
            gamma,
            &[&pixels],
        );
        let optimised = optimise(&original, iterations()).unwrap();
        assert!(optimised.len() < original.len());
        assert!(optimised.windows(4).any(|w| w == b"gAMA"));
        assert!(same_image(&original, &optimised).unwrap());
    }

    #[test]
    fn compares_every_animation_frame() {
        let format = (ColorType::Rgb, BitDepth::Eight);
        let first = vec![10; 8 * 8 * 3];
        let second = vec![200; 8 * 8 * 3];
        let mut changed = second.clone();
        changed[0] = 201;
        let animation = encode((8, 8), format, None, &[&first, &second]);

        let optimised = optimise(&animation, iterations()).unwrap();
        assert!(same_image(&animation, &optimised).unwrap());
        let different = encode((8, 8), format, None, &[&first, &changed]);
        assert!(!same_image(&animation, &different).unwrap());
        let shorter = encode((8, 8), format, None, &[&first]);
        assert!(!same_image(&animation, &shorter).unwrap());
    }
}
