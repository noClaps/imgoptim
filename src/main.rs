mod avif;
mod convert;
mod jpeg;
mod jxl;
mod optimise;
mod png;
mod svg;
mod webp;

use std::{
    ffi::OsStr,
    num::{NonZeroU32, NonZeroU64},
    path::{Path, PathBuf},
    process::ExitCode,
};

use argp::FromArgs;

/// Optimise and convert PNG, JPEG, SVG, WebP, AVIF, HEIF and JPEG XL images.
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

/// Losslessly compress PNG, JPEG, SVG, WebP and JPEG XL images at maximum effort. AVIF and HEIF
/// files are left as they are, since they can't be recompressed losslessly.
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

/// Convert an image between PNG, JPEG, SVG, WebP, AVIF, HEIF and JPEG XL, optionally resizing it.
/// The output format comes from the output path's extension.
#[derive(FromArgs)]
#[argp(subcommand, name = "convert")]
struct ConvertArgs {
    /// Output width in pixels. Without --height, the height keeps the original aspect ratio.
    #[argp(option, short = 'W', from_str_fn(parse_dimension))]
    width: Option<NonZeroU32>,

    /// Output height in pixels. Without --width, the width keeps the original aspect ratio.
    #[argp(option, short = 'H', from_str_fn(parse_dimension))]
    height: Option<NonZeroU32>,

    /// Quality from 1 to 100 for lossy output: JPEG, WebP, AVIF, HEIF and JPEG XL (default: 85).
    #[argp(option, short = 'q', default = "85", from_str_fn(parse_quality))]
    quality: u8,

    /// Store WebP and JPEG XL output losslessly, ignoring --quality. A JPEG converted to JPEG XL
    /// without resizing is recompressed bit-exactly, so the original JPEG can be recovered.
    #[argp(switch, short = 'l')]
    lossless: bool,

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

    /// Where to write the converted image, ending in .png, .jpg, .jpeg, .svg, .webp, .avif,
    /// .heic, .heif or .jxl.
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
    Webp,
    Avif,
    Heif,
    Jxl,
}

impl Format {
    fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(OsStr::to_str)?;
        [
            ("png", Self::Png),
            ("jpg", Self::Jpeg),
            ("jpeg", Self::Jpeg),
            ("svg", Self::Svg),
            ("webp", Self::Webp),
            ("avif", Self::Avif),
            ("heic", Self::Heif),
            ("heif", Self::Heif),
            ("jxl", Self::Jxl),
        ]
        .into_iter()
        .find(|(ext, _)| extension.eq_ignore_ascii_case(ext))
        .map(|(_, format)| format)
    }
}

/// Tell AVIF from HEIF by the brands in an ISO media file's `ftyp` box.
fn isobmff_format(data: &[u8]) -> Option<Format> {
    if data.get(4..8)? != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let ftyp = data.get(8..size.min(data.len()))?;
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
    let has = |names: &[&[u8; 4]]| {
        brands
            .iter()
            .any(|brand| names.iter().any(|name| *brand == *name))
    };
    if has(&[b"avif", b"avis"]) {
        Some(Format::Avif)
    } else if has(&[
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1",
    ]) {
        Some(Format::Heif)
    } else {
        None
    }
}

fn detect(path: &Path, data: &[u8]) -> Option<Format> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if data.get(..4) == Some(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some(Format::Webp)
    } else if data.starts_with(&[0xFF, 0x0A]) || data.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        Some(Format::Jxl)
    } else if let Some(format) = isobmff_format(data) {
        Some(format)
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
                lossless: args.lossless,
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
