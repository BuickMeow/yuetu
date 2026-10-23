//! Image resampling.
//!
//! M1 ships the two algorithms that matter for a converter: area resampling
//! (stable colors when shrinking a photo) and nearest neighbor (crisp pixel
//! art). The `ResizeAlgorithm` enum is the extension point for the rest of the
//! original tool's filters.

use crate::error::{Error, Result};
use crate::image::RgbaImage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResizeAlgorithm {
    /// Weighted average of every source pixel touched by the output footprint.
    #[default]
    Area,
    /// Pick the nearest source pixel; preserves hard pixel edges.
    Nearest,
}

impl ResizeAlgorithm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Area => "区域平均",
            Self::Nearest => "最近邻",
        }
    }
}

pub fn resize(
    image: &RgbaImage,
    target_width: u32,
    target_height: u32,
    algorithm: ResizeAlgorithm,
) -> Result<RgbaImage> {
    if target_width == 0 || target_height == 0 {
        return Err(Error::Config("缩放后的宽高必须大于 0".into()));
    }
    if image.is_empty() {
        return Err(Error::InvalidImage("源图为空".into()));
    }
    if image.width == target_width && image.height == target_height {
        return Ok(image.clone());
    }

    match algorithm {
        ResizeAlgorithm::Nearest => Ok(resize_nearest(image, target_width, target_height)),
        ResizeAlgorithm::Area => Ok(resize_area(image, target_width, target_height)),
    }
}

fn resize_nearest(image: &RgbaImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let mut out = vec![0u8; dst_w as usize * dst_h as usize * 4];
    let x_ratio = f64::from(image.width) / f64::from(dst_w);
    let y_ratio = f64::from(image.height) / f64::from(dst_h);

    for y in 0..dst_h {
        let sy = (((f64::from(y) + 0.5) * y_ratio).floor() as u32).min(image.height - 1);
        for x in 0..dst_w {
            let sx = (((f64::from(x) + 0.5) * x_ratio).floor() as u32).min(image.width - 1);
            let src = image.pixel_unchecked(sx, sy);
            let i = ((y as usize * dst_w as usize) + x as usize) * 4;
            out[i..i + 4].copy_from_slice(&[src.r, src.g, src.b, src.a]);
        }
    }

    RgbaImage {
        width: dst_w,
        height: dst_h,
        pixels: out,
    }
}

fn resize_area(image: &RgbaImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let mut out = vec![0u8; dst_w as usize * dst_h as usize * 4];
    let x_ratio = f64::from(image.width) / f64::from(dst_w);
    let y_ratio = f64::from(image.height) / f64::from(dst_h);

    for y in 0..dst_h {
        let sy0 = f64::from(y) * y_ratio;
        let sy1 = f64::from(y + 1) * y_ratio;
        let iy0 = sy0.floor() as u32;
        let iy1 = (sy1.ceil() as u32).min(image.height);

        for x in 0..dst_w {
            let sx0 = f64::from(x) * x_ratio;
            let sx1 = f64::from(x + 1) * x_ratio;
            let ix0 = sx0.floor() as u32;
            let ix1 = (sx1.ceil() as u32).min(image.width);

            let mut sum = [0.0f64; 4];
            let mut weight_sum = 0.0f64;

            for sy in iy0..iy1 {
                let wy = (f64::from(sy) + 1.0).min(sy1) - f64::from(sy).max(sy0);
                if wy <= 0.0 {
                    continue;
                }
                for sx in ix0..ix1 {
                    let wx = (f64::from(sx) + 1.0).min(sx1) - f64::from(sx).max(sx0);
                    if wx <= 0.0 {
                        continue;
                    }
                    let w = wx * wy;
                    let c = image.pixel_unchecked(sx, sy);
                    sum[0] += f64::from(c.r) * w;
                    sum[1] += f64::from(c.g) * w;
                    sum[2] += f64::from(c.b) * w;
                    sum[3] += f64::from(c.a) * w;
                    weight_sum += w;
                }
            }

            let i = ((y as usize * dst_w as usize) + x as usize) * 4;
            if weight_sum > 0.0 {
                out[i] = (sum[0] / weight_sum).round().clamp(0.0, 255.0) as u8;
                out[i + 1] = (sum[1] / weight_sum).round().clamp(0.0, 255.0) as u8;
                out[i + 2] = (sum[2] / weight_sum).round().clamp(0.0, 255.0) as u8;
                out[i + 3] = (sum[3] / weight_sum).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    RgbaImage {
        width: dst_w,
        height: dst_h,
        pixels: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn area_resize_averages_a_uniform_image() {
        let image = RgbaImage::new(4, 4, Color::rgb(10, 20, 30));
        let resized = resize(&image, 2, 2, ResizeAlgorithm::Area).unwrap();
        assert_eq!(resized.pixel(0, 0), Some(Color::rgb(10, 20, 30)));
        assert_eq!(resized.pixel(1, 1), Some(Color::rgb(10, 20, 30)));
    }

    #[test]
    fn nearest_resize_preserves_colors() {
        let mut image = RgbaImage::new(2, 1, Color::BLACK);
        image.set_pixel(1, 0, Color::WHITE);
        let resized = resize(&image, 4, 1, ResizeAlgorithm::Nearest).unwrap();
        assert_eq!(resized.pixel(0, 0), Some(Color::BLACK));
        assert_eq!(resized.pixel(3, 0), Some(Color::WHITE));
    }

    #[test]
    fn same_size_is_a_clone() {
        let image = RgbaImage::new(3, 2, Color::rgb(1, 2, 3));
        let resized = resize(&image, 3, 2, ResizeAlgorithm::Area).unwrap();
        assert_eq!(resized.width, 3);
        assert_eq!(resized.height, 2);
        assert_eq!(resized.pixel(2, 1), Some(Color::rgb(1, 2, 3)));
    }
}
