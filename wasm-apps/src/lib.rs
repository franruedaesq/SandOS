//! SandOS Guest Wasm Application — Phase 1, 2, 3 & 4
//!
//! This is the "brain" that runs inside the Wasm sandbox on Core 0.
//! It cannot access any hardware directly; all hardware interactions
//! go through the Host-Guest ABI.
//!
//! ## ABI imports
//!
//! The linker resolves these imports against the functions registered by
//! `firmware/src/core0/wasm_vm.rs`.
//!
//! ## Build
//!
//! ```sh
//! rustup target add wasm32-unknown-unknown
//! cargo build --release --target wasm32-unknown-unknown
//! cp target/wasm32-unknown-unknown/release/wasm_apps.wasm ../firmware/guest.wasm
//! ```
#![no_std]
// `wasm32-unknown-unknown` does not have a standard allocator by default.
// For now all guest logic is stack-only (no heap required in Phase 1/2).

// ── ABI Imports (Phase 1) ─────────────────────────────────────────────────────

extern "C" {
    /// Toggle the onboard LED.  Returns `ABI_OK` (0) on success.
    fn host_toggle_led() -> i32;

    /// Return the number of milliseconds since firmware boot.
    fn host_get_uptime_ms() -> i64;

    /// Write a UTF-8 debug string to the Host log.
    fn host_debug_log(ptr: *const u8, len: i32) -> i32;
}

// ── ABI Imports (Phase 2 — Display) ──────────────────────────────────────────

extern "C" {
    /// Render an eye expression on the display.
    /// `expression` must be a valid [`EyeExpression`] discriminant (0–6).
    fn host_draw_eye(expression: i32) -> i32;

    /// Write a UTF-8 string to the display text area.
    fn host_write_text(ptr: *const u8, len: i32) -> i32;

    /// Set the display backlight brightness (0–255).
    fn host_set_brightness(value: i32) -> i32;
}

// ── ABI Imports (Phase 2 — Audio) ────────────────────────────────────────────

extern "C" {
    /// Begin streaming from the I2S microphone.
    fn host_start_audio_capture() -> i32;

    /// Stop microphone streaming.
    fn host_stop_audio_capture() -> i32;

    /// Return the number of audio bytes currently available.
    fn host_get_audio_avail() -> i32;

    /// Copy up to `max_len` bytes of audio into the buffer at `ptr`.
    /// Returns the number of bytes actually copied.
    fn host_read_audio(ptr: *mut u8, max_len: i32) -> i32;
}

// ── ABI Imports (Phase 3 — Sensors) ──────────────────────────────────────────

extern "C" {
    /// Read the latest IMU pitch and roll values into the provided pointers.
    /// Both `pitch_ptr` and `roll_ptr` must point to valid i32 slots in Wasm memory.
    fn host_get_pitch_roll(pitch_ptr: *mut i32, roll_ptr: *mut i32) -> i32;
}

// ── ABI Imports (Phase 4 — Motors) ───────────────────────────────────────────

extern "C" {
    /// Set the target speed for the left and right drive motors.
    ///
    /// Speeds are in the range `[-255, 255]`.  Positive values drive forward,
    /// negative values drive backward.  Returns `ABI_OK` (0) on success,
    /// `ERR_INVALID_ARG` (2) if a speed is out of range, or `ERR_BUSY` (3)
    /// if the ULP safe-shutdown is active.
    fn host_set_motor_speed(left: i32, right: i32) -> i32;
}

// ── ABI Imports (Phase 6 — Structured Telemetry) ─────────────────────────────

extern "C" {
    /// Emit a CDR-encoded ImuTelemetry payload from Wasm linear memory.
    ///
    /// `ptr` points to [`IMU_CDR_SIZE`] bytes of CDR data; `len` must equal
    /// [`IMU_CDR_SIZE`].  The host deserializes the payload and pushes it to
    /// the async radio TX queue.
    ///
    /// Returns `ABI_OK` (0) on success, `ERR_BOUNDS` (5) for wrong size, or
    /// `ERR_BUSY` (3) when the TX queue is full.
    fn host_emit_imu_telemetry(ptr: *const u8, len: i32) -> i32;

    /// Emit a CDR-encoded OdometryTelemetry payload from Wasm linear memory.
    ///
    /// `ptr` points to [`ODOM_CDR_SIZE`] bytes of CDR data; `len` must equal
    /// [`ODOM_CDR_SIZE`].
    fn host_emit_odom_telemetry(ptr: *const u8, len: i32) -> i32;

    /// Return the number of packets currently queued in the telemetry TX channel.
    ///
    /// The guest can poll this value for flow-control (e.g., back off if the
    /// queue is nearly full).
    fn host_get_telemetry_queue_len() -> i32;
}

// ── Command IDs ───────────────────────────────────────────────────────────────

/// Well-known command IDs matching [`abi::cmd`].
mod cmd {
    pub const TOGGLE_LED:         u8 = 0x01;
    pub const DRAW_EYE:           u8 = 0x10;
    pub const WRITE_TEXT:         u8 = 0x11;
    pub const CLEAR_DISPLAY:      u8 = 0x12;
    pub const SET_MOTOR_SPEED:    u8 = 0x20;
    pub const EMERGENCY_STOP:     u8 = 0x21;
    pub const EMIT_IMU_TELEMETRY: u8 = 0x30;
    pub const EMIT_ODOM_TELEMETRY: u8 = 0x31;
}

// ── Eye expression discriminants (must match [`abi::EyeExpression`]) ──────────

mod eye {
    pub const NEUTRAL:   i32 = 0;
    pub const HAPPY:     i32 = 1;
    pub const SAD:       i32 = 2;
    pub const ANGRY:     i32 = 3;
    pub const SURPRISED: i32 = 4;
    pub const THINKING:  i32 = 5;
    pub const BLINK:     i32 = 6;
}

// ── Guest state ───────────────────────────────────────────────────────────────

/// Number of LED toggles performed (for demo telemetry).
static mut TOGGLE_COUNT: u32 = 0;

/// Phase 6: CDR serialized size constants (must match `abi::ImuTelemetry::SERIALIZED_SIZE`).
const IMU_CDR_SIZE:  usize = 36;
/// Phase 6: CDR serialized size for OdometryTelemetry.
const ODOM_CDR_SIZE: usize = 20;

/// Phase 6: monotonic sequence counter for IMU telemetry packets.
static mut IMU_SEQ: u32 = 0;
/// Phase 6: monotonic sequence counter for Odometry telemetry packets.
static mut ODOM_SEQ: u32 = 0;

/// Nominal Core 1 loop period in microseconds (2 ms = 2 000 µs).
///
/// Used in stub telemetry packets; the real firmware measures the actual loop
/// duration from `embassy_time::Instant`.
const NOMINAL_LOOP_TIME_US: u32 = 2_000;

/// Telemetry queue high-water mark for flow control.
///
/// When the host-side telemetry TX queue has this many (or more) packets
/// pending, the guest skips the current emission cycle rather than risk
/// returning `ERR_BUSY`.  This leaves 4 free slots (87.5% utilisation) as a
/// safety margin for bursts from Core 1.
const TELEMETRY_QUEUE_HIGH_WATER_MARK: i32 = 28;

// ── Exported entry points ─────────────────────────────────────────────────────

/// Dispatch a command received via ESP-NOW.
///
/// Called by the Host once per received ESP-NOW packet.  The `cmd_id`
/// matches the [`cmd`] constants above.
///
/// Returns `0` (ABI_OK) for known commands, non-zero for unrecognised ones.
#[no_mangle]
pub extern "C" fn run_command(cmd_id: i32) -> i32 {
    match cmd_id as u8 {
        cmd::TOGGLE_LED      => toggle_led_handler(),
        cmd::DRAW_EYE        => draw_eye_handler(eye::HAPPY),
        cmd::WRITE_TEXT      => write_text_handler(b"Hello, World!"),
        cmd::CLEAR_DISPLAY   => clear_display_handler(),
        cmd::SET_MOTOR_SPEED     => balance_handler(),
        cmd::EMERGENCY_STOP      => emergency_stop_handler(),
        cmd::EMIT_IMU_TELEMETRY  => emit_imu_telemetry_handler(),
        cmd::EMIT_ODOM_TELEMETRY => emit_odom_telemetry_handler(),
        _                        => 1, // Unknown command
    }
}

/// Initialise the guest application.
///
/// Called once by the Host after the Wasm module is instantiated.
/// Sets the display to its default state and starts the audio pipeline.
#[no_mangle]
pub extern "C" fn guest_init() -> i32 {
    unsafe {
        host_draw_eye(eye::NEUTRAL);
        host_set_brightness(128);
    }
    0
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// Phase 1: Toggle the LED and log how many times it has been toggled.
fn toggle_led_handler() -> i32 {
    let status = unsafe { host_toggle_led() };
    if status == 0 {
        unsafe {
            TOGGLE_COUNT = TOGGLE_COUNT.wrapping_add(1);
        }
    }
    status
}

/// Phase 2: Draw a happy eye expression.
fn draw_eye_handler(expression: i32) -> i32 {
    unsafe { host_draw_eye(expression) }
}

/// Phase 2: Write a static greeting to the display.
fn write_text_handler(text: &[u8]) -> i32 {
    unsafe { host_write_text(text.as_ptr(), text.len() as i32) }
}

/// Phase 2: Clear the display by drawing a neutral expression and blank text.
fn clear_display_handler() -> i32 {
    let status = unsafe { host_draw_eye(eye::NEUTRAL) };
    if status != 0 {
        return status;
    }
    unsafe { host_write_text(b" ".as_ptr(), 1) }
}

/// Phase 2: Process an LLM text response from the PC.
///
/// The PC sends the raw text via ESP-NOW payload; the Host copies it into
/// Wasm memory and calls this function.
///
/// `ptr` points to a UTF-8 string in Wasm linear memory; `len` is its byte
/// length.  `mood` is an [`EyeExpression`] discriminant.
#[no_mangle]
pub extern "C" fn handle_llm_response(ptr: *const u8, len: i32, mood: i32) -> i32 {
    // 1. Update the eye expression to match the LLM's mood.
    let eye_status = unsafe { host_draw_eye(mood) };
    if eye_status != 0 {
        return eye_status;
    }

    // 2. Display the LLM's text.
    unsafe { host_write_text(ptr, len) }
}

/// Phase 4: Read current pitch from the IMU and set motor speeds proportionally.
///
/// This function implements the Wasm-side of the balance loop: it requests
/// the latest IMU reading and translates pitch into a symmetric motor command.
/// The real balance correction is performed by the PID controller on Core 1;
/// this call provides a high-level "intent" for steering/speed adjustment.
#[no_mangle]
pub extern "C" fn balance_handler() -> i32 {
    let mut pitch: i32 = 0;
    let mut roll: i32 = 0;
    let imu_status = unsafe { host_get_pitch_roll(&mut pitch as *mut i32, &mut roll as *mut i32) };
    if imu_status != 0 {
        return imu_status;
    }
    // Scale pitch (millideg) to a PWM duty cycle in [-255, 255].
    // 255_000 millideg = 255 degrees maps to full speed.
    let duty = (pitch / 1_000).clamp(-255, 255);
    unsafe { host_set_motor_speed(duty, duty) }
}

/// Phase 4: Set both motors to zero — immediate stop.
fn emergency_stop_handler() -> i32 {
    unsafe { host_set_motor_speed(0, 0) }
}

// ── Phase 6 handlers ─────────────────────────────────────────────────────────

/// Write a `u32` into `buf[offset..offset+4]` in little-endian byte order.
#[inline]
fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    let b = val.to_le_bytes();
    buf[offset..offset + 4].copy_from_slice(&b);
}

/// Write a `u64` into `buf[offset..offset+8]` in little-endian byte order.
#[inline]
fn write_u64_le(buf: &mut [u8], offset: usize, val: u64) {
    let b = val.to_le_bytes();
    buf[offset..offset + 8].copy_from_slice(&b);
}

/// Write an `i32` into `buf[offset..offset+4]` in little-endian byte order.
#[inline]
fn write_i32_le(buf: &mut [u8], offset: usize, val: i32) {
    write_u32_le(buf, offset, val as u32);
}

/// Write an `i16` into `buf[offset..offset+2]` in little-endian byte order.
#[inline]
fn write_i16_le(buf: &mut [u8], offset: usize, val: i16) {
    let b = (val as u16).to_le_bytes();
    buf[offset..offset + 2].copy_from_slice(&b);
}

/// Phase 6: Construct an ImuTelemetry CDR payload from current sensor data and
/// emit it to the host radio TX queue.
///
/// Reads the latest pitch/roll from the IMU, builds a 36-byte CDR-encoded
/// `ImuTelemetry` in Wasm linear memory, and calls `host_emit_imu_telemetry`.
#[no_mangle]
pub extern "C" fn emit_imu_telemetry_handler() -> i32 {
    let mut pitch: i32 = 0;
    let mut roll:  i32 = 0;
    let imu_status = unsafe {
        host_get_pitch_roll(&mut pitch as *mut i32, &mut roll as *mut i32)
    };
    if imu_status != 0 {
        return imu_status;
    }

    // Flow control: skip if the TX queue is nearly full (>= high-water mark).
    let qlen = unsafe { host_get_telemetry_queue_len() };
    if qlen >= TELEMETRY_QUEUE_HIGH_WATER_MARK {
        return 0; // ABI_OK — silently drop rather than return an error
    }

    let uptime_us = unsafe { host_get_uptime_ms() as u64 } * 1_000;
    let seq = unsafe {
        let s = IMU_SEQ;
        IMU_SEQ = IMU_SEQ.wrapping_add(1);
        s
    };

    // Build ImuTelemetry CDR payload (36 bytes) on the stack.
    let mut buf = [0u8; IMU_CDR_SIZE];
    write_u32_le(&mut buf,  0, seq);                      // sequence
    write_u64_le(&mut buf,  4, uptime_us);                // timestamp_us
    write_u32_le(&mut buf, 12, NOMINAL_LOOP_TIME_US);     // loop_time_us
    write_i32_le(&mut buf, 16, pitch);                    // pitch_millideg
    write_i32_le(&mut buf, 20, roll);                     // roll_millideg
    // yaw_rate_millideg_s, linear_accel_x/y remain zero (stubs)

    unsafe { host_emit_imu_telemetry(buf.as_ptr(), IMU_CDR_SIZE as i32) }
}

/// Phase 6: Construct an OdometryTelemetry CDR payload from current motor
/// state and emit it to the host radio TX queue.
#[no_mangle]
pub extern "C" fn emit_odom_telemetry_handler() -> i32 {
    // Flow control: skip if the TX queue is nearly full.
    let qlen = unsafe { host_get_telemetry_queue_len() };
    if qlen >= TELEMETRY_QUEUE_HIGH_WATER_MARK {
        return 0;
    }

    let uptime_us = unsafe { host_get_uptime_ms() as u64 } * 1_000;
    let seq = unsafe {
        let s = ODOM_SEQ;
        ODOM_SEQ = ODOM_SEQ.wrapping_add(1);
        s
    };

    // Build OdometryTelemetry CDR payload (20 bytes) on the stack.
    let mut buf = [0u8; ODOM_CDR_SIZE];
    write_u32_le(&mut buf,  0, seq);                      // sequence
    write_u64_le(&mut buf,  4, uptime_us);                // timestamp_us
    write_u32_le(&mut buf, 12, NOMINAL_LOOP_TIME_US);     // loop_time_us
    // left_speed and right_speed are 0 (stub; real impl reads motor state)
    write_i16_le(&mut buf, 16, 0);                        // left_speed
    write_i16_le(&mut buf, 18, 0);                        // right_speed

    unsafe { host_emit_odom_telemetry(buf.as_ptr(), ODOM_CDR_SIZE as i32) }
}

// ── panic handler (required for no_std) ──────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
