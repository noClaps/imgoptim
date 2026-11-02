use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use oxvg_ast::{
    parse::roxmltree::{ParsingOptions, parse_with_options},
    serialize::Node as _,
    visitor::Info,
};
use oxvg_optimiser::{
    CleanupListOfValues, CleanupNumericValues, ConvertOneStopGradients, ConvertPathData,
    ConvertShapeToPath, ConvertTransform, Jobs, RemoveOffCanvasPaths, RemoveXlink, ReusePaths,
};
use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{self, fontdb},
};

/// The most passes to run before giving up on further gains, matching SVGO's multipass limit.
const MAX_PASSES: usize = 10;

/// The decimal places to try rounding numbers to, from smallest output to most precise. 5 is the
/// most the optimiser allows, and finer than the 32-bit floats renderers use for coordinates in
/// the thousands.
const PRECISIONS: [u8; 3] = [3, 4, 5];

/// The default SVGO preset plus the opt-in plugins that shrink output without changing how the
/// image renders, with numbers rounded to `decimals` places and no approximations of curves.
fn jobs(decimals: u8) -> Jobs {
    let precision = i32::from(decimals);
    let mut path_data = ConvertPathData {
        // Replaces curves with circular arcs that only approximate them.
        arc_curves: false,
        smart_arc_rounding: false,
        ..ConvertPathData::default()
    };
    path_data.tolerance.precision = precision;
    path_data.tolerance.positional = 10f64.powi(-precision);
    Jobs {
        cleanup_numeric_values: Some(CleanupNumericValues {
            float_precision: decimals,
            ..CleanupNumericValues::default()
        }),
        cleanup_list_of_values: Some(CleanupListOfValues {
            float_precision: decimals,
            ..CleanupListOfValues::default()
        }),
        convert_shape_to_path: Some(ConvertShapeToPath {
            float_precision: precision,
            ..ConvertShapeToPath::default()
        }),
        convert_transform: Some(ConvertTransform {
            float_precision: precision,
            transform_precision: precision + 2,
            ..ConvertTransform::default()
        }),
        convert_path_data: Some(path_data),
        convert_one_stop_gradients: Some(ConvertOneStopGradients::default()),
        remove_off_canvas_paths: Some(RemoveOffCanvasPaths::default()),
        reuse_paths: Some(ReusePaths::default()),
        remove_xlink: Some(RemoveXlink::default()),
        ..Jobs::default()
    }
}

fn optimise_pass(source: &str, jobs: &Jobs, pass: usize) -> Result<String, String> {
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    parse_with_options(source, options, |dom, allocator| {
        let mut info = Info::new(allocator);
        info.multipass_count = pass;
        jobs.run(dom, &info).map_err(|e| e.to_string())?;
        dom.serialize().map_err(|e| e.to_string())
    })
    .map_err(|e| e.to_string())?
}

/// Optimise an SVG document with numbers rounded to `decimals` places, re-running the optimiser
/// until it stops getting smaller.
fn optimise_with(source: &str, decimals: u8) -> Result<String, String> {
    let jobs = jobs(decimals);
    let mut best = optimise_pass(source, &jobs, 0)?;
    for pass in 1..MAX_PASSES {
        let next = optimise_pass(&best, &jobs, pass)?;
        if next.len() >= best.len() {
            break;
        }
        best = next;
    }
    Ok(best)
}

/// The system's fonts, loaded once, for rendering text.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone()
}

/// The longest side, in pixels, that SVGs are rendered at for comparison: large enough to show
/// sub-pixel shifts in small icons.
const RENDER_SIZE: f32 = 1024.0;

/// Render an SVG with its longest side [`RENDER_SIZE`] pixels long, resolving linked files
/// relative to `directory`.
fn render(data: &[u8], directory: Option<&Path>) -> Result<Pixmap, String> {
    let options = usvg::Options {
        resources_dir: directory.map(Path::to_path_buf),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let scale = RENDER_SIZE / size.width().max(size.height());
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the result is between 1 and RENDER_SIZE"
    )]
    let pixels = |length: f32| (length * scale).ceil().max(1.0) as u32;
    let mut pixmap = Pixmap::new(pixels(size.width()), pixels(size.height()))
        .ok_or("the SVG has no area to render")?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// The largest difference in any channel of any pixel, or `None` if the sizes differ.
fn largest_difference(a: &Pixmap, b: &Pixmap) -> Option<u8> {
    ((a.width(), a.height()) == (b.width(), b.height())).then(|| {
        a.data()
            .iter()
            .zip(b.data())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0)
    })
}

/// Optimise an SVG document with numbers rounded as far as possible without changing how it
/// renders. Each precision is checked by rendering, and the least precise one that renders like
/// the original is kept. If none does, the original is returned unchanged. `path` is the SVG's
/// location, for resolving any files it links to.
pub fn optimise(data: &[u8], path: &Path) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(data).map_err(|e| format!("SVG is not UTF-8: {e}"))?;
    let reference = render(data, path.parent())
        .map_err(|e| format!("the SVG can't be rendered to check the result: {e}"))?;
    for decimals in PRECISIONS {
        let candidate = optimise_with(source, decimals)?;
        let rendered = render(candidate.as_bytes(), path.parent())?;
        if largest_difference(&reference, &rendered).is_some_and(|d| d <= TOLERANCE) {
            return Ok(candidate.into_bytes());
        }
    }
    Ok(data.to_vec())
}

/// How far a channel may differ after optimising, out of 255. resvg anti-aliases edges in
/// sixteenths of a pixel's coverage, so 16 lets an edge move by less than a sixteenth of a pixel
/// at [`RENDER_SIZE`], and nothing more. Optimising often moves edges that much even without
/// rounding, since paths are rewritten.
const TOLERANCE: u8 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minifies() {
        let original = br##"<?xml version="1.0" encoding="UTF-8"?>
<!-- Generator: Some Editor -->
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
    <metadata>editor junk</metadata>
    <g>
        <rect x="10.000000" y="10.000000" width="80.000000" height="80.000000" fill="#ff0000"/>
    </g>
</svg>
"##;
        let optimised =
            String::from_utf8(optimise(original, Path::new("test.svg")).unwrap()).unwrap();
        assert!(optimised.len() < original.len() / 2, "{optimised}");
        assert!(optimised.starts_with("<svg"), "{optimised}");
        assert!(!optimised.contains("metadata"), "{optimised}");
        assert!(optimised.contains("viewBox"), "{optimised}");
    }

    #[test]
    fn rejects_invalid_xml() {
        assert!(optimise(b"<svg><g></svg>", Path::new("test.svg")).is_err());
    }

    #[test]
    fn keeps_precision_that_shows() {
        // In a 1×1 view box, rounding to 3 decimals moves this edge by about half a pixel at
        // the render size, so more decimals are kept.
        let original = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><path d="M0.12345 0H1V1H0.12345Z"/></svg>"#;
        let optimised =
            String::from_utf8(optimise(original, Path::new("test.svg")).unwrap()).unwrap();
        assert!(optimised.contains(".1235"), "{optimised}");
    }

    #[test]
    fn rounds_where_it_doesnt_show() {
        // In a 1000×1000 view box, rounding to 3 decimals moves nothing by a visible amount.
        let original = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000"><path d="M100.12345 0H1000V1000H100.12345Z"/></svg>"#;
        let optimised =
            String::from_utf8(optimise(original, Path::new("test.svg")).unwrap()).unwrap();
        assert!(
            optimised.contains("100.123") && !optimised.contains("100.1234"),
            "{optimised}"
        );
    }

    #[test]
    fn rejects_unrenderable_svgs() {
        // Valid XML, but not an SVG resvg can render.
        assert!(optimise(b"<html/>", Path::new("test.svg")).is_err());
    }
}
