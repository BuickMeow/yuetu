use std::path::PathBuf;

use yuetu_vector::{VectorDocument, VectorFormat};

const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata");

fn load(name: &str) -> VectorDocument {
    let path = PathBuf::from(TESTDATA).join(name);
    VectorDocument::load(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()))
}

#[test]
fn vector_fixtures_render() {
    for (name, format, pages) in [
        ("vector.svg", VectorFormat::Svg, 1),
        ("vector.pdf", VectorFormat::Pdf, 2),
        ("vector.ai", VectorFormat::Ai, 2),
        ("vector.eps", VectorFormat::Eps, 1),
        ("vector-eps.ai", VectorFormat::Ai, 1),
    ] {
        let document = load(name);
        assert_eq!(document.format(), format, "{name}");
        assert_eq!(document.page_count(), pages, "{name}");
        let image = document
            .render_page_to_long_side(0, 400)
            .unwrap_or_else(|error| panic!("failed to render {name}: {error}"));
        assert!(image.width > 0 && image.height > 0, "{name}");
        assert!(
            image.pixels.iter().any(|byte| *byte != 0),
            "{name} rendered empty"
        );
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 4,
            "{name} has malformed RGBA data"
        );
    }
}
