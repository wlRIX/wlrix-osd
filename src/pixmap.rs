// SPDX-License-Identifier: GPL-3.0-or-later
//! A notification's picture, however it arrived.
//!
//! Three sources feed one type. A themed icon name and an image file are decoded by
//! [`wlrix_ui::image`]; `image-data` is raw bytes straight off the bus. The painter should not
//! care which, so all of them become a [`Pixmap`] here.
//!
//! # The wire format
//!
//! `image-data` is GdkPixbuf's layout, sent as `(iiibiiay)`: width, height, *rowstride*, whether
//! there is alpha, bits per sample, channels, then the bytes. Two things about it are easy to
//! get wrong and neither looks wrong on screen until the right picture comes along:
//!
//! - **Rows are `rowstride` bytes apart, not `width * channels`.** GdkPixbuf pads each row to a
//!   multiple of four, so a 3-channel image whose width is not a multiple of four has padding
//!   between rows. Ignoring it shears the picture diagonally -- and only for some widths.
//! - **The last row need not be padded.** A buffer is `rowstride * (height - 1)` plus one
//!   unpadded row, so checking the length against `rowstride * height` refuses valid images.
//!
//! The bytes are R, G, B (, A) in that order and **not** premultiplied -- unlike the tray's
//! `IconPixmap`, which is big-endian ARGB. Both are converted to premultiplied native ARGB once,
//! here, which is what [`wlrix_ui::canvas::Canvas::blend_premultiplied`] wants.
//!
//! # Why this is not shared with the tray
//!
//! The tray's `pixmap.rs` decodes a different wire format and scales to a square cell; this one
//! keeps the aspect ratio, because a notification's picture is as often a photo or an album
//! cover as an icon. They share the averaging loop and nothing else, which is not enough to be
//! worth a crate.

use wlrix_ui::canvas::Canvas;

/// The largest picture believed. Album art is commonly 512 square and some players send 1000;
/// past this it is a bug on the other end of the bus, and believing it costs a great deal of
/// memory for something drawn 48 pixels across.
const MAX_SIDE: i32 = 2048;

/// A decoded picture: premultiplied native-endian ARGB, ready to composite.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pixels: Vec<u32>,
}

/// The six numbers in front of the bytes in `image-data`.
#[derive(Debug, Clone, Copy)]
pub struct Raw {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bits_per_sample: i32,
    pub channels: i32,
}

impl Pixmap {
    /// Whether there is anything to draw.
    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0 || self.pixels.is_empty()
    }

    /// One pixel, or fully transparent off the edge.
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[(y * self.width + x) as usize]
    }

    /// Decode `image-data`.
    ///
    /// `None` for anything that does not describe a picture this can read: a dimension out of
    /// range, a sample size other than eight bits (the only one GdkPixbuf has ever produced), a
    /// channel count that disagrees with `has_alpha`, or too few bytes for what the header
    /// claims.
    pub fn from_raw(raw: Raw, bytes: &[u8]) -> Option<Self> {
        let Raw {
            width,
            height,
            rowstride,
            has_alpha,
            bits_per_sample,
            channels,
        } = raw;
        if width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        if bits_per_sample != 8 || channels != if has_alpha { 4 } else { 3 } {
            return None;
        }
        let row_bytes = width as usize * channels as usize;
        if rowstride < 0 || (rowstride as usize) < row_bytes {
            return None;
        }
        let needed = rowstride as usize * (height as usize - 1) + row_bytes;
        if bytes.len() < needed {
            return None;
        }

        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height as usize {
            let row = &bytes[y * rowstride as usize..][..row_bytes];
            for pixel in row.chunks_exact(channels as usize) {
                let a = if has_alpha { pixel[3] } else { 0xff };
                let premultiply = |channel: u8| (channel as u32 * a as u32 + 127) / 255;
                pixels.push(
                    ((a as u32) << 24)
                        | (premultiply(pixel[0]) << 16)
                        | (premultiply(pixel[1]) << 8)
                        | premultiply(pixel[2]),
                );
            }
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Copy a decoded file into the same representation. [`wlrix_ui::image::Image`] is already
    /// premultiplied native ARGB, so this is a transcription rather than a conversion.
    pub fn from_image(image: &wlrix_ui::image::Image) -> Self {
        Self {
            width: image.width,
            height: image.height,
            pixels: image.pixels().to_vec(),
        }
    }

    /// This picture shrunk to fit inside `size` square, keeping its shape.
    ///
    /// Never enlarged: a 32-pixel icon drawn at 48 is blurrier and no more legible, so it is
    /// drawn at its own size and centered instead. Area averaging in premultiplied space, so a
    /// shrunk photo does not grow a dark halo around its transparent edges.
    pub fn fit(&self, size: i32) -> Self {
        if self.is_empty() || size <= 0 {
            return Self::default();
        }
        if self.width <= size && self.height <= size {
            return self.clone();
        }
        let (width, height) = if self.width >= self.height {
            (size, (self.height * size / self.width).max(1))
        } else {
            ((self.width * size / self.height).max(1), size)
        };
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let (y0, y1) = span(y, height, self.height);
            for x in 0..width {
                let (x0, x1) = span(x, width, self.width);
                let (mut a, mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let pixel = self.get(sx, sy);
                        a += (pixel >> 24) & 0xff;
                        r += (pixel >> 16) & 0xff;
                        g += (pixel >> 8) & 0xff;
                        b += pixel & 0xff;
                        n += 1;
                    }
                }
                let n = n.max(1);
                pixels.push(((a / n) << 24) | ((r / n) << 16) | ((g / n) << 8) | (b / n));
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Every pixel recolored to `color`, keeping its coverage.
    ///
    /// For symbolic icons, which are drawn in one gray on the understanding that whoever shows
    /// them recolors them to the text color -- left as they are, a dark scheme draws them dark
    /// gray on dark gray.
    pub fn tinted(&self, color: wlrix_ui::color::Rgb) -> Self {
        let (r, g, b) = color.channels();
        let pixels = self
            .pixels
            .iter()
            .map(|&pixel| {
                let a = pixel >> 24;
                let scale = |channel: u8| (channel as u32 * a + 127) / 255;
                (a << 24) | (scale(r) << 16) | (scale(g) << 8) | scale(b)
            })
            .collect();
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Draw with the top-left at `(x, y)`, compositing over what is there.
    pub fn draw(&self, canvas: &mut Canvas, x: i32, y: i32) {
        for row in 0..self.height {
            for column in 0..self.width {
                let pixel = self.get(column, row);
                if pixel >> 24 == 0 {
                    continue;
                }
                canvas.blend_premultiplied(x + column, y + row, pixel);
            }
        }
    }
}

/// The half-open source span a destination pixel covers, never empty.
fn span(index: i32, out_of: i32, source: i32) -> (i32, i32) {
    let start = index * source / out_of;
    let span = (index + 1) * source;
    let end = ((span + out_of - 1) / out_of).max(start + 1);
    (start, end.min(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(width: i32, height: i32, rowstride: i32, has_alpha: bool) -> Raw {
        Raw {
            width,
            height,
            rowstride,
            has_alpha,
            bits_per_sample: 8,
            channels: if has_alpha { 4 } else { 3 },
        }
    }

    #[test]
    fn rows_are_rowstride_apart() {
        // 1x2 RGB with each row padded to four bytes. Reading it as packed would put the
        // padding byte into the second pixel's red.
        let bytes = [10, 20, 30, 0xEE, 40, 50, 60];
        let image = Pixmap::from_raw(raw(1, 2, 4, false), &bytes).unwrap();
        assert_eq!(image.get(0, 0), 0xff0a141e);
        assert_eq!(image.get(0, 1), 0xff28323c);
    }

    #[test]
    fn the_last_row_need_not_be_padded() {
        // Seven bytes, not eight: `rowstride * height` would wrongly refuse this.
        let bytes = [1, 2, 3, 0, 4, 5, 6];
        assert!(Pixmap::from_raw(raw(1, 2, 4, false), &bytes).is_some());
    }

    #[test]
    fn nonsense_headers_are_refused() {
        let bytes = vec![0u8; 64];
        assert!(Pixmap::from_raw(raw(0, 1, 4, true), &bytes).is_none());
        assert!(
            Pixmap::from_raw(raw(1, 1, 2, true), &bytes).is_none(),
            "rowstride too short"
        );
        let mut sixteen = raw(1, 1, 8, true);
        sixteen.bits_per_sample = 16;
        assert!(Pixmap::from_raw(sixteen, &bytes).is_none());
        let mut confused = raw(1, 1, 4, true);
        confused.channels = 3;
        assert!(
            Pixmap::from_raw(confused, &bytes).is_none(),
            "alpha with three channels"
        );
        assert!(Pixmap::from_raw(raw(4096, 1, 16384, true), &bytes).is_none());
    }

    #[test]
    fn alpha_is_premultiplied() {
        let image = Pixmap::from_raw(raw(1, 1, 4, true), &[255, 255, 255, 0]).unwrap();
        assert_eq!(
            image.get(0, 0),
            0,
            "fully transparent white is nothing at all"
        );
    }

    #[test]
    fn tinting_keeps_coverage_and_takes_the_color() {
        let gray =
            Pixmap::from_raw(raw(2, 1, 8, true), &[46, 52, 54, 255, 46, 52, 54, 128]).unwrap();
        let white = gray.tinted(wlrix_ui::color::Rgb(0xff_ffffff));
        assert_eq!(white.get(0, 0), 0xffffffff);
        // Half covered: half of white, premultiplied.
        assert_eq!(white.get(1, 0), 0x80808080);
    }

    #[test]
    fn fitting_keeps_the_shape_and_never_enlarges() {
        let wide = Pixmap::from_raw(raw(100, 50, 400, true), &vec![255; 400 * 50]).unwrap();
        let fitted = wide.fit(48);
        assert_eq!((fitted.width, fitted.height), (48, 24));
        assert_eq!(fitted.get(10, 10), 0xffffffff);

        let tall = Pixmap::from_raw(raw(10, 40, 40, true), &vec![255; 40 * 40]).unwrap();
        assert_eq!((tall.fit(20).width, tall.fit(20).height), (5, 20));

        let small = Pixmap::from_raw(raw(16, 16, 64, true), &vec![255; 64 * 16]).unwrap();
        assert_eq!(small.fit(48), small);
    }
}
