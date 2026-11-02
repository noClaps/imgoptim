use std::{
    collections::HashSet,
    fs::{
        DirEntry, OpenOptions, canonicalize, create_dir_all, metadata, read, read_dir, remove_file,
        rename,
    },
    io::{self, Write},
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{Args, Format, detect, jpeg, png, svg};

/// Collect every file under `dir` with a supported extension, in a stable order. Symlinked
/// directories aren't followed, so links can't cause loops.
fn find_images(dir: &Path, images: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(DirEntry::file_name);
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

/// Replace `path` with `data` all at once: write a temporary file beside it, flush it to disk, then
/// rename it into place. An interruption never leaves a partly written image. The new file keeps
/// the permissions of the file it replaces, or of `like` when it's new, and a symlink's target is
/// replaced rather than the link.
fn write_atomically(path: &Path, data: &[u8], like: &Path) -> io::Result<()> {
    let path = canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_dir_all(directory)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("the output path has no file name"))?;
    let temporary = directory.join(format!(
        ".{}.imgoptim-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let permissions = metadata(&path).or_else(|_| metadata(like))?.permissions();

    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(data)?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        rename(&temporary, &path)
    })();
    if written.is_err() {
        let _ = remove_file(&temporary);
    }
    written
}

/// An image's size in bytes before and after optimising.
struct Sizes {
    before: usize,
    after: usize,
}

fn optimise(path: &Path, output: &Path, zopfli_iterations: NonZeroU64) -> Result<Sizes, String> {
    let data = read(path).map_err(|e| e.to_string())?;
    let format = detect(path, &data).ok_or("not a PNG, JPEG or SVG")?;
    let optimised = match format {
        Format::Png => png::optimise(&data, zopfli_iterations)?,
        Format::Jpeg => jpeg::optimise(&data)?,
        Format::Svg => svg::optimise(&data)?,
    };

    let before = data.len();
    let result = if optimised.len() < before {
        // Check the result against the original before trusting it with the image.
        let same = match format {
            Format::Jpeg => jpeg::same_image(&data, &optimised)?,
            Format::Png | Format::Svg => true,
        };
        if !same {
            return Err(
                "the optimised image didn't match the original, so it was left unchanged"
                    .to_owned(),
            );
        }
        optimised
    } else {
        data
    };
    let after = result.len();

    if after < before || output != path {
        write_atomically(output, &result, path).map_err(|e| e.to_string())?;
    }
    Ok(Sizes { before, after })
}

/// Print how optimising an image went: its sizes to stdout, or its error to stderr. Returns
/// whether it succeeded.
fn report(path: &Path, result: Result<Sizes, String>) -> bool {
    match result {
        Ok(Sizes { before, after }) => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "f64 holds file sizes exactly up to 8 PiB, and only two decimals are shown"
            )]
            let change = (after as f64 - before as f64) / before as f64 * 100.0;
            // Ignore failures, e.g. when piped into `head`, so the remaining images still get
            // optimised.
            let _ = writeln!(
                io::stdout(),
                "{}: {before} → {after} bytes ({change:.2}%)",
                path.display(),
            );
            true
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            false
        }
    }
}

pub fn run(args: &Args) -> ExitCode {
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

    // The same file named twice, or inside a directory also given, would be written by two
    // threads at once.
    let mut seen = HashSet::new();
    images.retain(|(path, _)| seen.insert(canonicalize(path).unwrap_or_else(|_| path.clone())));

    // Optimise the images in parallel, sharing oxipng's thread pool, reporting each as soon as
    // it's done. Each report is one locked write, so lines don't interleave.
    let failures = images
        .par_iter()
        .map(|(path, output)| report(path, optimise(path, output, args.zopfli_iterations)))
        .filter(|&succeeded| !succeeded)
        .count();
    if failures > 0 {
        status = ExitCode::FAILURE;
    }
    status
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        os::unix::fs::{PermissionsExt, symlink},
    };

    use super::*;

    /// A new, empty directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("imgoptim-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn replaces_contents_and_keeps_permissions() {
        let directory = scratch("atomic");
        let image = directory.join("image.png");
        fs::write(&image, b"old").unwrap();
        fs::set_permissions(&image, Permissions::from_mode(0o640)).unwrap();

        write_atomically(&image, b"new", &image).unwrap();
        assert_eq!(fs::read(&image).unwrap(), b"new");
        assert_eq!(
            metadata(&image).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // No temporary file is left behind.
        assert_eq!(read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn creates_missing_directories_with_the_inputs_permissions() {
        let directory = scratch("atomic-new");
        let input = directory.join("input.png");
        fs::write(&input, b"input").unwrap();
        fs::set_permissions(&input, Permissions::from_mode(0o600)).unwrap();

        let output = directory.join("nested/output.png");
        write_atomically(&output, b"output", &input).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"output");
        assert_eq!(
            metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn writes_through_symlinks() {
        let directory = scratch("atomic-link");
        let target = directory.join("target.png");
        let link = directory.join("link.png");
        fs::write(&target, b"old").unwrap();
        symlink(&target, &link).unwrap();

        write_atomically(&link, b"new", &link).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }
}
