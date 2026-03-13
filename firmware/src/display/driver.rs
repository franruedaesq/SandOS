extern crate alloc;
use alloc::boxed::Box;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb565,
    Pixel,
};
use esp_hal::{
    gpio::Output,
    spi::master::Spi,
    Async,
};
use embedded_hal_async::spi::SpiBus;

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;
const DISPLAY_BUF_SIZE: usize = (DISPLAY_WIDTH as usize) * (DISPLAY_HEIGHT as usize);

pub struct TftDisplay {
    spi: Spi<'static, Async>,
    dc: Output<'static>,
    cs: Output<'static>,
    buffer: Box<[u16; DISPLAY_BUF_SIZE]>,
}

impl TftDisplay {
    pub fn new(spi: Spi<'static, Async>, dc: Output<'static>, cs: Output<'static>) -> Self {
        // Allocate framebuffer in PSRAM
        let buffer = unsafe {
            let layout = core::alloc::Layout::new::<[u16; DISPLAY_BUF_SIZE]>();
            let ptr = alloc::alloc::alloc_zeroed(layout) as *mut [u16; DISPLAY_BUF_SIZE];
            if ptr.is_null() {
                alloc::alloc::handle_alloc_error(layout);
            }
            Box::from_raw(ptr)
        };

        Self {
            spi,
            dc,
            cs,
            buffer,
        }
    }

    async fn write_cmd(&mut self, cmd: u8) {
        self.dc.set_low();
        self.cs.set_low();
        let _ = self.spi.write(&[cmd]).await;
        self.cs.set_high();
    }

    async fn write_data(&mut self, data: &[u8]) {
        self.dc.set_high();
        self.cs.set_low();
        let _ = self.spi.write(data).await;
        self.cs.set_high();
    }

    pub async fn init(&mut self) {
        // Basic ILI9341 Initialization
        self.write_cmd(0x01).await; // Software Reset
        Timer::after(Duration::from_millis(5)).await;
        self.write_cmd(0x11).await; // Sleep Out
        Timer::after(Duration::from_millis(120)).await;

        self.write_cmd(0x36).await; // Memory Access Control
        self.write_data(&[0x08]).await; // BGR

        self.write_cmd(0x3A).await; // Pixel Format
        self.write_data(&[0x55]).await; // 16-bit

        self.write_cmd(0x29).await; // Display On
        Timer::after(Duration::from_millis(20)).await;
    }

    pub async fn flush(&mut self) -> Result<(), ()> {
        // Set column address
        self.write_cmd(0x2A).await;
        self.write_data(&[0, 0, (DISPLAY_WIDTH >> 8) as u8, (DISPLAY_WIDTH & 0xFF) as u8]).await;

        // Set page address
        self.write_cmd(0x2B).await;
        self.write_data(&[0, 0, (DISPLAY_HEIGHT >> 8) as u8, (DISPLAY_HEIGHT & 0xFF) as u8]).await;

        // Memory write
        self.write_cmd(0x2C).await;

        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(
                self.buffer.as_ptr() as *const u8,
                self.buffer.len() * 2,
            )
        };

        self.dc.set_high();
        self.cs.set_low();
        for chunk in bytes.chunks(8192) {
            if self.spi.write(chunk).await.is_err() {
                self.cs.set_high();
                return Err(());
            }
        }
        self.cs.set_high();
        Ok(())
    }
}

impl OriginDimensions for TftDisplay {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for TftDisplay {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        use embedded_graphics::prelude::IntoStorage;
        let c = color.into_storage();
        let c_swap = c.to_be();
        self.buffer.fill(c_swap);
        Ok(())
    }

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        use embedded_graphics::prelude::IntoStorage;
        for Pixel(coord, color) in pixels {
            if coord.x < 0 || coord.y < 0 {
                continue;
            }
            let x = coord.x as usize;
            let y = coord.y as usize;

            if x >= DISPLAY_WIDTH as usize || y >= DISPLAY_HEIGHT as usize {
                continue;
            }

            let idx = x + y * DISPLAY_WIDTH as usize;
            let c = color.into_storage();
            self.buffer[idx] = c.to_be();
        }
        Ok(())
    }
}
