use std::{fs, path::PathBuf};

use clap::Parser;
use fast_image_resize::{PixelType, Resizer, images::Image};
use ravif::{Encoder, Img, RGB8};
use zune_png::{
    PngDecoder,
    zune_core::{bytestream::ZCursor, options::DecoderOptions},
};

#[derive(Parser)]
/// A command line utility to optimise images
struct Args {
    #[arg(short = 'W', long)]
    /// Set the width of the output image in pixels.
    width: Option<u32>,

    /// Path to the image you'd like to optimise.
    path: PathBuf,
    /// Output path for the optimised image.
    out_path: PathBuf,
}

fn main() {
    let args = Args::parse();

    let file = fs::read(&args.path).expect("Error reading file");

    let mut decoder = PngDecoder::new_with_options(
        ZCursor::new(file),
        DecoderOptions::new_fast().png_set_strip_to_8bit(true),
    );
    let src_image = decoder
        .decode()
        .expect("Error decoding png")
        .u8()
        .expect("Error getting bytes from PNG");
    let (width, height) = decoder
        .dimensions()
        .map(|(w, h)| (w as u32, h as u32))
        .expect("Error getting dimensions from PNG");

    let (resized_width, resized_height) = match args.width {
        Some(w) => (w, height * w / width),
        None => (width, height),
    };

    let src_image = Image::from_vec_u8(width, height, src_image, PixelType::U8x3)
        .expect("Error converting image");
    let mut dest_image = Image::new(resized_width, resized_height, PixelType::U8x3);
    Resizer::new()
        .resize(&src_image, &mut dest_image, None)
        .expect("Error while resizing");

    let pixels: Vec<_> = dest_image
        .buffer()
        .chunks_exact(3)
        .map(|px| RGB8::new(px[0], px[1], px[2]))
        .collect();
    let buffer = Img::new(
        pixels.as_slice(),
        resized_width as usize,
        resized_height as usize,
    );

    let encoded = Encoder::new()
        .encode_rgb(buffer)
        .expect("Error encoding to AVIF");

    fs::write(args.out_path, encoded.avif_file).expect("Error writing output file");

    println!("{} {}", resized_width, resized_height);
}
