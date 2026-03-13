//! Software mock of the ESP32-S3 hardware state.
//!
//! [`MockHost`] implements the same method signatures as the firmware's
//! `AbiHost` struct so the host-side tests exercise the same logic paths
//! that will run on the chip.

use abi::{
    status, EyeExpression, ImuReading, ImuTelemetry, InferenceResult, MovementIntent,
    OdometryTelemetry, OtaState, OtaStatus, RoutingMode, TelemetryPacket,
    crc32, DEAD_MANS_SWITCH_MS, INFERENCE_RESULT_SIZE, MAX_AUDIO_READ, MAX_BRIGHTNESS,
    MAX_MOTOR_SPEED, MAX_TEXT_BYTES, OTA_MAX_BINARY_SIZE, OTA_STATUS_SIZE, TELEMETRY_TX_CAPACITY,
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

    /// Audio bytes played by the guest.
    pub audio_tx_buf: std::collections::VecDeque<u8>,

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

    // Phase 5 — Message Bus & Routing
    /// Current routing mode for the OS Message Bus.
    pub routing_mode: RoutingMode,

    /// Log of every [`MovementIntent`] published to the OS Message Bus.
    ///
    /// Tests can inspect this to verify that the ABI publishes an intent
    /// before routing it, regardless of the current `routing_mode`.
    pub intent_log: Vec<MovementIntent>,

    /// Millisecond timestamp of the most recent successfully published intent.
    ///
    /// Updated by [`MockHost::set_motor_speed`] from `simulated_uptime_ms`.
    /// Used by [`MockHost::check_dead_mans_switch`] to detect staleness.
    pub last_intent_ms: u64,

    /// Whether the dead-man's switch is currently active.
    ///
    /// Set to `true` by [`MockHost::check_dead_mans_switch`] when the gap
    /// since the last intent exceeds [`DEAD_MANS_SWITCH_MS`], and reset to
    /// `false` when a fresh intent arrives.
    pub dead_mans_active: bool,

    /// Intents that were routed via ESP-NOW (Distributed mode only).
    ///
    /// In Single-Board mode this Vec remains empty; in Distributed mode it
    /// accumulates every intent that would be serialised and sent over the air.
    pub distributed_intents: Vec<MovementIntent>,

    // Phase 6 — Structured Telemetry
    /// Telemetry packets emitted by the Wasm guest (via the ABI) or by Core 1.
    ///
    /// Acts as the mock telemetry TX queue.  Tests can inspect this to verify
    /// that telemetry packets are correctly constructed and enqueued.
    pub telemetry_queue: Vec<TelemetryPacket>,

    // Phase 7 — Local AI Subsystem
    /// Most recent result from the local inference engine.
    ///
    /// Tests set this directly to simulate what the embedded `TinyMlEngine`
    /// would have written; [`MockHost::get_local_inference`] returns it.
    pub inference_result: InferenceResult,

    /// Simulated radio link state.
    ///
    /// When `false` (link silent), `check_radio_link_alive` treats the link
    /// as lost and the fallback inference pipeline becomes eligible to run.
    pub radio_link_alive: bool,

    /// Audio snapshots queued for fallback inference (mirrors the firmware's
    /// `AUDIO_INFERENCE_CHANNEL`).
    pub audio_inference_queue: Vec<Vec<i8>>,

    // Phase 8 — OTA Hot-Swap Engine
    /// Current state of the OTA state machine.
    pub ota_state: OtaState,

    /// PSRAM staging buffer for the incoming Wasm binary.
    ///
    /// Pre-allocated to `ota_expected_size` on `ota_begin`, then filled
    /// by successive `ota_receive_chunk` calls.
    pub ota_buffer: Vec<u8>,

    /// Total expected binary size declared in `OTA_BEGIN`.
    pub ota_expected_size: u32,

    /// Running count of payload bytes written to `ota_buffer`.
    pub ota_bytes_received: u32,

    /// Number of successful hot-swaps completed since the host was created.
    pub hot_swap_count: u32,

    /// Whether the Wasm VM is currently paused during a hot-swap.
    pub vm_paused: bool,

    /// The binary currently loaded in the simulated Wasm VM.
    ///
    /// Starts empty (the static `guest.wasm` binary is assumed pre-loaded).
    /// After a successful hot-swap this field holds the newly installed binary.
    pub active_wasm_binary: Vec<u8>,
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            led_on: false,
            toggle_count: 0,
            display: MockDisplay::default(),
            audio_active: false,
            audio_buf: std::collections::VecDeque::new(),
            audio_tx_buf: std::collections::VecDeque::new(),
            log_messages: Vec::new(),
            simulated_uptime_ms: 0,
            imu_reading: ImuReading::default(),
            motor_left_speed: 0,
            motor_right_speed: 0,
            motors_enabled: true,
            watchdog_feed_count: 0,
            routing_mode: RoutingMode::SingleBoard,
            intent_log: Vec::new(),
            last_intent_ms: 0,
            dead_mans_active: false,
            distributed_intents: Vec::new(),
            telemetry_queue: Vec::new(),
            inference_result: InferenceResult::default(),
            radio_link_alive: true,
            audio_inference_queue: Vec::new(),
            ota_state: OtaState::Idle,
            ota_buffer: Vec::new(),
            ota_expected_size: 0,
            ota_bytes_received: 0,
            hot_swap_count: 0,
            vm_paused: false,
            active_wasm_binary: Vec::new(),
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

    pub fn play_audio(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() as u32 > abi::MAX_AUDIO_PLAY {
            return status::ERR_BOUNDS;
        }
        self.audio_tx_buf.extend(bytes.iter().copied());
        status::OK
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
    /// Validates the range `[-MAX_MOTOR_SPEED, MAX_MOTOR_SPEED]` and the
    /// `motors_enabled` flag, mirroring the firmware's `AbiHost::set_motor_speed`.
    ///
    /// ### Phase 5 behaviour
    ///
    /// On success the method creates a [`MovementIntent`] and appends it to
    /// `intent_log` (always), then routes it:
    ///
    /// * **Single-Board** — updates `motor_left_speed` / `motor_right_speed`
    ///   directly, mirroring the Core 1 bridge.
    /// * **Distributed** — appends the intent to `distributed_intents` and
    ///   leaves the local motor speeds unchanged (they would be set by the
    ///   remote Worker board).
    ///
    /// Also updates `last_intent_ms` from `simulated_uptime_ms` and resets
    /// `dead_mans_active`, then increments `watchdog_feed_count`.
    pub fn set_motor_speed(&mut self, left: i32, right: i32) -> i32 {
        if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
            return status::ERR_INVALID_ARG;
        }
        if !self.motors_enabled {
            return status::ERR_BUSY;
        }

        // Phase 5: publish a MovementIntent to the OS Message Bus.
        let intent = MovementIntent::new(left as i16, right as i16);
        self.intent_log.push(intent);
        self.last_intent_ms = self.simulated_uptime_ms;
        self.dead_mans_active = false;

        // Route based on the current mode.
        match self.routing_mode {
            RoutingMode::SingleBoard => {
                // Forward directly to Core 1's motor bridge.
                self.motor_left_speed = left;
                self.motor_right_speed = right;
            }
            RoutingMode::Distributed => {
                // Serialise and transmit over ESP-NOW (recorded for test assertions).
                self.distributed_intents.push(intent);
            }
        }

        self.watchdog_feed_count += 1;
        status::OK
    }

    // ── Phase 5 — Dead-Man's Switch ───────────────────────────────────────────

    // ── Phase 6 — Structured Telemetry ───────────────────────────────────────

    /// Emit a CDR-encoded [`ImuTelemetry`] packet from the Wasm sandbox.
    ///
    /// Mirrors `AbiHost::emit_imu_telemetry` on the firmware.  Validates the
    /// byte length, deserializes the CDR payload, and appends the packet to
    /// `telemetry_queue`.  Returns [`status::ERR_BOUNDS`] for bad lengths or
    /// [`status::ERR_BUSY`] when the queue is at capacity.
    pub fn emit_imu_telemetry(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() != ImuTelemetry::SERIALIZED_SIZE {
            return status::ERR_BOUNDS;
        }
        match ImuTelemetry::from_cdr(bytes) {
            Some(imu) => {
                if self.telemetry_queue.len() >= TELEMETRY_TX_CAPACITY {
                    return status::ERR_BUSY;
                }
                self.telemetry_queue.push(TelemetryPacket::Imu(imu));
                status::OK
            }
            None => status::ERR_BOUNDS,
        }
    }

    /// Emit a CDR-encoded [`OdometryTelemetry`] packet from the Wasm sandbox.
    ///
    /// Mirrors `AbiHost::emit_odom_telemetry` on the firmware.
    pub fn emit_odom_telemetry(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() != OdometryTelemetry::SERIALIZED_SIZE {
            return status::ERR_BOUNDS;
        }
        match OdometryTelemetry::from_cdr(bytes) {
            Some(odom) => {
                if self.telemetry_queue.len() >= TELEMETRY_TX_CAPACITY {
                    return status::ERR_BUSY;
                }
                self.telemetry_queue.push(TelemetryPacket::Odometry(odom));
                status::OK
            }
            None => status::ERR_BOUNDS,
        }
    }

    /// Return the number of packets currently in the telemetry TX queue.
    pub fn get_telemetry_queue_len(&self) -> i32 {
        self.telemetry_queue.len() as i32
    }

    ///
    /// Compares `current_ms` against `last_intent_ms`.  If the gap exceeds
    /// [`DEAD_MANS_SWITCH_MS`], `dead_mans_active` is set to `true` and the
    /// motor speeds are zeroed (the control loop is shut down safely).
    ///
    /// Call this from test code to simulate the Router's timeout logic.
    pub fn check_dead_mans_switch(&mut self, current_ms: u64) {
        if current_ms.saturating_sub(self.last_intent_ms) > DEAD_MANS_SWITCH_MS {
            self.dead_mans_active = true;
            self.motor_left_speed = 0;
            self.motor_right_speed = 0;
        }
    }

    // ── Phase 7 — Local AI Subsystem ─────────────────────────────────────────

    /// Return the current inference result.
    ///
    /// Mirrors `AbiHost::get_local_inference` on the firmware.  Writes the
    /// 12-byte serialized [`InferenceResult`] into `out` and returns
    /// [`status::OK`], or [`status::ERR_BOUNDS`] if `out` is too small.
    pub fn get_local_inference(&self, out: &mut [u8]) -> i32 {
        if out.len() < INFERENCE_RESULT_SIZE as usize {
            return status::ERR_BOUNDS;
        }
        self.inference_result.to_bytes(out);
        status::OK
    }

    /// Return `true` when the simulated radio link is alive.
    ///
    /// Uses `simulated_uptime_ms` as the "current time" and compares the
    /// last received packet timestamp (approximated here by `last_intent_ms`
    /// for simplicity — in the firmware this is `RADIO_LAST_RX_MS`).
    pub fn is_radio_link_alive(&self) -> bool {
        self.radio_link_alive
    }

    /// Push audio samples into the fallback inference queue.
    ///
    /// Mirrors `AbiHost::push_audio_for_inference`.  The samples are stored
    /// in `audio_inference_queue` so tests can assert that the fallback
    /// pipeline received the correct data.
    pub fn push_audio_for_inference(&mut self, samples: Vec<i8>) {
        self.audio_inference_queue.push(samples);
    }

    /// Run the fallback inference stub on the most recent audio snapshot.
    ///
    /// Pops one snapshot from `audio_inference_queue`, runs the same
    /// deterministic stub as `TinyMlEngine::run`, and stores the result in
    /// `inference_result`.  Returns `true` if a snapshot was available.
    pub fn run_fallback_inference(&mut self) -> bool {
        if self.audio_inference_queue.is_empty() {
            return false;
        }
        let snapshot = self.audio_inference_queue.remove(0);
        self.inference_result = run_stub_inference(&snapshot);
        true
    }

    // ── Phase 8 — OTA Hot-Swap Engine ─────────────────────────────────────────

    /// Begin an OTA session.
    ///
    /// Declares the total binary size, resets the staging buffer, and
    /// transitions the state machine to [`OtaState::Receiving`].
    ///
    /// An in-progress `Receiving` session is silently cancelled and replaced
    /// by the new one, allowing the PC to restart a failed transfer without
    /// a manual reset.
    ///
    /// Returns [`status::ERR_INVALID_ARG`] if `total_size` is zero or exceeds
    /// [`OTA_MAX_BINARY_SIZE`].  Returns [`status::ERR_BUSY`] when a hot-swap
    /// is currently in progress.
    pub fn ota_begin(&mut self, total_size: u32) -> i32 {
        if total_size == 0 || total_size as usize > OTA_MAX_BINARY_SIZE {
            return status::ERR_INVALID_ARG;
        }
        if self.ota_state == OtaState::Swapping {
            return status::ERR_BUSY;
        }
        self.ota_state = OtaState::Receiving;
        self.ota_expected_size = total_size;
        self.ota_bytes_received = 0;
        // Pre-allocate the staging buffer filled with zeros so chunks can be
        // written at any offset without gaps.
        self.ota_buffer = vec![0u8; total_size as usize];
        status::OK
    }

    /// Write one chunk of OTA binary data into the PSRAM staging buffer.
    ///
    /// `offset` is the byte offset within the final binary.  `data` must not
    /// extend beyond `ota_expected_size`.
    ///
    /// Returns [`status::ERR_BUSY`] if no session is active,
    /// [`status::ERR_BOUNDS`] if `offset + data.len() > ota_expected_size`, or
    /// [`status::ERR_INVALID_ARG`] if `data` is empty.
    pub fn ota_receive_chunk(&mut self, offset: u32, data: &[u8]) -> i32 {
        if self.ota_state != OtaState::Receiving {
            return status::ERR_BUSY;
        }
        if data.is_empty() {
            return status::ERR_INVALID_ARG;
        }
        let end = (offset as usize).saturating_add(data.len());
        if end > self.ota_expected_size as usize {
            return status::ERR_BOUNDS;
        }
        self.ota_buffer[offset as usize..end].copy_from_slice(data);
        self.ota_bytes_received += data.len() as u32;
        status::OK
    }

    /// Finalise the OTA session by verifying the CRC-32 of the staged binary.
    ///
    /// If the computed CRC matches `expected_crc32` the state machine advances
    /// to [`OtaState::Ready`]; otherwise it transitions to [`OtaState::Failed`]
    /// and returns [`status::ERR_INVALID_ARG`].
    ///
    /// Returns [`status::ERR_BUSY`] if no session is in progress.
    pub fn ota_finalize(&mut self, expected_crc32: u32) -> i32 {
        if self.ota_state != OtaState::Receiving {
            return status::ERR_BUSY;
        }
        let actual = crc32(&self.ota_buffer);
        if actual != expected_crc32 {
            self.ota_state = OtaState::Failed;
            return status::ERR_INVALID_ARG;
        }
        self.ota_state = OtaState::Ready;
        status::OK
    }

    /// Execute the Wasm hot-swap routine.
    ///
    /// Implements the four-step Core 0 critical section:
    /// 1. **Pause** — signal the Wasm VM to stop accepting new commands.
    /// 2. **Flush** — drop the old Wasm linear memory sandbox.
    /// 3. **Instantiate** — load the new binary from the staging buffer.
    /// 4. **Resume** — unblock the Wasm VM command loop.
    ///
    /// Because this mock runs on a single host thread, Core 1's motor state is
    /// never touched — demonstrating that the hot-swap is isolated to Core 0.
    ///
    /// Returns [`status::ERR_BUSY`] if the binary has not yet been verified
    /// (i.e. `ota_state != Ready`).
    pub fn hot_swap_wasm(&mut self) -> i32 {
        if self.ota_state != OtaState::Ready {
            return status::ERR_BUSY;
        }

        // Step 1: Gracefully pause the VM (no new commands accepted).
        self.vm_paused = true;
        self.ota_state = OtaState::Swapping;

        // Step 2: Flush the old Wasm sandbox to prevent memory leaks.
        self.active_wasm_binary.clear();

        // Step 3: Instantiate the new binary from the PSRAM staging area.
        // In the real firmware this rebuilds the wasmi Engine + Store + Module
        // + Instance chain from the bytes in the PSRAM staging sector.
        self.active_wasm_binary = core::mem::take(&mut self.ota_buffer);

        // Step 4: Resume execution.
        self.vm_paused = false;
        self.hot_swap_count += 1;
        self.ota_state = OtaState::Idle;
        self.ota_expected_size = 0;
        self.ota_bytes_received = 0;

        status::OK
    }

    /// Return a serialized [`OtaStatus`] snapshot into `out`.
    ///
    /// Mirrors `AbiHost::get_ota_status` on the firmware.  Writes
    /// [`OTA_STATUS_SIZE`] bytes into `out` and returns [`status::OK`], or
    /// [`status::ERR_BOUNDS`] if `out` is too small.
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
}

// ── Phase 7 helpers ───────────────────────────────────────────────────────────

/// Stub inference matching `TinyMlEngine::run` in `firmware/src/inference.rs`.
///
/// This is the same deterministic algorithm, reproduced here for the test
/// harness so tests can compute expected outputs without depending on the
/// firmware crate.
pub fn run_stub_inference(tensor: &[i8]) -> InferenceResult {
    use abi::INFERENCE_TENSOR_SIZE;

    if tensor.is_empty() {
        return InferenceResult::default();
    }

    let mut freq = [0u32; 256];
    for &s in tensor.iter().take(INFERENCE_TENSOR_SIZE) {
        freq[s as u8 as usize] += 1;
    }
    let top_byte = freq
        .iter()
        .enumerate()
        .max_by_key(|&(_, &count)| count)
        .map(|(idx, _)| idx)
        .unwrap_or(0) as u8;
    let top_class = top_byte % 8;

    let sum: u32 = tensor
        .iter()
        .take(INFERENCE_TENSOR_SIZE)
        .map(|&s| s.unsigned_abs() as u32)
        .sum();
    let n = tensor.len().min(INFERENCE_TENSOR_SIZE) as u32;
    let mean_abs = sum / n;
    let confidence_pct = ((mean_abs * 100) / 127).min(100) as u8;

    InferenceResult { active: true, top_class, confidence_pct }
}
