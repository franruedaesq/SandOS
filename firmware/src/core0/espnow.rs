//! ESP-NOW receiver + telemetry transmitter task for Core 0.
//!
//! Listens for raw ESP-NOW packets from the PC, validates the SandOS
//! magic header, and forwards well-formed commands to the Wasm engine
//! via the shared `CMD_CHANNEL`.
//!
//! ## Phase 6 — Async Telemetry TX
//!
//! [`espnow_rx_task`] also drains the [`crate::telemetry::TELEMETRY_TX_CHANNEL`]
//! opportunistically between receive polls.  Structured telemetry packets
//! (built by Core 1) are serialised into ESP-NOW frames and broadcast to any
//! listening PC, enabling high-frequency (100 pps) data streams without
//! blocking Core 0's Wasm engine loop.
//!
//! ## Phase 7 — Radio Link Monitoring
//!
//! [`espnow_rx_task`] records the timestamp of every successfully received
//! command packet in [`RADIO_LAST_RX_MS`].  The fallback router queries
//! [`is_radio_link_alive`] to decide whether to activate the local
//! inference pipeline.
//!
//! ## Phase 8 — OTA Receiver
//!
//! OTA command IDs (`OTA_BEGIN`, `OTA_CHUNK`, `OTA_FINALIZE`) are intercepted
//! here *before* being forwarded to the Wasm command queue.  An [`OtaReceiver`]
//! state machine accumulates chunked binary data into PSRAM.  On successful
//! CRC-32 verification [`super::OTA_SWAP_SIGNAL`] is fired, which wakes the
//! [`super::wasm_vm::wasm_run_task`] to perform the live hot-swap.
use core::sync::atomic::Ordering;
use portable_atomic::AtomicU64;

use abi::{cmd, EspNowCommand, TelemetryPacket, ESPNOW_MAX_PAYLOAD, RADIO_SILENCE_THRESHOLD_MS};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use esp_hal::peripherals::WIFI;
use esp_wifi::esp_now::{EspNow, EspNowReceiver, BROADCAST_ADDRESS};

use crate::core0::ota::OtaReceiver;
use crate::core0::WasmCommand;
use crate::telemetry;

// ── Radio link state ──────────────────────────────────────────────────────────

/// Millisecond timestamp of the most recently received valid command packet.
///
/// Written by [`espnow_rx_task`] every time a well-formed SandOS frame arrives.
/// Read by [`is_radio_link_alive`] to decide whether the fallback inference
/// pipeline should be activated.
///
/// Initialised to `0` so the link is considered silent until the first packet
/// arrives.
pub static RADIO_LAST_RX_MS: AtomicU64 = AtomicU64::new(0);

/// Return `true` when a valid command was received within the radio-silence
/// threshold window.
///
/// `now_ms` should be the current uptime in milliseconds (e.g. from
/// `embassy_time::Instant::now().as_millis()`).
#[inline]
pub fn is_radio_link_alive(now_ms: u64) -> bool {
    let last = RADIO_LAST_RX_MS.load(Ordering::Acquire);
    now_ms.saturating_sub(last) < RADIO_SILENCE_THRESHOLD_MS
}

// ── Broadcast address used to announce the device ────────────────────────────

/// Interval between keep-alive beacons (milliseconds).
const BEACON_INTERVAL_MS: u64 = 1_000;

/// Maximum number of queued telemetry packets to drain per loop iteration.
///
/// Limiting the drain count prevents the telemetry path from starving the
/// ESP-NOW RX path.  At 100 pps and a 1 ms beacon interval, draining 8
/// packets per iteration provides ample throughput while still servicing
/// incoming commands promptly.
const MAX_TELEMETRY_DRAIN_PER_ITER: usize = 8;

// ── Task ──────────────────────────────────────────────────────────────────────

/// Receive ESP-NOW packets, validate them, enqueue Wasm commands, and
/// transmit outgoing telemetry packets from the telemetry TX channel.
///
/// Also periodically broadcasts a presence beacon so the PC knows the device
/// is alive and reachable.
///
/// ## Phase 8 — OTA routing
///
/// OTA command packets (`OTA_BEGIN`, `OTA_CHUNK`, `OTA_FINALIZE`) are handled
/// here by the embedded [`OtaReceiver`] before any Wasm command is dispatched.
/// When `OTA_FINALIZE` succeeds, [`super::OTA_SWAP_SIGNAL`] is signalled with
/// the binary length so the Wasm task can initiate the hot-swap.
#[embassy_executor::task]
pub async fn espnow_rx_task(
    wifi: WIFI,
    sender: Sender<'static, CriticalSectionRawMutex, WasmCommand, 8>,
) {
    // Phase 8: OTA receiver owns the PSRAM staging buffer for this task.
    let mut ota = OtaReceiver::new();

    // Initialise the Wi-Fi radio in Station mode for ESP-NOW.
    let init = esp_wifi::init(
        esp_hal::timer::systimer::SystemTimer::new(unsafe {
            esp_hal::peripherals::SYSTIMER::steal()
        })
        .alarm0,
        esp_hal::rng::Rng::new(unsafe { esp_hal::peripherals::RNG::steal() }),
        unsafe { esp_hal::peripherals::RADIO_CLK::steal() },
    )
    .unwrap();

    let mut esp_now = EspNow::new(&init, wifi).unwrap();

    // Send an initial beacon so the PC sees us immediately.
    send_beacon(&mut esp_now).await;

    let mut beacon_deadline = embassy_time::Instant::now()
        + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);

    loop {
        // ── Phase 6: drain outgoing telemetry (non-blocking) ─────────────────
        for _ in 0..MAX_TELEMETRY_DRAIN_PER_ITER {
            match telemetry::TELEMETRY_TX_CHANNEL.try_receive() {
                Ok(packet) => {
                    send_telemetry_packet(&mut esp_now, &packet).await;
                }
                Err(_) => break,
            }
        }

        // ── RX: wait for incoming packet or beacon deadline ───────────────────
        match embassy_time::with_timeout(
            beacon_deadline.saturating_duration_since(embassy_time::Instant::now()),
            esp_now.receive_async(),
        )
        .await
        {
            Ok(frame) => {
                let now_ms = embassy_time::Instant::now().as_millis();
                handle_frame(frame.data(), &sender, &mut ota, now_ms);
            }
            Err(_timeout) => {
                send_beacon(&mut esp_now).await;
                beacon_deadline = embassy_time::Instant::now()
                    + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse and validate a raw ESP-NOW payload, then push to the command queue.
///
/// OTA commands (`OTA_BEGIN`, `OTA_CHUNK`, `OTA_FINALIZE`) are intercepted and
/// routed to `ota` rather than the Wasm command queue.  When `OTA_FINALIZE`
/// succeeds, [`super::OTA_SWAP_SIGNAL`] is fired to wake the Wasm task.
///
/// `now_ms` is the current uptime used to update [`RADIO_LAST_RX_MS`] when a
/// valid command is received.
fn handle_frame(
    data: &[u8],
    sender: &Sender<'static, CriticalSectionRawMutex, WasmCommand, 8>,
    ota: &mut OtaReceiver,
    now_ms: u64,
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

    // Phase 7: update radio-link timestamp on every valid command.
    RADIO_LAST_RX_MS.store(now_ms, Ordering::Release);

    // Phase 8: intercept OTA commands before they reach the Wasm queue.
    if ota.handle_command(cmd_id, payload_slice) {
        // If OTA_FINALIZE succeeded, signal the Wasm task to hot-swap.
        if cmd_id == cmd::OTA_FINALIZE {
            if let Some(binary) = ota.ready_binary() {
                super::OTA_SWAP_SIGNAL.signal(binary.len() as u32);
            }
        }
        return; // OTA commands are not forwarded to the Wasm engine.
    }

    let mut payload: heapless::Vec<u8, 64> = heapless::Vec::new();
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

/// Serialize a [`TelemetryPacket`] into a SandOS ESP-NOW frame and broadcast it.
///
/// Frame layout:
/// ```text
/// [magic[0]] [magic[1]] [cmd_id] [payload_len] [CDR payload bytes …]
/// ```
/// The `cmd_id` is taken from the packet's type discriminant so the receiver
/// can identify the packet type from the header alone.
async fn send_telemetry_packet(esp_now: &mut EspNow<'_>, packet: &TelemetryPacket) {
    // Allocate a frame on the stack — no heap allocation required.
    let mut frame = [0u8; ESPNOW_MAX_PAYLOAD];

    // Write SandOS header.
    frame[0] = EspNowCommand::MAGIC[0];
    frame[1] = EspNowCommand::MAGIC[1];

    // Serialize the CDR payload starting at byte 4 (after the 4-byte header).
    let payload_len = packet.serialize(&mut frame[4..]);
    if payload_len == 0 {
        return; // serialization failed (shouldn't happen)
    }

    // The cmd_id is the discriminant byte that was written as frame[4].
    frame[2] = frame[4];
    // payload_len in the header field = total bytes after the 4-byte header.
    frame[3] = payload_len as u8;

    // Shift the CDR payload left by 1 to remove the duplicate discriminant byte
    // that was placed at frame[4] by serialize().  After the shift, frame[4..]
    // contains the raw CDR bytes without the leading discriminant.
    let body_len = payload_len - 1;
    frame.copy_within(5..5 + body_len, 4);

    let total_len = 4 + body_len;
    esp_now.send_async(&BROADCAST_ADDRESS, &frame[..total_len]).await.ok();
}
