use embassy_executor::Spawner;
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::gpio::{GpioPin, Input};
use esp_hal::i2c::master::{I2c, Config as I2cConfig, BusTimeout};
use esp_hal::peripherals::I2C0;
use esp_hal::time::RateExtU32;
use ft6x36::Ft6x36;

use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

/// High-level touch actions sent to the UI.
/// Coordinates are in landscape space (320×240).
#[derive(Clone, Copy)]
pub enum TouchAction {
    Tap(i32, i32),
    SwipeRight, // left → right: open menu
    SwipeLeft,  // right → left: close menu
    SwipeUp,    // drag up (scroll down)
    SwipeDown,  // drag down (scroll up)
}

/// Minimum horizontal/vertical displacement in landscape pixels to count as a swipe.
const SWIPE_THRESHOLD: i32 = 40;

/// If no interrupt arrives within this window, we assume the finger has lifted.
const LIFT_TIMEOUT_MS: u64 = 150;

pub static TOUCH_EVENTS: Channel<CriticalSectionRawMutex, TouchAction, 16> = Channel::new();

pub fn spawn_touch_task(
    spawner: Spawner,
    i2c0: I2C0,
    sda: GpioPin<16>,
    scl: GpioPin<15>,
    mut rst: esp_hal::gpio::Output<'static>,
    interrupt: Input<'static>,
) {
    rst.set_low();
    esp_hal::delay::Delay::new().delay_millis(20);
    rst.set_high();
    esp_hal::delay::Delay::new().delay_millis(200);

    let mut cfg = I2cConfig::default();
    cfg.frequency = 400.kHz();
    cfg.timeout = BusTimeout::BusCycles(100_000);

    let i2c = match I2c::new(i2c0, cfg) {
        Ok(bus) => bus.with_sda(sda).with_scl(scl),
        Err(err) => {
            log::error!("[touch] I2C init failed: {:?}", err);
            return;
        }
    };

    spawner.spawn(touch_task(i2c, interrupt)).unwrap();
}

#[embassy_executor::task]
async fn touch_task(
    i2c: I2c<'static, esp_hal::Blocking>,
    mut interrupt: Input<'static>,
) {
    let mut touch = Ft6x36::new(i2c, ft6x36::Dimension(240, 320));

    if let Err(e) = touch.init() {
        log::error!("[touch] Failed to initialize FT6336G: {:?}", e);
        return;
    }

    log::info!("[touch] FT6336G initialized");

    // Gesture tracking state (portrait coordinates).
    // portrait (tx, ty) → landscape (ty, 240-tx)
    let mut press_start: Option<(u16, u16)> = None; // portrait coords at first touch
    let mut last_pos: Option<(u16, u16)> = None;    // portrait coords of most recent sample

    loop {
        // Wait for next touch interrupt, or timeout if finger has lifted.
        let got_interrupt = with_timeout(
            Duration::from_millis(LIFT_TIMEOUT_MS),
            interrupt.wait_for_falling_edge(),
        )
        .await
        .is_ok();

        if got_interrupt {
            // Read the touch report
            if let Ok(event) = touch.get_touch_event() {
                if let Some(p1) = event.p1 {
                    if press_start.is_none() {
                        press_start = Some((p1.x, p1.y));
                    }
                    last_pos = Some((p1.x, p1.y));
                }
                // If p1 is None the finger may have lifted; handled by timeout below.
            }

            // Small debounce between samples
            Timer::after(Duration::from_millis(10)).await;
        } else {
            // Timeout — no interrupt for LIFT_TIMEOUT_MS → finger has lifted.
            if let (Some((sx, sy)), Some((ex, ey))) = (press_start.take(), last_pos.take()) {
                // Map portrait Y → landscape X to compute horizontal delta
                let dx = ey as i32 - sy as i32;
                // Map portrait X → landscape Y (with 240 inversion) to compute vertical delta
                // tap_y = 240 - x
                let start_y = 240 - sx as i32;
                let end_y = 240 - ex as i32;
                let dy = end_y - start_y;

                if dx.abs() > dy.abs() {
                    // Horizontal swipe dominant
                    if dx > SWIPE_THRESHOLD {
                        log::debug!("[touch] SwipeRight dx={}", dx);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::SwipeRight);
                    } else if dx < -SWIPE_THRESHOLD {
                        log::debug!("[touch] SwipeLeft dx={}", dx);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::SwipeLeft);
                    } else {
                        // Small movement or stationary → treat as tap at end position
                        let tap_x = ey as i32;          // portrait Y → landscape X
                        let tap_y = 240 - ex as i32;    // portrait X → landscape Y
                        log::debug!("[touch] Tap ({}, {})", tap_x, tap_y);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::Tap(tap_x, tap_y));
                    }
                } else {
                    // Vertical swipe dominant
                    if dy > SWIPE_THRESHOLD {
                        log::debug!("[touch] SwipeDown dy={}", dy);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::SwipeDown);
                    } else if dy < -SWIPE_THRESHOLD {
                        log::debug!("[touch] SwipeUp dy={}", dy);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::SwipeUp);
                    } else {
                        // Small movement or stationary → treat as tap at end position
                        let tap_x = ey as i32;
                        let tap_y = 240 - ex as i32;
                        log::debug!("[touch] Tap ({}, {})", tap_x, tap_y);
                        let _ = TOUCH_EVENTS.try_send(TouchAction::Tap(tap_x, tap_y));
                    }
                }
            }
            // Reset in case only press_start was set (no subsequent samples)
            press_start = None;
            last_pos = None;
        }
    }
}
