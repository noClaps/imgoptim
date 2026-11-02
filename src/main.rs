mod cache;
mod jpeg;
mod optimise;
mod png;
mod svg;

use std::{
    ffi::OsStr,
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

use argp::FromArgs;

/// Losslessly compress PNG, JPEG and SVG images at maximum effort.
#[derive(FromArgs)]
struct Args {
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

    /// Report how much each image would shrink, without changing any files.
    #[argp(switch, short = 'n')]
    dry_run: bool,

    /// Optimise every image, even ones an earlier run already optimised. Normally those are
    /// remembered in ~/.cache/imgoptim and skipped.
    #[argp(switch)]
    no_cache: bool,

    /// Images, or directories with -r, to optimise.
    #[argp(positional)]
    paths: Vec<PathBuf>,
}

fn parse_iterations(value: &str) -> Result<NonZeroU64, String> {
    value
        .parse()
        .map_err(|_| "expected a whole number of at least 1".to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
    Svg,
}

impl Format {
    fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(OsStr::to_str)?;
        [
            ("png", Self::Png),
            ("jpg", Self::Jpeg),
            ("jpeg", Self::Jpeg),
            ("svg", Self::Svg),
        ]
        .into_iter()
        .find(|(ext, _)| extension.eq_ignore_ascii_case(ext))
        .map(|(_, format)| format)
    }
}

fn detect(path: &Path, data: &[u8]) -> Option<Format> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if Format::from_extension(path) == Some(Format::Svg)
        || data.trim_ascii_start().starts_with(b"<")
    {
        Some(Format::Svg)
    } else {
        None
    }
}

fn main() -> ExitCode {
    let args: Args = argp::parse_args_or_exit(argp::DEFAULT);
    optimise::run(&args)
}
