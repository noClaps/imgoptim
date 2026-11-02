use std::num::NonZeroU64;

use oxipng::{Deflater, Options, StripChunks, ZopfliOptions, indexset};

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
