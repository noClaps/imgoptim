use std::{
    fs::{create_dir_all, read, read_dir, write},
    io::{self, Write},
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

use crate::{Format, OptimiseArgs, detect, jpeg, png, svg};

/// Collect every file under `dir` with a supported extension, in a stable order. Symlinked
/// directories aren't followed, so links can't cause loops.
fn find_images(dir: &Path, images: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            find_images(&path, images)?;
        } else if Format::from_extension(&path).is_some() {
            images.push(path);
        }
    }
    Ok(())
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
    // Ignore failures, e.g. when piped into `head`, so the remaining images still get optimised.
    let _ = writeln!(
        io::stdout(),
        "{}: {before} → {after} bytes ({:.2}%)",
        path.display(),
        (after as f64 - before as f64) / before as f64 * 100.0
    );
    Ok(())
}

pub fn run(args: OptimiseArgs) -> ExitCode {
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
