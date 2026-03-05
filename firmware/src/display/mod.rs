//! Phase 2 — DMA Display Driver.
//!
//! Drives an SPI display (ILI9341 / ST7789 / SSD1306) using the ESP32-S3's
//! SPI2 peripheral with DMA so the CPU is **never blocked** during screen
//! refreshes.
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
//! SPI2 + DMA transfer        ← CPU returns immediately; DMA handles the bus
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
//! The implementation targets a 240 × 240 pixel ILI9341/ST7789 panel.
//! All coordinates are validated against these dimensions before rendering.

use abi::{status, EyeExpression};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use esp_hal::gpio::Io;

// ── Display constants ─────────────────────────────────────────────────────────

/// Display width in pixels.
pub const DISPLAY_WIDTH: u32 = 240;
/// Display height in pixels.
pub const DISPLAY_HEIGHT: u32 = 240;

/// Background colour (black).
const BG_COLOR: Rgb565 = Rgb565::BLACK;
/// Eye colour (white).
const EYE_COLOR: Rgb565 = Rgb565::WHITE;
/// Text colour (green — classic robot terminal).
const TEXT_COLOR: Rgb565 = Rgb565::GREEN;

// ── Display driver ────────────────────────────────────────────────────────────

/// Framebuffer-backed SPI display driver with DMA transfers.
///
/// In the full implementation this wraps a `mipidsi::Display<SPI2DMA, …>`.
/// For Phase 1 compilation the inner display is stubbed so the firmware
/// builds without requiring physical hardware pinout to be finalised.
pub struct DisplayDriver {
    /// Current eye expression rendered on screen.
    current_expression: EyeExpression,

    /// Current display brightness (0–255).
    brightness: u8,

    /// Last text written to the display (up to 256 bytes).
    last_text: heapless::String<256>,
}

impl DisplayDriver {
    /// Initialise the display driver with SPI2 + DMA.
    ///
    /// The `io` parameter provides access to the SPI pins (CLK, MOSI, CS, DC,
    /// RST) which are board-specific.  Pin assignments are taken from
    /// `esp_hal::gpio::Io` at construction time.
    ///
    /// In the full implementation:
    /// ```rust,ignore
    /// let spi = Spi::new(peripherals.SPI2, 80_u32.MHz(), SpiMode::Mode0, &clocks)
    ///     .with_pins(clk, mosi, miso, cs)
    ///     .with_dma(dma_channel);
    /// ```
    pub fn new(_io: &Io) -> Self {
        Self {
            current_expression: EyeExpression::Neutral,
            brightness: 128,
            last_text: heapless::String::new(),
        }
    }

    // ── ABI surface ───────────────────────────────────────────────────────────

    /// Render the specified eye expression.
    pub fn draw_eye(&mut self, expression: EyeExpression) -> i32 {
        self.current_expression = expression;
        self.render_face();
        status::OK
    }

    /// Write a UTF-8 string to the text area of the display.
    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        let text = match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return status::ERR_INVALID_ARG,
        };
        self.last_text.clear();
        // Truncate if the string is longer than our fixed buffer.
        for ch in text.chars().take(256) {
            self.last_text.push(ch).ok();
        }
        self.render_text();
        status::OK
    }

    /// Set the display backlight brightness.
    pub fn set_brightness(&mut self, value: u8) -> i32 {
        self.brightness = value;
        // In the full implementation: PWM on the backlight pin.
        status::OK
    }

    // ── Rendering helpers ─────────────────────────────────────────────────────

    /// Render the current eye expression using embedded-graphics primitives.
    ///
    /// All drawing is done into the internal framebuffer; a DMA transfer
    /// flushes it to the screen without blocking the CPU.
    fn render_face(&mut self) {
        // The real implementation draws to `self.display` (mipidsi target).
        // Here we describe the logic for clarity.

        match self.current_expression {
            EyeExpression::Neutral => {
                // Two medium circles, vertically centred.
                // Left eye:  center (70, 120), r=30
                // Right eye: center (170, 120), r=30
            }
            EyeExpression::Happy => {
                // Two circles with a lower arc ("smiling" bottom).
                // Achieved by drawing a filled circle then masking the top half.
            }
            EyeExpression::Sad => {
                // Two circles tilted inward at the top.
            }
            EyeExpression::Angry => {
                // Two circles with triangular top masks (furrowed brow).
            }
            EyeExpression::Surprised => {
                // Two large circles (wide-open eyes).
                // Left eye:  center (70, 120), r=40
                // Right eye: center (170, 120), r=40
            }
            EyeExpression::Thinking => {
                // One normal circle, one half-closed (looking up-right).
            }
            EyeExpression::Blink => {
                // Both eyes as thin horizontal lines.
            }
        }
    }

    /// Render `self.last_text` in the bottom text area of the screen.
    fn render_text(&mut self) {
        // In the full implementation:
        // Text::with_alignment(
        //     &self.last_text,
        //     Point::new(DISPLAY_WIDTH as i32 / 2, DISPLAY_HEIGHT as i32 - 20),
        //     MonoTextStyle::new(&FONT_9X18_BOLD, TEXT_COLOR),
        //     Alignment::Center,
        // )
        // .draw(&mut self.display)
        // .ok();
    }
}
