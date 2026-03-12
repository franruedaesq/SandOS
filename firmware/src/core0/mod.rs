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
#[cfg(feature = "espnow")]
pub mod espnow;

use embassy_executor::Spawner;
use esp_hal::gpio::Io;

// ── Main Brain task ───────────────────────────────────────────────────────────

/// Core 0 top-level task.
#[embassy_executor::task]
pub async fn brain_task(
    _spawner: Spawner,
    _io: Io,
    _wifi_init: &'static esp_wifi::EspWifiController<'static>,
) {
    log::info!("[brain] task starting");

    #[cfg(feature = "espnow")]
    {
        // Safe to transmute since WiFi is initialized.
        let token: esp_wifi::esp_now::EspNowWithWifiCreateToken = unsafe { core::mem::transmute(()) };
        spawner.spawn(espnow::espnow_rx_task(wifi_init, token)).unwrap();
    }
}
