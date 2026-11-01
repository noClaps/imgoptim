# imgoptim

A command line utility to optimise images.

## Build instructions

You'll need [Rust](https://rust-lang.org).

```sh
cargo install --git https://github.com/noClaps/imgoptim
```

## Usage

```
A command line utility to optimise images

Usage: imgoptim <COMMAND>

Commands:
  optimise  Optimise HEIC images to AVIF
  height    Get the height of an AVIF image
  width     Get the width of an AVIF image
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

You can view the help for the command using `--help` or `-h`:

```sh
imgoptim --help
imgoptim -h
```

### Optimise

```
Optimise HEIC images to AVIF

Usage: imgoptim optimise [OPTIONS] <PATH> <OUT_PATH>

Arguments:
  <PATH>      Path to the image you'd like to optimise
  <OUT_PATH>  Output path for the optimised image

Options:
  -W, --width <WIDTH>  Set the width of the output image in pixels
  -h, --help           Print help
```

You need to pass in a path to a HEIC image, and an output path:

```sh
imgoptim optimise path/to/image.heic path/to/image.avif
```

This will optimise your input HEIC image to an AVIF image, retaining the same dimensions. If you'd like to customise the width of the image, you can do so with the `--width` or `-W` option:

```sh
imgoptim optimise path/to/image.png path/to/image.avif --width 800 # pixels
imgoptim optimise path/to/image.png path/to/image.avif -W 800 # pixels
```

This will retain the aspect ratio of the input image, but will resize it to be the width you've set.

```sh
imgoptim optimise image.png image.avif # 4032x3024
imgoptim optimise image.png image.avif --width 800 # 800x600
```

You can view the help for the command using `--help` or `-h`:

```sh
imgoptim optimise --help
imgoptim optimise -h
```

### Height

```
Get the height of an AVIF image

Usage: imgoptim height <PATH>

Arguments:
  <PATH>  Path to the image to get the height of

Options:
  -h, --help  Print help
```

You can get the height of a given AVIF image using the `imgoptim height` command.

```sh
imgoptim height path/to/image.avif # 600
```

You can view the help for the command using `--help` or `-h`:

```sh
imgoptim height --help
imgoptim height -h
```

### Width

```
Get the width of an AVIF image

Usage: imgoptim width <PATH>

Arguments:
  <PATH>  Path to the image to get the width of

Options:
  -h, --help  Print help
```

You can get the width of a given AVIF image using the `imgoptim width` command.

```sh
imgoptim width path/to/image.avif # 800
```

You can view the help for the command using `--help` or `-h`:

```sh
imgoptim width --help
imgoptim width -h
```
