//! OS Routing Engine — the flexible nervous system for Phase 5.
//!
//! The Router task runs on Core 0 alongside the Wasm engine.  It drains
//! [`MovementIntent`]s from the OS Message Bus and dispatches them to the
//! correct hardware backend based on the current [`RoutingMode`]:
//!
//! | Mode           | Backend                              |
//! |----------------|--------------------------------------|
//! | `SingleBoard`  | Core 1 motor-command bridge          |
//! | `Distributed`  | ESP-NOW radio (serialised packet)    |
//!
//! ## Dead-Man's Switch
//!
//! The Router waits for each intent with a [`DEAD_MANS_SWITCH_MS`]-millisecond
//! timeout.  If no intent arrives in time it immediately zeroes all motor
//! outputs, regardless of operating mode — this prevents a runaway robot when:
//!
//! * The Wasm application crashes or hangs.
//! * The ESP-NOW link from the Brain to the Worker is lost (Distributed mode).
//! * The robot's balancing loop loses its reference input.
//!
//! ## Phase 7 — Fallback Inference Pipeline
//!
//! When the dead-man's switch fires *and* the radio link has been silent for
//! longer than [`RADIO_SILENCE_THRESHOLD_MS`], the Router activates the local
//! inference fallback:
//!
//! 1. It reads the current audio ring-buffer snapshot from the shared
//!    [`AUDIO_INFERENCE_BUF`] channel (populated by [`AbiHost`] whenever audio
//!    capture is active).
//! 2. It passes the samples to the [`TinyMlEngine`] for a local inference pass.
//! 3. The result is stored in [`inference::INFERENCE_RESULT`] so the Wasm
//!    guest can query it via `host_get_local_inference()`.

use abi::{RoutingMode, DEAD_MANS_SWITCH_MS};
use embassy_time::Duration;

use crate::{inference, message_bus, motors};
use crate::core0::espnow;

// ── Inference audio snapshot channel ─────────────────────────────────────────

/// Maximum number of audio snapshot buffers queued for fallback inference.
const AUDIO_INFERENCE_QUEUE_DEPTH: usize = 2;

/// A fixed-size audio snapshot passed from the audio pipeline to the Router
/// for fallback inference.
///
/// Holds up to [`abi::INFERENCE_TENSOR_SIZE`] signed 8-bit samples that
/// represent one window of I2S microphone data.
pub type AudioSnapshot = heapless::Vec<i8, { abi::INFERENCE_TENSOR_SIZE }>;

/// Shared channel: audio pipeline → Router (for fallback inference).
///
/// [`AbiHost`] pushes a snapshot here when audio is active and the link is
/// silent.  The Router drains it during each dead-man's-switch timeout to
/// trigger a local inference pass.
pub static AUDIO_INFERENCE_CHANNEL: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    AudioSnapshot,
    AUDIO_INFERENCE_QUEUE_DEPTH,
> = embassy_sync::channel::Channel::new();

// ── Router Task ───────────────────────────────────────────────────────────────

/// Core 0 routing task.
///
/// Spawned by [`crate::core0::brain_task`] alongside the Wasm engine and the
/// ESP-NOW receiver.  Runs for the lifetime of the firmware.
///
/// ## Dead-Man's Switch behaviour
///
/// Each iteration waits for at most [`DEAD_MANS_SWITCH_MS`] milliseconds for a
/// new [`abi::MovementIntent`].  On timeout, motor outputs are zeroed so the
/// robot halts safely.  As soon as a fresh intent arrives the outputs are
/// restored — no operator intervention required.
///
/// ## Phase 7 — Fallback
///
/// When the timeout fires and the radio link is also detected as silent
/// (no valid packet within [`abi::RADIO_SILENCE_THRESHOLD_MS`] ms), the
/// Router additionally drains any pending audio snapshot from
/// [`AUDIO_INFERENCE_CHANNEL`] and runs the [`inference::ENGINE`] on it,
/// storing the result in [`inference::INFERENCE_RESULT`].
#[embassy_executor::task]
pub async fn router_task() {
    loop {
        match embassy_time::with_timeout(
            Duration::from_millis(DEAD_MANS_SWITCH_MS),
            message_bus::INTENT_CHANNEL.receive(),
        )
        .await
        {
            Ok(intent) => {
                // Valid intent received — route it to the appropriate backend.
                route_intent(intent);
            }
            Err(_timeout) => {
                // Dead-Man's Switch: no intent for >50 ms.
                // Zero all motor outputs to halt the robot safely.
                motors::store_motor_command(0, 0);

                // Phase 7: if the radio link is also silent, activate the
                // local inference pipeline with any pending audio snapshot.
                let now_ms = embassy_time::Instant::now().as_millis();
                if !espnow::is_radio_link_alive(now_ms) {
                    run_fallback_inference();
                }
            }
        }
    }
}

// ── Routing logic ─────────────────────────────────────────────────────────────

/// Dispatch a validated [`abi::MovementIntent`] to the correct backend.
fn route_intent(intent: abi::MovementIntent) {
    match message_bus::get_routing_mode() {
        RoutingMode::SingleBoard => {
            // Forward directly to Core 1's motor-command bridge.
            // Core 1 blends this with its PID balance correction each tick.
            motors::store_motor_command(intent.left_speed, intent.right_speed);
        }
        RoutingMode::Distributed => {
            // Serialise the intent as a SET_MOTOR_SPEED ESP-NOW packet and
            // transmit it to the remote Worker board.
            //
            // Payload layout (4 bytes, little-endian i16 each):
            //   [left_lo, left_hi, right_lo, right_hi]
            let left_bytes  = intent.left_speed.to_le_bytes();
            let right_bytes = intent.right_speed.to_le_bytes();
            let _payload = [
                abi::EspNowCommand::MAGIC[0],
                abi::EspNowCommand::MAGIC[1],
                abi::cmd::SET_MOTOR_SPEED,
                4u8, // payload_len
                left_bytes[0],
                left_bytes[1],
                right_bytes[0],
                right_bytes[1],
            ];
            // TODO: push `_payload` to the ESP-NOW TX queue once that
            // channel exists.  For now the serialisation logic is in place
            // and the motors on this board are left untouched (they are
            // driven by the remote Worker).
        }
    }
}

// ── Fallback inference ────────────────────────────────────────────────────────

/// Drain the audio snapshot channel and run a local inference pass.
///
/// Called by the Router whenever the dead-man's switch fires *and* the radio
/// link is confirmed silent.  If no snapshot is available (the audio pipeline
/// has not produced data yet) the function returns immediately without
/// modifying [`inference::INFERENCE_RESULT`].
fn run_fallback_inference() {
    // Drain up to one snapshot per Router tick to keep latency bounded.
    if let Ok(snapshot) = AUDIO_INFERENCE_CHANNEL.try_receive() {
        let result = inference::ENGINE.run(&snapshot);
        inference::store_inference_result(result);
    }
}
