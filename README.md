# imgoptim

Losslessly compress SVG, PNG and JPEG images at maximum effort.

## Installation

```sh
cargo install --path .
```

## Usage

```
imgoptim [-o <output>] [-i <zopfli-iterations>] [-r] [paths...]
```

| Option                            | Description                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| `paths...`                        | One or more images, or directories with `-r`, to optimise. Without `-o`, each image is overwritten in place. |
| `-o`, `--output <output>`         | Write the result to this path instead of overwriting the input. Only valid with a single file. |
| `-i`, `--zopfli-iterations <n>`   | Zopfli iterations per PNG trial (default: 15). Higher is slower and rarely smaller.           |
| `-r`, `--recursive`               | Optimise every PNG, JPEG and SVG in the given directories and all their subdirectories.       |
| `-h`, `--help`                    | Show the help message.                                                                        |

Options can go before or after the paths.

### Examples

```sh
imgoptim image.png photo.jpg icon.svg      # optimise in place
imgoptim photo.jpg -o photo.min.jpg        # write elsewhere
imgoptim -i 50 image.png                   # more Zopfli iterations
imgoptim -r assets                         # every image in a folder, recursively
```

### Behaviour

- With `-r`, directories are searched for files ending in `.png`, `.jpg`, `.jpeg` or `.svg` (in
  any case); other files are ignored. Symlinked directories aren't followed. Passing a directory
  without `-r` is an error.
- Files are detected by their contents, not their extension. SVGs are recognised by a `.svg`
  extension or by starting with `<`.
- A file is only rewritten when the result is smaller. With `-o`, the output is always written,
  falling back to a copy of the input.
- Each file prints one line with its size before and after:

  ```
  photo.png: 654749 → 480070 bytes (-26.68%)
  ```

- Errors are printed to stderr and the remaining files are still processed. The exit code is `1`
  if any file failed, otherwise `0`.
- PNG optimisation is slow: Zopfli across every filter strategy takes around a minute for an
  800×800 photo, using all CPU cores. Lower `-i` to trade size for speed.

## What each format gets

| Format | Engine                                                           | Settings                                                                                               | Equivalent to                     |
| ------ | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | --------------------------------- |
| PNG    | [oxipng](https://github.com/oxipng/oxipng)                       | Preset 6 (every filter strategy, full evaluation), Zopfli deflate, safe chunk stripping, alpha optimisation, no interlacing | `oxipng -o max -Z -s -a -i 0`     |
| JPEG   | [mozjpeg](https://github.com/mozilla/mozjpeg)                    | Coefficient-level transcode with optimised Huffman tables; tries progressive (with scan optimisation) and baseline and keeps the smaller | `jpegtran -copy none -optimize`   |
| SVG    | [OXVG](https://github.com/noahbald/oxvg)                         | SVGO's default preset plus `cleanupListOfValues`, `convertOneStopGradients`, `removeOffCanvasPaths`, `reusePaths` and `removeXlink`, run multipass | `svgo --multipass`                |

## How lossless is it?

- **PNG**: decoded pixels are identical. Fully transparent pixels may change colour, as with
  `oxipng -a`, but they're invisible either way.
- **JPEG**: the DCT coefficients and quantisation tables are copied unchanged, so decoders produce
  the same pixels. Metadata is dropped except the ICC colour profile and a non-default EXIF
  orientation, which is rewritten as a minimal EXIF block.
- **SVG**: the same as SVGO. Numbers are rounded to 3 decimal places, which can shift anti-aliased
  edges by a fraction of a pixel. Comments, metadata and editor data are removed.
