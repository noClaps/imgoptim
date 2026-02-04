# imgoptim

A command line utility to optimise images.

## Build instructions

You'll need [Rust](https://rust-lang.org).

Clone the repository:

```sh
git clone https://codeberg.org/noClaps/imgoptim.git
cd imgoptim
```

Build the project:

```sh
cargo build
cargo build --release # to build in release mode
```

Run the executable:

```sh
cargo run -- --help
```

## Usage

```
USAGE: imgoptim <path> <out-path> [--width <width>]

A command line utility to optimise images

POSITIONALS:
  <path>                 Path to the image you'd like to optimise.
  <out-path>             Output path for the optimised image.

FLAGS:
  -W, --width <width>    Set the width of the output image in pixels.
  -h, --help             Display this help and exit.
```

To use `imgoptim`, you need to pass in a path to a JPEG or PNG image, and an output path:

```sh
imgoptim path/to/image.jpeg path/to/image.avif
imgoptim path/to/image.png path/to/image.avif
```

This will optimise your input JPEG/PNG image to an AVIF image, retaining the same dimensions. If you'd like to customise the width of the image, you can do so with the `--width` or `-W` option:

```sh
imgoptim path/to/image.png path/to/image.avif --width 800 # pixels
imgoptim path/to/image.png path/to/image.avif -W 800 # pixels
```

This will retain the aspect ratio of the input image, but will resize it to be the width you've set. Both of these commands will output the AVIF image's dimensions in the format `<width> <height>`:

```sh
imgoptim image.png image.avif
# 4032 3024
imgoptim image.png image.avif --width 800
# 800 600
```

You can view the help for the command using `--help` or `-h`:

```sh
imgoptim --help
imgoptim -h
```
