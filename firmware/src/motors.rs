//! Shared motor command state — the safe bridge between Core 0 and Core 1.
//!
//! ## Memory layout
//!
//! These statics live in internal SRAM so Core 1's write/read path is fully
//! deterministic (no PSRAM latency).
//!
//! ## Thread safety
//!
//! [`MOTOR_COMMAND`] is an `AtomicU32` that packs the left and right target
//! speeds as two `i16` halves (little-endian: bits 31–16 = left, 15–0 = right).
//! Core 0 writes with `Release`; Core 1 reads with `Acquire`.
//!
//! [`MOTOR_ENABLED`] is an `AtomicU8` (0 = disabled, 1 = enabled).  It is
//! written by Core 1's safe-shutdown logic when the ULP detects a critical
//! voltage drop, ensuring motors are cut before Core 0 can issue another
//! command.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

// ── Shared atomics ────────────────────────────────────────────────────────────

/// Packed motor command written by Core 0 (via ABI) and consumed by Core 1.
///
/// Layout: `[left_speed as u16][right_speed as u16]`
/// (left in bits 31–16, right in bits 15–0).
/// Speeds are signed i16 values in the range −255 … +255.
pub static MOTOR_COMMAND: AtomicU32 = AtomicU32::new(0);

/// Motor enable flag: `1` = enabled, `0` = disabled.
///
/// Set to `0` by Core 1 when the ULP paramedic signals a critical voltage
/// drop.  Once disabled, `store_motor_command` silently rejects new commands
/// until the flag is restored (e.g., after a watchdog reset).
pub static MOTOR_ENABLED: AtomicU8 = AtomicU8::new(1);

// ── Writer (Core 0 via ABI) ───────────────────────────────────────────────────

/// Store a new motor command from the ABI layer (Core 0).
///
/// Returns `true` if the command was accepted (motors enabled), `false` if
/// the safe-shutdown flag is active and the command was rejected.
#[inline]
pub fn store_motor_command(left: i16, right: i16) -> bool {
    if MOTOR_ENABLED.load(Ordering::Acquire) == 0 {
        return false;
    }
    let packed = ((left as u16 as u32) << 16) | (right as u16 as u32);
    MOTOR_COMMAND.store(packed, Ordering::Release);
    true
}

// ── Reader (Core 1) ───────────────────────────────────────────────────────────

/// Load the latest motor command (called by Core 1 every loop tick).
///
/// Returns `(left, right)` target speeds in the range −255 … +255.
#[inline]
pub fn load_motor_command() -> (i16, i16) {
    let packed = MOTOR_COMMAND.load(Ordering::Acquire);
    let left = (packed >> 16) as u16 as i16;
    let right = (packed & 0xFFFF) as u16 as i16;
    (left, right)
}

// ── Safe-shutdown control (Core 1) ────────────────────────────────────────────

/// Enable or disable all motors.
///
/// Called by Core 1 when the ULP paramedic sets the low-voltage flag.
/// Disabling zeroes the motor command atomically.
#[inline]
pub fn set_motor_enabled(enabled: bool) {
    MOTOR_ENABLED.store(u8::from(enabled), Ordering::Release);
    if !enabled {
        // Zero the command so the next load by Core 1 returns (0, 0).
        MOTOR_COMMAND.store(0, Ordering::Release);
    }
}

/// Return `true` when motors are enabled.
#[inline]
pub fn is_motor_enabled() -> bool {
    MOTOR_ENABLED.load(Ordering::Acquire) != 0
}
