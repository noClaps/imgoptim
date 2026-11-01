use oxvg_ast::{
    parse::roxmltree::{ParsingOptions, parse_with_options},
    serialize::Node as _,
    visitor::Info,
};
use oxvg_optimiser::{
    CleanupListOfValues, ConvertOneStopGradients, Jobs, RemoveOffCanvasPaths, RemoveXlink,
    ReusePaths,
};

/// The most passes to run before giving up on further gains, matching SVGO's multipass limit.
const MAX_PASSES: usize = 10;

/// The default SVGO preset plus the opt-in plugins that shrink output without changing how the
/// image renders.
fn jobs() -> Jobs {
    Jobs {
        cleanup_list_of_values: Some(CleanupListOfValues::default()),
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
