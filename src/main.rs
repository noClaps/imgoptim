use std::{
    fs::{File, read},
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use fast_image_resize::{PixelType, Resizer, images::Image};
use heic::{DecoderConfig, PixelLayout};
use image::{ExtendedColorType, ImageEncoder, RgbImage, codecs::avif::AvifEncoder};

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
            let data = read(&path).unwrap();
            let decoded = DecoderConfig::new()
                .decode(&data, PixelLayout::Rgb8)
                .unwrap();

            let src_image =
                RgbImage::from_raw(decoded.width, decoded.height, decoded.data).unwrap();

            let (src_width, src_height) = (decoded.width, decoded.height);
            let (resized_width, resized_height) = match width {
                Some(w) => (w, src_height * w / src_width),
                None => (src_width, src_height),
            };

            let mut resized = Image::new(resized_width, resized_height, PixelType::U8x3);

            let mut resizer = Resizer::new();
            resizer.resize(&src_image, &mut resized, None).unwrap();

            let file = File::create(out_path).unwrap();
            AvifEncoder::new_with_speed_quality(file, 10, 80)
                .write_image(
                    resized.buffer(),
                    resized_width,
                    resized_height,
                    ExtendedColorType::Rgb8,
                )
                .unwrap();
        }
        Subcommands::Height { path } => {
            let bytes = read(&path).unwrap();
            let ispe = bytes.windows(4).position(|b| b == b"ispe").unwrap();
            let height = u32::from_be_bytes(*bytes[ispe + 12..].first_chunk::<4>().unwrap());
            println!("{height}");
        }
    }
}
