use std::{
    fs::{self, File},
    path::PathBuf,
    process::exit,
};

use clap::Parser;
use fast_image_resize::{PixelType, Resizer, images::Image};
use image::{ExtendedColorType, ImageEncoder, codecs::avif::AvifEncoder};
use zune_core::{bytestream::ZCursor, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

mod png_decoder;

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

    const JPEG_MAGIC_BYTES: [u8; 3] = [0xff, 0xd8, 0xff];

    let decoder_options = DecoderOptions::new_fast();
    let (src_image, width, height) = if file.starts_with(&JPEG_MAGIC_BYTES) {
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(file), decoder_options);
        let bytes = decoder.decode().expect("Error decoding JPEG");
        let (width, height) = decoder.dimensions().expect("Error reading dimensions");
        (bytes, width as u32, height as u32)
    } else if file.starts_with(&png_decoder::PNG_MAGIC_BYTES) {
        png_decoder::decode(&file).expect("Error decoding PNG")
    } else {
        eprintln!("Only JPEG and PNG images are allowed as input");
        exit(1)
    };

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

    let outfile = File::create(&args.out_path).expect("Error creating output file");

    // cavif default settings
    const AVIF_SPEED: u8 = 4;
    const AVIF_QUALITY: u8 = 80;
    AvifEncoder::new_with_speed_quality(outfile, AVIF_SPEED, AVIF_QUALITY)
        .write_image(
            dest_image.buffer(),
            dest_image.width(),
            dest_image.height(),
            ExtendedColorType::Rgb8,
        )
        .expect("Error while encoding AVIF");

    println!("{} {}", resized_width, resized_height);
}
