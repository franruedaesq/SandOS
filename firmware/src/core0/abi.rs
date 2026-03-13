//! Host-Guest ABI — the Host side.
//!
//! This module owns all mutable hardware state that the Wasm guest can affect
//! through ABI calls.  Every public method validates its arguments *before*
//! touching hardware, ensuring the sandbox cannot corrupt hardware state.
//!
//! ## Phase 1 functions
//! - [`AbiHost::toggle_led`]
//! - [`AbiHost::get_uptime_ms`]
//! - [`AbiHost::debug_log`]
//!
//! ## Phase 2 functions
//! - [`AbiHost::draw_eye`]
//! - [`AbiHost::write_text`]
//! - [`AbiHost::set_brightness`]
//! - [`AbiHost::start_audio_capture`]
//! - [`AbiHost::stop_audio_capture`]
//! - [`AbiHost::get_audio_avail`]
//! - [`AbiHost::read_audio`]
//!
//! ## Phase 3 functions
//! - [`AbiHost::get_pitch_roll`]
//!
//! ## Phase 4 functions
//! - [`AbiHost::set_motor_speed`]
//!
//! ## Phase 5 — Message Bus
//!
//! `set_motor_speed` no longer writes to the motor-command bridge directly.
//! Instead it creates a [`abi::MovementIntent`] and publishes it to the OS
//! Message Bus ([`crate::message_bus`]).  The Routing Engine
//! ([`crate::router`]) then dispatches the intent to either Core 1's local
//! balancing loop (Single-Board mode) or the ESP-NOW radio stack (Distributed
//! mode) — the Wasm sandbox never knows the difference.
//!
//! ## Phase 6 — Structured Telemetry
//!
//! - [`AbiHost::emit_imu_telemetry`] — the Wasm guest passes a CDR-encoded
//!   [`abi::ImuTelemetry`] payload; the host deserializes it and pushes it to
//!   the telemetry TX channel for asynchronous radio broadcast.
//! - [`AbiHost::emit_odom_telemetry`] — same for [`abi::OdometryTelemetry`].
//! - [`AbiHost::get_telemetry_queue_len`] — returns the number of packets
//!   currently queued in the telemetry TX channel.
//!
//! ## Phase 7 — Local AI Subsystem
//!
//! - [`AbiHost::get_local_inference`] — returns the latest result from the
//!   embedded [`crate::inference::TinyMlEngine`].  When the radio link is
//!   alive this returns an inactive result (`active = false`).  Once the
//!   fallback router engages (link silent) the field is populated with the
//!   most recent class prediction and confidence score.
//! - [`AbiHost::push_audio_for_inference`] — called internally by
//!   [`AbiHost::start_audio_capture`] / [`AbiHost::stop_audio_capture`] to
//!   push audio snapshots into the fallback inference channel when the radio
//!   link is detected as silent.
use abi::{
    status, EyeExpression, ImuReading, ImuTelemetry, MovementIntent,
    OdometryTelemetry, OtaState, OtaStatus, TelemetryPacket, INFERENCE_RESULT_SIZE, MAX_AUDIO_READ, MAX_BRIGHTNESS,
    MAX_MOTOR_SPEED, MAX_TEXT_BYTES, OTA_STATUS_SIZE,
};
use esp_hal::gpio::Io;

use crate::display::DisplayDriver;
use crate::rgb_led::RgbLedDriver;
use crate::{inference, message_bus, motors, router, sensors, telemetry};

// ── Host state ────────────────────────────────────────────────────────────────

/// All mutable state accessible through the Host-Guest ABI.
///
/// This struct is stored in the `wasmi::Store` and is passed to every host
/// function via `Caller::data_mut()`.
pub struct AbiHost {
    /// Current state of the onboard LED.
    pub led_on: bool,

    /// Millisecond timestamp of firmware boot (set in `brain_task`).
    pub boot_time_ms: u64,

    // Phase 2
    /// Handle to the DMA display driver.
    pub display: DisplayDriver,

    /// Whether the I2S microphone is currently streaming.
    pub audio_active: bool,

    /// Simple ring buffer for incoming I2S audio data (8 KiB).
    pub audio_buf: heapless::Deque<u8, 8192>,

    // Phase 8 — OTA Hot-Swap Engine
    /// Current state of the OTA state machine.
    pub ota_state: OtaState,

    /// Total expected binary size declared in OTA_BEGIN.
    pub ota_expected_size: u32,

    /// Running count of payload bytes received.
    pub ota_bytes_received: u32,

    /// Number of successful hot-swaps completed.
    pub hot_swap_count: u32,

    /// RGB LED driver for controlling the WS2812 LED.
    pub rgb_led: RgbLedDriver,

    /// GPIO IO handle (kept alive for LED control).
    _io: Io,
}

#[allow(dead_code)]
impl AbiHost {
    /// Construct a new [`AbiHost`] with all peripherals in their reset state.
    pub fn new(io: Io, display: DisplayDriver, rgb_led: RgbLedDriver) -> Self {
        Self {
            led_on: false,
            boot_time_ms: 0,
            display,
            audio_active: false,
            audio_buf: heapless::Deque::new(),
            ota_state: OtaState::Idle,
            ota_expected_size: 0,
            ota_bytes_received: 0,
            hot_swap_count: 0,
            rgb_led,
            _io: io,
        }
    }

    // ── Phase 1 ──────────────────────────────────────────────────────────────

    /// Toggle the onboard LED and return [`status::OK`].
    ///
    /// This is the simplest possible ABI call and serves as the Phase 1
    /// smoke test.
    pub fn toggle_led(&mut self) -> i32 {
        self.led_on = !self.led_on;
        // Physical GPIO is driven via the Gpio OutputPin stored elsewhere;
        // in the real firmware `led_pin.set_level(self.led_on)` is called here.
        // The state is exposed so host-tests can assert on it.
        status::OK
    }

    /// Return the number of milliseconds since firmware boot.
    pub fn get_uptime_ms(&self) -> u64 {
        embassy_time::Instant::now().as_millis()
    }

    /// Log a UTF-8 string from the Wasm guest memory (best-effort).
    ///
    /// `bytes` is the slice already copied from Wasm linear memory by the VM.
    /// Silently ignores non-UTF-8 content.
    pub fn debug_log(&self, bytes: &[u8]) -> i32 {
        if let Ok(_s) = core::str::from_utf8(bytes) {
            // In production: use defmt::info!("{}", s);
        }
        status::OK
    }

    // ── Phase 2 — Display ─────────────────────────────────────────────────────

    /// Render a robot eye expression on the display.
    ///
    /// Returns [`status::ERR_INVALID_ARG`] for unknown expression values.
    pub fn draw_eye(&mut self, expression_raw: i32) -> i32 {
        match EyeExpression::from_i32(expression_raw) {
            Some(expr) => self.display.draw_eye(expr),
            None => status::ERR_INVALID_ARG,
        }
    }

    /// Write a UTF-8 string from the Wasm guest onto the display.
    ///
    /// Returns [`status::ERR_BOUNDS`] if `len` exceeds [`MAX_TEXT_BYTES`].
    /// The caller is responsible for pre-validating the (ptr, len) against
    /// Wasm linear memory bounds before invoking this method.
    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() as u32 > MAX_TEXT_BYTES {
            return status::ERR_BOUNDS;
        }
        self.display.write_text(bytes)
    }

    /// Set the display backlight brightness (0–255).
    ///
    /// Returns [`status::ERR_INVALID_ARG`] for values above [`MAX_BRIGHTNESS`].
    pub fn set_brightness(&mut self, value: i32) -> i32 {
        if value < 0 || value > MAX_BRIGHTNESS {
            return status::ERR_INVALID_ARG;
        }
        self.display.set_brightness(value as u8)
    }

    // ── Phase 2 — Audio ───────────────────────────────────────────────────────

    /// Begin streaming from the I2S microphone.
    pub fn start_audio_capture(&mut self) -> i32 {
        self.audio_active = true;
        self.audio_buf.clear();
        status::OK
    }

    /// Stop microphone streaming.
    pub fn stop_audio_capture(&mut self) -> i32 {
        self.audio_active = false;
        status::OK
    }

    /// Return the number of bytes currently available in the audio ring buffer.
    pub fn get_audio_avail(&self) -> i32 {
        self.audio_buf.len() as i32
    }

    /// Copy up to `max_len` bytes from the audio buffer into `out`.
    ///
    /// Returns the number of bytes actually copied, or [`status::ERR_BOUNDS`]
    /// if `max_len` exceeds [`MAX_AUDIO_READ`].
    pub fn read_audio(&mut self, out: &mut [u8]) -> i32 {
        // Try to empty AUDIO_RX_CHANNEL into audio_buf
        while let Ok(chunk) = crate::audio::AUDIO_RX_CHANNEL.try_receive() {
            for &byte in chunk.iter() {
                // If the buffer is full, we drop the oldest samples
                if self.audio_buf.is_full() {
                    let _ = self.audio_buf.pop_front();
                }
                let _ = self.audio_buf.push_back(byte);
            }
        }

        if out.len() as u32 > MAX_AUDIO_READ {
            return status::ERR_BOUNDS;
        }
        let n = out.len().min(self.audio_buf.len());
        for byte in out.iter_mut().take(n) {
            *byte = self.audio_buf.pop_front().unwrap_or(0);
        }
        n as i32
    }

    /// Play audio by sending the buffer to the I2S DMA TX channel.
    ///
    /// Returns [`status::OK`] if the audio chunk was queued, or
    /// [`status::ERR_BUSY`] if the transmission queue is full.
    pub fn play_audio(&mut self, bytes: &[u8]) -> i32 {
        use abi::MAX_AUDIO_PLAY;
        if bytes.len() as u32 > MAX_AUDIO_PLAY {
            return status::ERR_BOUNDS;
        }
        let mut chunk = crate::audio::AudioChunk::new();
        for &byte in bytes {
            // Push cannot fail since len <= MAX_AUDIO_PLAY
            let _ = chunk.push(byte);
        }
        if crate::audio::AUDIO_TX_CHANNEL.try_send(chunk).is_ok() {
            status::OK
        } else {
            status::ERR_BUSY
        }
    }

    // ── Phase 3 — Sensors ─────────────────────────────────────────────────────

    /// Return the latest IMU reading from the shared atomic bridge.
    ///
    /// The value is written by Core 1's 500 Hz real-time polling loop; this
    /// method reads it with `Acquire` ordering so it always reflects the
    /// most recently *completed* write.
    ///
    /// Returns an [`ImuReading`] with `pitch_millideg` and `roll_millideg`.
    pub fn get_pitch_roll(&self) -> ImuReading {
        sensors::load_imu()
    }

    // ── Phase 4 / Phase 5 — Motors ────────────────────────────────────────────

    /// Set the target speed for the left and right drive motors.
    ///
    /// Speeds are in the signed range `[-MAX_MOTOR_SPEED, MAX_MOTOR_SPEED]`
    /// where positive values drive the wheel forward and negative values drive
    /// it backward.
    ///
    /// ## Phase 5 — Message Bus abstraction
    ///
    /// This method no longer writes to the motor-command bridge directly.
    /// Instead it publishes a [`MovementIntent`] to the OS Message Bus so that
    /// the Routing Engine ([`crate::router::router_task`]) can decide whether
    /// to forward the intent to Core 1's local balancing loop (Single-Board
    /// mode) or serialise it for ESP-NOW transmission (Distributed mode).
    /// The Wasm sandbox is completely unaware of which backend is active.
    ///
    /// ## Motors-enabled gate
    ///
    /// The ULP safe-shutdown check (`MOTOR_ENABLED`) is applied here at the
    /// ABI boundary — before the intent is published — so that a locked-out
    /// safe-shutdown immediately surfaces as [`status::ERR_BUSY`] to the Wasm
    /// guest rather than silently queueing intents that the Router would then
    /// have to discard.  This matches the Phase 4 behaviour where
    /// `store_motor_command` performed the same check.
    ///
    /// ## Return codes
    ///
    /// | Value                       | Meaning                                    |
    /// |-----------------------------|------------------------------------------- |
    /// | [`status::OK`]              | Intent published and enqueued.             |
    /// | [`status::ERR_INVALID_ARG`] | Speed out of `[-255, 255]` range.          |
    /// | [`status::ERR_BUSY`]        | Motors disabled (ULP safe-shutdown active),|
    /// |                             | or the intent queue is momentarily full.   |
    pub fn set_motor_speed(&self, left: i32, right: i32) -> i32 {
        if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
            return status::ERR_INVALID_ARG;
        }
        if !motors::is_motor_enabled() {
            return status::ERR_BUSY;
        }
        let intent = MovementIntent::new(left as i16, right as i16);
        if message_bus::publish_intent(intent) {
            status::OK
        } else {
            status::ERR_BUSY
        }
    }

    // ── Phase 6 — Structured Telemetry ───────────────────────────────────────

    /// Emit a CDR-encoded [`ImuTelemetry`] packet from the Wasm sandbox.
    ///
    /// The Wasm guest writes a 36-byte CDR payload into its linear memory and
    /// calls this function with `(ptr, len)`.  The host deserializes the
    /// payload and pushes a [`TelemetryPacket::Imu`] to the async telemetry TX
    /// channel.  The ESP-NOW task broadcasts it to the PC asynchronously.
    ///
    /// ## Return codes
    ///
    /// | Value                       | Meaning                              |
    /// |-----------------------------|--------------------------------------|
    /// | [`status::OK`]              | Packet enqueued for transmission.    |
    /// | [`status::ERR_BOUNDS`]      | `len` ≠ [`ImuTelemetry::SERIALIZED_SIZE`]. |
    /// | [`status::ERR_BUSY`]        | Telemetry TX queue is full.          |
    pub fn emit_imu_telemetry(&self, bytes: &[u8]) -> i32 {
        if bytes.len() != ImuTelemetry::SERIALIZED_SIZE {
            return status::ERR_BOUNDS;
        }
        match ImuTelemetry::from_cdr(bytes) {
            Some(imu) => {
                if telemetry::push_telemetry(TelemetryPacket::Imu(imu)) {
                    status::OK
                } else {
                    status::ERR_BUSY
                }
            }
            None => status::ERR_BOUNDS,
        }
    }

    /// Emit a CDR-encoded [`OdometryTelemetry`] packet from the Wasm sandbox.
    ///
    /// The Wasm guest writes a 20-byte CDR payload into its linear memory and
    /// calls this function with `(ptr, len)`.  The host deserializes the
    /// payload and pushes a [`TelemetryPacket::Odometry`] to the async
    /// telemetry TX channel.
    ///
    /// ## Return codes
    ///
    /// | Value                       | Meaning                              |
    /// |-----------------------------|--------------------------------------|
    /// | [`status::OK`]              | Packet enqueued for transmission.    |
    /// | [`status::ERR_BOUNDS`]      | `len` ≠ [`OdometryTelemetry::SERIALIZED_SIZE`]. |
    /// | [`status::ERR_BUSY`]        | Telemetry TX queue is full.          |
    pub fn emit_odom_telemetry(&self, bytes: &[u8]) -> i32 {
        if bytes.len() != OdometryTelemetry::SERIALIZED_SIZE {
            return status::ERR_BOUNDS;
        }
        match OdometryTelemetry::from_cdr(bytes) {
            Some(odom) => {
                if telemetry::push_telemetry(TelemetryPacket::Odometry(odom)) {
                    status::OK
                } else {
                    status::ERR_BUSY
                }
            }
            None => status::ERR_BOUNDS,
        }
    }

    /// Return the number of packets currently queued in the telemetry TX channel.
    ///
    /// The Wasm guest can poll this value to implement flow control and avoid
    /// flooding the radio queue.
    pub fn get_telemetry_queue_len(&self) -> i32 {
        telemetry::TELEMETRY_TX_CHANNEL.len() as i32
    }

    // ── Phase 7 — Local AI Subsystem ─────────────────────────────────────────

    /// Return the latest result from the local inference engine.
    ///
    /// Writes three `i32` little-endian values into `out`:
    ///
    /// | Offset  | Field            | Range     |
    /// |---------|------------------|-----------|
    /// | `[0..4]`  | `active`       | 0 or 1    |
    /// | `[4..8]`  | `top_class`    | 0 – 255   |
    /// | `[8..12]` | `confidence_pct` | 0 – 100 |
    ///
    /// `active` is `1` only when the fallback inference pipeline has run at
    /// least once since the radio link went silent.  While the radio link is
    /// alive `active` is `0` and the other fields are zero-filled.
    ///
    /// ## Return codes
    ///
    /// | Value                  | Meaning                                       |
    /// |------------------------|-----------------------------------------------|
    /// | [`status::OK`]         | Result written to `out`.                      |
    /// | [`status::ERR_BOUNDS`] | `out.len()` < [`INFERENCE_RESULT_SIZE`] (12). |
    pub fn get_local_inference(&self, out: &mut [u8]) -> i32 {
        if out.len() < INFERENCE_RESULT_SIZE as usize {
            return status::ERR_BOUNDS;
        }
        let result = inference::load_inference_result();
        result.to_bytes(out);
        status::OK
    }

    /// Push a snapshot of the current audio buffer into the fallback inference
    /// channel so the Router can feed it to the [`inference::TinyMlEngine`].
    ///
    /// This is called internally whenever audio data has been written to
    /// `audio_buf` and the radio link is detected as silent.  The audio bytes
    /// are reinterpreted as signed 8-bit samples (the quantised format expected
    /// by TFLite Micro INT8 models).
    ///
    /// The channel is non-blocking: if it is full the snapshot is silently
    /// dropped.  This is intentional — old snapshots are less useful than fresh
    /// ones, so we prefer freshness over completeness.
    pub fn push_audio_for_inference(&mut self) {
        use abi::INFERENCE_TENSOR_SIZE;
        let n = self.audio_buf.len().min(INFERENCE_TENSOR_SIZE);
        if n == 0 {
            return;
        }
        let mut snapshot = router::AudioSnapshot::new();
        for &byte in self.audio_buf.iter().take(n) {
            // Reinterpret the raw PCM byte as a signed i8 sample.
            let sample = byte as i8;
            snapshot.push(sample).ok();
        }
        // Non-blocking: silently discard if the queue is full.
        router::AUDIO_INFERENCE_CHANNEL.try_send(snapshot).ok();
    }

    // ── Phase 8 ──────────────────────────────────────────────────────────────

    /// Return a serialized [`OtaStatus`] snapshot into `out`.
    ///
    /// Writes [`OTA_STATUS_SIZE`] bytes (16 bytes: 4 × u32) into `out` and
    /// returns [`status::OK`], or [`status::ERR_BOUNDS`] if `out` is too small.
    ///
    /// The status includes:
    /// - Current OTA state (Idle, Receiving, or Ready)
    /// - Bytes received so far
    /// - Total expected size
    /// - Number of hot-swaps completed
    pub fn get_ota_status(&self, out: &mut [u8]) -> i32 {
        if out.len() < OTA_STATUS_SIZE as usize {
            return status::ERR_BOUNDS;
        }
        let snapshot = OtaStatus {
            state:          self.ota_state,
            bytes_received: self.ota_bytes_received,
            total_size:     self.ota_expected_size,
            swap_count:     self.hot_swap_count,
        };
        snapshot.to_bytes(out);
        status::OK
    }

    // ── Phase 9 — RGB LED Control ────────────────────────────────────────────

    /// Set the RGB LED to the specified color.
    ///
    /// Each component (red, green, blue) must be in the range 0-255.
    /// Returns [`status::ERR_INVALID_ARG`] for out-of-range values.
    pub fn set_rgb_led(&mut self, red: i32, green: i32, blue: i32) -> i32 {
        // Validate color components are within 0-255 range
        if red < 0 || red > 255 || green < 0 || green > 255 || blue < 0 || blue > 255 {
            return status::ERR_INVALID_ARG;
        }

        self.rgb_led.set_color(red as u8, green as u8, blue as u8);
        status::OK
    }

    /// Get the current RGB LED color.
    ///
    /// Writes the current red, green, and blue values to the provided pointers.
    /// Each pointer must point to a valid i32 memory location.
    /// Returns [`status::OK`] on success.
    pub fn get_rgb_led(&self, red_ptr: *mut i32, green_ptr: *mut i32, blue_ptr: *mut i32) -> i32 {
        let (r, g, b) = self.rgb_led.get_color();

        // SAFETY: The Wasm VM ensures these pointers are valid within Wasm memory.
        unsafe {
            *red_ptr = r as i32;
            *green_ptr = g as i32;
            *blue_ptr = b as i32;
        }

        status::OK
    }
}
