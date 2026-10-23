//! Headless converter, useful for testing the core without opening the GUI.
//!
//! ```bash
//! cargo run -p yuetu-core --example cli -- input.png output.mid
//! ```

use std::path::PathBuf;

use yuetu_core::{ConversionConfig, MidiOutputConfig, convert, load_image, write_midi};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(args.next().ok_or("usage: cli <input> <output.mid>")?);
    let output = PathBuf::from(args.next().ok_or("usage: cli <input> <output.mid>")?);

    let image = load_image(&input)?;
    let config = ConversionConfig::default();
    let result = convert(&image, &config)?;
    write_midi(&output, &result, &MidiOutputConfig::default())?;

    println!(
        "{} × {} → {} notes / {} tracks → {}",
        image.width,
        image.height,
        result.note_count(),
        result.track_count(),
        output.display()
    );
    Ok(())
}
