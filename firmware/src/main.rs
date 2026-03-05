//! SandOS — Entry point for the ESP32-S3 firmware.
//!
//! ## Dual-core boot sequence
//!
//! 1. `main()` runs on **Core 0** (the Brain).
//! 2. The heap is initialised first so the Wasm engine can allocate.
//! 3. Core 1 (the Muscle) is started with its own Embassy executor before
//!    any Wasm code is loaded — guaranteeing the real-time loop is never
//!    blocked by the VM.
//! 4. The ULP paramedic program is uploaded and started.
//! 5. Core 0 enters the async Embassy executor and runs [`core0::brain_task`].
#![no_std]
#![no_main]

extern crate alloc;

use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use esp_hal::{
    cpu_control::{CpuControl, Stack},
    gpio::Io,
    timer::timg::TimerGroup,
};
use static_cell::StaticCell;

mod core0;
mod core1;
mod display;
mod inference;
mod message_bus;
mod motors;
mod router;
mod sensors;
mod telemetry;
mod ulp;

// ── Panic handler ─────────────────────────────────────────────────────────────

use core::panic::PanicInfo;

/// Panic handler for no_std environment.
///
/// In a real deployment you might want to log to flash or transmit via ESP-NOW,
/// but for now we just loop infinitely.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

// ── Global heap allocator (PSRAM region for the Wasm engine) ─────────────────
//
// esp-alloc 0.6 declares the #[global_allocator] internally; we just need to
// add memory regions to it via the psram_allocator! macro at runtime.

/// Initialise the heap allocator in external PSRAM.
///
/// The Wasm engine (engine + store + module) is large and latency-tolerant,
/// so it belongs in the slower external PSRAM rather than the scarce internal
/// SRAM.  Internal SRAM is reserved for Core 1's stack, Embassy task arenas,
/// and the IMU atomic variable so those paths stay deterministic.
///
/// The `psram_allocator!` macro locates the PSRAM region at the address
/// provided by `esp_hal::psram` and registers it with the allocator.
fn init_heap(psram: esp_hal::peripherals::PSRAM) {
    esp_alloc::psram_allocator!(psram, esp_hal::psram);
}

// ── Core 1 stack ──────────────────────────────────────────────────────────────

/// Dedicated stack for Core 1 (64 KiB — enough for Embassy + motor loops).
static mut APP_CORE_STACK: Stack<65536> = Stack::new();

// ── Core 1 executor ───────────────────────────────────────────────────────────

static CORE1_EXECUTOR: StaticCell<esp_hal_embassy::Executor> = StaticCell::new();

/// Entry point for Core 1 (The Muscle).
///
/// Runs an Embassy executor that only handles hard real-time tasks.
/// This function never returns in practice.
fn core1_entry() {
    let executor = CORE1_EXECUTOR.init(esp_hal_embassy::Executor::new());
    executor.run(|spawner| {
        spawner.spawn(core1::realtime_task()).unwrap();
    });
}

// ── Main (Core 0) ─────────────────────────────────────────────────────────────

/// Embassy entry point — runs on **Core 0**.
#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // 1. Initialise the HAL and obtain peripherals.
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // 2. Allocate heap on PSRAM before any `alloc` call.
    init_heap(peripherals.PSRAM);

    // 3. Initialise Embassy's time driver.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    // 4. Set up GPIO.
    let io = Io::new(peripherals.IO_MUX);

    // 5. Start Core 1 (The Muscle) before doing anything else on Core 0.
    //    This guarantees the real-time loop is live before the Wasm VM starts.
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, core1_entry)
        .unwrap();

    // 6. Upload the ULP paramedic program and start it.
    ulp::start(peripherals.LPWR);

    // 7. Core 0 starts its own tasks (Wasm VM + ESP-NOW).
    spawner
        .spawn(core0::brain_task(spawner, peripherals.WIFI, io))
        .unwrap();
}
