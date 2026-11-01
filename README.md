# imgoptim

Losslessly compress PNG, JPEG, SVG, WebP and JPEG XL images at maximum effort, and convert
between those and AVIF and HEIF.

Every codec is pure Rust or bundled C compiled with the crate, so no system libraries are needed.

## Installation

```sh
cargo install --path .
```

## Usage

```
imgoptim optimise [-o <output>] [-i <zopfli-iterations>] [-r] [paths...]
imgoptim convert [-W <width>] [-H <height>] [-q <quality>] [-l] [-i <zopfli-iterations>] <input> <output>
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
  `.webp`, `.avif`, `.heic`, `.heif` or `.jxl` (in any case); other files are ignored. Symlinked directories aren't followed. Passing a directory
  without `-r` is an error.
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

Converts an image between PNG, JPEG, SVG, WebP, AVIF, HEIF and JPEG XL, optionally resizing it.
The output format comes from the output path's extension (`.png`, `.jpg`, `.jpeg`, `.svg`,
`.webp`, `.avif`, `.heic`, `.heif` or `.jxl`), and every encoder runs at its slowest, smallest
setting.

| Option                            | Description                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| `input`                           | The image to convert.                                                                         |
| `output`                          | Where to write the result. Missing directories are created.                                  |
| `-W`, `--width <width>`           | Output width in pixels.                                                                       |
| `-H`, `--height <height>`         | Output height in pixels.                                                                      |
| `-q`, `--quality <quality>`       | Quality from 1 to 100 for lossy output: JPEG, WebP, AVIF, HEIF and JPEG XL (default: 85).     |
| `-l`, `--lossless`                | Store WebP and JPEG XL output losslessly, ignoring `-q`. An error for JPEG, AVIF and HEIF.    |
| `-i`, `--zopfli-iterations <n>`   | Zopfli iterations per PNG trial (default: 15).                                                |
| `-h`, `--help`                    | Show the help message.                                                                        |

```sh
imgoptim convert photo.jpg photo.png             # change format, keep the size
imgoptim convert photo.jpg -W 800 thumb.jpg      # 800px wide, height keeps the aspect ratio
imgoptim convert logo.svg -H 512 logo.png        # rasterise at 512px tall
imgoptim convert logo.svg -W 64 -H 64 icon.png   # exact size
imgoptim convert logo.svg -W 32 small.svg        # resize an SVG, staying vector
imgoptim convert IMG_1234.heic -W 2000 photo.avif  # iPhone photo to a smaller AVIF
imgoptim convert -l photo.jpg photo.jxl          # recompress a JPEG losslessly into JPEG XL
imgoptim convert -l screenshot.png shot.webp     # lossless WebP
```

- **Sizing**: with only `-W` or `-H`, the other dimension is scaled to keep the original aspect
  ratio, rounded to the nearest pixel. With both, the image is stretched to exactly that size,
  except for SVG to SVG, where the drawing keeps its aspect ratio inside the new size (following
  its `preserveAspectRatio`, centred by default). With neither, the original size is kept; SVGs
  use their intrinsic size.
- **Raster inputs** are rotated upright according to their orientation, resampled with Lanczos3,
  and keep their ICC colour profile. AVIF inputs are assumed to be sRGB, and animated WebP and
  JPEG XL inputs use their first frame.
- **SVG to PNG or JPEG** renders with [resvg](https://github.com/linebender/resvg), using the
  system's fonts for text.
- **SVG to SVG** stays vector: the root element's `width` and `height` are replaced, and a
  `viewBox` is added if missing so the drawing scales. The result is then optimised.
- **Raster to SVG** embeds the image in an SVG at the output size. JPEG sources without
  transparency are embedded as JPEG (unless `-l`), everything else as PNG. It isn't traced into
  vector shapes.
- **JPEG to JPEG XL with `-l`** and no resizing recompresses the JPEG's DCT coefficients rather
  than its pixels: around 20% smaller, and the JPEG can be rebuilt byte for byte from the JPEG XL
  file (e.g. with `djxl`). Progressive JPEGs are first rewritten losslessly as baseline, so
  that's the JPEG they rebuild into, with identical pixels.
- **Lossy output** is the only step that changes pixels:

  | Format   | Encoder                                                  | Settings                                                                    |
  | -------- | -------------------------------------------------------- | --------------------------------------------------------------------------- |
  | JPEG     | [mozjpeg](https://github.com/mozilla/mozjpeg)            | Maximum compression profile: progressive, optimised scans, trellis quantisation. Transparency is blended onto white |
  | WebP     | [libwebp](https://chromium.googlesource.com/webm/libwebp) | Method 6, 10 entropy passes, sharp YUV, lossless alpha                      |
  | AVIF     | [ravif](https://github.com/kornelski/cavif-rs) (rav1e)   | Speed 1 (slowest), 10-bit                                                    |
  | HEIF     | [heifer](https://github.com/HarpeLm/Heifer)              | HEVC with rate-distortion optimised block, transform and prediction choices |
  | JPEG XL  | [jxl-encoder](https://github.com/imazen/jxl-encoder)     | Efforts 9 and 10, keeping the smaller                                       |

  Lossless WebP uses `cwebp -lossless -z 9` settings, and lossless JPEG XL the same efforts as
  lossy.
- AVIF is by far the slowest: about 13 seconds for a 1600×900 photo.
- Each conversion prints one line, e.g. `logo.svg → logo.png: 1683 bytes`. On error the exit
  code is `1`.

## What `optimise` does to each format

| Format | Engine                                                           | Settings                                                                                               | Equivalent to                     |
| ------ | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | --------------------------------- |
| PNG    | [oxipng](https://github.com/oxipng/oxipng)                       | Preset 6 (every filter strategy, full evaluation), Zopfli deflate, safe chunk stripping, alpha optimisation, no interlacing | `oxipng -o max -Z -s -a -i 0`     |
| JPEG   | [mozjpeg](https://github.com/mozilla/mozjpeg)                    | Coefficient-level transcode with optimised Huffman tables; tries progressive (with scan optimisation) and baseline and keeps the smaller | `jpegtran -copy none -optimize`   |
| SVG    | [OXVG](https://github.com/noahbald/oxvg)                         | SVGO's default preset plus `cleanupListOfValues`, `convertOneStopGradients`, `removeOffCanvasPaths`, `reusePaths` and `removeXlink`, run multipass | `svgo --multipass`                |
| WebP   | [libwebp](https://chromium.googlesource.com/webm/libwebp)        | Strips EXIF and XMP. Lossless WebPs are also re-encoded at the highest lossless effort             | `cwebp -lossless -z 9 -metadata icc` |
| JPEG XL | [jxl-encoder](https://github.com/imazen/jxl-encoder)            | Strips EXIF, XMP and JUMBF boxes. Losslessly stored images are also re-encoded at efforts 9 and 10 | `cjxl -d 0 -e 10`                 |
| AVIF, HEIF | —                                                            | Left unchanged                                                                                     | —                                 |

## How lossless is `optimise`?

- **PNG**: decoded pixels are identical. Fully transparent pixels may change colour, as with
  `oxipng -a`, but they're invisible either way.
- **JPEG**: the DCT coefficients and quantisation tables are copied unchanged, so decoders produce
  the same pixels. Metadata is dropped except the ICC colour profile and a non-default EXIF
  orientation, which is rewritten as a minimal EXIF block.
- **WebP**: re-encoded lossless images are decoded and compared before being kept: pixels are
  identical, apart from the colour of fully transparent pixels. Lossy and animated WebPs only
  lose their metadata chunks; the ICC profile is kept.
- **JPEG XL**: re-encoded images are decoded and must match the original's pixels exactly to be
  kept. Files that can be rebuilt into a JPEG are left untouched, since stripping their metadata
  would break that.
- **SVG**: the same as SVGO. Numbers are rounded to 3 decimal places, which can shift anti-aliased
  edges by a fraction of a pixel. Comments, metadata and editor data are removed.

## Licensing

The JPEG XL encoder, [jxl-encoder](https://github.com/imazen/jxl-encoder), is licensed under the
AGPL-3.0 (or a commercial licence), which applies to this tool as a whole when it's distributed.
Every other dependency is under a permissive licence.
