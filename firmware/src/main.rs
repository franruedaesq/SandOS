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

use embassy_executor::Spawner;
use esp_hal::{
    clock::ClockControl,
    cpu_control::{CpuControl, Stack},
    gpio::Io,
    peripherals::Peripherals,
    prelude::*,
    timer::timg::TimerGroup,
};
use static_cell::StaticCell;

mod core0;
mod core1;
mod display;
mod ulp;

// ── Global heap allocator (PSRAM region for the Wasm engine) ─────────────────

#[global_allocator]
static ALLOCATOR: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();

/// Reserve 512 KiB of PSRAM for the Wasm engine heap.
///
/// The internal SRAM is left untouched so Core 1's real-time stack and
/// Embassy task arenas remain in fast memory.
fn init_heap() {
    use core::mem::MaybeUninit;
    const HEAP_SIZE: usize = 512 * 1024;
    static mut HEAP: MaybeUninit<[u8; HEAP_SIZE]> = MaybeUninit::uninit();
    unsafe {
        ALLOCATOR.init(HEAP.as_mut_ptr() as *mut u8, HEAP_SIZE);
    }
}

// ── Core 1 stack ──────────────────────────────────────────────────────────────

/// Dedicated stack for Core 1 (64 KiB — enough for Embassy + motor loops).
static mut APP_CORE_STACK: Stack<65536> = Stack::new();

// ── Core 1 executor ───────────────────────────────────────────────────────────

static CORE1_EXECUTOR: StaticCell<esp_hal_embassy::Executor> = StaticCell::new();

/// Entry point for Core 1 (The Muscle).
///
/// Runs an Embassy executor that only handles hard real-time tasks.
/// This function never returns.
fn core1_entry() -> ! {
    let executor = CORE1_EXECUTOR.init(esp_hal_embassy::Executor::new());
    executor.run(|spawner| {
        spawner.spawn(core1::realtime_task()).unwrap();
    })
}

// ── Main (Core 0) ─────────────────────────────────────────────────────────────

/// Embassy entry point — runs on **Core 0**.
#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // 1. Allocate heap on PSRAM before any `alloc` call.
    init_heap();

    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();

    // 2. Initialise Embassy's time driver.
    let timg0 = TimerGroup::new(peripherals.TIMG0, &clocks);
    esp_hal_embassy::init(&clocks, timg0.timer0);

    // 3. Set up GPIO.
    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);

    // 4. Start Core 1 (The Muscle) before doing anything else on Core 0.
    //    This guarantees the real-time loop is live before the Wasm VM starts.
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control
        .start_app_core(unsafe { &mut APP_CORE_STACK }, core1_entry)
        .unwrap();

    // 5. Upload the ULP paramedic program and start it.
    ulp::start(peripherals.LPWR);

    // 6. Core 0 starts its own tasks (Wasm VM + ESP-NOW).
    spawner
        .spawn(core0::brain_task(spawner, peripherals.WIFI, io))
        .unwrap();
}
