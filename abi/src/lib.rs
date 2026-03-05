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

// Phase 5 — Routing control
pub const FN_GET_ROUTING_MODE:        &str = "host_get_routing_mode";

// Phase 6 — Structured telemetry
pub const FN_EMIT_IMU_TELEMETRY:      &str = "host_emit_imu_telemetry";
pub const FN_EMIT_ODOM_TELEMETRY:     &str = "host_emit_odom_telemetry";
pub const FN_GET_TELEMETRY_QUEUE_LEN: &str = "host_get_telemetry_queue_len";

// ── Phase 5 — Message Bus Constants ──────────────────────────────────────────

/// Dead-man's switch timeout in milliseconds.
///
/// If the OS Message Bus Router receives no valid [`MovementIntent`] within
/// this window, it zeroes all motor control loops to prevent a runaway robot.
pub const DEAD_MANS_SWITCH_MS: u64 = 50;

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
    /// Emit a structured IMU telemetry packet (Phase 6).
    pub const EMIT_IMU_TELEMETRY:  u8 = 0x30;
    /// Emit a structured Odometry telemetry packet (Phase 6).
    pub const EMIT_ODOM_TELEMETRY: u8 = 0x31;
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

// ── Phase 5 — Movement Intent & Routing ──────────────────────────────────────

/// A movement intent published by the Wasm ABI to the OS Message Bus.
///
/// The Wasm guest calls `host_set_motor_speed()` which creates a
/// `MovementIntent` and posts it to the internal bus.  The Routing Engine
/// then decides whether to forward the intent to Core 1's local balancing
/// loop (Single-Board mode) or to serialise it for ESP-NOW transmission
/// (Distributed mode).  The Wasm sandbox never knows the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MovementIntent {
    /// Target speed for the left drive motor (−255 … +255).
    pub left_speed: i16,
    /// Target speed for the right drive motor (−255 … +255).
    pub right_speed: i16,
}

impl MovementIntent {
    /// Create a new `MovementIntent` from validated speed values.
    #[inline]
    pub fn new(left: i16, right: i16) -> Self {
        Self { left_speed: left, right_speed: right }
    }

    /// Create a zero-speed (full stop) intent.
    #[inline]
    pub fn zero() -> Self {
        Self { left_speed: 0, right_speed: 0 }
    }
}

/// Routing mode for the OS Message Bus.
///
/// Configured at OS boot time (or toggled at runtime) to switch between
/// single-chip and distributed operation without the Wasm sandbox knowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum RoutingMode {
    /// The Router forwards movement intents directly to Core 1's local
    /// balancing loop via the shared motor-command bridge.
    #[default]
    SingleBoard = 0,
    /// The Router intercepts intents, serialises them, and transmits them
    /// over the ESP-NOW radio stack to a remote Worker board.
    Distributed = 1,
}

// ── Phase 6 — CDR Serializer ──────────────────────────────────────────────────

/// A zero-allocation little-endian CDR (Common Data Representation) serializer.
///
/// CDR is the wire format used by DDS and ROS 2 for structured message
/// serialization.  This implementation uses little-endian byte order to match
/// the ESP32-S3 architecture and produces compact, alignment-free payloads
/// suitable for transmission over ESP-NOW.
///
/// The const-generic `N` parameter defines the maximum buffer capacity in bytes.
/// Attempting to write beyond `N` bytes returns `Err(())` without panicking,
/// making this safe for `no_std` embedded use.
pub struct CdrSerializer<const N: usize> {
    buf: [u8; N],
    pos: usize,
}

impl<const N: usize> CdrSerializer<N> {
    /// Create a new serializer with an empty buffer.
    #[inline]
    pub const fn new() -> Self {
        Self { buf: [0u8; N], pos: 0 }
    }

    /// Write a single `u8` byte.
    #[inline]
    pub fn write_u8(&mut self, v: u8) -> Result<(), ()> {
        if self.pos + 1 > N { return Err(()); }
        self.buf[self.pos] = v;
        self.pos += 1;
        Ok(())
    }

    /// Write a `u16` as two little-endian bytes.
    #[inline]
    pub fn write_u16(&mut self, v: u16) -> Result<(), ()> {
        if self.pos + 2 > N { return Err(()); }
        let b = v.to_le_bytes();
        self.buf[self.pos]     = b[0];
        self.buf[self.pos + 1] = b[1];
        self.pos += 2;
        Ok(())
    }

    /// Write an `i16` as two little-endian bytes.
    #[inline]
    pub fn write_i16(&mut self, v: i16) -> Result<(), ()> {
        self.write_u16(v as u16)
    }

    /// Write a `u32` as four little-endian bytes.
    #[inline]
    pub fn write_u32(&mut self, v: u32) -> Result<(), ()> {
        if self.pos + 4 > N { return Err(()); }
        let b = v.to_le_bytes();
        self.buf[self.pos..self.pos + 4].copy_from_slice(&b);
        self.pos += 4;
        Ok(())
    }

    /// Write an `i32` as four little-endian bytes.
    #[inline]
    pub fn write_i32(&mut self, v: i32) -> Result<(), ()> {
        self.write_u32(v as u32)
    }

    /// Write a `u64` as eight little-endian bytes.
    #[inline]
    pub fn write_u64(&mut self, v: u64) -> Result<(), ()> {
        if self.pos + 8 > N { return Err(()); }
        let b = v.to_le_bytes();
        self.buf[self.pos..self.pos + 8].copy_from_slice(&b);
        self.pos += 8;
        Ok(())
    }

    /// Return a slice over the bytes written so far.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.pos]
    }

    /// Return the number of bytes written so far.
    #[inline]
    pub fn len(&self) -> usize {
        self.pos
    }

    /// Return `true` if no bytes have been written yet.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }
}

// ── Phase 6 — Structured Telemetry Payloads ───────────────────────────────────

/// Structured IMU telemetry payload.
///
/// Modelled after the ROS 2 `sensor_msgs/Imu` message, simplified for
/// zero-allocation embedded use.  All angles and rates are expressed as
/// fixed-point integers to avoid floating-point in `no_std` contexts.
///
/// Core 1 builds this struct every real-time loop tick and pushes it to the
/// telemetry TX channel.  The host serialises it to CDR bytes for radio
/// transmission via ESP-NOW.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImuTelemetry {
    /// Monotonically incrementing packet sequence number (wrapping on overflow).
    pub sequence: u32,
    /// Timestamp in microseconds since firmware boot.
    pub timestamp_us: u64,
    /// Core 1 loop execution time in microseconds (for jitter monitoring).
    pub loop_time_us: u32,
    /// Pitch angle in millidegrees (positive = nose up).
    pub pitch_millideg: i32,
    /// Roll angle in millidegrees (positive = right side down).
    pub roll_millideg: i32,
    /// Yaw rate in millidegrees/second (stub — requires gyroscope integration).
    pub yaw_rate_millideg_s: i32,
    /// Linear acceleration X in mm/s² (stub — requires accelerometer integration).
    pub linear_accel_x_mm_s2: i32,
    /// Linear acceleration Y in mm/s² (stub — requires accelerometer integration).
    pub linear_accel_y_mm_s2: i32,
}

impl ImuTelemetry {
    /// Total serialized size in bytes (CDR little-endian, no padding).
    ///
    /// Layout:
    /// - `[0..4]`   `sequence`           (u32 LE)
    /// - `[4..12]`  `timestamp_us`       (u64 LE)
    /// - `[12..16]` `loop_time_us`       (u32 LE)
    /// - `[16..20]` `pitch_millideg`     (i32 LE)
    /// - `[20..24]` `roll_millideg`      (i32 LE)
    /// - `[24..28]` `yaw_rate_millideg_s`(i32 LE)
    /// - `[28..32]` `linear_accel_x_mm_s2` (i32 LE)
    /// - `[32..36]` `linear_accel_y_mm_s2` (i32 LE)
    pub const SERIALIZED_SIZE: usize = 36;

    /// Serialize this packet to CDR little-endian bytes.
    ///
    /// Writes exactly [`SERIALIZED_SIZE`] bytes into `buf`.
    /// Returns the number of bytes written, or `0` if `buf` is too small.
    pub fn to_cdr(&self, buf: &mut [u8]) -> usize {
        if buf.len() < Self::SERIALIZED_SIZE {
            return 0;
        }
        let mut s = CdrSerializer::<{ Self::SERIALIZED_SIZE }>::new();
        s.write_u32(self.sequence).ok();
        s.write_u64(self.timestamp_us).ok();
        s.write_u32(self.loop_time_us).ok();
        s.write_i32(self.pitch_millideg).ok();
        s.write_i32(self.roll_millideg).ok();
        s.write_i32(self.yaw_rate_millideg_s).ok();
        s.write_i32(self.linear_accel_x_mm_s2).ok();
        s.write_i32(self.linear_accel_y_mm_s2).ok();
        let bytes = s.as_bytes();
        buf[..bytes.len()].copy_from_slice(bytes);
        bytes.len()
    }

    /// Deserialize an [`ImuTelemetry`] from a CDR little-endian byte slice.
    ///
    /// Returns `None` if `buf` is shorter than [`SERIALIZED_SIZE`].
    pub fn from_cdr(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::SERIALIZED_SIZE {
            return None;
        }
        Some(Self {
            sequence: u32::from_le_bytes([buf[0],  buf[1],  buf[2],  buf[3]]),
            timestamp_us: u64::from_le_bytes([
                buf[4], buf[5], buf[6],  buf[7],
                buf[8], buf[9], buf[10], buf[11],
            ]),
            loop_time_us: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            pitch_millideg:       i32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            roll_millideg:        i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            yaw_rate_millideg_s:  i32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
            linear_accel_x_mm_s2: i32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]),
            linear_accel_y_mm_s2: i32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]),
        })
    }
}

/// Structured Odometry telemetry payload.
///
/// Modelled after the ROS 2 `nav_msgs/Odometry` message, simplified for
/// embedded use.  Carries the current motor speeds and loop timing so the
/// remote receiver can reconstruct position estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OdometryTelemetry {
    /// Monotonically incrementing packet sequence number (wrapping on overflow).
    pub sequence: u32,
    /// Timestamp in microseconds since firmware boot.
    pub timestamp_us: u64,
    /// Core 1 loop execution time in microseconds.
    pub loop_time_us: u32,
    /// Left motor speed in the range −255 … +255.
    pub left_speed: i16,
    /// Right motor speed in the range −255 … +255.
    pub right_speed: i16,
}

impl OdometryTelemetry {
    /// Total serialized size in bytes (CDR little-endian, no padding).
    ///
    /// Layout:
    /// - `[0..4]`   `sequence`     (u32 LE)
    /// - `[4..12]`  `timestamp_us` (u64 LE)
    /// - `[12..16]` `loop_time_us` (u32 LE)
    /// - `[16..18]` `left_speed`   (i16 LE)
    /// - `[18..20]` `right_speed`  (i16 LE)
    pub const SERIALIZED_SIZE: usize = 20;

    /// Serialize this packet to CDR little-endian bytes.
    ///
    /// Writes exactly [`SERIALIZED_SIZE`] bytes into `buf`.
    /// Returns the number of bytes written, or `0` if `buf` is too small.
    pub fn to_cdr(&self, buf: &mut [u8]) -> usize {
        if buf.len() < Self::SERIALIZED_SIZE {
            return 0;
        }
        let mut s = CdrSerializer::<{ Self::SERIALIZED_SIZE }>::new();
        s.write_u32(self.sequence).ok();
        s.write_u64(self.timestamp_us).ok();
        s.write_u32(self.loop_time_us).ok();
        s.write_i16(self.left_speed).ok();
        s.write_i16(self.right_speed).ok();
        let bytes = s.as_bytes();
        buf[..bytes.len()].copy_from_slice(bytes);
        bytes.len()
    }

    /// Deserialize an [`OdometryTelemetry`] from a CDR little-endian byte slice.
    ///
    /// Returns `None` if `buf` is shorter than [`SERIALIZED_SIZE`].
    pub fn from_cdr(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::SERIALIZED_SIZE {
            return None;
        }
        Some(Self {
            sequence:     u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            timestamp_us: u64::from_le_bytes([
                buf[4], buf[5], buf[6],  buf[7],
                buf[8], buf[9], buf[10], buf[11],
            ]),
            loop_time_us: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            left_speed:   i16::from_le_bytes([buf[16], buf[17]]),
            right_speed:  i16::from_le_bytes([buf[18], buf[19]]),
        })
    }
}

/// A telemetry packet queued for radio transmission.
///
/// Produced by Core 1 (and optionally by the Wasm ABI) and consumed by the
/// ESP-NOW TX task.  The `#[repr(u8)]`-tagged variants carry type discriminants
/// that are prepended to the CDR payload during serialization so the receiver
/// can decode them correctly.
#[derive(Debug, Clone, Copy)]
pub enum TelemetryPacket {
    /// Structured IMU data (ROS 2 `sensor_msgs/Imu`-inspired).
    Imu(ImuTelemetry),
    /// Structured odometry data (ROS 2 `nav_msgs/Odometry`-inspired).
    Odometry(OdometryTelemetry),
}

impl TelemetryPacket {
    /// Type discriminant byte written before the CDR payload for `Imu` packets.
    pub const TYPE_IMU:      u8 = cmd::EMIT_IMU_TELEMETRY;
    /// Type discriminant byte written before the CDR payload for `Odometry` packets.
    pub const TYPE_ODOMETRY: u8 = cmd::EMIT_ODOM_TELEMETRY;

    /// Maximum serialized size of any packet variant (1-byte discriminant + payload).
    pub const MAX_SERIALIZED_SIZE: usize = 1 + ImuTelemetry::SERIALIZED_SIZE;

    /// Serialize the packet to `buf`.
    ///
    /// The first byte is the type discriminant ([`TYPE_IMU`] or
    /// [`TYPE_ODOMETRY`]); the remaining bytes are the CDR-encoded payload.
    ///
    /// Returns the total number of bytes written, or `0` if `buf` is too small.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        match self {
            TelemetryPacket::Imu(imu) => {
                let needed = 1 + ImuTelemetry::SERIALIZED_SIZE;
                if buf.len() < needed { return 0; }
                buf[0] = Self::TYPE_IMU;
                1 + imu.to_cdr(&mut buf[1..])
            }
            TelemetryPacket::Odometry(odom) => {
                let needed = 1 + OdometryTelemetry::SERIALIZED_SIZE;
                if buf.len() < needed { return 0; }
                buf[0] = Self::TYPE_ODOMETRY;
                1 + odom.to_cdr(&mut buf[1..])
            }
        }
    }

    /// Deserialize a packet from `buf`.
    ///
    /// The first byte must be a valid type discriminant; the remainder is the
    /// CDR-encoded payload.  Returns `None` for unknown discriminants or
    /// truncated buffers.
    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        if buf.is_empty() { return None; }
        match buf[0] {
            Self::TYPE_IMU      => ImuTelemetry::from_cdr(&buf[1..]).map(TelemetryPacket::Imu),
            Self::TYPE_ODOMETRY => OdometryTelemetry::from_cdr(&buf[1..]).map(TelemetryPacket::Odometry),
            _ => None,
        }
    }
}

// ── Phase 6 — Telemetry capacity constant ─────────────────────────────────────

/// Capacity of the telemetry TX queue.
///
/// At 100 packets/second (one every 10 ms) and a 32-slot queue the system can
/// absorb up to 320 ms of radio back-pressure before dropping packets.  Core 1
/// uses non-blocking `try_send` so it never stalls the hard real-time loop.
pub const TELEMETRY_TX_CAPACITY: usize = 32;

/// How many Core 1 ticks to skip between emitted telemetry packets.
///
/// Core 1 runs at 500 Hz (2 ms per tick).  With a decimation of 5 we emit one
/// packet every 10 ms = 100 packets/second — matching the design target.
pub const TELEMETRY_DECIMATION: u64 = 5;

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
            cmd::EMIT_IMU_TELEMETRY,
            cmd::EMIT_ODOM_TELEMETRY,
        ];
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            assert!(seen.insert(id), "duplicate command ID: 0x{:02X}", id);
        }
    }

    // ── Phase 5 tests ─────────────────────────────────────────────────────────

    #[test]
    fn movement_intent_new() {
        let intent = MovementIntent::new(100, -50);
        assert_eq!(intent.left_speed, 100);
        assert_eq!(intent.right_speed, -50);
    }

    #[test]
    fn movement_intent_zero() {
        let intent = MovementIntent::zero();
        assert_eq!(intent.left_speed, 0);
        assert_eq!(intent.right_speed, 0);
    }

    #[test]
    fn movement_intent_default_is_zero() {
        let intent = MovementIntent::default();
        assert_eq!(intent.left_speed, 0);
        assert_eq!(intent.right_speed, 0);
    }

    #[test]
    fn routing_mode_default_is_single_board() {
        assert_eq!(RoutingMode::default(), RoutingMode::SingleBoard);
    }

    #[test]
    fn routing_mode_discriminants_are_stable() {
        assert_eq!(RoutingMode::SingleBoard as u8, 0);
        assert_eq!(RoutingMode::Distributed  as u8, 1);
    }

    #[test]
    fn dead_mans_switch_timeout_is_50ms() {
        assert_eq!(DEAD_MANS_SWITCH_MS, 50);
    }

    // ── Phase 6 tests — CDR Serializer ────────────────────────────────────────

    #[test]
    fn cdr_serializer_write_u8() {
        let mut s = CdrSerializer::<4>::new();
        s.write_u8(0xAB).unwrap();
        assert_eq!(s.as_bytes(), &[0xAB]);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn cdr_serializer_write_u16_le() {
        let mut s = CdrSerializer::<4>::new();
        s.write_u16(0x1234).unwrap();
        assert_eq!(s.as_bytes(), &[0x34, 0x12]);
    }

    #[test]
    fn cdr_serializer_write_i16_le() {
        let mut s = CdrSerializer::<4>::new();
        s.write_i16(-1).unwrap();
        assert_eq!(s.as_bytes(), &[0xFF, 0xFF]);
    }

    #[test]
    fn cdr_serializer_write_u32_le() {
        let mut s = CdrSerializer::<8>::new();
        s.write_u32(0x12345678).unwrap();
        assert_eq!(s.as_bytes(), &[0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn cdr_serializer_write_u64_le() {
        let mut s = CdrSerializer::<8>::new();
        s.write_u64(0x0102030405060708u64).unwrap();
        assert_eq!(s.as_bytes(), &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn cdr_serializer_overflow_returns_err() {
        let mut s = CdrSerializer::<2>::new();
        s.write_u16(0x1234).unwrap();
        // Next write must fail — buffer is full.
        assert!(s.write_u8(0x00).is_err());
        assert!(s.write_u16(0x0000).is_err());
    }

    #[test]
    fn cdr_serializer_is_empty_after_new() {
        let s = CdrSerializer::<8>::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    // ── Phase 6 tests — ImuTelemetry ──────────────────────────────────────────

    #[test]
    fn imu_telemetry_serialized_size_is_36() {
        assert_eq!(ImuTelemetry::SERIALIZED_SIZE, 36);
    }

    #[test]
    fn imu_telemetry_cdr_roundtrip() {
        let original = ImuTelemetry {
            sequence:              42,
            timestamp_us:          1_000_000,
            loop_time_us:          1_987,
            pitch_millideg:        15_000,
            roll_millideg:         -3_500,
            yaw_rate_millideg_s:   200,
            linear_accel_x_mm_s2:  9_810,
            linear_accel_y_mm_s2:  0,
        };
        let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
        let written = original.to_cdr(&mut buf);
        assert_eq!(written, ImuTelemetry::SERIALIZED_SIZE);
        let decoded = ImuTelemetry::from_cdr(&buf).expect("must decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn imu_telemetry_to_cdr_returns_0_for_short_buffer() {
        let imu = ImuTelemetry::default();
        let mut buf = [0u8; 10]; // too short
        assert_eq!(imu.to_cdr(&mut buf), 0);
    }

    #[test]
    fn imu_telemetry_from_cdr_returns_none_for_short_buffer() {
        let buf = [0u8; 10]; // too short
        assert!(ImuTelemetry::from_cdr(&buf).is_none());
    }

    #[test]
    fn imu_telemetry_negative_values_roundtrip() {
        let original = ImuTelemetry {
            pitch_millideg: i32::MIN,
            roll_millideg:  i32::MAX,
            ..Default::default()
        };
        let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
        original.to_cdr(&mut buf);
        let decoded = ImuTelemetry::from_cdr(&buf).unwrap();
        assert_eq!(decoded.pitch_millideg, i32::MIN);
        assert_eq!(decoded.roll_millideg,  i32::MAX);
    }

    // ── Phase 6 tests — OdometryTelemetry ────────────────────────────────────

    #[test]
    fn odom_telemetry_serialized_size_is_20() {
        assert_eq!(OdometryTelemetry::SERIALIZED_SIZE, 20);
    }

    #[test]
    fn odom_telemetry_cdr_roundtrip() {
        let original = OdometryTelemetry {
            sequence:     7,
            timestamp_us: 500_000,
            loop_time_us: 2_001,
            left_speed:   127,
            right_speed:  -127,
        };
        let mut buf = [0u8; OdometryTelemetry::SERIALIZED_SIZE];
        let written = original.to_cdr(&mut buf);
        assert_eq!(written, OdometryTelemetry::SERIALIZED_SIZE);
        let decoded = OdometryTelemetry::from_cdr(&buf).expect("must decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn odom_telemetry_to_cdr_returns_0_for_short_buffer() {
        let odom = OdometryTelemetry::default();
        let mut buf = [0u8; 5]; // too short
        assert_eq!(odom.to_cdr(&mut buf), 0);
    }

    // ── Phase 6 tests — TelemetryPacket ──────────────────────────────────────

    #[test]
    fn telemetry_packet_imu_serialize_roundtrip() {
        let imu = ImuTelemetry {
            sequence: 1,
            pitch_millideg: 5_000,
            roll_millideg: -2_000,
            ..Default::default()
        };
        let packet = TelemetryPacket::Imu(imu);
        let mut buf = [0u8; TelemetryPacket::MAX_SERIALIZED_SIZE];
        let written = packet.serialize(&mut buf);
        assert_eq!(written, 1 + ImuTelemetry::SERIALIZED_SIZE);
        assert_eq!(buf[0], TelemetryPacket::TYPE_IMU);

        let decoded = TelemetryPacket::deserialize(&buf[..written]).expect("must decode");
        match decoded {
            TelemetryPacket::Imu(d) => {
                assert_eq!(d.pitch_millideg, 5_000);
                assert_eq!(d.roll_millideg, -2_000);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn telemetry_packet_odometry_serialize_roundtrip() {
        let odom = OdometryTelemetry {
            left_speed: 100, right_speed: -80, ..Default::default()
        };
        let packet = TelemetryPacket::Odometry(odom);
        let mut buf = [0u8; TelemetryPacket::MAX_SERIALIZED_SIZE];
        let written = packet.serialize(&mut buf);
        assert_eq!(written, 1 + OdometryTelemetry::SERIALIZED_SIZE);
        assert_eq!(buf[0], TelemetryPacket::TYPE_ODOMETRY);

        let decoded = TelemetryPacket::deserialize(&buf[..written]).unwrap();
        match decoded {
            TelemetryPacket::Odometry(d) => {
                assert_eq!(d.left_speed, 100);
                assert_eq!(d.right_speed, -80);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn telemetry_packet_deserialize_empty_returns_none() {
        assert!(TelemetryPacket::deserialize(&[]).is_none());
    }

    #[test]
    fn telemetry_packet_deserialize_unknown_discriminant_returns_none() {
        let buf = [0xFF, 0x00, 0x00];
        assert!(TelemetryPacket::deserialize(&buf).is_none());
    }

    #[test]
    fn telemetry_packet_max_serialized_size_matches_imu() {
        assert_eq!(TelemetryPacket::MAX_SERIALIZED_SIZE, 1 + ImuTelemetry::SERIALIZED_SIZE);
    }

    #[test]
    fn telemetry_tx_capacity_is_32() {
        assert_eq!(TELEMETRY_TX_CAPACITY, 32);
    }

    #[test]
    fn telemetry_decimation_gives_100_pps() {
        // At 500 Hz with decimation 5: 500 / 5 = 100 pps.
        let pps = 500u64 / TELEMETRY_DECIMATION;
        assert_eq!(pps, 100);
    }
}
