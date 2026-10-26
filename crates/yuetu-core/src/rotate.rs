//! Geometric source-image rotation.
//!
//! Rotation happens before resizing and palette generation. Positive angles
//! rotate clockwise in screen coordinates (y grows downward). The output is
//! expanded to contain the whole rotated image; the new corners are
//! transparent, which the converter already treats as silence.

use crate::error::{Error, Result};
use crate::image::RgbaImage;

const EPSILON: f32 = 1e-4;
const MAX_ROTATED_PIXELS: u64 = 268_435_456;

/// Normalize an angle to `(-180, 180]`.
pub fn normalize_degrees(degrees: f32) -> f32 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let mut angle = degrees % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    } else if angle > 180.0 {
        angle -= 360.0;
    }
    if angle.abs() < EPSILON { 0.0 } else { angle }
}

/// Dimensions of the expanded canvas after rotation, without rotating pixels.
pub fn rotated_dimensions(width: u32, height: u32, degrees: f32) -> (u32, u32) {
    let (width, height) = rotated_dimensions_f32(width as f32, height as f32, degrees);
    (width.ceil().max(1.0) as u32, height.ceil().max(1.0) as u32)
}

/// Continuous dimensions of the expanded canvas after rotation.
pub fn rotated_dimensions_f32(width: f32, height: f32, degrees: f32) -> (f32, f32) {
    let angle = normalize_degrees(degrees);
    if angle == 0.0 || (angle - 180.0).abs() < EPSILON {
        return (width.max(1.0), height.max(1.0));
    }
    if (angle - 90.0).abs() < EPSILON || (angle + 90.0).abs() < EPSILON {
        return (height.max(1.0), width.max(1.0));
    }

    let radians = f64::from(angle).to_radians();
    let (sin, cos) = radians.sin_cos();
    let source_width = f64::from(width.max(1.0));
    let source_height = f64::from(height.max(1.0));
    (
        (source_width * cos.abs() + source_height * sin.abs()) as f32,
        (source_width * sin.abs() + source_height * cos.abs()) as f32,
    )
}

pub fn rotate_image(image: &RgbaImage, degrees: f32) -> Result<RgbaImage> {
    if image.is_empty() {
        return Err(Error::InvalidImage("源图为空".into()));
    }

    let angle = normalize_degrees(degrees);
    if angle == 0.0 {
        return Ok(image.clone());
    }

    if (angle - 90.0).abs() < EPSILON {
        return Ok(rotate_quarter(image, Quarter::Clockwise90));
    }
    if (angle - 180.0).abs() < EPSILON {
        return Ok(rotate_quarter(image, Quarter::Half));
    }
    if (angle + 90.0).abs() < EPSILON {
        return Ok(rotate_quarter(image, Quarter::CounterClockwise90));
    }

    rotate_arbitrary(image, angle)
}

#[derive(Clone, Copy)]
enum Quarter {
    Clockwise90,
    Half,
    CounterClockwise90,
}

fn rotate_quarter(image: &RgbaImage, quarter: Quarter) -> RgbaImage {
    let (width, height) = match quarter {
        Quarter::Half => (image.width, image.height),
        Quarter::Clockwise90 | Quarter::CounterClockwise90 => (image.height, image.width),
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];

    for y in 0..height {
        for x in 0..width {
            let (sx, sy) = match quarter {
                // new(x, y) = old(y, h - 1 - x)
                Quarter::Clockwise90 => (y, image.height - 1 - x),
                // new(x, y) = old(w - 1 - x, h - 1 - y)
                Quarter::Half => (image.width - 1 - x, image.height - 1 - y),
                // new(x, y) = old(w - 1 - y, x)
                Quarter::CounterClockwise90 => (image.width - 1 - y, x),
            };
            let source = image.pixel_unchecked(sx, sy);
            let index = ((y as usize * width as usize) + x as usize) * 4;
            pixels[index..index + 4].copy_from_slice(&[source.r, source.g, source.b, source.a]);
        }
    }

    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn rotate_arbitrary(image: &RgbaImage, angle: f32) -> Result<RgbaImage> {
    let radians = f64::from(angle).to_radians();
    let (sin, cos) = radians.sin_cos();
    let source_width = f64::from(image.width);
    let source_height = f64::from(image.height);

    let output_width = (source_width * cos.abs() + source_height * sin.abs())
        .ceil()
        .max(1.0) as u32;
    let output_height = (source_width * sin.abs() + source_height * cos.abs())
        .ceil()
        .max(1.0) as u32;
    let output_pixels = u64::from(output_width) * u64::from(output_height);
    if output_pixels > MAX_ROTATED_PIXELS {
        return Err(Error::Config(format!(
            "旋转后的图像 {output_width}×{output_height} 超过 {} 百万像素上限",
            MAX_ROTATED_PIXELS / 1_000_000
        )));
    }

    let source_center_x = source_width * 0.5;
    let source_center_y = source_height * 0.5;
    let output_center_x = f64::from(output_width) * 0.5;
    let output_center_y = f64::from(output_height) * 0.5;

    let mut pixels = vec![0u8; output_width as usize * output_height as usize * 4];
    for y in 0..output_height {
        let dy = f64::from(y) + 0.5 - output_center_y;
        for x in 0..output_width {
            let dx = f64::from(x) + 0.5 - output_center_x;

            // Inverse rotation: output -> source. Positive angles rotate
            // clockwise on screen.
            let sx = source_center_x + cos * dx + sin * dy;
            let sy = source_center_y - sin * dx + cos * dy;

            let color = sample_bilinear(image, sx, sy);
            let index = ((y as usize * output_width as usize) + x as usize) * 4;
            pixels[index..index + 4].copy_from_slice(&color);
        }
    }

    RgbaImage::from_rgba(output_width, output_height, pixels)
}

/// Bilinear sample at continuous pixel-edge coordinates. Returns straight RGBA.
pub fn sample_bilinear(image: &RgbaImage, x: f64, y: f64) -> [u8; 4] {
    let px = x - 0.5;
    let py = y - 0.5;
    let x0 = px.floor() as i64;
    let y0 = py.floor() as i64;
    let fx = px - x0 as f64;
    let fy = py - y0 as f64;

    let mut premultiplied = [0.0f64; 4];
    for (dy, wy) in [(0i64, 1.0 - fy), (1, fy)] {
        if wy <= 0.0 {
            continue;
        }
        for (dx, wx) in [(0i64, 1.0 - fx), (1, fx)] {
            if wx <= 0.0 {
                continue;
            }
            let weight = wx * wy;
            if let Some(color) = sample_premultiplied(image, x0 + dx, y0 + dy) {
                for channel in 0..4 {
                    premultiplied[channel] += color[channel] * weight;
                }
            }
        }
    }

    let alpha = premultiplied[3];
    if alpha <= 1e-9 {
        return [0, 0, 0, 0];
    }
    [
        (premultiplied[0] / alpha * 255.0).round().clamp(0.0, 255.0) as u8,
        (premultiplied[1] / alpha * 255.0).round().clamp(0.0, 255.0) as u8,
        (premultiplied[2] / alpha * 255.0).round().clamp(0.0, 255.0) as u8,
        (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn sample_premultiplied(image: &RgbaImage, x: i64, y: i64) -> Option<[f64; 4]> {
    if x < 0 || y < 0 || x >= i64::from(image.width) || y >= i64::from(image.height) {
        return None;
    }
    let color = image.pixel_unchecked(x as u32, y as u32);
    let alpha = f64::from(color.a) / 255.0;
    Some([
        f64::from(color.r) / 255.0 * alpha,
        f64::from(color.g) / 255.0 * alpha,
        f64::from(color.b) / 255.0 * alpha,
        alpha,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    fn quad() -> RgbaImage {
        let mut image = RgbaImage::new(2, 2, Color::BLACK);
        image.set_pixel(0, 0, Color::rgb(255, 0, 0));
        image.set_pixel(1, 0, Color::rgb(0, 255, 0));
        image.set_pixel(0, 1, Color::rgb(0, 0, 255));
        image.set_pixel(1, 1, Color::rgb(255, 255, 255));
        image
    }

    #[test]
    fn zero_rotation_is_a_clone() {
        let image = quad();
        let rotated = rotate_image(&image, 0.0).unwrap();
        assert_eq!(rotated.pixel(0, 0), image.pixel(0, 0));
        assert_eq!(rotated.width, 2);
        assert_eq!(rotated.height, 2);
    }

    #[test]
    fn ninety_degrees_is_clockwise() {
        let rotated = rotate_image(&quad(), 90.0).unwrap();
        assert_eq!((rotated.width, rotated.height), (2, 2));
        assert_eq!(rotated.pixel(0, 0), Some(Color::rgb(0, 0, 255)));
        assert_eq!(rotated.pixel(1, 0), Some(Color::rgb(255, 0, 0)));
        assert_eq!(rotated.pixel(0, 1), Some(Color::rgb(255, 255, 255)));
        assert_eq!(rotated.pixel(1, 1), Some(Color::rgb(0, 255, 0)));
    }

    #[test]
    fn one_eighty_degrees_reverses_both_axes() {
        let rotated = rotate_image(&quad(), 180.0).unwrap();
        assert_eq!(rotated.pixel(0, 0), Some(Color::rgb(255, 255, 255)));
        assert_eq!(rotated.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn rotated_dimensions_match_the_expanded_canvas() {
        assert_eq!(rotated_dimensions(200, 100, 0.0), (200, 100));
        assert_eq!(rotated_dimensions(200, 100, 90.0), (100, 200));
        assert_eq!(rotated_dimensions(200, 100, 180.0), (200, 100));
        assert_eq!(rotated_dimensions(200, 100, 270.0), (100, 200));
        assert_eq!(rotated_dimensions(2, 2, 45.0), (3, 3));
    }

    #[test]
    fn arbitrary_rotation_expands_and_adds_transparent_corners() {
        let rotated = rotate_image(&quad(), 45.0).unwrap();
        assert!(rotated.width > 2);
        assert!(rotated.height > 2);
        assert!(
            rotated
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] < 255)
        );
    }
}
