//! Small synthetic benchmark for the conversion stages.
//!
//! Run with:
//! `cargo run --release -p yuetu-core --example benchmark`

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use yuetu_core::{Color, ConversionConfig, RgbaImage, convert_frames_with_progress};

fn main() {
    let frame_count = 30usize;
    let frames: Vec<RgbaImage> = (0..frame_count)
        .map(|frame| {
            let mut image = RgbaImage::new(512, 512, Color::BLACK);
            for y in 0..512 {
                for x in 0..512 {
                    image.set_pixel(
                        x,
                        y,
                        Color::rgb((x + frame as u32) as u8, y as u8, frame as u8),
                    );
                }
            }
            image
        })
        .collect();
    let frame_refs: Vec<&RgbaImage> = frames.iter().collect();
    let delays = vec![100; frame_count];
    let config = ConversionConfig::default();
    let cancel = AtomicBool::new(false);

    let started = Instant::now();
    let first =
        convert_frames_with_progress(&frames[0], &frame_refs, &delays, &config, None, &cancel)
            .unwrap();
    println!(
        "full: total {:.2} ms | palette {:.2} | transform {:.2} | quantize {:.2} | notes {:.2}",
        first.timings.total_ms,
        first.timings.palette_ms,
        first.timings.transform_ms,
        first.timings.quantize_ms,
        first.timings.notes_ms
    );
    println!("wall: {:.2} ms", started.elapsed().as_secs_f64() * 1000.0);

    let notes_config = ConversionConfig {
        max_note_length: 8,
        ..config
    };
    let second = convert_frames_with_progress(
        &frames[0],
        &frame_refs,
        &delays,
        &notes_config,
        None,
        &cancel,
    )
    .unwrap();
    println!(
        "full again: total {:.2} ms | palette {:.2} | transform {:.2} | quantize {:.2} | notes {:.2}",
        second.timings.total_ms,
        second.timings.palette_ms,
        second.timings.transform_ms,
        second.timings.quantize_ms,
        second.timings.notes_ms
    );
}
