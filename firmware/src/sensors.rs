//! Shared IMU sensor data — the safe bridge between Core 1 and Core 0.
//!
//! ## Memory Mapping
//!
//! This static lives in internal SRAM (`.bss` / `.data`), ensuring Core 1's
//! write path stays entirely within the ultra-fast TCM/SRAM region.
//! Core 0 reads it after the Wasm VM (which lives on PSRAM) requests data.
//!
//! ## Thread Safety
//!
//! [`IMU_DATA`] is an `AtomicU64`.  Core 1 writes with `Ordering::Release`
//! after every 2 ms poll; Core 0 reads with `Ordering::Acquire` inside the
//! ABI handler.  The acquire/release pair guarantees Core 0 always sees the
//! most recently *completed* write — no locks, no blocking.
//!
//! The encoding is defined in [`abi::ImuReading`]:
//! - bits 63–32 → `pitch_millideg` (i32 reinterpreted as u32)
//! - bits 31–0  → `roll_millideg`  (i32 reinterpreted as u32)

use core::sync::atomic::Ordering;
use portable_atomic::AtomicU64;

use abi::ImuReading;

// ── Shared atomic ─────────────────────────────────────────────────────────────

/// Latest IMU reading, shared between Core 1 (writer) and Core 0 (reader).
///
/// Initialised to zero (flat/level) at boot.
pub static IMU_DATA: AtomicU64 = AtomicU64::new(0);

// ── Writer (Core 1) ───────────────────────────────────────────────────────────

/// Store a new [`ImuReading`] into the shared atomic (called by Core 1).
///
/// Uses `Release` ordering so the subsequent `Acquire` read on Core 0 is
/// guaranteed to observe the fully written value.
#[inline]
pub fn store_imu(reading: ImuReading) {
    IMU_DATA.store(reading.encode(), Ordering::Release);
}

// ── Reader (Core 0 ABI) ───────────────────────────────────────────────────────

/// Load the latest [`ImuReading`] from the shared atomic (called by Core 0).
///
/// Uses `Acquire` ordering, pairing with the `Release` write on Core 1.
#[inline]
pub fn load_imu() -> ImuReading {
    ImuReading::decode(IMU_DATA.load(Ordering::Acquire))
}

// ── Battery ADC status ────────────────────────────────────────────────────────

pub static BATTERY_MV: portable_atomic::AtomicU16 = portable_atomic::AtomicU16::new(0);

#[inline]
pub fn store_battery_mv(mv: u16) {
    BATTERY_MV.store(mv, Ordering::Release);
}

#[inline]
pub fn load_battery_mv() -> u16 {
    BATTERY_MV.load(Ordering::Acquire)
}

// ── Touchscreen coordinates ───────────────────────────────────────────────────

pub static TOUCH_COORDS: portable_atomic::AtomicU32 = portable_atomic::AtomicU32::new(0);

#[inline]
pub fn store_touch_coords(x: u16, y: u16) {
    // Top bit indicates valid touch (1 << 31).
    let packed = (1 << 31) | ((x as u32) << 16) | (y as u32);
    TOUCH_COORDS.store(packed, Ordering::Release);
}

#[inline]
pub fn clear_touch_coords() {
    TOUCH_COORDS.store(0, Ordering::Release);
}

#[inline]
pub fn load_touch_coords() -> Option<(u16, u16)> {
    let packed = TOUCH_COORDS.load(Ordering::Acquire);
    if (packed & (1 << 31)) != 0 {
        let x = (packed >> 16) as u16 & 0x7FFF;
        let y = packed as u16;
        Some((x, y))
    } else {
        None
    }
}
