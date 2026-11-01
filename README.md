# imgoptim

Losslessly compress PNG, JPEG and SVG images at maximum effort, and convert between those and
AVIF and HEIF.

## Installation

```sh
cargo install --path .
```

Nothing needs to be installed beyond the Rust toolchain: no CMake, nasm, pkg-config or system
libraries. Every codec is pure Rust or bundled C that the crate compiles itself, and the binary
links only the operating system's own libraries.

AV1 assembly (for AVIF) is only enabled on ARM, where it builds with the standard toolchain; on
x86 it would need nasm, so AVIF uses rav1e's and rav1d's pure-Rust code paths there, which are
slower. For the same reason, [`avif-decode`](https://github.com/kornelski/avif-decode) is vendored
in `vendor/avif-decode`, unchanged apart from its manifest. mozjpeg uses its x86 SIMD only if
nasm happens to be installed.

## Usage

```
imgoptim optimise [-o <output>] [-i <zopfli-iterations>] [-r] [paths...]
imgoptim convert [-W <width>] [-H <height>] [-q <quality>] [-i <zopfli-iterations>] <input> <output>
```

Run `imgoptim <command> --help` for a command's options. Options can go before or after the paths.

### `optimise`

Losslessly compresses images at maximum effort.

| Option                            | Description                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| `paths...`                        | One or more images, or directories with `-r`, to optimise. Without `-o`, each image is overwritten in place. |
| `-o`, `--output <output>`         | Write the result to this path instead of overwriting the input. Only valid with a single input. With `-r` and a directory, the directory structure is mirrored under this path. |
| `-i`, `--zopfli-iterations <n>`   | Zopfli iterations per PNG trial (default: 15). Higher is slower and rarely smaller.           |
| `-r`, `--recursive`               | Optimise every supported image in the given directories and all their subdirectories.         |
| `-h`, `--help`                    | Show the help message.                                                                        |

```sh
imgoptim optimise image.png photo.jpg icon.svg   # optimise in place
imgoptim optimise photo.jpg -o photo.min.jpg     # write elsewhere
imgoptim optimise -i 50 image.png                # more Zopfli iterations
imgoptim optimise -r assets                      # every image in a folder, recursively
imgoptim optimise -r assets -o dist/assets       # mirror the folder's optimised images into dist/assets
```

- With `-r`, directories are searched for files ending in `.png`, `.jpg`, `.jpeg`, `.svg`,
  `.avif`, `.heic` or `.heif` (in any case); other files are ignored. Symlinked directories
  aren't followed. Passing a directory without `-r` is an error.
- With `-r` and `-o`, each image is written to the same relative path under the output
  directory, e.g. `assets/icons/logo.svg` → `dist/assets/icons/logo.svg`. Missing directories are
  created and the input is left untouched. Only supported images are written, not other files.
- Files are detected by their contents, not their extension. SVGs are recognised by a `.svg`
  extension or by starting with `<`.
- AVIF and HEIF files are left unchanged, since their encoders are lossy and re-encoding would
  change the image. Their line ends with `(can't be recompressed losslessly)`.
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

### `convert`

Converts an image between PNG, JPEG, SVG, AVIF and HEIF, optionally resizing it. The output
format comes from the output path's extension (`.png`, `.jpg`, `.jpeg`, `.svg`, `.avif`, `.heic`
or `.heif`), and every encoder runs at its slowest, smallest setting.

| Option                            | Description                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| `input`                           | The image to convert.                                                                         |
| `output`                          | Where to write the result. Missing directories are created.                                  |
| `-W`, `--width <width>`           | Output width in pixels.                                                                       |
| `-H`, `--height <height>`         | Output height in pixels.                                                                      |
| `-q`, `--quality <quality>`       | Quality from 1 to 100 for lossy output: JPEG, AVIF and HEIF (default: 85).                    |
| `-i`, `--zopfli-iterations <n>`   | Zopfli iterations per PNG trial (default: 15).                                                |
| `-h`, `--help`                    | Show the help message.                                                                        |

```sh
imgoptim convert photo.jpg photo.png             # change format, keep the size
imgoptim convert photo.jpg -W 800 thumb.jpg      # 800px wide, height keeps the aspect ratio
imgoptim convert logo.svg -H 512 logo.png        # rasterise at 512px tall
imgoptim convert logo.svg -W 64 -H 64 icon.png   # exact size
imgoptim convert logo.svg -W 32 small.svg        # resize an SVG, staying vector
imgoptim convert IMG_1234.heic -W 2000 photo.avif  # iPhone photo to a smaller AVIF
```

- **Sizing**: with only `-W` or `-H`, the other dimension is scaled to keep the original aspect
  ratio, rounded to the nearest pixel. With both, the image is stretched to exactly that size,
  except for SVG to SVG, where the drawing keeps its aspect ratio inside the new size (following
  its `preserveAspectRatio`, centred by default). With neither, the original size is kept; SVGs
  use their intrinsic size.
- **Raster inputs** are rotated upright according to their orientation, resampled with Lanczos3,
  and keep their ICC colour profile. AVIF inputs are assumed to be sRGB.
- **SVG to PNG or JPEG** renders with [resvg](https://github.com/linebender/resvg), using the
  system's fonts for text.
- **SVG to SVG** stays vector: the root element's `width` and `height` are replaced, and a
  `viewBox` is added if missing so the drawing scales. The result is then optimised.
- **Raster to SVG** embeds the image in an SVG at the output size. JPEG sources without
  transparency are embedded as JPEG, everything else as PNG. It isn't traced into vector shapes.
- **Lossy output** is the only step that changes pixels:

  | Format   | Encoder                                                  | Settings                                                                    |
  | -------- | -------------------------------------------------------- | --------------------------------------------------------------------------- |
  | JPEG     | [mozjpeg](https://github.com/mozilla/mozjpeg)            | Maximum compression profile: progressive, optimised scans, trellis quantisation. Transparency is blended onto white |
  | AVIF     | [ravif](https://github.com/kornelski/cavif-rs) (rav1e)   | Speed 1 (slowest), 10-bit                                                    |
  | HEIF     | [heifer](https://github.com/HarpeLm/Heifer)              | HEVC with rate-distortion optimised block, transform and prediction choices |

- AVIF is by far the slowest: about 13 seconds for a 1600×900 photo.
- Each conversion prints one line, e.g. `logo.svg → logo.png: 1683 bytes`. On error the exit
  code is `1`.

## What `optimise` does to each format

| Format | Engine                                                           | Settings                                                                                               | Equivalent to                     |
| ------ | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | --------------------------------- |
| PNG    | [oxipng](https://github.com/oxipng/oxipng)                       | Preset 6 (every filter strategy, full evaluation), Zopfli deflate, safe chunk stripping, alpha optimisation, no interlacing | `oxipng -o max -Z -s -a -i 0`     |
| JPEG   | [mozjpeg](https://github.com/mozilla/mozjpeg)                    | Coefficient-level transcode with optimised Huffman tables; tries progressive (with scan optimisation) and baseline and keeps the smaller | `jpegtran -copy none -optimize`   |
| SVG    | [OXVG](https://github.com/noahbald/oxvg)                         | SVGO's default preset plus `cleanupListOfValues`, `convertOneStopGradients`, `removeOffCanvasPaths`, `reusePaths` and `removeXlink`, run multipass | `svgo --multipass`                |
| AVIF, HEIF | —                                                            | Left unchanged                                                                                     | —                                 |

## How lossless is `optimise`?

- **PNG**: decoded pixels are identical. Fully transparent pixels may change colour, as with
  `oxipng -a`, but they're invisible either way.
- **JPEG**: the DCT coefficients and quantisation tables are copied unchanged, so decoders produce
  the same pixels. Metadata is dropped except the ICC colour profile and a non-default EXIF
  orientation, which is rewritten as a minimal EXIF block.
- **SVG**: the same as SVGO. Numbers are rounded to 3 decimal places, which can shift anti-aliased
  edges by a fraction of a pixel. Comments, metadata and editor data are removed.

## Licensing

Every dependency is under a permissive licence, apart from a few under the MPL-2.0: `avif-parse`,
and `lightningcss` and its CSS parsers, which the SVG optimiser uses. The MPL-2.0 only covers
changes to those crates' own files, not the rest of this tool.
