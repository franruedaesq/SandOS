import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

# Make sure imports are correct
code = code.replace("use embedded_graphics::{\n    draw_target::DrawTarget,", "use embedded_graphics::{\n    draw_target::DrawTarget,\n    pixelcolor::Rgb565,\n    pixelcolor::RgbColor,\n    pixelcolor::BinaryColor,")
code = code.replace("use esp_hal::{", "use esp_hal::{\n    gpio::{Level, Output},")

# Update the display task signature
code = re.sub(
    r"async fn display_task\(\s*i2c0: I2C0,\s*sda: GpioPin<8>,\s*scl: GpioPin<9>,\s*\) \{",
    "async fn display_task(\n    spi2: esp_hal::peripherals::SPI2,\n    sck: GpioPin<9>,\n    mosi: GpioPin<8>,\n    dc: GpioPin<10>,\n    cs: GpioPin<11>,\n    rst: GpioPin<12>,\n) {",
    code,
    flags=re.MULTILINE
)

# Update display dimensions
code = code.replace("pub const DISPLAY_WIDTH: u32 = 128;", "pub const DISPLAY_WIDTH: u32 = 240;")
code = code.replace("pub const DISPLAY_HEIGHT: u32 = 64;", "pub const DISPLAY_HEIGHT: u32 = 240;")
code = code.replace("const DISPLAY_BUF_SIZE: usize = (DISPLAY_WIDTH as usize) * (DISPLAY_HEIGHT as usize / 8);", "const DISPLAY_BUF_SIZE: usize = (DISPLAY_WIDTH as usize) * (DISPLAY_HEIGHT as usize) * 2;")

# Replace the I2C/SSD1306 init with SPI/ST7789 init
code = re.sub(
    r"let mut cfg = I2cConfig::default\(\);.*?let mut oled = OledDisplay::new\(i2c\);\n    oled\.init\(\)\.await;",
    """let mut config = esp_hal::spi::master::Config::default();
    config.frequency = 40_u32.MHz();
    config.mode = esp_hal::spi::SpiMode::Mode3;

    let spi = esp_hal::spi::master::Spi::new(spi2, config)
        .expect("SPI init failed")
        .with_sck(sck)
        .with_mosi(mosi)
        .into_async();

    let dc_pin = Output::new(dc, Level::Low);
    let cs_pin = Output::new(cs, Level::High);
    let rst_pin = Output::new(rst, Level::High);

    let mut oled = St7789Display::new(spi, dc_pin, cs_pin, rst_pin);
    oled.init().await;""",
    code,
    flags=re.DOTALL
)

# Replace spawn signature
code = re.sub(
    r"pub fn spawn_display_task\(\s*spawner: Spawner,\s*i2c0: I2C0,\s*sda: GpioPin<8>,\s*scl: GpioPin<9>,\s*boot_btn: Input<'static>,\s*\) \{.*?spawner\.spawn\(display_task\(i2c0, sda, scl\)\)\.unwrap\(\);\n\}",
    """pub fn spawn_display_task(
    spawner: Spawner,
    spi2: esp_hal::peripherals::SPI2,
    sck: GpioPin<9>,
    mosi: GpioPin<8>,
    dc: GpioPin<10>,
    cs: GpioPin<11>,
    rst: GpioPin<12>,
    boot_btn: Input<'static>,
) {
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(spi2, sck, mosi, dc, cs, rst)).unwrap();
}""",
    code,
    flags=re.DOTALL
)

st7789_struct_static_var = """
// Large framebuffer in global BSS
static mut DISPLAY_BUFFER: [u8; 240 * 240 * 2] = [0; 240 * 240 * 2];

struct St7789Display {
    spi: esp_hal::spi::master::Spi<'static, esp_hal::Async>,
    dc: Output<'static>,
    cs: Output<'static>,
    rst: Output<'static>,
}

impl St7789Display {
    fn new(spi: esp_hal::spi::master::Spi<'static, esp_hal::Async>, dc: Output<'static>, cs: Output<'static>, rst: Output<'static>) -> Self {
        Self {
            spi,
            dc,
            cs,
            rst,
        }
    }

    async fn write_command(&mut self, cmd: u8) {
        self.dc.set_low();
        self.cs.set_low();
        let _ = self.spi.write_bytes(&[cmd]).await;
        self.cs.set_high();
    }

    async fn write_data(&mut self, data: &[u8]) {
        self.dc.set_high();
        self.cs.set_low();
        let _ = self.spi.write_bytes(data).await;
        self.cs.set_high();
    }

    async fn init(&mut self) {
        // Hard reset
        self.rst.set_low();
        Timer::after(Duration::from_millis(50)).await;
        self.rst.set_high();
        Timer::after(Duration::from_millis(50)).await;

        self.write_command(0x01).await; // SWRESET
        Timer::after(Duration::from_millis(150)).await;

        self.write_command(0x11).await; // SLPOUT
        Timer::after(Duration::from_millis(10)).await;

        self.write_command(0x3A).await; // COLMOD
        self.write_data(&[0x55]).await; // 16-bit color

        self.write_command(0x36).await; // MADCTL
        self.write_data(&[0x00]).await; // RGB order

        self.write_command(0x21).await; // INVON

        self.write_command(0x13).await; // NORON

        self.write_command(0x29).await; // DISPON
        Timer::after(Duration::from_millis(10)).await;

        let _ = self.clear(Rgb565::BLACK);
        let _ = self.flush().await;
    }

    async fn flush(&mut self) -> Result<(), ()> {
        self.write_command(0x2A).await; // CASET
        self.write_data(&[0x00, 0x00, (DISPLAY_WIDTH >> 8) as u8, (DISPLAY_WIDTH & 0xFF) as u8]).await;

        self.write_command(0x2B).await; // RASET
        self.write_data(&[0x00, 0x00, (DISPLAY_HEIGHT >> 8) as u8, (DISPLAY_HEIGHT & 0xFF) as u8]).await;

        self.write_command(0x2C).await; // RAMWR

        self.dc.set_high();
        self.cs.set_low();

        let buffer = unsafe { &DISPLAY_BUFFER };

        // Write chunks
        let chunks = buffer.chunks(4096);
        for chunk in chunks {
            let _ = self.spi.write_bytes(chunk).await;
        }

        self.cs.set_high();
        Ok(())
    }
}

impl OriginDimensions for St7789Display {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for St7789Display {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let r = color.r();
        let g = color.g();
        let b = color.b();
        let high = (r << 3) | (g >> 3);
        let low = ((g & 0x07) << 5) | b;

        let buffer = unsafe { &mut DISPLAY_BUFFER };

        for i in (0..buffer.len()).step_by(2) {
            buffer[i] = high;
            buffer[i+1] = low;
        }
        Ok(())
    }

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let buffer = unsafe { &mut DISPLAY_BUFFER };
        for Pixel(coord, color) in pixels {
            if coord.x < 0 || coord.y < 0 {
                continue;
            }
            let x = coord.x as usize;
            let y = coord.y as usize;

            if x >= DISPLAY_WIDTH as usize || y >= DISPLAY_HEIGHT as usize {
                continue;
            }

            let idx = (x + y * DISPLAY_WIDTH as usize) * 2;
            let r = color.r();
            let g = color.g();
            let b = color.b();

            // Rgb565 format: rrrrrggg gggbbbbb
            buffer[idx] = (r << 3) | (g >> 3);
            buffer[idx+1] = ((g & 0x07) << 5) | b;
        }
        Ok(())
    }
}
"""

code = re.sub(r"struct OledDisplay \{.*?impl DrawTarget for OledDisplay \{.*?\n\}\n\}", st7789_struct_static_var, code, flags=re.DOTALL)
code = re.sub(r"OledDisplay", "St7789Display", code)

# Replace instances of BinaryColor with Rgb565 in function calls
code = code.replace("BinaryColor::On", "Rgb565::WHITE")
code = code.replace("BinaryColor::Off", "Rgb565::BLACK")
code = code.replace("BinaryColor", "Rgb565")

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
