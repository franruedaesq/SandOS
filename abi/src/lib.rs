//! # SandOS Host-Guest ABI
//!
//! The contract between the Wasm Sandbox (Guest) and the Rust Host OS.
//!
//! ## Zero-Trust Model
//!
//! The Wasm guest **cannot** access hardware directly. Every hardware
//! interaction is expressed as a typed ABI call that the Host validates
//! before execution. If the guest passes an invalid argument (e.g. a
//! draw coordinate beyond the screen edge, or a pointer outside its own
//! Wasm memory), the Host returns an error code and ignores the request.
//!
//! ## Usage
//!
//! ### Guest side (in Wasm, compiled to `wasm32-unknown-unknown`)
//! ```c
//! // Wasm imports (auto-linked by the wasmi linker)
//! extern int32_t host_toggle_led(void);
//! extern int32_t host_draw_eye(int32_t expression);
//! extern int32_t host_write_text(int32_t ptr, int32_t len);
//! ```
//!
//! ### Host side (Rust firmware)
//! ```rust,ignore
//! linker.func_wrap("env", "host_toggle_led", |mut caller| -> i32 {
//!     caller.data_mut().led_state.toggle();
//!     abi::status::OK as i32
//! }).unwrap();
//! ```
#![cfg_attr(not(feature = "std"), no_std)]

// ── ABI Function Names ────────────────────────────────────────────────────────

/// Module name used for all host imports inside the Wasm binary.
pub const HOST_MODULE: &str = "env";

// Phase 1 — Core functions
pub const FN_TOGGLE_LED:         &str = "host_toggle_led";
pub const FN_GET_UPTIME_MS:      &str = "host_get_uptime_ms";
pub const FN_DEBUG_LOG:          &str = "host_debug_log";

// Phase 2 — Display & Audio functions
pub const FN_DRAW_EYE:           &str = "host_draw_eye";
pub const FN_WRITE_TEXT:         &str = "host_write_text";
pub const FN_SET_BRIGHTNESS:     &str = "host_set_brightness";
pub const FN_START_AUDIO:        &str = "host_start_audio_capture";
pub const FN_STOP_AUDIO:         &str = "host_stop_audio_capture";
pub const FN_GET_AUDIO_AVAIL:    &str = "host_get_audio_avail";
pub const FN_READ_AUDIO:         &str = "host_read_audio";

// Phase 3 — Sensor functions
pub const FN_GET_PITCH_ROLL:     &str = "host_get_pitch_roll";

// Phase 4 — Motor functions
pub const FN_SET_MOTOR_SPEED:    &str = "host_set_motor_speed";

// ── Status Codes ──────────────────────────────────────────────────────────────

/// ABI status codes returned from every host function as `i32`.
pub mod status {
    /// Call completed successfully.
    pub const OK: i32 = 0;
    /// Unknown or unimplemented function.
    pub const ERR_UNKNOWN_FN: i32 = 1;
    /// An argument is outside its valid range.
    pub const ERR_INVALID_ARG: i32 = 2;
    /// The hardware resource is temporarily busy.
    pub const ERR_BUSY: i32 = 3;
    /// Insufficient memory to fulfil the request.
    pub const ERR_NO_MEM: i32 = 4;
    /// A pointer + length pair would access memory outside the Wasm sandbox.
    pub const ERR_BOUNDS: i32 = 5;
}

// ── Eye Expressions ───────────────────────────────────────────────────────────

/// Eye expression variants for `host_draw_eye`.
///
/// The integer discriminants are stable ABI — do not reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EyeExpression {
    Neutral   = 0,
    Happy     = 1,
    Sad       = 2,
    Angry     = 3,
    Surprised = 4,
    Thinking  = 5,
    Blink     = 6,
}

impl EyeExpression {
    /// Parse from the raw `i32` ABI argument.
    ///
    /// Returns `None` for unknown values so the Host can return
    /// [`status::ERR_INVALID_ARG`] instead of crashing.
    #[inline]
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Neutral),
            1 => Some(Self::Happy),
            2 => Some(Self::Sad),
            3 => Some(Self::Angry),
            4 => Some(Self::Surprised),
            5 => Some(Self::Thinking),
            6 => Some(Self::Blink),
            _ => None,
        }
    }
}

// ── ESP-NOW Packet ────────────────────────────────────────────────────────────

/// Maximum payload size for an ESP-NOW packet (ESP32-S3 hardware limit).
pub const ESPNOW_MAX_PAYLOAD: usize = 250;

/// A command packet received via ESP-NOW from the PC.
///
/// The layout is `#[repr(C)]` so it can be zero-copy cast from the raw bytes
/// received by the ESP-NOW driver.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EspNowCommand {
    /// Magic header: `0xSA` `0xND` (SaND — SandOS protocol).
    pub magic: [u8; 2],
    /// Command identifier.
    pub cmd_id: u8,
    /// Length of the payload that follows (0..=`ESPNOW_MAX_PAYLOAD - 3`).
    pub payload_len: u8,
    /// Inline payload bytes.
    pub payload: [u8; ESPNOW_MAX_PAYLOAD - 4],
}

impl EspNowCommand {
    /// The two-byte magic number that all valid SandOS packets must start with.
    pub const MAGIC: [u8; 2] = [0x5A, 0x4E]; // 'Z', 'N' — "ZeroNet"

    /// Returns `true` when the magic header is valid.
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.magic == Self::MAGIC
    }
}

/// Well-known command IDs for [`EspNowCommand::cmd_id`].
pub mod cmd {
    /// Toggle the onboard LED (Phase 1).
    pub const TOGGLE_LED: u8 = 0x01;
    /// Load a new Wasm binary from the PC (future OTA).
    pub const LOAD_WASM:  u8 = 0x02;
    /// Draw a robot expression on the display (Phase 2).
    pub const DRAW_EYE:   u8 = 0x10;
    /// Display LLM text response on screen (Phase 2).
    pub const WRITE_TEXT: u8 = 0x11;
    /// Flush / clear the display (Phase 2).
    pub const CLEAR_DISPLAY: u8 = 0x12;
    /// Set motor speeds (Phase 4): payload = [left_hi, left_lo, right_hi, right_lo].
    pub const SET_MOTOR_SPEED: u8 = 0x20;
    /// Emergency stop — zero all motors immediately (Phase 4).
    pub const EMERGENCY_STOP: u8 = 0x21;
}

// ── IMU Sensor Data ───────────────────────────────────────────────────────────

/// A pitch/roll reading from the IMU, expressed in millidegrees.
///
/// Using millidegrees (i32) avoids floating-point in `no_std` contexts while
/// providing ±2,147,483 degrees of range — far more than any physical angle.
///
/// The integer discriminants are stable ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImuReading {
    /// Pitch angle in millidegrees (positive = nose up).
    pub pitch_millideg: i32,
    /// Roll angle in millidegrees (positive = right side down).
    pub roll_millideg: i32,
}

impl ImuReading {
    /// Pack the reading into a single `u64` for atomic storage.
    ///
    /// Layout: `[pitch_millideg as u32][roll_millideg as u32]`
    /// (pitch in bits 63–32, roll in bits 31–0).
    #[inline]
    pub fn encode(self) -> u64 {
        ((self.pitch_millideg as u32 as u64) << 32) | (self.roll_millideg as u32 as u64)
    }

    /// Unpack a reading previously encoded with [`ImuReading::encode`].
    #[inline]
    pub fn decode(raw: u64) -> Self {
        Self {
            pitch_millideg: (raw >> 32) as u32 as i32,
            roll_millideg:  (raw & 0xFFFF_FFFF) as u32 as i32,
        }
    }
}

// ── Constraint Constants ──────────────────────────────────────────────────────

/// Maximum byte length accepted by `host_write_text`.
pub const MAX_TEXT_BYTES: u32 = 256;

/// Maximum display brightness value accepted by `host_set_brightness`.
pub const MAX_BRIGHTNESS: i32 = 255;

/// Maximum audio chunk size for a single `host_read_audio` call (bytes).
pub const MAX_AUDIO_READ: u32 = 1024;

/// Maximum motor speed magnitude accepted by `host_set_motor_speed`.
///
/// Speeds are in the signed range `[-MAX_MOTOR_SPEED, MAX_MOTOR_SPEED]`
/// where positive values drive forward and negative values drive backward.
pub const MAX_MOTOR_SPEED: i32 = 255;

/// Wasm linear memory page size (64 KiB).
pub const WASM_PAGE_SIZE: u32 = 65_536;

/// Validate that a (ptr, len) pair stays within `memory_size` bytes.
///
/// Returns `Ok(())` on success, or `Err(status::ERR_BOUNDS)` if the region
/// would overflow or exceed the allocated memory.
#[inline]
pub fn validate_ptr_len(ptr: u32, len: u32, memory_size: u32) -> Result<(), i32> {
    match ptr.checked_add(len) {
        Some(end) if end <= memory_size => Ok(()),
        _ => Err(status::ERR_BOUNDS),
    }
}

// ── ULP Shared Memory Layout ──────────────────────────────────────────────────

/// Byte offsets inside the ULP-accessible RTC SLOW memory region.
pub mod ulp_mem {
    /// Flag set by the ULP when temperature exceeds the threshold (u32: 0/1).
    pub const TEMP_OVER_THRESHOLD: usize = 0;
    /// Last measured temperature in tenths of a degree Celsius (u32).
    pub const LAST_TEMP_TENTHS: usize = 4;
    /// Flag set by the ULP when VDD is below the critical voltage (u32: 0/1).
    pub const LOW_VOLTAGE_FLAG: usize = 8;
    /// Last measured supply voltage in millivolts (u32).
    pub const LAST_VOLTAGE_MV: usize = 12;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_expression_round_trips() {
        for i in 0..=6i32 {
            let expr = EyeExpression::from_i32(i).expect("valid expression");
            assert_eq!(expr as i32, i);
        }
    }

    #[test]
    fn unknown_eye_expression_returns_none() {
        assert!(EyeExpression::from_i32(-1).is_none());
        assert!(EyeExpression::from_i32(7).is_none());
        assert!(EyeExpression::from_i32(255).is_none());
    }

    #[test]
    fn validate_ptr_len_in_bounds() {
        assert!(validate_ptr_len(0, 100, 1024).is_ok());
        assert!(validate_ptr_len(1000, 24, 1024).is_ok());
        assert!(validate_ptr_len(1024, 0, 1024).is_ok());
    }

    #[test]
    fn validate_ptr_len_out_of_bounds() {
        assert_eq!(validate_ptr_len(1000, 25, 1024), Err(status::ERR_BOUNDS));
        assert_eq!(validate_ptr_len(u32::MAX, 1, 1024), Err(status::ERR_BOUNDS));
    }

    #[test]
    fn espnow_command_magic_valid() {
        let cmd = EspNowCommand {
            magic: EspNowCommand::MAGIC,
            cmd_id: cmd::TOGGLE_LED,
            payload_len: 0,
            payload: [0; ESPNOW_MAX_PAYLOAD - 4],
        };
        assert!(cmd.is_valid());
    }

    #[test]
    fn espnow_command_magic_invalid() {
        let cmd = EspNowCommand {
            magic: [0xFF, 0xFF],
            cmd_id: 0,
            payload_len: 0,
            payload: [0; ESPNOW_MAX_PAYLOAD - 4],
        };
        assert!(!cmd.is_valid());
    }

    #[test]
    fn imu_reading_encode_decode_roundtrip() {
        let reading = ImuReading {
            pitch_millideg: 45_000,
            roll_millideg: -12_500,
        };
        let decoded = ImuReading::decode(reading.encode());
        assert_eq!(decoded.pitch_millideg, 45_000);
        assert_eq!(decoded.roll_millideg, -12_500);
    }

    #[test]
    fn imu_reading_zero_roundtrip() {
        let reading = ImuReading::default();
        assert_eq!(ImuReading::decode(reading.encode()), reading);
    }

    #[test]
    fn imu_reading_negative_pitch_roundtrip() {
        let reading = ImuReading {
            pitch_millideg: -90_000,
            roll_millideg: 180_000,
        };
        let decoded = ImuReading::decode(reading.encode());
        assert_eq!(decoded.pitch_millideg, -90_000);
        assert_eq!(decoded.roll_millideg, 180_000);
    }

    #[test]
    fn motor_speed_bounds_are_symmetric() {
        assert_eq!(MAX_MOTOR_SPEED, 255);
        assert!(-MAX_MOTOR_SPEED <= 0);
    }

    #[test]
    fn motor_cmd_ids_are_unique() {
        let ids = [
            cmd::TOGGLE_LED,
            cmd::LOAD_WASM,
            cmd::DRAW_EYE,
            cmd::WRITE_TEXT,
            cmd::CLEAR_DISPLAY,
            cmd::SET_MOTOR_SPEED,
            cmd::EMERGENCY_STOP,
        ];
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            assert!(seen.insert(id), "duplicate command ID: 0x{:02X}", id);
        }
    }
}
