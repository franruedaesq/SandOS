//! ESP-NOW receiver task for Core 0.
//!
//! Listens for raw ESP-NOW packets from the PC, validates the SandOS
//! magic header, and forwards well-formed commands to the Wasm engine
//! via the shared `CMD_CHANNEL`.
use abi::{EspNowCommand, ESPNOW_MAX_PAYLOAD};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use esp_hal::peripherals::WIFI;
use esp_wifi::esp_now::{EspNow, EspNowReceiver, BROADCAST_ADDRESS};

use crate::core0::WasmCommand;

// ── Broadcast address used to announce the device ────────────────────────────

/// Interval between keep-alive beacons (milliseconds).
const BEACON_INTERVAL_MS: u64 = 1_000;

// ── Task ──────────────────────────────────────────────────────────────────────

/// Receive ESP-NOW packets, validate them, and enqueue Wasm commands.
///
/// Also periodically broadcasts a presence beacon so the PC knows the device
/// is alive and reachable.
#[embassy_executor::task]
pub async fn espnow_rx_task(
    wifi: WIFI,
    sender: Sender<'static, CriticalSectionRawMutex, WasmCommand, 8>,
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

    let mut beacon_deadline = embassy_time::Instant::now()
        + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);

    loop {
        // Use a timeout so we can still send beacons even when idle.
        match embassy_time::with_timeout(
            beacon_deadline.saturating_duration_since(embassy_time::Instant::now()),
            esp_now.receive_async(),
        )
        .await
        {
            Ok(Ok(frame)) => {
                handle_frame(frame.data(), &sender);
            }
            Ok(Err(_)) => {
                // Radio error — keep running.
            }
            Err(_timeout) => {
                // Beacon interval elapsed.
                send_beacon(&mut esp_now).await;
                beacon_deadline = embassy_time::Instant::now()
                    + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);
            }
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
