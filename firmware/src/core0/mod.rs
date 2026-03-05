//! Core 0 — The Brain.
//!
//! Orchestrates the Wasm VM, the ESP-NOW radio, and the Host-Guest ABI.
//!
//! ## Task structure on Core 0
//!
//! ```text
//! brain_task
//!   ├─ espnow_rx_task  (receives commands from PC)
//!   ├─ wasm_run_task   (runs the Wasm app; calls ABI on behalf of guest)
//!   └─ router_task     (reads MovementIntents from the OS Message Bus;
//!                        routes to Core 1 or ESP-NOW based on RoutingMode)
//! ```
//!
//! The Wasm VM and the Router communicate through a lock-free Embassy channel
//! (the OS Message Bus) so the real-time routing loop never blocks the engine.
pub mod abi;
pub mod espnow;
pub mod wasm_vm;

use abi::AbiHost;
use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::{gpio::Io, peripherals::WIFI};

use crate::display::DisplayDriver;
use crate::router;

// ── Inter-task channel ────────────────────────────────────────────────────────

/// A command queued from the ESP-NOW receiver for the Wasm task to process.
#[derive(Clone)]
pub struct WasmCommand {
    /// The raw command ID received in the ESP-NOW packet.
    pub cmd_id: u8,
    /// Up to 64 bytes of inline payload.
    pub payload: heapless::Vec<u8, 64>,
}

/// Capacity of the command queue between the radio and the Wasm engine.
const CMD_QUEUE_DEPTH: usize = 8;

/// Shared channel: ESP-NOW receiver → Wasm engine.
static CMD_CHANNEL: Channel<CriticalSectionRawMutex, WasmCommand, CMD_QUEUE_DEPTH> =
    Channel::new();

// ── Main Brain task ───────────────────────────────────────────────────────────

/// Core 0 top-level task.
///
/// Spawns the ESP-NOW receiver, the Wasm engine, and the OS Router tasks,
/// then returns (the spawned tasks keep Core 0 occupied).
#[embassy_executor::task]
pub async fn brain_task(spawner: Spawner, wifi: WIFI, io: Io) {
    // Initialise the display (Phase 2).
    let display = DisplayDriver::new(&io);

    // Build the ABI host context (LED pin, display handle, …).
    let abi_host = AbiHost::new(io, display);

    // Start the ESP-NOW receiver task.
    spawner
        .spawn(espnow::espnow_rx_task(wifi, CMD_CHANNEL.sender()))
        .unwrap();

    // Start the Wasm engine task.
    spawner
        .spawn(wasm_vm::wasm_run_task(CMD_CHANNEL.receiver(), abi_host))
        .unwrap();

    // Start the OS Router task (Phase 5).
    // The Router drains MovementIntents from the OS Message Bus and
    // dispatches them to Core 1 (Single-Board) or ESP-NOW (Distributed).
    spawner
        .spawn(router::router_task())
        .unwrap();
}
