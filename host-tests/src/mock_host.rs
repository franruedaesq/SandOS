//! Software mock of the ESP32-S3 hardware state.
//!
//! [`MockHost`] implements the same method signatures as the firmware's
//! `AbiHost` struct so the host-side tests exercise the same logic paths
//! that will run on the chip.

use abi::{
    status, EyeExpression, ImuReading, MAX_AUDIO_READ, MAX_BRIGHTNESS, MAX_MOTOR_SPEED,
    MAX_TEXT_BYTES, WorkerPacket,
};

// ── Mock display ──────────────────────────────────────────────────────────────

/// Simulated display state (replaces the real SPI + DMA driver in tests).
#[derive(Debug, Default)]
pub struct MockDisplay {
    /// Most recently rendered eye expression.
    pub current_expression: Option<EyeExpression>,
    /// Most recently written display text.
    pub display_text: String,
    /// Current brightness (0–255).
    pub brightness: u8,
}

impl MockDisplay {
    pub fn draw_eye(&mut self, expression: EyeExpression) -> i32 {
        self.current_expression = Some(expression);
        status::OK
    }

    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                self.display_text = s.to_owned();
                status::OK
            }
            Err(_) => status::ERR_INVALID_ARG,
        }
    }

    pub fn set_brightness(&mut self, value: u8) -> i32 {
        self.brightness = value;
        status::OK
    }
}

// ── Mock host ─────────────────────────────────────────────────────────────────

/// Full mock of the `AbiHost` struct.
///
/// All state transitions that the firmware would perform on hardware are
/// instead recorded in this struct so tests can assert on them.
#[derive(Debug)]
pub struct MockHost {
    /// Current LED state.
    pub led_on: bool,

    /// Number of times `toggle_led` has been called.
    pub toggle_count: u32,

    /// Simulated display.
    pub display: MockDisplay,

    /// Whether the microphone is currently active.
    pub audio_active: bool,

    /// Audio bytes fed into the mock buffer by the test harness.
    pub audio_buf: std::collections::VecDeque<u8>,

    /// Debug log messages received from the guest.
    pub log_messages: Vec<String>,

    /// Simulated uptime in milliseconds (controlled by tests).
    pub simulated_uptime_ms: u64,

    // Phase 3 — Sensors
    /// Simulated IMU reading (set by tests to inject sensor data).
    pub imu_reading: ImuReading,

    // Phase 4 — Motors
    /// Most recently commanded left motor speed (−255 … +255).
    pub motor_left_speed: i32,

    /// Most recently commanded right motor speed (−255 … +255).
    pub motor_right_speed: i32,

    /// Whether the motors are currently enabled (false = safe-shutdown active).
    pub motors_enabled: bool,

    /// Number of times the watchdog has been fed (incremented per ABI call
    /// in the harness to simulate the firmware's post-command WDT feed).
    pub watchdog_feed_count: u32,

    // Phase 5 — Distributed robotics
    /// Motor speed pairs queued for transmission to the Worker as ESP-NOW packets.
    ///
    /// Each entry represents a `WorkerPacket::motor_speed(left, right)` that the
    /// Brain would send over the air.  Tests can inspect this queue to verify
    /// that `set_motor_speed` encodes and enqueues the command correctly.
    pub outgoing_worker_cmds: Vec<[u8; 8]>,
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            led_on: false,
            toggle_count: 0,
            display: MockDisplay::default(),
            audio_active: false,
            audio_buf: std::collections::VecDeque::new(),
            log_messages: Vec::new(),
            simulated_uptime_ms: 0,
            imu_reading: ImuReading::default(),
            motor_left_speed: 0,
            motor_right_speed: 0,
            motors_enabled: true,
            watchdog_feed_count: 0,
            outgoing_worker_cmds: Vec::new(),
        }
    }
}

impl MockHost {
    // ── Phase 1 ──────────────────────────────────────────────────────────────

    /// Toggle the LED; returns [`status::OK`].
    pub fn toggle_led(&mut self) -> i32 {
        self.led_on = !self.led_on;
        self.toggle_count += 1;
        status::OK
    }

    /// Return `simulated_uptime_ms` (tests can set this to any value).
    pub fn get_uptime_ms(&self) -> i64 {
        self.simulated_uptime_ms as i64
    }

    /// Record a UTF-8 string from the guest.
    pub fn debug_log(&mut self, bytes: &[u8]) -> i32 {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                self.log_messages.push(s.to_owned());
                status::OK
            }
            Err(_) => status::ERR_INVALID_ARG,
        }
    }

    // ── Phase 2 — Display ─────────────────────────────────────────────────────

    /// Render an eye expression; validates the argument.
    pub fn draw_eye(&mut self, expression_raw: i32) -> i32 {
        match EyeExpression::from_i32(expression_raw) {
            Some(expr) => self.display.draw_eye(expr),
            None => status::ERR_INVALID_ARG,
        }
    }

    /// Write text to the display; validates length and UTF-8 encoding.
    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() as u32 > MAX_TEXT_BYTES {
            return status::ERR_BOUNDS;
        }
        self.display.write_text(bytes)
    }

    /// Set brightness; validates the range.
    pub fn set_brightness(&mut self, value: i32) -> i32 {
        if value < 0 || value > MAX_BRIGHTNESS {
            return status::ERR_INVALID_ARG;
        }
        self.display.set_brightness(value as u8)
    }

    // ── Phase 2 — Audio ───────────────────────────────────────────────────────

    pub fn start_audio_capture(&mut self) -> i32 {
        self.audio_active = true;
        self.audio_buf.clear();
        status::OK
    }

    pub fn stop_audio_capture(&mut self) -> i32 {
        self.audio_active = false;
        status::OK
    }

    pub fn get_audio_avail(&self) -> i32 {
        self.audio_buf.len() as i32
    }

    /// Copy up to `out.len()` bytes from the audio buffer into `out`.
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

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// Feed raw PCM bytes into the mock audio ring buffer.
    pub fn feed_audio(&mut self, data: &[u8]) {
        self.audio_buf.extend(data.iter().copied());
    }

    // ── Phase 3 — Sensors ─────────────────────────────────────────────────────

    /// Return the current simulated IMU reading.
    ///
    /// Tests set [`MockHost::imu_reading`] directly to inject sensor data.
    pub fn get_pitch_roll(&self) -> ImuReading {
        self.imu_reading
    }

    // ── Phase 4 — Motors ──────────────────────────────────────────────────────

    /// Set the target speed for both drive motors.
    ///
    /// ## Phase 5 behaviour
    ///
    /// In addition to updating `motor_left_speed` / `motor_right_speed` (kept
    /// for backward compatibility with Phase 4 tests), this method now also
    /// encodes a [`WorkerPacket::motor_speed`] frame and pushes it to
    /// [`MockHost::outgoing_worker_cmds`].  Phase 5 tests can inspect this
    /// queue to verify that the Brain correctly encodes and enqueues motor
    /// commands for the Worker.
    ///
    /// Validates the range `[-MAX_MOTOR_SPEED, MAX_MOTOR_SPEED]` and the
    /// `motors_enabled` flag, mirroring the firmware's `AbiHost::set_motor_speed`.
    ///
    /// Increments `watchdog_feed_count` to simulate the post-command WDT feed
    /// that the firmware performs in `wasm_run_task`.
    pub fn set_motor_speed(&mut self, left: i32, right: i32) -> i32 {
        if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
            return status::ERR_INVALID_ARG;
        }
        if !self.motors_enabled {
            return status::ERR_BUSY;
        }
        self.motor_left_speed = left;
        self.motor_right_speed = right;
        self.watchdog_feed_count += 1;
        // Phase 5: also encode as a WorkerPacket and push to the outgoing queue.
        self.outgoing_worker_cmds
            .push(WorkerPacket::motor_speed(left as i16, right as i16));
        status::OK
    }
}
