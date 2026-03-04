//! Worker shared motor state — the safe bridge between the ESP-NOW task and
//! the motor PWM task.
//!
//! ## Thread safety
//!
//! [`MOTOR_COMMAND`] is an `AtomicU32` that packs the left and right target
//! speeds as two `i16` halves (bits 31–16 = left, bits 15–0 = right).
//! The ESP-NOW task writes with `Release`; the motor task reads with `Acquire`.
//!
//! [`MOTOR_ENABLED`] is an `AtomicU8` (`0` = disabled, `1` = enabled).
//! The ESP-NOW task clears it when the dead-man's switch fires and sets it
//! again on the next valid packet.
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use embassy_time::{Duration, Ticker};

// ── Shared atomics ────────────────────────────────────────────────────────────

/// Packed motor command written by the ESP-NOW task and consumed by the motor task.
///
/// Layout: `[left_speed as u16][right_speed as u16]`
/// (left in bits 31–16, right in bits 15–0).
/// Speeds are signed i16 values in the range −255 … +255.
pub static MOTOR_COMMAND: AtomicU32 = AtomicU32::new(0);

/// Motor enable flag: `1` = enabled, `0` = disabled (dead-man's switch active).
///
/// Cleared by the ESP-NOW task when no heartbeat is received within
/// [`WORKER_TIMEOUT_MS`].  Set again on the next valid packet.
pub static MOTOR_ENABLED: AtomicU8 = AtomicU8::new(1);

// ── Writer (ESP-NOW task) ─────────────────────────────────────────────────────

/// Store a new motor command from the ESP-NOW task.
#[inline]
pub fn store_motor_command(left: i16, right: i16) {
    let packed = ((left as u16 as u32) << 16) | (right as u16 as u32);
    MOTOR_COMMAND.store(packed, Ordering::Release);
}

/// Enable or disable all motors.
///
/// Disabling zeroes the command so the next load returns `(0, 0)`.
#[inline]
pub fn set_motor_enabled(enabled: bool) {
    MOTOR_ENABLED.store(u8::from(enabled), Ordering::Release);
    if !enabled {
        MOTOR_COMMAND.store(0, Ordering::Release);
    }
}

// ── Reader (motor task) ───────────────────────────────────────────────────────

/// Load the latest motor command.
///
/// Returns `(left, right)` target speeds in the range −255 … +255.
#[inline]
pub fn load_motor_command() -> (i16, i16) {
    let packed = MOTOR_COMMAND.load(Ordering::Acquire);
    let left  = (packed >> 16) as u16 as i16;
    let right = (packed & 0xFFFF) as u16 as i16;
    (left, right)
}

/// Return `true` when the motors are enabled.
#[inline]
pub fn is_motor_enabled() -> bool {
    MOTOR_ENABLED.load(Ordering::Acquire) != 0
}

// ── Motor PWM task ────────────────────────────────────────────────────────────

/// Worker Core 0 motor-control task — runs at 500 Hz (2 ms period).
///
/// Reads the shared `MOTOR_COMMAND` atomic and applies the speeds to the
/// motor PWM peripheral.  If `MOTOR_ENABLED` is cleared (dead-man's switch)
/// the PWM outputs are zeroed immediately.
///
/// ## PWM hardware stub
///
/// The actual LEDC / MCPWM peripheral writes are marked as stubs pending
/// hardware bringup.  Replace the `let _ = (left, right)` lines with real
/// peripheral writes once the pinout is confirmed.
#[embassy_executor::task]
pub async fn motor_task() {
    let mut ticker = Ticker::every(Duration::from_millis(2));

    loop {
        if is_motor_enabled() {
            let (left, right) = load_motor_command();

            // ── Apply PWM ──────────────────────────────────────────────────
            // In production, write `left` and `right` to the LEDC peripheral:
            //
            //   ledc_left.set_duty(left.unsigned_abs() as u32).ok();
            //   dir_left.set_level(left >= 0).ok();
            //   ledc_right.set_duty(right.unsigned_abs() as u32).ok();
            //   dir_right.set_level(right >= 0).ok();
            let _ = (left, right);
        } else {
            // Dead-man's switch active — zero all PWM outputs.
            //
            //   ledc_left.set_duty(0).ok();
            //   ledc_right.set_duty(0).ok();
        }

        ticker.next().await;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
