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
use abi::{
    status, EyeExpression, ImuReading, MAX_AUDIO_READ, MAX_BRIGHTNESS, MAX_MOTOR_SPEED,
    MAX_TEXT_BYTES,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use esp_hal::gpio::Io;

use crate::display::DisplayDriver;
use crate::{motors, sensors};

// ── Phase 5: outgoing motor command sender type ───────────────────────────────

/// Sender half of the static channel that carries `(left, right)` motor speed
/// pairs from the Wasm ABI layer to the ESP-NOW transmitter task.
///
/// The channel depth of 4 provides a small burst buffer while keeping memory
/// usage minimal.  If the channel is full the ABI returns [`status::ERR_BUSY`].
pub type MotorCmdSender = Sender<'static, CriticalSectionRawMutex, (i16, i16), 4>;

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

    /// GPIO IO handle (kept alive for LED control).
    _io: Io,

    // Phase 5 — outgoing motor command queue
    /// Sender that forwards validated motor speed pairs to the ESP-NOW task.
    ///
    /// The ESP-NOW task drains this channel and sends each pair to the Worker
    /// chip as a [`abi::WorkerPacket::motor_speed`] packet.
    motor_tx: MotorCmdSender,
}

impl AbiHost {
    /// Construct a new [`AbiHost`] with all peripherals in their reset state.
    pub fn new(io: Io, display: DisplayDriver, motor_tx: MotorCmdSender) -> Self {
        Self {
            led_on: false,
            boot_time_ms: 0,
            display,
            audio_active: false,
            audio_buf: heapless::Deque::new(),
            _io: io,
            motor_tx,
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
        if out.len() as u32 > MAX_AUDIO_READ {
            return status::ERR_BOUNDS;
        }
        let n = out.len().min(self.audio_buf.len());
        for byte in out.iter_mut().take(n) {
            *byte = self.audio_buf.pop_front().unwrap_or(0);
        }
        n as i32
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
    /// ## Phase 5 behaviour
    ///
    /// Instead of writing to the local `MOTOR_COMMAND` atomic (which drove
    /// Core 1's on-board PWM in Phase 4), this method now **queues an
    /// ESP-NOW packet** for the Worker chip.  The ESP-NOW task picks up the
    /// `(left, right)` pair and transmits a [`abi::WorkerPacket::motor_speed`]
    /// frame to the Worker, which translates it to motor PWM output.
    ///
    /// The ULP safe-shutdown flag is still checked: if the Brain's ULP
    /// paramedic signals a critical voltage drop, new motor commands are
    /// rejected to protect the battery.
    ///
    /// ## Return codes
    ///
    /// | Value                       | Meaning                                        |
    /// |-----------------------------|------------------------------------------------|
    /// | [`status::OK`]              | Command accepted and enqueued for transmission.|
    /// | [`status::ERR_INVALID_ARG`] | Speed out of `[-255, 255]` range.              |
    /// | [`status::ERR_BUSY`]        | ULP safe-shutdown active *or* TX queue full.   |
    pub fn set_motor_speed(&self, left: i32, right: i32) -> i32 {
        if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
            return status::ERR_INVALID_ARG;
        }
        // Phase 4 ULP safety: still reject commands when the Brain detects a
        // critical voltage drop.
        if !motors::is_motor_enabled() {
            return status::ERR_BUSY;
        }
        // Phase 5: enqueue for the ESP-NOW transmitter task.
        match self.motor_tx.try_send((left as i16, right as i16)) {
            Ok(()) => status::OK,
            Err(_) => status::ERR_BUSY, // TX queue full
        }
    }
}
