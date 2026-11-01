use std::num::NonZeroU64;

use oxipng::{Deflater, Options, StripChunks, ZopfliOptions};

/// Equivalent to `oxipng -o max -Z --zi <iterations> -s -a -i 0`.
pub fn optimise(data: &[u8], zopfli_iterations: NonZeroU64) -> Result<Vec<u8>, String> {
    let options = Options {
        deflater: Deflater::Zopfli(ZopfliOptions {
            iteration_count: zopfli_iterations,
            ..ZopfliOptions::default()
        }),
        // Only drops chunks that don't affect how the image is displayed.
        strip: StripChunks::Safe,
        // Rewrites the colour of fully transparent pixels, which are invisible either way.
        optimize_alpha: true,
        // Interlacing always costs bytes.
        interlace: Some(false),
        ..Options::max_compression()
    };
    oxipng::optimize_from_memory(data, &options).map_err(|e| e.to_string())
}
