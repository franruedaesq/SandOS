//! Global RGB LED state manager with hardware control
//!
//! Provides thread-safe access to the RGB LED state for the web server
//! and controls the actual WS2812B LED via RMT.

use portable_atomic::{AtomicU8, Ordering};
use crate::rgb_led;

/// Current LED red channel
pub static LED_R: AtomicU8 = AtomicU8::new(0);

/// Current LED green channel
pub static LED_G: AtomicU8 = AtomicU8::new(0);

/// Current LED blue channel
pub static LED_B: AtomicU8 = AtomicU8::new(0);

/// Set the RGB LED color
pub fn set_led_color(r: u8, g: u8, b: u8) {
    // Update the atomic state
    LED_R.store(r, Ordering::Relaxed);
    LED_G.store(g, Ordering::Relaxed);
    LED_B.store(b, Ordering::Relaxed);

    // Control the actual hardware
    if let Some(led) = unsafe { &mut rgb_led::RGB_LED } {
        led.set_color(r, g, b);
    } else {
        log::warn!("[led_state] RGB_LED controller not initialized");
    }
}

/// Get the current RGB LED color
pub fn get_led_color() -> (u8, u8, u8) {
    (
        LED_R.load(Ordering::Relaxed),
        LED_G.load(Ordering::Relaxed),
        LED_B.load(Ordering::Relaxed),
    )
}
