//! Core 0 — The Brain.
//!
//! Orchestrates the Wasm VM, the ESP-NOW radio, and the Host-Guest ABI.
//!
//! ## Task structure on Core 0
//!
//! ```text
//! brain_task
//!   ├─ espnow_rx_task  (receives commands from PC; handles OTA packets)
//!   ├─ wasm_run_task   (runs the Wasm app; calls ABI on behalf of guest;
//!   │                   performs hot-swap when OTA_READY signal fires)
//!   └─ router_task     (reads MovementIntents from the OS Message Bus;
//!                        routes to Core 1 or ESP-NOW based on RoutingMode)
//! ```
//!
//! The Wasm VM and the Router communicate through a lock-free Embassy channel
//! (the OS Message Bus) so the real-time routing loop never blocks the engine.
pub mod abi;
#[cfg(feature = "espnow")]
pub mod espnow;
pub mod ota;
pub mod wasm_vm;

#[allow(dead_code)]
use abi::AbiHost;
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
    signal::Signal,
};
use esp_hal::gpio::Io;
use portable_atomic::AtomicU32;
use core::sync::atomic::Ordering;

use crate::rgb_led::RgbLedDriver;

use crate::display::DisplayDriver;
use crate::router;

// ── Hot-swap counter (read by the web server for the dashboard) ───────────────

/// Total number of successful Wasm hot-swaps completed since boot.
///
/// The Wasm VM task increments this after every successful OTA hot-swap.
/// The web server task reads it to populate the `/api/stats` JSON.
pub static HOT_SWAP_COUNT: AtomicU32 = AtomicU32::new(0);

/// Increment the hot-swap counter (called from `wasm_vm` after each swap).
#[inline]
pub fn increment_hot_swap_count() {
    HOT_SWAP_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ── Inter-task channel ────────────────────────────────────────────────────────

/// A command queued from the ESP-NOW receiver for the Wasm task to process.
#[derive(Clone)]
pub struct WasmCommand {
    /// The raw command ID received in the ESP-NOW packet.
    pub cmd_id: u8,
    /// Up to 64 bytes of inline payload.
    #[allow(dead_code)]
    pub payload: heapless::Vec<u8, 64>,
}

/// Capacity of the command queue between the radio and the Wasm engine.
const CMD_QUEUE_DEPTH: usize = 8;

/// Shared channel: ESP-NOW receiver → Wasm engine.
static CMD_CHANNEL: Channel<CriticalSectionRawMutex, WasmCommand, CMD_QUEUE_DEPTH> =
    Channel::new();

// ── Phase 8 — OTA hot-swap signal ────────────────────────────────────────────

/// Signal sent from the ESP-NOW OTA handler to the Wasm engine task when a
/// verified binary is ready for hot-swapping.
///
/// The signal carries the exact byte count of the new binary so the Wasm task
/// can allocate the right amount of memory from PSRAM before reading the
/// staging buffer.
pub static OTA_SWAP_SIGNAL: Signal<CriticalSectionRawMutex, u32> = Signal::new();

// ── Main Brain task ───────────────────────────────────────────────────────────

/// Core 0 top-level task.
///
/// Spawns the ESP-NOW receiver, the Wasm engine, and the OS Router tasks,
/// then returns (the spawned tasks keep Core 0 occupied).
#[embassy_executor::task]
pub async fn brain_task(
    spawner: Spawner,
    io: Io,
    _wifi_init: &'static esp_wifi::EspWifiController<'static>,
    #[cfg(feature = "espnow")] esp_now_token: esp_wifi::esp_now::EspNowWithWifiCreateToken,
) {
    log::info!("[brain] task starting");

    // Display task is spawned directly from main() before network tasks
    // so it gets exclusive CPU time for init + splash.  Only the ABI
    // channel handle lives here (cross-task via CriticalSectionRawMutex).
    let display = DisplayDriver::new();

    // Initialise the RGB LED (Phase 9).
    let rgb_led = RgbLedDriver::new();

    // Build the ABI host context (LED pin, display handle, RGB LED, …).
    let abi_host = AbiHost::new(io, display, rgb_led);

    // Start the ESP-NOW task if enabled.
    #[cfg(feature = "espnow")]
    {
        log::info!("[brain] spawning espnow_rx_task");
        spawner
            .spawn(espnow::espnow_rx_task(
                _wifi_init,
                esp_now_token,
                CMD_CHANNEL.sender(),
            ))
            .unwrap();
    }

    // Start the Wasm engine task (also handles OTA hot-swap signals).
    log::info!("[brain] spawning wasm_run_task");
    spawner
        .spawn(wasm_vm::wasm_run_task(CMD_CHANNEL.receiver(), abi_host))
        .unwrap();

    // Start the OS Router task (Phase 5).
    log::info!("[brain] spawning router_task");
    spawner
        .spawn(router::router_task())
        .unwrap();

    log::info!("[brain] all sub-tasks spawned");
}
