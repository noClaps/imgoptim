use std::{fs::File, path::PathBuf};

use argparse::ArgParse;
use fast_image_resize::{IntoImageView, Resizer, images::Image};
use image::{GenericImageView, ImageEncoder, ImageReader, codecs::avif::AvifEncoder};

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

    let src_image = ImageReader::open(&path)
        .expect(&format!("Failed to open {:?}", &path))
        .decode()
        .expect(&format!("Failed to decode {:?}", &path));
    let (width, height) = src_image.dimensions();

    let (resized_width, resized_height) = match out_width {
        Some(w) => (w, height * w / width),
        None => (width, height),
    };

    let mut dest_image = Image::new(
        resized_width,
        resized_height,
        src_image
            .pixel_type()
            .expect(&format!("Failed to get pixel type from {:?}", &path)),
    );
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
            src_image.color().into(),
        )
        .expect(&format!("Error while encoding {:?}", &out_path));

    println!("{} {}", resized_width, resized_height);
}
