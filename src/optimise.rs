use std::{
    collections::HashSet,
    fs::{
        DirEntry, OpenOptions, canonicalize, create_dir_all, metadata, read, read_dir, remove_file,
        rename, write,
    },
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{Args, Format, cache::Cache, detect, jpeg, png, svg};

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
/// replaced rather than the link. Outputs that aren't regular files, like `/dev/null` or a pipe,
/// are written directly, since they can't be replaced.
fn write_atomically(path: &Path, data: &[u8], like: &Path) -> io::Result<()> {
    let path = canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if metadata(&path).is_ok_and(|existing| !existing.is_file()) {
        return write(&path, data);
    }
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

/// What optimising an image did.
enum Outcome {
    /// It was optimised, or couldn't be improved: its sizes in bytes before and after.
    Optimised { before: usize, after: usize },
    /// An earlier run already optimised it, so it was skipped. Its size in bytes.
    Cached { size: usize },
}

fn optimise(
    path: &Path,
    output: &Path,
    args: &Args,
    cache: Option<&Cache>,
) -> Result<Outcome, String> {
    let data = read(path).map_err(|e| e.to_string())?;
    let format = detect(path, &data).ok_or("not a PNG, JPEG or SVG")?;
    if cache.is_some_and(|cache| cache.contains(format, &data)) {
        if output != path && !args.dry_run {
            write_atomically(output, &data, path).map_err(|e| e.to_string())?;
        }
        return Ok(Outcome::Cached { size: data.len() });
    }

    let optimised = match format {
        Format::Png => png::optimise(&data, args.zopfli_iterations)?,
        Format::Jpeg => jpeg::optimise(&data)?,
        Format::Svg => svg::optimise(&data, path)?,
    };

    let before = data.len();
    let result = if optimised.len() < before {
        // Check the result against the original before trusting it with the image.
        let same = match format {
            Format::Jpeg => jpeg::same_image(&data, &optimised)?,
            Format::Png => png::same_image(&data, &optimised)?,
            Format::Svg => true,
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

    if args.dry_run {
        return Ok(Outcome::Optimised { before, after });
    }
    if after < before || output != path {
        write_atomically(output, &result, path).map_err(|e| e.to_string())?;
    }
    if let Some(cache) = cache
        && let Err(e) = cache.insert(format, &result)
    {
        eprintln!("Couldn't update the cache: {e}");
    }
    Ok(Outcome::Optimised { before, after })
}

/// Totals across all the images, for the summary.
#[derive(Clone, Copy, Default)]
struct Tally {
    optimised: usize,
    unchanged: usize,
    cached: usize,
    failed: usize,
    before: u64,
    after: u64,
}

impl Tally {
    fn add(self, other: Self) -> Self {
        Self {
            optimised: self.optimised + other.optimised,
            unchanged: self.unchanged + other.unchanged,
            cached: self.cached + other.cached,
            failed: self.failed + other.failed,
            before: self.before + other.before,
            after: self.after + other.after,
        }
    }
}

/// The percentage `after` is smaller or larger than `before`.
#[expect(
    clippy::cast_precision_loss,
    reason = "f64 holds file sizes exactly up to 8 PiB, and only two decimals are shown"
)]
fn change(before: u64, after: u64) -> f64 {
    if before == 0 {
        0.0
    } else {
        (after as f64 - before as f64) / before as f64 * 100.0
    }
}

/// A size in bytes, kilobytes or megabytes, whichever reads best.
#[expect(
    clippy::cast_precision_loss,
    reason = "f64 holds file sizes exactly up to 8 PiB, and only one decimal is shown"
)]
fn readable_size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{:.1} kB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// Print how optimising an image went: its sizes to stdout, or its error to stderr. Returns its
/// contribution to the totals.
fn report(path: &Path, result: Result<Outcome, String>) -> Tally {
    // Ignore failures to print, e.g. when piped into `head`, so the remaining images still get
    // optimised.
    match result {
        Ok(Outcome::Cached { size }) => {
            let _ = writeln!(io::stdout(), "{}: already optimised", path.display());
            let size = size as u64;
            Tally {
                cached: 1,
                before: size,
                after: size,
                ..Tally::default()
            }
        }
        Ok(Outcome::Optimised { before, after }) => {
            let (before, after) = (before as u64, after as u64);
            let _ = writeln!(
                io::stdout(),
                "{}: {before} → {after} bytes ({:.2}%)",
                path.display(),
                change(before, after),
            );
            Tally {
                optimised: usize::from(after < before),
                unchanged: usize::from(after >= before),
                before,
                after,
                ..Tally::default()
            }
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            Tally {
                failed: 1,
                ..Tally::default()
            }
        }
    }
}

/// One line totalling the run, e.g. "12 images: 9 optimised, 2 already optimal, 1 failed. Saved
/// 1.4 MB of 3.2 MB (-43.75%)." A dry run says what would have been saved.
fn summary(tally: Tally, dry_run: bool) -> String {
    let images = tally.optimised + tally.unchanged + tally.cached + tally.failed;
    let mut counts = vec![format!("{} optimised", tally.optimised)];
    if tally.unchanged > 0 {
        counts.push(format!("{} already optimal", tally.unchanged));
    }
    if tally.cached > 0 {
        counts.push(format!("{} skipped as already optimised", tally.cached));
    }
    if tally.failed > 0 {
        counts.push(format!("{} failed", tally.failed));
    }
    format!(
        "{images} image{}: {}. {} {} of {} ({:.2}%).",
        if images == 1 { "" } else { "s" },
        counts.join(", "),
        if dry_run { "Would save" } else { "Saved" },
        readable_size(tally.before.saturating_sub(tally.after)),
        readable_size(tally.before),
        change(tally.before, tally.after),
    )
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

    let cache = if args.no_cache {
        None
    } else {
        Cache::open(args.zopfli_iterations)
            .inspect_err(|e| eprintln!("Not using the cache: {e}"))
            .ok()
    };

    // Optimise the images in parallel, sharing oxipng's thread pool, reporting each as soon as
    // it's done. Each report is one locked write, so lines don't interleave.
    let tally = images
        .par_iter()
        .map(|(path, output)| {
            let result = optimise(path, output, args, cache.as_ref());
            report(path, result)
        })
        .reduce(Tally::default, Tally::add);
    if tally.failed > 0 {
        status = ExitCode::FAILURE;
    }
    if !images.is_empty() {
        let _ = writeln!(io::stdout(), "{}", summary(tally, args.dry_run));
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

    /// A temporary directory for one test, deleted afterwards.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    fn scratch(name: &str) -> Scratch {
        let directory =
            std::env::temp_dir().join(format!("imgoptim-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        create_dir_all(&directory).unwrap();
        Scratch(directory)
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
        assert_eq!(read_dir(&*directory).unwrap().count(), 1);
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
    fn summarises_the_run() {
        let tally = Tally {
            optimised: 9,
            unchanged: 2,
            cached: 0,
            failed: 1,
            before: 3_200_000,
            after: 1_800_000,
        };
        assert_eq!(
            summary(tally, false),
            "12 images: 9 optimised, 2 already optimal, 1 failed. Saved 1.4 MB of 3.2 MB (-43.75%)."
        );
        assert!(summary(tally, true).contains(". Would save 1.4 MB of 3.2 MB"));
        let one = Tally {
            cached: 1,
            before: 500,
            after: 500,
            ..Tally::default()
        };
        assert_eq!(
            summary(one, false),
            "1 image: 0 optimised, 1 skipped as already optimised. Saved 0 bytes of 500 bytes (0.00%)."
        );
    }

    #[test]
    fn writes_to_special_files_directly() {
        write_atomically(Path::new("/dev/null"), b"discarded", Path::new("/dev/null")).unwrap();
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
