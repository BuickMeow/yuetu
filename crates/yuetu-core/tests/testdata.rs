use std::path::PathBuf;

use yuetu_core::load_image_sequence;

const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata");

fn assert_sequence(name: &str, expected_frames: usize, width: u32, height: u32) {
    let path = PathBuf::from(TESTDATA).join(name);
    let sequence = load_image_sequence(&path)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

    assert_eq!(
        sequence.frames.len(),
        expected_frames,
        "frame count changed for {name}"
    );
    assert_eq!(
        sequence.delays_ms.len(),
        expected_frames,
        "delay count changed for {name}"
    );
    let frame = &sequence.frames[0];
    assert_eq!(
        (frame.width, frame.height),
        (width, height),
        "size changed for {name}"
    );
}

#[test]
fn testdata_fixtures_decode() {
    assert_sequence("grid800.png", 1, 800, 800);
    assert_sequence("sparse800.png", 1, 800, 800);
    assert_sequence("ruler-test.png", 1, 800, 600);
    assert_sequence("frames40.gif", 40, 240, 160);
    assert_sequence("frames300.gif", 300, 160, 100);
    assert_sequence("shared-palette.gif", 2, 300, 200);
    assert_sequence("scrub30.gif", 30, 800, 800);
}

#[test]
fn sparse_fixture_keeps_its_transparent_background() {
    let path = PathBuf::from(TESTDATA).join("sparse800.png");
    let sequence = load_image_sequence(&path).unwrap();
    let frame = &sequence.frames[0];
    assert_eq!(frame.pixel(0, 0).map(|color| color.a), Some(0));
    assert_eq!(frame.pixel(400, 400).map(|color| color.a), Some(255));
}
