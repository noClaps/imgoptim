mod jpeg;
mod png;
mod svg;

use std::{
    ffi::OsStr,
    fs::{create_dir_all, read, read_dir, write},
    io,
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

use argp::FromArgs;

/// Losslessly compress SVG, PNG and JPEG images at maximum effort.
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

    /// Optimise every PNG, JPEG and SVG in the given directories and their subdirectories.
    #[argp(switch, short = 'r')]
    recursive: bool,

    /// Images, or directories with -r, to optimise.
    #[argp(positional)]
    paths: Vec<PathBuf>,
}

const EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "svg"];

/// Collect every file under `dir` with a supported extension, in a stable order. Symlinked
/// directories aren't followed, so links can't cause loops.
fn find_images(dir: &Path, images: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            find_images(&path, images)?;
        } else if path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
        {
            images.push(path);
        }
    }
    Ok(())
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

    if output != path
        && let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty())
    {
        create_dir_all(parent).map_err(|e| e.to_string())?;
    }
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
        eprintln!("--output can only be used with a single file or directory.");
        return ExitCode::FAILURE;
    }

    let mut status = ExitCode::SUCCESS;
    // Pairs of (input, output) paths.
    let mut images = Vec::new();
    for path in &args.paths {
        if !path.is_dir() {
            let output = args.output.clone().unwrap_or_else(|| path.clone());
            images.push((path.clone(), output));
            continue;
        }
        if !args.recursive {
            eprintln!(
                "{}: is a directory (use -r to optimise its images)",
                path.display()
            );
            status = ExitCode::FAILURE;
            continue;
        }

        let mut found = Vec::new();
        if let Err(e) = find_images(path, &mut found) {
            eprintln!("{}: {e}", path.display());
            status = ExitCode::FAILURE;
            continue;
        }
        // With an output directory, mirror each image's location relative to the input directory.
        images.extend(found.into_iter().map(|image| {
            let output = match &args.output {
                Some(dir) => dir.join(image.strip_prefix(path).expect("found under path")),
                None => image.clone(),
            };
            (image, output)
        }));
    }

    for (path, output) in &images {
        if let Err(e) = optimise(path, output, args.zopfli_iterations) {
            eprintln!("{}: {e}", path.display());
            status = ExitCode::FAILURE;
        }
    }
    status
}
