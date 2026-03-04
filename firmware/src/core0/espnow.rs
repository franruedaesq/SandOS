//! ESP-NOW receiver/transmitter task for Core 0.
//!
//! ## Phase 1–4 responsibilities
//! - Listens for raw ESP-NOW packets from the PC.
//! - Validates the SandOS magic header.
//! - Forwards well-formed commands to the Wasm engine via `CMD_CHANNEL`.
//!
//! ## Phase 5 additions
//! - Forwards `(left, right)` motor speed pairs from the `MOTOR_OUT_CHANNEL`
//!   to the Worker chip as [`abi::WorkerPacket::motor_speed`] frames.
//! - Sends a [`abi::WorkerPacket::heartbeat`] to the Worker every
//!   [`abi::HEARTBEAT_INTERVAL_MS`] milliseconds.  The Worker's dead-man's
//!   switch halts all motors if it misses heartbeats for more than
//!   [`abi::WORKER_TIMEOUT_MS`] milliseconds.
use abi::{EspNowCommand, WorkerPacket, ESPNOW_MAX_PAYLOAD, HEARTBEAT_INTERVAL_MS};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::{Receiver, Sender}};
use embassy_time::{Duration, Instant};
use esp_hal::peripherals::WIFI;
use esp_wifi::esp_now::{EspNow, BROADCAST_ADDRESS};

use crate::core0::WasmCommand;

// ── Broadcast address used to announce the device ────────────────────────────

/// Interval between keep-alive beacons sent to the PC (milliseconds).
const BEACON_INTERVAL_MS: u64 = 1_000;

// ── Worker address ────────────────────────────────────────────────────────────

/// MAC address of the Worker ESP32-S3.
///
/// **Replace this with the actual Worker MAC address before flashing.**
/// You can read the Worker's MAC via `esp_wifi::get_base_mac_addr()` and
/// print it over serial during first boot.
const WORKER_ADDRESS: [u8; 6] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];

// ── Task ──────────────────────────────────────────────────────────────────────

/// ESP-NOW receive/transmit task for the Brain.
///
/// Responsibilities (in priority order each loop iteration):
/// 1. **Drain outgoing motor commands** — any `(left, right)` pair in the
///    `motor_rx` channel is immediately encoded as a `WorkerPacket::motor_speed`
///    frame and sent to the Worker via ESP-NOW.
/// 2. **Heartbeat** — every [`HEARTBEAT_INTERVAL_MS`] ms a keep-alive packet
///    is sent to the Worker so its dead-man's switch does not trigger.
/// 3. **Receive incoming** — wait up to 1 ms for an incoming PC command, then
///    validate and enqueue to the Wasm engine.
/// 4. **Beacon** — every [`BEACON_INTERVAL_MS`] ms a presence beacon is
///    broadcast so the PC knows the Brain is alive.
#[embassy_executor::task]
pub async fn espnow_rx_task(
    wifi: WIFI,
    sender: Sender<'static, CriticalSectionRawMutex, WasmCommand, 8>,
    motor_rx: Receiver<'static, CriticalSectionRawMutex, (i16, i16), 4>,
) {
    // Initialise the Wi-Fi radio in Station mode for ESP-NOW.
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

    let mut esp_now = EspNow::new(&init, wifi).unwrap();

    // Send an initial beacon so the PC sees us immediately.
    send_beacon(&mut esp_now).await;

    let mut beacon_deadline =
        Instant::now() + Duration::from_millis(BEACON_INTERVAL_MS);
    let mut heartbeat_deadline =
        Instant::now() + Duration::from_millis(HEARTBEAT_INTERVAL_MS);

    loop {
        // ── 1. Drain outgoing motor commands (non-blocking) ───────────────────
        while let Ok((left, right)) = motor_rx.try_recv() {
            let pkt = WorkerPacket::motor_speed(left, right);
            esp_now.send_async(&WORKER_ADDRESS, &pkt).await.ok();
        }

        // ── 2. Heartbeat ──────────────────────────────────────────────────────
        let now = Instant::now();
        if now >= heartbeat_deadline {
            let hb = WorkerPacket::heartbeat();
            esp_now.send_async(&WORKER_ADDRESS, &hb).await.ok();
            // Advance by a fixed interval to avoid cumulative drift.
            heartbeat_deadline += Duration::from_millis(HEARTBEAT_INTERVAL_MS);
        }

        // ── 3. Receive incoming (short timeout so we stay responsive) ─────────
        // Use the minimum of 1 ms and the time until the next deadline so the
        // loop wakes up frequently to drain outgoing commands.
        let now = Instant::now();
        let next_deadline = heartbeat_deadline.min(beacon_deadline);
        let max_wait = next_deadline.saturating_duration_since(now);
        let poll_timeout = max_wait.min(Duration::from_millis(1));

        match embassy_time::with_timeout(poll_timeout, esp_now.receive_async()).await {
            Ok(Ok(frame)) => {
                handle_frame(frame.data(), &sender);
            }
            Ok(Err(_)) => {
                // Radio error — keep running.
            }
            Err(_timeout) => {
                // Timeout elapsed — loop again to check deadlines.
            }
        }

        // ── 4. Beacon ─────────────────────────────────────────────────────────
        let now = Instant::now();
        if now >= beacon_deadline {
            send_beacon(&mut esp_now).await;
            // Advance by a fixed interval to avoid cumulative drift.
            beacon_deadline += Duration::from_millis(BEACON_INTERVAL_MS);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse and validate a raw ESP-NOW payload, then push to the command queue.
fn handle_frame(
    data: &[u8],
    sender: &Sender<'static, CriticalSectionRawMutex, WasmCommand, 8>,
) {
    // Need at least the 4-byte header.
    if data.len() < 4 {
        return;
    }

    // Validate magic header.
    if data[0] != EspNowCommand::MAGIC[0] || data[1] != EspNowCommand::MAGIC[1] {
        return;
    }

    let cmd_id = data[2];
    let payload_len = data[3] as usize;

    // Sanity-check the declared length against actual packet size.
    let max_payload = ESPNOW_MAX_PAYLOAD - 4;
    if payload_len > max_payload || 4 + payload_len > data.len() {
        return;
    }

    let payload_slice = &data[4..4 + payload_len];
    let mut payload: heapless::Vec<u8, 64> = heapless::Vec::new();
    // Truncate silently if the payload exceeds 64 bytes (shouldn't happen for
    // well-formed Phase 1/2 commands, but protects the heapless Vec).
    let copy_len = payload_slice.len().min(64);
    payload.extend_from_slice(&payload_slice[..copy_len]).ok();

    // Best-effort enqueue — drop if queue is full (back-pressure).
    sender.try_send(WasmCommand { cmd_id, payload }).ok();
}

/// Broadcast a minimal presence beacon over ESP-NOW.
async fn send_beacon(esp_now: &mut EspNow<'_>) {
    // Beacon payload: magic + cmd_id=0x00 (heartbeat) + len=0
    let beacon = [EspNowCommand::MAGIC[0], EspNowCommand::MAGIC[1], 0x00, 0x00];
    esp_now.send_async(&BROADCAST_ADDRESS, &beacon).await.ok();
}
