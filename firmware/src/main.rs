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
//!
//! ## Web UI
//!
//! WiFi is initialised once and shared between:
//! - **ESP-NOW** — the existing command + telemetry radio link.
//! - **WiFi STA** — connects to the local network; serves the HTTP dashboard
//!   on port 80 at the DHCP-assigned IP address.
#![no_std]
#![no_main]

extern crate alloc;
use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use esp_hal::{
    cpu_control::{CpuControl, Stack},
    gpio::Io,
    rmt::{TxChannelConfig, TxChannelCreator},
    time::RateExtU32,
    timer::timg::TimerGroup,
};
use static_cell::StaticCell;

mod core0;
mod core1;
mod display;
mod inference;
mod led_state;
mod message_bus;
mod motors;
mod rgb_led;
mod router;
mod sensors;
mod telemetry;
mod ulp;
mod web_server;
mod wifi;

// ── Panic handler + exception handler ────────────────────────────────────────
use esp_backtrace as _;

// ── Global heap allocator ────────────────────────────────────────────────────
//
// esp-alloc 0.6 owns the #[global_allocator]; we just add regions to it.
// PSRAM (external, large) is registered at runtime via the psram_allocator!
// macro.  A small internal-SRAM region is also added as a fallback so the
// allocator is always usable even on boards without PSRAM.

// ── Core 1 stack ──────────────────────────────────────────────────────────────

/// Dedicated stack for Core 1 (64 KiB — enough for Embassy + motor loops).
static mut APP_CORE_STACK: Stack<32768> = Stack::new();

// ── Core 1 executor ───────────────────────────────────────────────────────────

static CORE1_EXECUTOR: StaticCell<esp_hal_embassy::Executor> = StaticCell::new();

/// Entry point for Core 1 (The Muscle). Never returns.
fn core1_entry() {
    let executor = CORE1_EXECUTOR.init(esp_hal_embassy::Executor::new());
    executor.run(|spawner| {
        spawner.spawn(core1::realtime_task()).unwrap();
    });
}

// ── Shared WiFi radio init ────────────────────────────────────────────────────
//
// ESP-NOW and WiFi STA both use the same physical radio.  They must share one
// `EspWifiController` that is initialised exactly once in `main()` and then
// borrowed as `&'static`.

use esp_wifi::{esp_now::EspNowWithWifiCreateToken, EspWifiController};
static WIFI_INIT: StaticCell<EspWifiController<'static>> = StaticCell::new();

// ── Main (Core 0) ─────────────────────────────────────────────────────────────

/// Embassy entry point — runs on **Core 0**.
#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    esp_println::println!("\n\n=== SandOS Starting ===");

    // ── 1. HAL init ──────────────────────────────────────────────────────────
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // ── 2. Heap init (must come before any `alloc` call) ─────────────────────
    // Add a small internal-SRAM region first so the allocator is always valid.
    // Keep this small — the real heap is PSRAM; internal SRAM is scarce.
    esp_alloc::heap_allocator!(90 * 1024);
    // Add external PSRAM (octal/quad) as a large, lower-priority region.
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // ── 3. Logger over UART ──────────────────────────────────────────────────
    esp_println::logger::init_logger(log::LevelFilter::Info);
    log::info!("SandOS v{} booting on ESP32-S3…", env!("CARGO_PKG_VERSION"));

    // ── 4. Embassy time driver (TIMG0) ───────────────────────────────────────
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    // ── 5. GPIO ──────────────────────────────────────────────────────────────
    let io = Io::new(peripherals.IO_MUX);

    // ── RGB LED init (GPIO 48 via RMT) ────────────────────────────────────────
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, 80_u32.MHz())
        .expect("Failed to initialize RMT");

    let tx_channel_48 = rmt
        .channel1
        .configure(
            peripherals.GPIO48,
            TxChannelConfig {
                clk_divider: 4,
                idle_output_level: false,
                idle_output: true,
                ..Default::default()
            },
        )
        .expect("Failed to configure RMT TX channel 1 (GPIO48)");

    unsafe {
        rgb_led::RGB_LED = Some(rgb_led::RgbLedDriver::new());
        if let Some(led) = &mut rgb_led::RGB_LED {
            led.attach_tx_channel_gpio48(tx_channel_48);
            led.off();
        }
    }
    log::info!("RGB LED initialized on GPIO48 with RMT");

    // ── 6. Core 1 — start before Wasm VM ────────────────────────────────────
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, core1_entry)
        .unwrap();
    log::info!("Core 1 started");

    // ── 7. ULP paramedic ─────────────────────────────────────────────────────
    ulp::start(peripherals.LPWR);

    // ── 8. WiFi radio — init ONCE (shared by ESP-NOW + WiFi STA) ────────────
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    let raw_init = esp_wifi::init(
        timg1.timer0,
        esp_hal::rng::Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    )
    .expect("esp_wifi::init failed");
    let wifi_init: &'static _ = WIFI_INIT.init(raw_init);
    log::info!("WiFi radio initialised");

    // ── 9. WiFi STA interface (shares the radio with ESP-NOW) ────────────────
    use esp_wifi::wifi::WifiStaDevice;
    let (wifi_interface, wifi_controller) =
        esp_wifi::wifi::new_with_mode(wifi_init, peripherals.WIFI, WifiStaDevice)
            .expect("wifi new_with_mode failed");

    // ── 10. embassy-net stack (DHCP) ─────────────────────────────────────────
    let (stack, runner) = wifi::make_stack(wifi_interface);

    // ── 11. Spawn network tasks ───────────────────────────────────────────────
    spawner.spawn(wifi::net_task(runner)).expect("net_task");
    spawner
        .spawn(wifi::wifi_task(wifi_controller, stack))
        .expect("wifi_task");
    spawner
        .spawn(web_server::web_server_task(stack))
        .expect("web_server_task");
    log::info!("WiFi + web-server tasks spawned — open http://<IP>/ in your browser");

    // ── 12. Core 0 tasks — Wasm VM + ESP-NOW + Router ────────────────────────
    //
    // SAFETY: EspNowWithWifiCreateToken is a ZST { _private: () } with no data.
    // We transmute () into it to satisfy the type-system proof that WiFi is
    // already initialised — which it is (new_with_mode ran above).
    let espnow_token: EspNowWithWifiCreateToken = unsafe { core::mem::transmute(()) };
    spawner
        .spawn(core0::brain_task(spawner, io, wifi_init, espnow_token))
        .unwrap();

    log::info!("All tasks spawned — entering executor");
}
