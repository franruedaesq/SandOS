//! ULP Coprocessor — The Paramedic.
//!
//! The ESP32-S3's Ultra-Low-Power (ULP) core (RISC-V LP core) is programmed
//! to independently monitor the chip's internal temperature and supply voltage.
//! It writes its findings into RTC Slow Memory so the main cores can read them
//! without polling overhead.
//!
//! ## Shared memory protocol
//!
//! The ULP writes to fixed offsets in RTC Slow Memory (see [`abi::ulp_mem`]):
//!
//! | Offset | Type | Description                            |
//! |--------|------|----------------------------------------|
//! | 0x00   | u32  | Temperature-over-threshold flag (0/1)  |
//! | 0x04   | u32  | Last temperature reading (tenths °C)   |
//! | 0x08   | u32  | Low-voltage flag (0/1)                 |
//! | 0x0C   | u32  | Last supply voltage reading (mV)       |
//!
//! ## Temperature threshold
//!
//! The ULP sets the flag when the temperature exceeds 85 °C.  At that point
//! the main firmware should initiate a safe shutdown.
//!
//! ## Implementation note
//!
//! The ULP binary is compiled separately using the ESP32-S3 ULP toolchain and
//! included as a static byte array.  For Phase 1, we rely on the LP I2C
//! temperature sensor reading.

use esp_hal::peripherals::LPWR;

/// Temperature threshold (in tenths of °C) at which the ULP sets its flag.
#[allow(dead_code)]
pub const TEMP_THRESHOLD_TENTHS: u32 = 850; // 85.0 °C

/// Minimum safe supply voltage in millivolts.
#[allow(dead_code)]
pub const VOLTAGE_MIN_MV: u32 = 3_000; // 3.0 V

/// ULP binary (assembled separately; placeholder until the real binary is
/// generated from `ulp/ulp_paramedic.S`).
///
/// In a full build this is replaced by:
/// ```rust,ignore
/// static ULP_BINARY: &[u8] = include_bytes!("../ulp/ulp_paramedic.bin");
/// ```
static ULP_BINARY: &[u8] = &[];

/// Upload the ULP program and start it.
///
/// Called once from `main` before the Core 0 Embassy executor starts.
/// Does nothing if the ULP binary is empty (development convenience).
pub fn start(_lpwr: LPWR) {
    if ULP_BINARY.is_empty() {
        // No binary compiled yet — skip in Phase 1 development.
        return;
    }

    // In the full implementation:
    // 1. Load ULP_BINARY into LP SRAM via `esp_hal::ulp_core::UlpCore`.
    // 2. Set shared-memory thresholds.
    // 3. Start the ULP with `ulp.run(ULP_ENTRY_POINT)`.
    //
    // Example (once esp-hal ULP API is stable):
    //   let mut ulp = UlpCore::new(lpwr);
    //   ulp.load(ULP_BINARY);
    //   ulp.run();
}

/// Read the temperature-over-threshold flag from RTC Slow Memory.
///
/// Returns `true` if the ULP has flagged an over-temperature condition.
#[allow(dead_code)]
pub fn is_temp_critical() -> bool {
    // SAFETY: RTC Slow Memory is always mapped at this address on ESP32-S3.
    // The ULP writes atomically to 32-bit aligned words.
    let flag_ptr = (0x5000_0000 + abi::ulp_mem::TEMP_OVER_THRESHOLD) as *const u32;
    unsafe { flag_ptr.read_volatile() != 0 }
}

/// Read the last temperature measured by the ULP (tenths of °C).
#[allow(dead_code)]
pub fn last_temp_tenths() -> u32 {
    let ptr = (0x5000_0000 + abi::ulp_mem::LAST_TEMP_TENTHS) as *const u32;
    unsafe { ptr.read_volatile() }
}

/// Read the low-voltage flag written by the ULP.
///
/// Returns `true` when supply voltage dropped below [`VOLTAGE_MIN_MV`].
pub fn is_voltage_critical() -> bool {
    let flag_ptr = (0x5000_0000 + abi::ulp_mem::LOW_VOLTAGE_FLAG) as *const u32;
    unsafe { flag_ptr.read_volatile() != 0 }
}
