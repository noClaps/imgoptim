use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use image::{GenericImageView, ImageFormat, imageops::FilterType};

#[derive(Clone, Subcommand)]
enum Subcommands {
    Optimise {
        #[arg(short = 'W', long)]
        /// Set the width of the output image in pixels.
        width: Option<u32>,

        /// Path to the image you'd like to optimise.
        path: PathBuf,
        /// Output path for the optimised image.
        out_path: PathBuf,
    },

    Dimensions {
        /// Path to the image to get the dimensions of.
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
            let src_image = image::open(&path).unwrap();

            let (src_width, src_height) = src_image.dimensions();
            let (resized_width, resized_height) = match width {
                Some(w) => (w, src_height * w / src_width),
                None => (src_width, src_height),
            };

            src_image
                .resize(resized_width, resized_height, FilterType::Lanczos3)
                .save_with_format(out_path, ImageFormat::Avif)
                .unwrap();
        }
        Subcommands::Dimensions { path } => {
            let bytes = fs::read(&path).unwrap();
            let ispe = bytes.windows(4).position(|b| b == b"ispe").unwrap();
            let width = u32::from_be_bytes(*bytes[ispe + 8..].first_chunk::<4>().unwrap());
            let height = u32::from_be_bytes(*bytes[ispe + 12..].first_chunk::<4>().unwrap());

            println!("{} {}", width, height);
        }
    }
}
