# imgoptim

Losslessly compress PNG, JPEG and SVG images, and convert between PNG, JPEG and AVIF.

## Installation

```sh
cargo install --path .
```

Only the Rust toolchain is needed, plus [nasm](https://www.nasm.us) on x86_64 (e.g.
`brew install nasm` or `apt install nasm`).

## Optimising

```sh
imgoptim optimise image.png photo.jpg icon.svg   # optimise in place
imgoptim optimise photo.jpg -o photo.min.jpg     # write elsewhere
imgoptim optimise -r assets                      # every image in a folder, recursively
imgoptim optimise -r assets -o dist/assets       # write to dist/assets, mirroring the folder structure
```

Files are only rewritten when they get smaller, and the images stay the same:

- **PNG**: identical pixels, including fully transparent ones; colour profile kept, other
  metadata removed.
- **JPEG**: identical pixels; colour profile and orientation kept, other metadata removed.
- **SVG**: minified like SVGO, but with numbers kept to 5 decimal places, so rendering doesn't
  visibly change.
- **AVIF**: left unchanged, as it can't be recompressed without losing quality.

PNGs are slow, around a minute for an 800×800 photo. Use `-i` to set fewer Zopfli iterations
(default 15) and trade size for speed.

## Converting

```sh
imgoptim convert photo.jpg photo.png             # change format
imgoptim convert photo.jpg -W 800 thumb.jpg      # 800px wide, height keeps the aspect ratio
imgoptim convert icon.png -W 64 -H 64 small.png  # exact size
imgoptim convert photo.png -q 70 photo.avif      # lossy output at quality 70 (default 90)
```

- The output format comes from the output file's extension.
- With only `-W` or `-H`, the other side keeps the aspect ratio. With both, the image is
  stretched to fit.
- JPEG and AVIF output are lossy, set by `-q`. AVIF is slow, around 30 seconds for a
  1600×900 photo.
- Colour is kept at full resolution (4:4:4).
- The colour profile is kept. AVIF can only store sRGB and Display P3, so other colour spaces are
  converted to Display P3.
- Transparency becomes white in JPEGs, and is stored losslessly in AVIFs.
- SVGs can only be optimised, not converted.

Run `imgoptim <command> --help` for every option.
