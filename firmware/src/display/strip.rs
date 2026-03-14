//! Strip-based DrawTarget for chunked DMA rendering.
//!
//! Instead of drawing directly to the display, the UI renders into a small
//! RAM buffer covering a horizontal strip of the screen. After each strip is
//! rendered, its contents are DMA-sent to the ILI9341. This keeps memory usage
//! low (~12.8 KB) while enabling bulk DMA transfers.

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    Pixel,
};

use super::{DISPLAY_WIDTH, DISPLAY_HEIGHT};

/// A small RAM buffer representing one horizontal strip of the screen.
///
/// `size()` returns the full screen dimensions so that embedded-graphics
/// computes primitive geometry correctly. `draw_iter()` silently discards
/// pixels that fall outside the current strip's Y range.
pub struct StripBuffer {
    /// Pixel data in big-endian RGB565 (ILI9341 native byte order).
    buf: [u8; Self::MAX_BYTES],
    /// Current strip Y offset in screen coordinates.
    strip_y: i32,
    /// Height of the current strip (may be less than max at screen bottom).
    strip_h: i32,
    /// Screen width in pixels.
    width: i32,
    /// Background color used to clear each strip.
    bg: Rgb565,
}

impl StripBuffer {
    /// Maximum strip size: 40 scanlines × 320 pixels × 2 bytes/pixel.
    const MAX_BYTES: usize = 30 * (DISPLAY_WIDTH as usize) * 2;

    pub fn new(width: i32, height: i32, bg: Rgb565) -> Self {
        let mut s = Self {
            buf: [0u8; Self::MAX_BYTES],
            strip_y: 0,
            strip_h: height,
            width,
            bg,
        };
        s.clear_to_bg();
        s
    }

    /// Prepare the buffer for a new strip at the given Y offset.
    pub fn begin_strip(&mut self, y: i32, h: i32) {
        self.strip_y = y;
        self.strip_h = h;
        self.clear_to_bg();
    }

    /// Fill the buffer with the background color.
    fn clear_to_bg(&mut self) {
        let raw = self.bg.into_storage();
        let hi = (raw >> 8) as u8;
        let lo = raw as u8;
        let pixel_count = (self.strip_h as usize) * (self.width as usize);
        for i in 0..pixel_count {
            self.buf[i * 2] = hi;
            self.buf[i * 2 + 1] = lo;
        }
    }

    /// Get the pixel data as a byte slice for DMA transfer.
    pub fn as_bytes(&self) -> &[u8] {
        let len = (self.strip_h as usize) * (self.width as usize) * 2;
        &self.buf[..len]
    }

    /// Write a single pixel if it falls within the current strip.
    #[inline]
    fn set_pixel(&mut self, x: i32, y: i32, color: Rgb565) {
        let local_y = y - self.strip_y;
        if x < 0 || x >= self.width || local_y < 0 || local_y >= self.strip_h {
            return;
        }
        let idx = ((local_y as usize) * (self.width as usize) + (x as usize)) * 2;
        let raw = color.into_storage();
        self.buf[idx] = (raw >> 8) as u8;
        self.buf[idx + 1] = raw as u8;
    }
}

impl OriginDimensions for StripBuffer {
    fn size(&self) -> Size {
        // Return full screen size so embedded-graphics primitives compute
        // geometry correctly across the entire display.
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for StripBuffer {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            self.set_pixel(point.x, point.y, color);
        }
        Ok(())
    }

    fn fill_contiguous<I>(
        &mut self,
        area: &Rectangle,
        colors: I,
    ) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        // Clip the area to the current strip for efficiency
        let strip_top = self.strip_y;
        let strip_bot = self.strip_y + self.strip_h;

        let area_left = area.top_left.x;
        let area_top = area.top_left.y;
        let area_w = area.size.width as i32;
        let area_h = area.size.height as i32;

        // If the area is completely outside the strip, consume and discard
        if area_top + area_h <= strip_top || area_top >= strip_bot
            || area_left + area_w <= 0 || area_left >= self.width
        {
            // Must consume the iterator even when discarding
            for _ in colors {}
            return Ok(());
        }

        // Otherwise, iterate and filter
        let mut x = area_left;
        let mut y = area_top;
        for color in colors {
            if x >= area_left + area_w {
                x = area_left;
                y += 1;
            }
            self.set_pixel(x, y, color);
            x += 1;
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let strip_top = self.strip_y;
        let strip_bot = self.strip_y + self.strip_h;

        // Clip to strip and screen bounds
        let x0 = area.top_left.x.max(0);
        let y0 = area.top_left.y.max(strip_top);
        let x1 = (area.top_left.x + area.size.width as i32).min(self.width);
        let y1 = (area.top_left.y + area.size.height as i32).min(strip_bot);

        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }

        let raw = color.into_storage();
        let hi = (raw >> 8) as u8;
        let lo = raw as u8;

        for y in y0..y1 {
            let local_y = y - self.strip_y;
            let row_start = (local_y as usize) * (self.width as usize);
            for x in x0..x1 {
                let idx = (row_start + x as usize) * 2;
                self.buf[idx] = hi;
                self.buf[idx + 1] = lo;
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let raw = color.into_storage();
        let hi = (raw >> 8) as u8;
        let lo = raw as u8;
        let pixel_count = (self.strip_h as usize) * (self.width as usize);
        for i in 0..pixel_count {
            self.buf[i * 2] = hi;
            self.buf[i * 2 + 1] = lo;
        }
        Ok(())
    }
}
