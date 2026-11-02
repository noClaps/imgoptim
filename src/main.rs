mod cache;
mod jpeg;
mod optimise;
mod png;
mod svg;

use std::{
    ffi::{OsStr, OsString},
    num::NonZeroU64,
    path::{Path, PathBuf},
    process::ExitCode,
};

const HELP: &str = "\
Usage: imgoptim [-o <output>] [-i <zopfli-iterations>] [-r] [-n] [--no-cache]
                [<paths...>]

Losslessly compress PNG, JPEG and SVG images at maximum effort.

Arguments:
  paths                  Images, or directories with -r, to optimise.

Options:
  -o, --output <output>  Write the result here instead of overwriting the input.
                         Only valid with a single input. For a directory with
                         -r, the input's directory structure is recreated under
                         this path.
  -i, --zopfli-iterations <zopfli-iterations>
                         Zopfli iterations per PNG trial (default: 15). Higher
                         is slower and rarely smaller.
  -r, --recursive        Optimise every supported image in the given directories
                         and their subdirectories.
  -n, --dry-run          Report how much each image would shrink, without
                         changing any files.
      --no-cache         Optimise every image, even ones an earlier run already
                         optimised. Normally those are remembered in
                         ~/.cache/imgoptim and skipped.
  -h, --help             Show this help message and exit.";

/// The command-line options, described in [`HELP`].
#[derive(Debug, PartialEq, Eq)]
struct Args {
    output: Option<PathBuf>,
    zopfli_iterations: NonZeroU64,
    recursive: bool,
    dry_run: bool,
    no_cache: bool,
    paths: Vec<PathBuf>,
}

/// Parse the command-line arguments after the program name. `None` means help was asked for.
fn parse_args(mut arguments: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let (mut output, mut zopfli_iterations) = (None, None);
    let (mut recursive, mut dry_run, mut no_cache) = (false, false, false);
    let mut paths = Vec::new();
    while let Some(argument) = arguments.next() {
        let Some(flag) = argument
            .to_str()
            .filter(|a| a.starts_with('-') && *a != "-")
        else {
            paths.push(argument.into());
            continue;
        };
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("Option '{flag}' requires a value."))
        };
        let once = |seen: bool| {
            if seen {
                Err(format!("Option '{flag}' can only be used once."))
            } else {
                Ok(())
            }
        };
        match flag {
            "-h" | "--help" => return Ok(None),
            "-o" | "--output" => {
                once(output.is_some())?;
                output = Some(value()?.into());
            }
            "-i" | "--zopfli-iterations" => {
                once(zopfli_iterations.is_some())?;
                let value = value()?;
                let parsed = value.to_str().and_then(|v| v.parse().ok()).ok_or_else(|| {
                    format!(
                        "Error parsing option '{flag}' with value '{}': expected a whole number \
                         of at least 1.",
                        value.display()
                    )
                })?;
                zopfli_iterations = Some(parsed);
            }
            "-r" | "--recursive" => recursive = true,
            "-n" | "--dry-run" => dry_run = true,
            "--no-cache" => no_cache = true,
            "--" => {
                paths.extend(arguments.map(PathBuf::from));
                break;
            }
            _ => return Err(format!("Unrecognized argument: {flag}")),
        }
    }
    Ok(Some(Args {
        output,
        zopfli_iterations: zopfli_iterations.unwrap_or(NonZeroU64::new(15).unwrap()),
        recursive,
        dry_run,
        no_cache,
        paths,
    }))
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
    match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(args)) => optimise::run(&args),
        Ok(None) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}\nRun imgoptim --help for more information.");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Option<Args>, String> {
        parse_args(arguments.iter().map(OsString::from))
    }

    #[test]
    fn parses_every_option() {
        let args = parse(&[
            "-r",
            "a",
            "--output",
            "out",
            "-n",
            "--no-cache",
            "-i",
            "3",
            "b",
        ]);
        assert_eq!(
            args,
            Ok(Some(Args {
                output: Some("out".into()),
                zopfli_iterations: NonZeroU64::new(3).unwrap(),
                recursive: true,
                dry_run: true,
                no_cache: true,
                paths: vec!["a".into(), "b".into()],
            }))
        );
    }

    #[test]
    fn defaults_to_fifteen_iterations() {
        let args = parse(&["a"]).unwrap().unwrap();
        assert_eq!(args.zopfli_iterations.get(), 15);
    }

    #[test]
    fn treats_everything_after_a_double_dash_as_paths() {
        let args = parse(&["--", "-r", "-"]).unwrap().unwrap();
        assert!(!args.recursive);
        assert_eq!(args.paths, [PathBuf::from("-r"), PathBuf::from("-")]);
    }

    #[test]
    fn handles_arguments_in_order_up_to_help() {
        assert_eq!(
            parse(&["--bogus-later", "-h"]),
            Err("Unrecognized argument: --bogus-later".into())
        );
        assert_eq!(parse(&["a", "--help", "--bogus"]), Ok(None));
    }

    #[test]
    fn rejects_bad_options() {
        let error = |arguments| parse(arguments).unwrap_err();
        assert_eq!(error(&["-x"]), "Unrecognized argument: -x");
        assert_eq!(error(&["-o"]), "Option '-o' requires a value.");
        assert_eq!(
            error(&["-o", "a", "-o", "b"]),
            "Option '-o' can only be used once."
        );
        assert!(error(&["-i", "0"]).contains("expected a whole number of at least 1"));
        assert!(error(&["--zopfli-iterations", "x"]).contains("expected a whole number"));
    }
}
