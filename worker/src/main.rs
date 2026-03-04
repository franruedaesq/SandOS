//! SandOS Worker Firmware — Entry Point.
//!
//! The Worker is a stripped-down, purely `no_std` Embassy firmware that runs
//! on a second ESP32-S3.  It has **no Wasm engine, no display, and no UI**.
//!
//! ## Architecture
//!
//! ```text
//! Worker ESP32-S3
//! ├─ espnow_task   ─ Receive Brain packets via ESP-NOW
//! │                  Dead-man's switch: halt motors if no packet for 50 ms
//! └─ motor_task    ─ Apply PWM to motor pins at 500 Hz
//! ```
//!
//! ## Boot sequence
//!
//! 1. Embassy is initialised on Core 0.
//! 2. `espnow_task` is spawned — it listens for packets from the Brain and
//!    updates the shared `MOTOR_COMMAND` atomic.
//! 3. `motor_task` is spawned — it reads `MOTOR_COMMAND` every 2 ms and writes
//!    to the LEDC PWM peripheral.
//!
//! ## Dead-Man's Switch
//!
//! The ESP-NOW task uses a [`abi::WORKER_TIMEOUT_MS`]-millisecond timeout on
//! its receive loop.  If no valid packet arrives within that window the task
//! zeroes the `MOTOR_COMMAND` atomic and disables the `MOTOR_ENABLED` flag.
//! The motor task sees the flag on the next 2 ms tick and cuts all PWM output.
//! Once the Brain reconnects and sends a new packet, `MOTOR_ENABLED` is
//! restored and normal operation resumes.
#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp_hal::{
    clock::ClockControl,
    peripherals::Peripherals,
    prelude::*,
    timer::timg::TimerGroup,
};

mod espnow;
mod motors;

// ── Main (Core 0) ─────────────────────────────────────────────────────────────

/// Embassy entry point — runs on Core 0.
#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();

    // Initialise Embassy's time driver.
    let timg0 = TimerGroup::new(peripherals.TIMG0, &clocks);
    esp_hal_embassy::init(&clocks, timg0.timer0);

    // Spawn the ESP-NOW receiver + dead-man's switch task.
    spawner
        .spawn(espnow::worker_espnow_task(peripherals.WIFI))
        .unwrap();

    // Spawn the motor PWM task.
    spawner.spawn(motors::motor_task()).unwrap();
}
