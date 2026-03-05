//! Telemetry TX channel — Core 1 → Core 0 → Radio.
//!
//! Core 1 pushes structured [`TelemetryPacket`]s here after each real-time
//! loop tick (decimated to 100 pps).  The ESP-NOW TX task on Core 0 drains
//! the channel asynchronously and serialises the packets for radio transmission,
//! ensuring Core 0's Wasm engine loop never blocks waiting for the antenna.
//!
//! ## Back-pressure
//!
//! [`push_telemetry`] uses a non-blocking `try_send`, so Core 1 simply drops
//! the oldest packet if the queue is momentarily full.  The capacity of
//! [`TELEMETRY_TX_CAPACITY`] (32 slots × 10 ms per packet = 320 ms headroom)
//! is more than sufficient to absorb transient radio congestion.

use abi::{TelemetryPacket, TELEMETRY_TX_CAPACITY};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

// ── TX channel ────────────────────────────────────────────────────────────────

/// Telemetry transmit channel: Core 1 → Core 0 → ESP-NOW radio.
///
/// Core 1 pushes [`TelemetryPacket`]s here after each real-time loop tick.
/// The [`crate::core0::espnow`] TX task drains the channel asynchronously
/// and serialises packets for radio transmission.
pub static TELEMETRY_TX_CHANNEL: Channel<
    CriticalSectionRawMutex,
    TelemetryPacket,
    TELEMETRY_TX_CAPACITY,
> = Channel::new();

// ── Publisher (Core 1) ────────────────────────────────────────────────────────

/// Push a telemetry packet to the TX queue (best-effort, non-blocking).
///
/// Returns `true` if the packet was enqueued, `false` if the queue was full
/// (the packet is silently dropped rather than blocking the real-time loop).
#[inline]
pub fn push_telemetry(packet: TelemetryPacket) -> bool {
    TELEMETRY_TX_CHANNEL.try_send(packet).is_ok()
}
