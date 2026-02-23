use std::path::PathBuf;

use clap::Parser;
use image::{GenericImageView, ImageFormat, imageops::FilterType};

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

    let src_image = image::open(&args.path).unwrap();

    let (width, height) = src_image.dimensions();
    let (resized_width, resized_height) = match args.width {
        Some(w) => (w, height * w / width),
        None => (width, height),
    };

    src_image
        .resize(resized_width, resized_height, FilterType::Lanczos3)
        .save_with_format(args.out_path, ImageFormat::Avif)
        .unwrap();
    println!("{} {}", resized_width, resized_height);
}
