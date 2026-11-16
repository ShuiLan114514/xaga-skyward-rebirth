//! A photo of the phone itself, shown in the home page's device card where
//! there is one: the packer puts `devices/<codename>.png` in the ramdisk as
//! `/skyward/portrait.png`, and a phone whose build carries none keeps the
//! outline the stock Settings draws.

use crate::canvas::Canvas;
use crate::color::Rgba;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// A straight-alpha RGBA image, as the packer put it in the ramdisk.
pub struct Portrait {
    pub w: u32,
    pub h: u32,
    pixels: Vec<u8>,
}

impl Portrait {
    /// The portrait at `path`, or none where the build carries none. A file
    /// that is there but cannot be read is an error worth saying, not one
    /// worth giving the page up for.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("open {}", path.display())),
        };
        Self::read(file).with_context(|| format!("read {}", path.display())).map(Some)
    }

    pub fn read<R: std::io::Read>(reader: R) -> Result<Self> {
        let decoder = png::Decoder::new(reader);
        let mut reader = decoder.read_info()?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer)?;
        buffer.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        if info.bit_depth != png::BitDepth::Eight {
            bail!("the portrait is {w}x{h} at {:?}, not 8-bit", info.bit_depth);
        }
        let pixels = match info.color_type {
            png::ColorType::Rgba => buffer,
            png::ColorType::Rgb => buffer.as_chunks::<3>().0.iter().flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255]).collect(),
            other => bail!("the portrait is {w}x{h} as {other:?}, which the recovery cannot draw"),
        };
        Ok(Portrait { w, h, pixels })
    }

    /// Draw the portrait over the circle at (`cx`, `cy`) of radius `r`,
    /// scaled to the circle's square, its transparent room left as it is.
    /// A pixel of the circle is covered as [`Canvas::fill_circle`] covers
    /// it, and the portrait's own alpha blends within that.
    pub fn draw(&self, c: &mut Canvas, cx: f32, cy: f32, r: f32) {
        if r <= 0.0 {
            return;
        }
        let (x0, x1) = ((cx - r - 1.0).floor() as i32, (cx + r + 1.0).ceil() as i32);
        let (y0, y1) = ((cy - r - 1.0).floor() as i32, (cy + r + 1.0).ceil() as i32);
        for py in y0..y1 {
            let dy = py as f32 + 0.5 - cy;
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let cov = (0.5 - ((dx * dx + dy * dy).sqrt() - r)).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                // The circle's square is the whole image, corner to corner.
                let u = (px as f32 + 0.5 - (cx - r)) / (2.0 * r);
                let v = (py as f32 + 0.5 - (cy - r)) / (2.0 * r);
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    continue;
                }
                let color = self.sample(u, v);
                if color.a > 0 {
                    c.blend(px, py, color, cov);
                }
            }
        }
    }

    /// The colour at (`u`, `v`) of the image, from 0 at its left or top to 1
    /// at its right or bottom, bilinear over its four nearest pixels. The
    /// alpha is weighed in first so a transparent edge does not darken the
    /// colour the phone's own edge keeps.
    fn sample(&self, u: f32, v: f32) -> Rgba {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let mut weighed = [0.0f32; 3];
        let mut alpha = 0.0f32;
        for (px, py, weight) in [(x0, y0, (1.0 - fx) * (1.0 - fy)), (x1, y0, fx * (1.0 - fy)), (x0, y1, (1.0 - fx) * fy), (x1, y1, fx * fy)] {
            let at = ((py * self.w + px) * 4) as usize;
            let a = self.pixels[at + 3] as f32 / 255.0;
            alpha += a * weight;
            for (n, value) in weighed.iter_mut().enumerate() {
                *value += self.pixels[at + n] as f32 * a * weight;
            }
        }
        if alpha <= 0.0 {
            return Rgba { r: 0, g: 0, b: 0, a: 0 };
        }
        let channel = |value: f32| (value / alpha).round().clamp(0.0, 255.0) as u8;
        Rgba { r: channel(weighed[0]), g: channel(weighed[1]), b: channel(weighed[2]), a: (alpha * 255.0).round().clamp(0.0, 255.0) as u8 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&color.repeat((w * h) as usize)).unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn a_portrait_is_read_as_straight_rgba() {
        let portrait = Portrait::read(std::io::Cursor::new(png_of(2, 1, [10, 20, 30, 40]))).unwrap();
        assert_eq!((portrait.w, portrait.h), (2, 1));
        assert_eq!(portrait.pixels, [10, 20, 30, 40, 10, 20, 30, 40]);
        assert_eq!(portrait.sample(0.0, 0.0), Rgba { r: 10, g: 20, b: 30, a: 40 });
    }
}

#[cfg(test)]
mod xaga {
    use super::*;

    #[test]
    fn the_phone_s_photo_keeps_its_colours() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/xaga.png");
        let portrait = Portrait::load(&path).unwrap().expect("the xaga portrait is in the repository");
        let blue = portrait.sample(0.4, 0.5);
        assert!(blue.b > blue.r && blue.b > 100, "the phone's blue back: {blue:?}");
        assert!(portrait.sample(0.0, 0.0).a == 0, "the room beside the phones is clear");
    }
}
