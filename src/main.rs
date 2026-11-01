mod avif;
mod convert;
mod jpeg;
mod optimise;
mod png;
mod svg;

use std::{
    ffi::OsStr,
    num::{NonZeroU32, NonZeroU64},
    path::{Path, PathBuf},
    process::ExitCode,
};

use argp::FromArgs;

/// Optimise and convert PNG, JPEG, SVG and AVIF images.
#[derive(FromArgs)]
struct Args {
    #[argp(subcommand)]
    command: Command,
}

#[derive(FromArgs)]
#[argp(subcommand)]
enum Command {
    Optimise(OptimiseArgs),
    Convert(ConvertArgs),
}

/// Losslessly compress PNG, JPEG and SVG images at maximum effort. AVIF files are left as they are,
/// since they can't be recompressed losslessly.
#[derive(FromArgs)]
#[argp(subcommand, name = "optimise")]
struct OptimiseArgs {
    /// Write the result here instead of overwriting the input. Only valid with a single input. For
    /// a directory with -r, the input's directory structure is recreated under this path.
    #[argp(option, short = 'o')]
    output: Option<PathBuf>,

    /// Zopfli iterations per PNG trial (default: 15). Higher is slower and rarely smaller.
    #[argp(
        option,
        short = 'i',
        default = "NonZeroU64::new(15).unwrap()",
        from_str_fn(parse_iterations)
    )]
    zopfli_iterations: NonZeroU64,

    /// Optimise every supported image in the given directories and their subdirectories.
    #[argp(switch, short = 'r')]
    recursive: bool,

    /// Images, or directories with -r, to optimise.
    #[argp(positional)]
    paths: Vec<PathBuf>,
}

/// Convert an image between PNG, JPEG and AVIF, optionally resizing it. The output format
/// comes from the output path's extension.
#[derive(FromArgs)]
#[argp(subcommand, name = "convert")]
struct ConvertArgs {
    /// Output width in pixels. Without --height, the height keeps the original aspect ratio.
    #[argp(option, short = 'W', from_str_fn(parse_dimension))]
    width: Option<NonZeroU32>,

    /// Output height in pixels. Without --width, the width keeps the original aspect ratio.
    #[argp(option, short = 'H', from_str_fn(parse_dimension))]
    height: Option<NonZeroU32>,

    /// Quality from 1 to 100 for lossy output: JPEG and AVIF (default: 90).
    #[argp(option, short = 'q', default = "90", from_str_fn(parse_quality))]
    quality: u8,

    /// Zopfli iterations per PNG trial (default: 15). Higher is slower and rarely smaller.
    #[argp(
        option,
        short = 'i',
        default = "NonZeroU64::new(15).unwrap()",
        from_str_fn(parse_iterations)
    )]
    zopfli_iterations: NonZeroU64,

    /// Image to convert.
    #[argp(positional)]
    input: PathBuf,

    /// Where to write the converted image, ending in .png, .jpg, .jpeg or .avif.
    #[argp(positional)]
    output: PathBuf,
}

fn parse_iterations(value: &str) -> Result<NonZeroU64, String> {
    value
        .parse()
        .map_err(|_| "expected a whole number of at least 1".to_owned())
}

fn parse_dimension(value: &str) -> Result<NonZeroU32, String> {
    value
        .parse()
        .map_err(|_| "expected a whole number of pixels, at least 1".to_owned())
}

fn parse_quality(value: &str) -> Result<u8, String> {
    value
        .parse()
        .ok()
        .filter(|quality| (1..=100).contains(quality))
        .ok_or_else(|| "expected a whole number from 1 to 100".to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
    Svg,
    Avif,
}

impl Format {
    fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(OsStr::to_str)?;
        [
            ("png", Self::Png),
            ("jpg", Self::Jpeg),
            ("jpeg", Self::Jpeg),
            ("svg", Self::Svg),
            ("avif", Self::Avif),
        ]
        .into_iter()
        .find(|(ext, _)| extension.eq_ignore_ascii_case(ext))
        .map(|(_, format)| format)
    }
}

/// Whether an ISO media file's `ftyp` box lists an AVIF brand.
fn is_avif(data: &[u8]) -> bool {
    if data.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let Some(size) = data.get(..4).and_then(|size| size.try_into().ok()) else {
        return false;
    };
    let size = u32::from_be_bytes(size) as usize;
    let Some(ftyp) = data.get(8..size.min(data.len())) else {
        return false;
    };
    // The major brand, then the compatible brands after the minor version.
    let brands: Vec<&[u8]> = ftyp
        .get(..4)
        .into_iter()
        .chain(
            ftyp.get(8..)
                .unwrap_or_default()
                .as_chunks::<4>()
                .0
                .iter()
                .map(<[u8; 4]>::as_slice),
        )
        .collect();
    brands
        .iter()
        .any(|brand| matches!(*brand, b"avif" | b"avis"))
}

fn detect(path: &Path, data: &[u8]) -> Option<Format> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if is_avif(data) {
        Some(Format::Avif)
    } else if matches!(Format::from_extension(path), Some(Format::Svg))
        || data.trim_ascii_start().starts_with(b"<")
    {
        Some(Format::Svg)
    } else {
        None
    }
}

fn main() -> ExitCode {
    let args: Args = argp::parse_args_or_exit(argp::DEFAULT);
    match args.command {
        Command::Optimise(args) => optimise::run(&args),
        Command::Convert(args) => {
            let settings = convert::Settings {
                width: args.width,
                height: args.height,
                quality: args.quality,
                zopfli_iterations: args.zopfli_iterations,
            };
            match convert::convert(&args.input, &args.output, &settings) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{}: {e}", args.input.display());
                    ExitCode::FAILURE
                }
            }
        }
    }
}
