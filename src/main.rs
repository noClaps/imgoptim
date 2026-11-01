mod jpeg;
mod png;
mod svg;

use std::{
    fs::{read, write},
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

use argp::FromArgs;

/// Losslessly compress SVG, PNG and JPEG images at maximum effort.
#[derive(FromArgs)]
struct Args {
    /// Write the result here instead of overwriting the input. Only valid with a single input.
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

    /// Images to optimise.
    #[argp(positional)]
    paths: Vec<PathBuf>,
}

fn parse_iterations(value: &str) -> Result<NonZeroU64, String> {
    value
        .parse()
        .map_err(|_| "expected a whole number of at least 1".to_owned())
}

enum Format {
    Png,
    Jpeg,
    Svg,
}

fn detect(path: &Path, data: &[u8]) -> Option<Format> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
        || data.trim_ascii_start().starts_with(b"<")
    {
        Some(Format::Svg)
    } else {
        None
    }
}

fn optimise(path: &Path, output: &Path, zopfli_iterations: NonZeroU64) -> Result<(), String> {
    let data = read(path).map_err(|e| e.to_string())?;
    let optimised = match detect(path, &data) {
        Some(Format::Png) => png::optimise(&data, zopfli_iterations)?,
        Some(Format::Jpeg) => jpeg::optimise(&data)?,
        Some(Format::Svg) => svg::optimise(&data)?,
        None => return Err("not a PNG, JPEG or SVG".to_owned()),
    };

    let before = data.len();
    let result = if optimised.len() < before {
        optimised
    } else {
        data
    };
    let after = result.len();

    if after < before || output != path {
        write(output, &result).map_err(|e| e.to_string())?;
    }
    println!(
        "{}: {before} → {after} bytes ({:.2}%)",
        path.display(),
        (after as f64 - before as f64) / before as f64 * 100.0
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Args = argp::parse_args_or_exit(argp::DEFAULT);
    if args.paths.is_empty() {
        eprintln!("No images given. Run with --help for usage.");
        return ExitCode::FAILURE;
    }
    if args.output.is_some() && args.paths.len() > 1 {
        eprintln!("--output can only be used with a single input.");
        return ExitCode::FAILURE;
    }

    let mut status = ExitCode::SUCCESS;
    for path in &args.paths {
        let output = args.output.as_deref().unwrap_or(path);
        if let Err(e) = optimise(path, output, args.zopfli_iterations) {
            eprintln!("{}: {e}", path.display());
            status = ExitCode::FAILURE;
        }
    }
    status
}
