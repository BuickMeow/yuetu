use std::path::PathBuf;
use std::time::Instant;

use yuetu_vector::{VectorDocument, ViewTransform};

const ITERATIONS: u32 = 20;

fn fit_transform(page: (f32, f32), width: u32, height: u32) -> ViewTransform {
    let scale = (width as f32 / page.0).min(height as f32 / page.1);
    ViewTransform {
        a: scale,
        b: 0.0,
        c: 0.0,
        d: scale,
        e: (width as f32 - page.0 * scale) * 0.5,
        f: (height as f32 - page.1 * scale) * 0.5,
    }
}

fn zoom_transform(page: (f32, f32), width: u32, height: u32, scale: f32) -> ViewTransform {
    ViewTransform {
        a: scale,
        b: 0.0,
        c: 0.0,
        d: scale,
        e: width as f32 * 0.5 - page.0 * scale * 0.5,
        f: height as f32 * 0.5 - page.1 * scale * 0.5,
    }
}

fn bench(
    document: &VectorDocument,
    label: &str,
    width: u32,
    height: u32,
    transform: ViewTransform,
) {
    // Warm up once, then report the best and mean of the remaining runs.
    document
        .render_page_view(0, width, height, transform)
        .unwrap();
    let mut best = f64::INFINITY;
    let mut total = 0.0;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let image = document
            .render_page_view(0, width, height, transform)
            .unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert!(image.width == width && image.height == height);
        best = best.min(elapsed);
        total += elapsed;
    }
    println!(
        "{label:20} {width}x{height}: best {best:.2} ms, mean {:.2} ms",
        total / f64::from(ITERATIONS)
    );
}

fn main() {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata"));
    for name in ["vector.svg", "vector.pdf", "vector.eps"] {
        let path = root.join(name);
        let document = VectorDocument::load(&path).unwrap();
        let page = document.page_size(0).unwrap();
        println!("{name} · page {:.0}x{:.0}", page.0, page.1);
        bench(
            &document,
            "fit 2000x1500",
            2000,
            1500,
            fit_transform(page, 2000, 1500),
        );
        bench(
            &document,
            "zoom 50x crop",
            2000,
            1500,
            zoom_transform(page, 2000, 1500, 50.0),
        );
    }
}
