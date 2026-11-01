use oxvg_ast::{
    parse::roxmltree::{ParsingOptions, parse_with_options},
    serialize::Node as _,
    visitor::Info,
};
use oxvg_optimiser::{
    CleanupListOfValues, CleanupNumericValues, ConvertOneStopGradients, ConvertPathData,
    ConvertShapeToPath, ConvertTransform, Jobs, RemoveOffCanvasPaths, RemoveXlink, ReusePaths,
};

/// The most passes to run before giving up on further gains, matching SVGO's multipass limit.
const MAX_PASSES: usize = 10;

/// Decimal places numbers are rounded to: the most the optimiser allows. SVGO's default of 3 can
/// visibly shift edges, while 5 is finer than the 32-bit floats renderers use for coordinates in
/// the thousands.
const PRECISION: u8 = 5;

/// The default SVGO preset plus the opt-in plugins that shrink output without changing how the
/// image renders, with numbers rounded to [`PRECISION`] and no approximations of curves.
fn jobs() -> Jobs {
    let precision = i32::from(PRECISION);
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
            float_precision: PRECISION,
            ..CleanupNumericValues::default()
        }),
        cleanup_list_of_values: Some(CleanupListOfValues {
            float_precision: PRECISION,
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

/// Optimise an SVG document, re-running the optimiser until it stops getting smaller.
pub fn optimise(data: &[u8]) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(data).map_err(|e| format!("SVG is not UTF-8: {e}"))?;
    let jobs = jobs();

    let mut best = optimise_pass(source, &jobs, 0)?;
    for pass in 1..MAX_PASSES {
        let next = optimise_pass(&best, &jobs, pass)?;
        if next.len() >= best.len() {
            break;
        }
        best = next;
    }
    Ok(best.into_bytes())
}

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
        let optimised = String::from_utf8(optimise(original).unwrap()).unwrap();
        assert!(optimised.len() < original.len() / 2, "{optimised}");
        assert!(optimised.starts_with("<svg"), "{optimised}");
        assert!(!optimised.contains("metadata"), "{optimised}");
        assert!(optimised.contains("viewBox"), "{optimised}");
    }

    #[test]
    fn rejects_invalid_xml() {
        assert!(optimise(b"<svg><g></svg>").is_err());
    }
}
