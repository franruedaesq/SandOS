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

use abi::{RoutingMode, DEAD_MANS_SWITCH_MS};
use embassy_time::Duration;

use crate::{message_bus, motors};

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
