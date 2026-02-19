use std::{fs, path::PathBuf, process::exit};

use clap::Parser;
use fast_image_resize::{PixelType, Resizer, images::Image};
use ravif::{Encoder, Img, RGB8};

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

    let (src_image, width, height) = match png_decoder::decode(&file) {
        Some(decoded) => decoded,
        None => {
            eprintln!("Only JPEG and PNG images are allowed as input");
            exit(1)
        }
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
