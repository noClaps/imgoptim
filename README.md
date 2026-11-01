# imgoptim

Losslessly compress PNG, JPEG and SVG images, and convert between PNG, JPEG, SVG, AVIF and HEIF.

## Installation

```sh
cargo install --path .
```

Only the Rust toolchain is needed.

## Optimising

```sh
imgoptim optimise image.png photo.jpg icon.svg   # optimise in place
imgoptim optimise photo.jpg -o photo.min.jpg     # write elsewhere
imgoptim optimise -r assets                      # every image in a folder, recursively
imgoptim optimise -r assets -o dist/assets       # write to dist/assets, mirroring the folder structure
```

Files are only rewritten when they get smaller, and the images look the same:

- **PNG**: identical pixels; colour profile kept, other metadata removed.
- **JPEG**: identical pixels; colour profile and orientation kept, other metadata removed.
- **SVG**: minified like SVGO. Numbers are rounded to 3 decimal places, which can shift edges by
  a fraction of a pixel.
- **AVIF and HEIF**: left unchanged, as they can't be recompressed without losing quality.

PNGs are slow, around a minute for an 800×800 photo. Use `-i` to set fewer Zopfli iterations
(default 15) and trade size for speed.

## Converting

```sh
imgoptim convert photo.jpg photo.png             # change format
imgoptim convert photo.jpg -W 800 thumb.jpg      # 800px wide, height keeps the aspect ratio
imgoptim convert logo.svg -W 64 -H 64 icon.png   # exact size
imgoptim convert logo.svg -W 32 small.svg        # resize an SVG, staying vector
imgoptim convert IMG_1234.heic -q 70 photo.avif  # lossy output at quality 70 (default 85)
```

- The output format comes from the output file's extension.
- With only `-W` or `-H`, the other side keeps the aspect ratio. With both, the image is
  stretched to fit, except SVG output, which keeps its proportions.
- Output is compressed as much as possible. JPEG, AVIF and HEIF output are lossy, set by `-q`;
  AVIF is slow, around 13 seconds for a 1600×900 photo.
- Transparency becomes white in JPEGs.
- Converting a raster image to SVG embeds it; it isn't traced into shapes.

Run `imgoptim <command> --help` for every option.
