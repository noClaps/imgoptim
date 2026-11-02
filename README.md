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
imgoptim -n -r assets                   # report what would be saved, changing nothing
```

Files are only rewritten when they get smaller, and the images stay the same:

- **PNG**: identical pixels, including fully transparent ones; colour information kept (profile,
  gamma, chromaticities and HDR metadata), other metadata removed.
- **JPEG**: identical pixels; colour profile and orientation kept, other metadata removed.
  Truncated or corrupt JPEGs are left unchanged.
- **SVG**: minified like SVGO, with numbers rounded to 3, 4 or 5 decimal places, whichever is
  the shortest that renders the same as the original. SVGs that would visibly change are left
  unchanged.

Every result is checked against the original before it's written: JPEGs must have the same
coefficients, PNGs the same pixels, and SVGs must render the same. Files are replaced atomically,
so an interrupted run never leaves a partly written image.

Images imgoptim has already optimised are remembered in `~/.cache/imgoptim` (or
`$XDG_CACHE_HOME/imgoptim`) and skipped on later runs; `--no-cache` optimises them again.

Files are optimised in parallel, each printing its size before and after as it finishes, followed
by a summary. PNGs are slow, around a minute for an 800×800 photo; use `-i` to set fewer Zopfli
iterations (default 15) and trade size for speed.

Run `imgoptim --help` for every option.
