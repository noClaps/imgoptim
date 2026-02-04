use std::{fs::File, path::PathBuf};

use argh::FromArgs;
use fast_image_resize::{IntoImageView, Resizer, images::Image};
use image::{GenericImageView, ImageEncoder, ImageReader, codecs::avif::AvifEncoder};

/// A command line utility to optimise images
#[derive(FromArgs)]
#[argh(help_triggers("-h", "--help"))]
struct Args {
    /// set the width of the output image in pixels
    #[argh(option, short = 'W')]
    width: Option<u32>,

    /// path to the image you'd like to optimise
    #[argh(positional)]
    path: PathBuf,
    /// output path for the optimised image
    #[argh(positional)]
    out_path: PathBuf,
}

fn main() {
    let args: Args = argh::from_env();

    let src_image = ImageReader::open(&args.path)
        .expect(&format!("Failed to open {:?}", &args.path))
        .decode()
        .expect(&format!("Failed to decode {:?}", &args.path));
    let (width, height) = src_image.dimensions();

    let (resized_width, resized_height) = match args.width {
        Some(w) => (w, height * w / width),
        None => (width, height),
    };

    let mut dest_image = Image::new(
        resized_width,
        resized_height,
        src_image
            .pixel_type()
            .expect(&format!("Failed to get pixel type from {:?}", &args.path)),
    );
    Resizer::new()
        .resize(&src_image, &mut dest_image, None)
        .expect(&format!("Error while resizing {:?}", &args.path));

    let outfile = File::create(&args.out_path).expect(&format!(
        "Error creating output file at {:?}",
        &args.out_path
    ));
    AvifEncoder::new_with_speed_quality(outfile, 10, 100)
        .write_image(
            dest_image.buffer(),
            dest_image.width(),
            dest_image.height(),
            src_image.color().into(),
        )
        .expect(&format!("Error while encoding {:?}", &args.out_path));

    println!("{} {}", resized_width, resized_height);
}
