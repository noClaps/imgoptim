//! Run the imgoptim binary on temporary folders, checking its output, exit codes and files.

use std::{
    env, fs,
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use mozjpeg::{ColorSpace, Compress};

/// A temporary folder for one test, with its own cache directory inside, deleted afterwards.
struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Deref for Folder {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

fn folder(name: &str) -> Folder {
    let folder = env::temp_dir().join(format!("imgoptim-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(folder.join("cache")).unwrap();
    Folder(folder)
}

fn run(folder: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_imgoptim"))
        .args(args)
        .args(["-i", "1"])
        .current_dir(folder)
        .env("XDG_CACHE_HOME", folder.join("cache"))
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A gradient PNG written without any optimisation, so there's always something to save.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the channel values are all at most 255"
)]
fn png() -> Vec<u8> {
    let pixels: Vec<u8> = (0..64u32 * 64)
        .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 128])
        .collect();
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, 64, 64);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_compression(png::Compression::NoCompression);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    data
}

/// A baseline JPEG with libjpeg's default Huffman tables, which optimising improves on.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the channel values are all at most 255"
)]
fn jpeg() -> Vec<u8> {
    let pixels: Vec<u8> = (0..96u32 * 64)
        .flat_map(|i| [(i % 96 * 2) as u8, (i / 96 * 3) as u8, (i % 7 * 30) as u8])
        .collect();
    let mut compress = Compress::new(ColorSpace::JCS_RGB);
    compress.set_fastest_defaults();
    compress.set_size(96, 64);
    compress.set_quality(90.0);
    let mut started = compress.start_compress(Vec::new()).unwrap();
    started.write_scanlines(&pixels).unwrap();
    started.finish().unwrap()
}

const SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!-- Generator: Some Editor -->
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
    <metadata>editor junk</metadata>
    <g>
        <rect x="10.000000" y="10.000000" width="80.000000" height="80.000000" fill="#ff0000"/>
    </g>
</svg>
"##;

/// Write one image of each format into `folder/images`, some in a subfolder.
fn images(folder: &Path) {
    fs::create_dir_all(folder.join("images/sub")).unwrap();
    fs::write(folder.join("images/a.png"), png()).unwrap();
    fs::write(folder.join("images/sub/b.jpg"), jpeg()).unwrap();
    fs::write(folder.join("images/sub/c.svg"), SVG).unwrap();
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

#[test]
fn optimises_files_in_place() {
    let folder = folder("in-place");
    images(&folder);
    let before = size(&folder.join("images/a.png"));

    let output = run(
        &folder,
        &["images/a.png", "images/sub/b.jpg", "images/sub/c.svg"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    for name in ["images/a.png", "images/sub/b.jpg", "images/sub/c.svg"] {
        assert!(stdout.contains(&format!("{name}: ")), "{stdout}");
    }
    let summary = stdout.lines().last().unwrap();
    assert!(
        summary.starts_with("3 images: 3 optimised. Saved "),
        "{stdout}"
    );
    assert!(size(&folder.join("images/a.png")) < before);
}

#[test]
fn mirrors_folders_into_the_output() {
    let folder = folder("mirror");
    images(&folder);
    let original = fs::read(folder.join("images/sub/b.jpg")).unwrap();

    let output = run(&folder, &["-r", "images", "-o", "out"]);
    assert!(output.status.success(), "{}", stderr(&output));
    for name in ["a.png", "sub/b.jpg", "sub/c.svg"] {
        assert!(folder.join("out").join(name).is_file(), "{name} missing");
    }
    assert_eq!(fs::read(folder.join("images/sub/b.jpg")).unwrap(), original);
}

#[test]
fn rejects_bad_arguments() {
    let folder = folder("arguments");
    images(&folder);

    let output = run(&folder, &["images"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("is a directory (use -r"),
        "{}",
        stderr(&output)
    );

    let output = run(&folder, &["images/a.png", "images/sub/b.jpg", "-o", "out"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("--output can only be used"));

    let output = Command::new(env!("CARGO_BIN_EXE_imgoptim"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn reports_broken_files_and_carries_on() {
    let folder = folder("broken");
    images(&folder);
    fs::write(folder.join("images/broken.png"), b"not an image").unwrap();

    let output = run(&folder, &["-r", "images"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("broken.png: not a PNG, JPEG or SVG"));
    let summary = stdout(&output).lines().last().unwrap().to_owned();
    assert!(
        summary.starts_with("4 images: 3 optimised, 1 failed."),
        "{summary}"
    );
}

#[test]
fn processes_repeated_files_once() {
    let folder = folder("repeated");
    images(&folder);

    let output = run(&folder, &["images/a.png", "./images/a.png"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output).matches("a.png: ").count(),
        1,
        "{}",
        stdout(&output)
    );
}

#[test]
fn skips_images_already_optimised() {
    let folder = folder("cache");
    images(&folder);

    assert!(run(&folder, &["-r", "images"]).status.success());
    let second = run(&folder, &["-r", "images"]);
    assert_eq!(
        stdout(&second)
            .lines()
            .filter(|line| line.ends_with(": already optimised"))
            .count(),
        3,
        "{}",
        stdout(&second)
    );

    let uncached = run(&folder, &["-r", "images", "--no-cache"]);
    assert!(
        !stdout(&uncached).contains(": already optimised\n"),
        "{}",
        stdout(&uncached)
    );
    assert!(
        stdout(&uncached).contains("3 already optimal"),
        "{}",
        stdout(&uncached)
    );
}

#[test]
fn dry_run_changes_nothing() {
    let folder = folder("dry-run");
    images(&folder);
    let original = fs::read(folder.join("images/a.png")).unwrap();

    let output = run(&folder, &["-n", "-r", "images"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Would save"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fs::read(folder.join("images/a.png")).unwrap(), original);
    assert!(fs::read_dir(folder.join("cache")).unwrap().next().is_none());
}
