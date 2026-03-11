//! Phase 2 — OLED Display Driver.
//!
//! Drives a 0.96" I2C OLED panel (`SSD1306`, 128×64) on the ESP32-S3.
//! Wiring used by this project:
//! - `VCC` -> `3V3`
//! - `GND` -> `GND`
//! - `SCL` -> `GPIO9`
//! - `SDA` -> `GPIO8`
//!
//! ## Architecture
//!
//! ```text
//! Wasm guest calls host_draw_eye(Happy)
//!      │
//!      ▼
//! AbiHost::draw_eye()        ← validates arg, delegates to DisplayDriver
//!      │
//!      ▼
//! DisplayDriver::draw_eye()  ← embedded-graphics renders to framebuffer
//!      │
//!      ▼
//! I2C transfer               ← frame is pushed to the OLED controller
//! ```
//!
//! ## Rendering primitives
//!
//! The display driver uses [`embedded_graphics`] to draw:
//! - **Eyes**: two filled circles with expression-specific modifiers
//!   (e.g., arcs for happy, downward tilt for sad)
//! - **Text**: scrolling text at the bottom of the screen
//! - **Status bar**: battery / Wi-Fi indicators (future phases)
//!
//! ## Display resolution
//!
//! The implementation targets a 128 × 64 pixel SSD1306 OLED panel.
//! All coordinates are validated against these dimensions before rendering.

use abi::{status, EyeExpression};
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive},
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::Text,
    Pixel,
};
use esp_hal::{
    gpio::GpioPin,
    i2c::master::{Config as I2cConfig, I2c},
    peripherals::I2C0,
    time::RateExtU32,
    Blocking,
};

const SSD1306_ADDR: u8 = 0x3C;
pub const DISPLAY_WIDTH: u32 = 128;
pub const DISPLAY_HEIGHT: u32 = 64;
const DISPLAY_BUF_SIZE: usize = (DISPLAY_WIDTH as usize) * (DISPLAY_HEIGHT as usize / 8);
const DISPLAY_QUEUE_DEPTH: usize = 8;

static DISPLAY_CHANNEL: Channel<CriticalSectionRawMutex, DisplayCommand, DISPLAY_QUEUE_DEPTH> =
    Channel::new();

#[derive(Clone)]
enum DisplayCommand {
    SetExpression(EyeExpression),
    SetText(heapless::String<64>),
    SetBrightness(u8),
}

pub struct DisplayDriver {
    current_expression: EyeExpression,
    brightness: u8,
    last_text: heapless::String<64>,
}

impl DisplayDriver {
    pub fn new() -> Self {
        Self {
            current_expression: EyeExpression::Neutral,
            brightness: 255,
            last_text: heapless::String::new(),
        }
    }

    pub fn draw_eye(&mut self, expression: EyeExpression) -> i32 {
        self.current_expression = expression;
        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetExpression(expression));
        status::OK
    }

    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        let text = match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return status::ERR_INVALID_ARG,
        };

        self.last_text.clear();
        for ch in text.chars().take(64) {
            let _ = self.last_text.push(ch);
        }

        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetText(self.last_text.clone()));
        status::OK
    }

    pub fn set_brightness(&mut self, value: u8) -> i32 {
        self.brightness = value;
        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetBrightness(value));
        status::OK
    }
}

impl Default for DisplayDriver {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn_display_task(spawner: Spawner, i2c0: I2C0, sda: GpioPin<8>, scl: GpioPin<9>) {
    spawner.spawn(display_task(i2c0, sda, scl)).unwrap();
}

#[embassy_executor::task]
async fn display_task(i2c0: I2C0, sda: GpioPin<8>, scl: GpioPin<9>) {
    let mut cfg = I2cConfig::default();
    cfg.frequency = 400.kHz();

    let i2c = match I2c::new(i2c0, cfg) {
        Ok(bus) => bus.with_sda(sda).with_scl(scl),
        Err(_) => return,
    };

    let mut oled = OledDisplay::new(i2c);
    oled.init();

    let mut state = FaceState::default();

    oled.clear(BinaryColor::Off);
    let title_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::new("SandOS", Point::new(34, 20), title_style).draw(&mut oled);
    let _ = Text::new("OLED OK", Point::new(34, 36), title_style).draw(&mut oled);
    let _ = oled.flush();
    Timer::after(Duration::from_millis(900)).await;

    let receiver = DISPLAY_CHANNEL.receiver();

    loop {
        while let Ok(cmd) = receiver.try_receive() {
            match cmd {
                DisplayCommand::SetExpression(expr) => state.expression = expr,
                DisplayCommand::SetText(text) => state.text = text,
                DisplayCommand::SetBrightness(value) => {
                    state.brightness = value;
                    oled.set_contrast(value);
                }
            }
        }

        render_kawaii_frame(&mut oled, &mut state);
        let _ = oled.flush();
        Timer::after(Duration::from_millis(120)).await;
    }
}

struct FaceState {
    expression: EyeExpression,
    text: heapless::String<64>,
    brightness: u8,
    frame: u32,
    blink_phase: u8,
}

impl Default for FaceState {
    fn default() -> Self {
        Self {
            expression: EyeExpression::Neutral,
            text: heapless::String::new(),
            brightness: 255,
            frame: 0,
            blink_phase: 0,
        }
    }
}

fn render_kawaii_frame(oled: &mut OledDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    if state.frame % 24 == 0 {
        state.blink_phase = 2;
    } else if state.blink_phase > 0 {
        state.blink_phase -= 1;
    }

    oled.clear(BinaryColor::Off);

    let eye_style = PrimitiveStyle::with_fill(BinaryColor::On);
    let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    let left_center = Point::new(36, 24);
    let right_center = Point::new(92, 24);

    if matches!(state.expression, EyeExpression::Blink) || state.blink_phase > 0 {
        let _ = Line::new(Point::new(24, 24), Point::new(48, 24))
            .into_styled(stroke)
            .draw(oled);
        let _ = Line::new(Point::new(80, 24), Point::new(104, 24))
            .into_styled(stroke)
            .draw(oled);
    } else {
        let mut r: i32 = 10;
        if matches!(state.expression, EyeExpression::Surprised) {
            r = 13;
        }
        let _ = Circle::new(left_center - Point::new(r, r), (r * 2) as u32)
            .into_styled(eye_style)
            .draw(oled);
        let _ = Circle::new(right_center - Point::new(r, r), (r * 2) as u32)
            .into_styled(eye_style)
            .draw(oled);

        if matches!(state.expression, EyeExpression::Thinking) {
            let pupil = PrimitiveStyle::with_fill(BinaryColor::Off);
            let _ = Circle::new(Point::new(38, 21), 5).into_styled(pupil).draw(oled);
            let _ = Circle::new(Point::new(94, 19), 5).into_styled(pupil).draw(oled);
        }
    }

    match state.expression {
        EyeExpression::Happy => {
            let _ = Line::new(Point::new(44, 44), Point::new(64, 50))
                .into_styled(stroke)
                .draw(oled);
            let _ = Line::new(Point::new(64, 50), Point::new(84, 44))
                .into_styled(stroke)
                .draw(oled);
        }
        EyeExpression::Sad => {
            let _ = Line::new(Point::new(44, 50), Point::new(64, 44))
                .into_styled(stroke)
                .draw(oled);
            let _ = Line::new(Point::new(64, 44), Point::new(84, 50))
                .into_styled(stroke)
                .draw(oled);
        }
        EyeExpression::Angry => {
            let _ = Line::new(Point::new(24, 12), Point::new(48, 18))
                .into_styled(stroke)
                .draw(oled);
            let _ = Line::new(Point::new(80, 18), Point::new(104, 12))
                .into_styled(stroke)
                .draw(oled);
            let _ = Rectangle::new(Point::new(52, 46), Size::new(24, 3))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(oled);
        }
        EyeExpression::Surprised => {
            let _ = Circle::new(Point::new(58, 44), 12)
                .into_styled(stroke)
                .draw(oled);
        }
        _ => {
            let y = if state.frame % 16 < 8 { 48 } else { 49 };
            let _ = Line::new(Point::new(48, y), Point::new(80, y))
                .into_styled(stroke)
                .draw(oled);
        }
    }

    if !state.text.is_empty() {
        let text_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        let _ = Rectangle::new(Point::new(0, 54), Size::new(DISPLAY_WIDTH, 10))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
            .draw(oled);
        let _ = Text::new(&state.text, Point::new(0, 63), text_style).draw(oled);
    }
}

struct OledDisplay {
    i2c: I2c<'static, Blocking>,
    buffer: [u8; DISPLAY_BUF_SIZE],
}

impl OledDisplay {
    fn new(i2c: I2c<'static, Blocking>) -> Self {
        Self {
            i2c,
            buffer: [0; DISPLAY_BUF_SIZE],
        }
    }

    fn init(&mut self) {
        self.cmd(0xAE);
        self.cmd(0xD5);
        self.cmd(0x80);
        self.cmd(0xA8);
        self.cmd(0x3F);
        self.cmd(0xD3);
        self.cmd(0x00);
        self.cmd(0x40);
        self.cmd(0x8D);
        self.cmd(0x14);
        self.cmd(0x20);
        self.cmd(0x00);
        self.cmd(0xA1);
        self.cmd(0xC8);
        self.cmd(0xDA);
        self.cmd(0x12);
        self.cmd(0x81);
        self.cmd(0xCF);
        self.cmd(0xD9);
        self.cmd(0xF1);
        self.cmd(0xDB);
        self.cmd(0x40);
        self.cmd(0xA4);
        self.cmd(0xA6);
        self.cmd(0x2E);
        self.cmd(0xAF);
    }

    fn set_contrast(&mut self, value: u8) {
        self.cmd(0x81);
        self.cmd(value);
    }

    fn cmd(&mut self, cmd: u8) {
        let _ = self.i2c.write(SSD1306_ADDR, &[0x00, cmd]);
    }

    fn flush(&mut self) -> Result<(), ()> {
        self.cmd(0x21);
        self.cmd(0x00);
        self.cmd((DISPLAY_WIDTH as u8) - 1);
        self.cmd(0x22);
        self.cmd(0x00);
        self.cmd((DISPLAY_HEIGHT as u8 / 8) - 1);

        let mut packet = [0u8; 17];
        packet[0] = 0x40;

        for chunk in self.buffer.chunks(16) {
            packet[1..(1 + chunk.len())].copy_from_slice(chunk);
            self.i2c
                .write(SSD1306_ADDR, &packet[..(1 + chunk.len())])
                .map_err(|_| ())?;
        }

        Ok(())
    }
}

impl OriginDimensions for OledDisplay {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for OledDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if coord.x < 0 || coord.y < 0 {
                continue;
            }
            let x = coord.x as usize;
            let y = coord.y as usize;

            if x >= DISPLAY_WIDTH as usize || y >= DISPLAY_HEIGHT as usize {
                continue;
            }

            let idx = x + (y / 8) * DISPLAY_WIDTH as usize;
            let bit = 1u8 << (y % 8);
            match color {
                BinaryColor::On => self.buffer[idx] |= bit,
                BinaryColor::Off => self.buffer[idx] &= !bit,
            }
        }
        Ok(())
    }
}
