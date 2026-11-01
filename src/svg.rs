use std::{cmp::Reverse, fmt::Write, num::NonZeroU32, ops::Range};

use oxvg_ast::{
    parse::roxmltree::{ParsingOptions, parse_with_options},
    serialize::Node as _,
    visitor::Info,
};
use oxvg_optimiser::{
    CleanupListOfValues, ConvertOneStopGradients, Jobs, RemoveOffCanvasPaths, RemoveXlink,
    ReusePaths,
};
use resvg::usvg::{self, Tree};

use crate::convert::target_size;

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

/// Set an SVG's rendered size without touching its content. A `viewBox` matching the original size
/// is added if there isn't one, so the drawing scales with the new size.
pub fn resize(
    data: &[u8],
    width: Option<NonZeroU32>,
    height: Option<NonZeroU32>,
) -> Result<Vec<u8>, String> {
    if width.is_none() && height.is_none() {
        return Ok(data.to_vec());
    }
    let source = std::str::from_utf8(data).map_err(|e| format!("SVG is not UTF-8: {e}"))?;
    let original = Tree::from_str(source, &usvg::Options::default())
        .map_err(|e| e.to_string())?
        .size();
    let (new_width, new_height) = target_size(
        (f64::from(original.width()), f64::from(original.height())),
        width,
        height,
    );

    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let document =
        roxmltree::Document::parse_with_options(source, options).map_err(|e| e.to_string())?;
    let root = document.root_element();

    // Replacements as (byte range, new text), applied back to front so earlier ranges stay valid.
    let mut edits: Vec<(Range<usize>, String)> = root
        .attributes()
        .filter(|a| a.namespace().is_none() && matches!(a.name(), "width" | "height"))
        .map(|a| (a.range(), String::new()))
        .collect();
    let tag_start = root.range().start + 1;
    let name_end = tag_start
        + source[tag_start..]
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .ok_or("unterminated root element")?;
    let mut attributes = format!(r#" width="{new_width}" height="{new_height}""#);
    if root.attribute("viewBox").is_none() {
        write!(
            attributes,
            r#" viewBox="0 0 {} {}""#,
            original.width(),
            original.height()
        )
        .expect("writing to a String can't fail");
    }
    edits.push((name_end..name_end, attributes));
    edits.sort_by_key(|(range, _)| Reverse(range.start));

    let mut resized = source.to_owned();
    for (range, text) in edits {
        resized.replace_range(range, &text);
    }
    Ok(resized.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizes_keeping_aspect_ratio() {
        let original = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100"/></svg>"#;
        let resized =
            String::from_utf8(resize(original, NonZeroU32::new(50), None).unwrap()).unwrap();
        assert_eq!(
            resized,
            r#"<svg width="50" height="25" viewBox="0 0 200 100" xmlns="http://www.w3.org/2000/svg"  ><rect width="200" height="100"/></svg>"#
        );
    }

    #[test]
    fn resize_keeps_existing_view_box() {
        let original = br#"<svg viewBox="0 0 10 20" xmlns="http://www.w3.org/2000/svg"/>"#;
        let resized =
            String::from_utf8(resize(original, None, NonZeroU32::new(40)).unwrap()).unwrap();
        assert_eq!(
            resized,
            r#"<svg width="20" height="40" viewBox="0 0 10 20" xmlns="http://www.w3.org/2000/svg"/>"#
        );
    }

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
