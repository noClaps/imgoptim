use std::{
    fs::{self, File},
    path::PathBuf,
    process::exit,
};

use argparse::ArgParse;
use fast_image_resize::{PixelType, Resizer, images::Image};
use image::{ExtendedColorType, ImageEncoder, codecs::avif::AvifEncoder};
use zune_core::{bytestream::ZCursor, options::DecoderOptions};
use zune_jpeg::JpegDecoder;
use zune_png::PngDecoder;

fn main() {
    let mut args = ArgParse::new()
        .about("A command line utility to optimise images")
        .flag(
            "width",
            "width",
            Some("W"),
            "Set the width of the output image in pixels.",
            false,
        )
        .positional("path", "Path to the image you'd like to optimise.")
        .positional("out-path", "Output path for the optimised image.");
    args.parse();
    let out_width: Option<u32> = args.get_flag("width");
    let path: PathBuf = args.get_positional("path").unwrap();
    let out_path: PathBuf = args.get_positional("out-path").unwrap();

    let file = fs::read(&path).expect("Error reading file");

    const JPEG_MAGIC_BYTES: [u8; 3] = [0xff, 0xd8, 0xff];
    const PNG_MAGIC_BYTES: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    let decoder_options = DecoderOptions::new_fast().png_set_strip_to_8bit(true);
    let (src_image, width, height) = match () {
        _ if file.starts_with(&JPEG_MAGIC_BYTES) => {
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(file), decoder_options);
            let bytes = decoder.decode().expect("Error decoding JPEG");
            let (width, height) = decoder.dimensions().expect("Error reading dimensions");
            (bytes, width as u32, height as u32)
        }
        _ if file.starts_with(&PNG_MAGIC_BYTES) => {
            let mut decoder = PngDecoder::new_with_options(ZCursor::new(file), decoder_options);
            let bytes = decoder
                .decode()
                .expect("Error decoding PNG")
                .u8()
                .expect("Error getting u8 slice");
            let (width, height) = decoder.dimensions().expect("Error reading dimensions");
            (bytes, width as u32, height as u32)
        }
        _ => {
            eprintln!("Only JPEG and PNG images are allowed as input");
            exit(1)
        }
    };

    let (resized_width, resized_height) = match out_width {
        Some(w) => (w, height * w / width),
        None => (width, height),
    };

    let src_image = Image::from_vec_u8(width, height, src_image, PixelType::U8x3)
        .expect("Error converting image");
    let mut dest_image = Image::new(resized_width, resized_height, PixelType::U8x3);
    Resizer::new()
        .resize(&src_image, &mut dest_image, None)
        .expect(&format!("Error while resizing {:?}", &path));

    let outfile =
        File::create(&out_path).expect(&format!("Error creating output file at {:?}", &out_path));
    AvifEncoder::new_with_speed_quality(outfile, 10, 100)
        .write_image(
            dest_image.buffer(),
            dest_image.width(),
            dest_image.height(),
            ExtendedColorType::Rgb8,
        )
        .expect(&format!("Error while encoding {:?}", &out_path));

    println!("{} {}", resized_width, resized_height);
}
