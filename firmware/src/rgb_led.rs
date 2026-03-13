//! # RGB LED Driver for ESP32-S3
//!
//! Controls the WS2812B addressable RGB LED on GPIO 42.

use esp_hal::rmt::{Channel, PulseCode, TxChannel};
use esp_hal::Blocking;
use smart_leds::RGB8;

/// Global RGB LED controller - accessible from web server
pub static mut RGB_LED: Option<RgbLedDriver> = None;

const WS2812_BIT_0_HIGH_TICKS: u16 = 8;
const WS2812_BIT_0_LOW_TICKS: u16 = 17;
const WS2812_BIT_1_HIGH_TICKS: u16 = 16;
const WS2812_BIT_1_LOW_TICKS: u16 = 9;
const WS2812_RESET_TICKS: u16 = 1200;

/// RGB LED driver for WS2812B addressable LED on GPIO 42
///
/// Stores current color state and drives hardware via RMT channel 0.
pub struct RgbLedDriver {
    current_color: RGB8,
    tx_channel_42: Option<Channel<Blocking, 1>>,
}

impl RgbLedDriver {
    /// Create a new RGB LED driver
    pub fn new() -> Self {
        log::info!("[rgb_led] RGB LED driver initialized");
        Self {
            current_color: RGB8::new(0, 0, 0),
            tx_channel_42: None,
        }
    }

    pub fn attach_tx_channel_gpio42(&mut self, tx_channel: Channel<Blocking, 1>) {
        self.tx_channel_42 = Some(tx_channel);
        log::info!("[rgb_led] RMT TX channel attached on GPIO42");
    }

    /// Set the RGB LED to a specific color (0-255 per channel)
    pub fn set_color(&mut self, red: u8, green: u8, blue: u8) {
        self.current_color = RGB8::new(red, green, blue);
        self.transmit_ws2812();
        log::info!(
            "[rgb_led] LED color set to R={} G={} B={}",
            red,
            green,
            blue
        );
    }

    fn transmit_ws2812(&mut self) {
        let mut frame = [u32::empty(); 25];
        let grb = [
            self.current_color.g,
            self.current_color.r,
            self.current_color.b,
        ];

        let mut idx = 0;
        for byte in grb {
            for bit in (0..8).rev() {
                let is_one = ((byte >> bit) & 0x01) != 0;
                frame[idx] = if is_one {
                    u32::new(true, WS2812_BIT_1_HIGH_TICKS, false, WS2812_BIT_1_LOW_TICKS)
                } else {
                    u32::new(true, WS2812_BIT_0_HIGH_TICKS, false, WS2812_BIT_0_LOW_TICKS)
                };
                idx += 1;
            }
        }

        frame[24] = u32::new(false, WS2812_RESET_TICKS, false, 0);

        if let Some(channel42) = self.tx_channel_42.take() {
            match channel42.transmit(&frame) {
                Ok(transaction) => match transaction.wait() {
                    Ok(channel_back) => {
                        self.tx_channel_42 = Some(channel_back);
                        log::info!("[rgb_led] RMT frame transmitted on GPIO42");
                    }
                    Err((e, channel_back)) => {
                        self.tx_channel_42 = Some(channel_back);
                        log::warn!("[rgb_led] RMT wait failed on GPIO42: {:?}", e);
                    }
                },
                Err(e) => {
                    log::warn!("[rgb_led] RMT transmit failed on GPIO42: {:?}", e);
                }
            }
        } else {
            log::warn!("[rgb_led] No RMT TX channels attached; skipping LED transmit");
        }
    }

    /// Get the current LED color
    pub fn get_color(&self) -> (u8, u8, u8) {
        (
            self.current_color.r,
            self.current_color.g,
            self.current_color.b,
        )
    }

    /// Turn LED off (black)
    pub fn off(&mut self) {
        self.set_color(0, 0, 0);
    }
}

impl Default for RgbLedDriver {
    fn default() -> Self {
        Self::new()
    }
}
