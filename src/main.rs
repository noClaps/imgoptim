use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use heic::{DecoderConfig, PixelLayout};
use image::{DynamicImage, ImageFormat, RgbImage, imageops::FilterType};

#[derive(Clone, Subcommand)]
enum Subcommands {
    /// Optimise HEIC images to AVIF.
    Optimise {
        #[arg(short = 'W', long)]
        /// Set the width of the output image in pixels.
        width: Option<u32>,

        /// Path to the image you'd like to optimise.
        path: PathBuf,
        /// Output path for the optimised image.
        out_path: PathBuf,
    },
    /// Get the height of an AVIF image.
    Height {
        /// Path to the image to get the height of.
        path: PathBuf,
    },
}

#[derive(Parser)]
/// A command line utility to optimise images
struct Args {
    #[command(subcommand)]
    subcommands: Subcommands,
}

fn main() {
    let args = Args::parse();
    match args.subcommands {
        Subcommands::Optimise {
            width,
            path,
            out_path,
        } => {
            let data = fs::read(&path).unwrap();
            let decoded = DecoderConfig::new()
                .decode(&data, PixelLayout::Rgb8)
                .unwrap();

            let src_image = DynamicImage::ImageRgb8(
                RgbImage::from_raw(decoded.width, decoded.height, decoded.data).unwrap(),
            );

            let resized_width = width.unwrap_or(decoded.width);

            src_image
                .resize(resized_width, u32::MAX, FilterType::Lanczos3)
                .save_with_format(out_path, ImageFormat::Avif)
                .unwrap();
        }
        Subcommands::Height { path } => {
            let bytes = fs::read(&path).unwrap();
            let ispe = bytes.windows(4).position(|b| b == b"ispe").unwrap();
            let height = u32::from_be_bytes(*bytes[ispe + 12..].first_chunk::<4>().unwrap());
            println!("{height}");
        }
    }
}
