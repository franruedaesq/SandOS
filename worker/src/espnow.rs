//! Worker ESP-NOW receive task.
//!
//! Listens for packets from the Brain and dispatches them:
//!
//! - [`worker_cmd::MOTOR_SPEED`] — decode left/right speeds and store to the
//!   shared `MOTOR_COMMAND` atomic.
//! - [`worker_cmd::HEARTBEAT`] — reset the dead-man's switch timer.
//! - [`worker_cmd::EMERGENCY_STOP`] — immediately zero motors and disable.
//! - **Timeout** ([`WORKER_TIMEOUT_MS`]) — if no packet arrives, trigger the
//!   dead-man's switch: disable `MOTOR_ENABLED` and zero `MOTOR_COMMAND`.
use abi::{WorkerPacket, WORKER_TIMEOUT_MS, worker_cmd};
use embassy_time::{Duration, Instant};
use esp_hal::peripherals::WIFI;

use crate::motors;

// ── Task ──────────────────────────────────────────────────────────────────────

/// Worker ESP-NOW receive task.
///
/// Runs the receive loop with a [`WORKER_TIMEOUT_MS`]-millisecond deadline.
/// Any valid SandOS packet resets the deadline; a timeout fires the
/// dead-man's switch.
#[embassy_executor::task]
pub async fn worker_espnow_task(wifi: WIFI) {
    let init = esp_wifi::initialize(
        esp_wifi::EspWifiInitFor::Wifi,
        esp_hal::timer::systimer::SystemTimer::new(unsafe {
            esp_hal::peripherals::SYSTIMER::steal()
        })
        .alarm0,
        esp_hal::rng::Rng::new(unsafe { esp_hal::peripherals::RNG::steal() }),
        unsafe { esp_hal::peripherals::RADIO_CLK::steal() },
        &esp_hal::clock::Clocks::get(),
    )
    .unwrap();

    let mut esp_now = esp_wifi::esp_now::EspNow::new(&init, wifi).unwrap();

    let timeout = Duration::from_millis(WORKER_TIMEOUT_MS);
    let mut deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());

        match embassy_time::with_timeout(remaining, esp_now.receive_async()).await {
            Ok(Ok(frame)) => {
                // Valid packet received — reset the dead-man's switch deadline.
                deadline = Instant::now() + timeout;
                // Re-enable motors (they may have been halted by a previous timeout).
                motors::set_motor_enabled(true);
                handle_packet(frame.data());
            }
            Ok(Err(_)) => {
                // Radio error — keep running; deadline unchanged.
            }
            Err(_timeout) => {
                // ── Dead-man's switch ─────────────────────────────────────
                // No packet from Brain for WORKER_TIMEOUT_MS ms.
                // Zero the motor command and disable motors immediately.
                motors::set_motor_enabled(false);
                // Reset deadline so the switch can re-arm when the Brain returns.
                deadline = Instant::now() + timeout;
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Decode and dispatch a raw ESP-NOW frame from the Brain.
fn handle_packet(data: &[u8]) {
    let Some((cmd_id, payload)) = WorkerPacket::decode(data) else {
        return; // Invalid magic or truncated — ignore.
    };

    match cmd_id {
        worker_cmd::MOTOR_SPEED => {
            if let Some((left, right)) = WorkerPacket::parse_motor_speed(payload) {
                motors::store_motor_command(left, right);
            }
        }
        worker_cmd::HEARTBEAT => {
            // Nothing to do beyond resetting the deadline (already done above).
        }
        worker_cmd::EMERGENCY_STOP => {
            motors::set_motor_enabled(false);
        }
        _ => {
            // Unknown command — ignore.
        }
    }
}
