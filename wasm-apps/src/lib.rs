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

// ── Command IDs ───────────────────────────────────────────────────────────────

/// Well-known command IDs matching [`abi::cmd`].
mod cmd {
    pub const TOGGLE_LED:      u8 = 0x01;
    pub const DRAW_EYE:        u8 = 0x10;
    pub const WRITE_TEXT:      u8 = 0x11;
    pub const CLEAR_DISPLAY:   u8 = 0x12;
    pub const SET_MOTOR_SPEED: u8 = 0x20;
    pub const EMERGENCY_STOP:  u8 = 0x21;
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
        cmd::SET_MOTOR_SPEED => balance_handler(),
        cmd::EMERGENCY_STOP  => emergency_stop_handler(),
        _                    => 1, // Unknown command
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

// ── panic handler (required for no_std) ──────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
