//! Core 0 — The Brain.
//!
//! Orchestrates the Wasm VM, the ESP-NOW radio, and the Host-Guest ABI.
//!
//! ## Task structure on Core 0
//!
//! ```text
//! brain_task
//!   ├─ espnow_rx_task  (receives commands from PC)
//!   └─ wasm_run_task   (runs the Wasm app; calls ABI on behalf of guest)
//! ```
//!
//! The two tasks communicate through a lock-free Embassy channel so the
//! radio loop never blocks the Wasm engine.
pub mod abi;
pub mod espnow;
pub mod wasm_vm;

use abi::{AbiHost, MotorCmdSender};
use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::{gpio::Io, peripherals::WIFI};

use crate::display::DisplayDriver;

// ── Inter-task channel (PC → Brain) ───────────────────────────────────────────

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

// ── Inter-task channel (Brain ABI → ESP-NOW transmitter) ─────────────────────

/// Capacity of the outgoing motor command queue.
///
/// A depth of 4 absorbs short command bursts while keeping memory use low.
const MOTOR_OUT_DEPTH: usize = 4;

/// Static channel that carries validated `(left, right)` speed pairs from the
/// Wasm ABI layer to the ESP-NOW transmitter task.
///
/// Core 0 (Wasm ABI) writes; the ESP-NOW task reads and forwards to the Worker.
static MOTOR_OUT_CHANNEL: Channel<CriticalSectionRawMutex, (i16, i16), MOTOR_OUT_DEPTH> =
    Channel::new();

// ── Main Brain task ───────────────────────────────────────────────────────────

/// Core 0 top-level task.
///
/// Spawns the ESP-NOW receiver and the Wasm engine tasks, then returns
/// (the spawned tasks keep Core 0 occupied).
#[embassy_executor::task]
pub async fn brain_task(spawner: Spawner, wifi: WIFI, io: Io) {
    // Initialise the display (Phase 2).
    let display = DisplayDriver::new(&io);

    // Build the ABI host context (LED pin, display handle, motor TX channel).
    let motor_tx: MotorCmdSender = MOTOR_OUT_CHANNEL.sender();
    let abi_host = AbiHost::new(io, display, motor_tx);

    // Start the ESP-NOW receiver/transmitter task (Phase 5: also receives the
    // motor command receiver so it can forward commands to the Worker).
    spawner
        .spawn(espnow::espnow_rx_task(
            wifi,
            CMD_CHANNEL.sender(),
            MOTOR_OUT_CHANNEL.receiver(),
        ))
        .unwrap();

    // Start the Wasm engine task.
    spawner
        .spawn(wasm_vm::wasm_run_task(CMD_CHANNEL.receiver(), abi_host))
        .unwrap();
}
