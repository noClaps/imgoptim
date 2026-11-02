# imgoptim

Losslessly compress PNG, JPEG and SVG images.

## Installation

```sh
cargo install --path .
```

imgoptim supports macOS on Apple silicon, and only needs the Rust toolchain.

## Usage

```sh
imgoptim image.png photo.jpg icon.svg   # optimise in place
imgoptim photo.jpg -o photo.min.jpg     # write elsewhere
imgoptim -r assets                      # every image in a folder, recursively
imgoptim -r assets -o dist/assets       # write to dist/assets, mirroring the folder structure
```

Files are only rewritten when they get smaller, and the images stay the same:

- **PNG**: identical pixels, including fully transparent ones; colour profile kept, other
  metadata removed.
- **JPEG**: identical pixels; colour profile and orientation kept, other metadata removed.
  Truncated or corrupt JPEGs are left unchanged.
- **SVG**: minified like SVGO, but with numbers kept to 5 decimal places, so rendering doesn't
  visibly change.

Files are optimised in parallel, and each prints its size before and after as it finishes.
PNGs are slow, around a minute for an 800×800 photo; use `-i` to set fewer Zopfli iterations
(default 15) and trade size for speed.

Run `imgoptim --help` for every option.
