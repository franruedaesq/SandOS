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
use embassy_time::{Duration, Timer};
use esp_hal::{
    cpu_control::{CpuControl, Stack},
    gpio::Io,
    rmt::{TxChannelConfig, TxChannelCreator},
    time::RateExtU32,
    timer::timg::TimerGroup,
};
use esp_wifi::EspWifiController;
use static_cell::StaticCell;

mod audio;
mod core0;
mod core1;
mod cpu_usage;
mod display;
mod inference;
mod led_state;
mod message_bus;
mod motors;
mod ntp;
mod rgb_led;
mod battery;
mod router;
mod sd_card;
mod sensors;
mod telemetry;
mod touch;
mod ulp;
mod vienna_fetch;
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

/// Dedicated stack for Core 1 (32 KiB — enough for Embassy + motor loops).
static mut APP_CORE_STACK: Stack<32768> = Stack::new();

// ── Core 1 executor ───────────────────────────────────────────────────────────

static CORE1_EXECUTOR: StaticCell<esp_hal_embassy::Executor> = StaticCell::new();

/// Entry point for Core 1 (The Muscle). Never returns.
fn core1_entry() {
    let executor = CORE1_EXECUTOR.init(esp_hal_embassy::Executor::new());
    executor.run(|spawner| {
        spawner.spawn(core1::realtime_task()).unwrap();
        spawner.spawn(cpu_usage::core1_idle_task()).unwrap();
    });
}

// ── Shared WiFi radio init ────────────────────────────────────────────────────
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
    // The WiFi blob allocates ~60-80 KB from this via the Rust global
    // allocator (wifi_malloc → alloc).  Those buffers MUST land in internal
    // SRAM — PSRAM pointers cause null-deref crashes in the blob's DMA paths.
    // With PSRAM enabled, keep this small so WiFi fits here; everything else
    // overflows to PSRAM automatically.
    esp_alloc::heap_allocator!(72 * 1024);
    // Add external PSRAM (octal/quad) as a large, lower-priority region.
    // Internal SRAM is tried first (WiFi blob needs it for DMA), then PSRAM
    // absorbs everything else (Wasm VM, audio buffers, etc.).
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // ── 3. Logger over UART ──────────────────────────────────────────────────
    esp_println::logger::init_logger(log::LevelFilter::Info);
    log::info!("SandOS v{} booting on ESP32-S3…", env!("CARGO_PKG_VERSION"));

    // ── 4. Embassy time driver (TIMG0) ───────────────────────────────────────
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    // ── 5. WiFi radio init (EARLY — before display/DMA/audio allocations) ───
    //
    // The WiFi blob allocates from internal SRAM via the global allocator.
    // Initialising it before any other tasks ensures the blob gets fresh,
    // unfragmented internal SRAM.  Later allocations (display DMA buffers,
    // audio, Wasm VM) safely overflow to PSRAM.
    //
    // TIMG1 is used for the esp-wifi timer so it does not conflict with the
    // Embassy time driver on TIMG0.  RNG and RADIO_CLK are consumed here and
    // must not be used elsewhere.
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    let wifi_controller_init = esp_wifi::init(
        timg1.timer0,
        esp_hal::rng::Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    )
    .expect("Failed to initialize esp-wifi");
    let wifi_init: &'static EspWifiController<'static> = WIFI_INIT.init(wifi_controller_init);
    log::info!("esp-wifi initialized");

    // ── 6. WiFi STA interface ────────────────────────────────────────────────
    let (wifi_interface, wifi_controller) = esp_wifi::wifi::new_with_mode(
        wifi_init,
        peripherals.WIFI,
        esp_wifi::wifi::WifiStaDevice,
    )
    .expect("Failed to create WiFi STA interface");

    // ── 7. Embassy-net stack ─────────────────────────────────────────────────
    let (stack, runner) = wifi::make_stack(wifi_interface);

    // ── 8. Network tasks ─────────────────────────────────────────────────────
    spawner.spawn(wifi::net_task(runner)).unwrap();
    spawner.spawn(wifi::wifi_task(wifi_controller, stack)).unwrap();
    log::info!("WiFi tasks spawned");

    // ── 9. GPIO ──────────────────────────────────────────────────────────────
    // BOOT button (GPIO 0, active-LOW with hardware pull-up) — must be taken
    // from peripherals before Io::new() consumes IO_MUX.
    let boot_btn = esp_hal::gpio::Input::new(peripherals.GPIO0, esp_hal::gpio::Pull::Up);

    let io = Io::new(peripherals.IO_MUX);

    // ── RGB LED init (GPIO 42 via RMT) ────────────────────────────────────────
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, 80_u32.MHz())
        .expect("Failed to initialize RMT");

    let tx_channel_42 = rmt
        .channel1
        .configure(
            peripherals.GPIO42,
            TxChannelConfig {
                clk_divider: 4,
                idle_output_level: false,
                idle_output: true,
                ..Default::default()
            },
        )
        .expect("Failed to configure RMT TX channel 1 (GPIO42)");

    unsafe {
        rgb_led::RGB_LED = Some(rgb_led::RgbLedDriver::new());
        if let Some(led) = (*core::ptr::addr_of_mut!(rgb_led::RGB_LED)).as_mut() {
            led.attach_tx_channel_gpio48(tx_channel_42);
            led.off();
        }
    }
    log::info!("RGB LED initialized on GPIO42 with RMT");

    // ── 10. Core 1 — start before Wasm VM ───────────────────────────────────
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, core1_entry)
        .unwrap();
    log::info!("Core 1 started");

    // ── 11. ULP paramedic ────────────────────────────────────────────────────
    ulp::start(peripherals.LPWR);

    // ── 12. Display + button tasks ───────────────────────────────────────────
    display::spawn_display_task(
        spawner,
        peripherals.SPI2,
        peripherals.GPIO11, // MOSI
        peripherals.GPIO12, // SCK
        peripherals.GPIO10, // CS
        peripherals.GPIO46, // DC
        peripherals.GPIO45, // RST
        boot_btn,
        peripherals.DMA_CH1,
    );
    log::info!("Display + button tasks spawned");

    // ── 13. Web server task (starts disabled) ────────────────────────────────
    spawner.spawn(web_server::web_server_task(stack)).unwrap();
    log::info!("Web server task spawned (disabled by default)");

    // ── 13b. Vienna departures fetch task ───────────────────────────────────
    spawner
        .spawn(vienna_fetch::vienna_fetch_task(stack))
        .unwrap();
    log::info!("Vienna fetch task spawned");

    // ── 13c. NTP time sync task ──────────────────────────────────────────────
    spawner.spawn(ntp::ntp_sync_task(stack)).unwrap();
    log::info!("NTP sync task spawned");

    // ── 14. SD Card Init (SDIO pins mapped to SPI3) ──────────────────────────
    let spi3 = esp_hal::spi::master::Spi::new(peripherals.SPI3, esp_hal::spi::master::Config::default())
        .expect("Failed to initialize SPI3 for SD Card")
        .with_mosi(peripherals.GPIO40)
        .with_miso(peripherals.GPIO39)
        .with_sck(peripherals.GPIO38);
    let sd_cs = esp_hal::gpio::Output::new(peripherals.GPIO47, esp_hal::gpio::Level::High);
    sd_card::init_sd_card(spi3, sd_cs);
    log::info!("SD Card initialized on SPI3 using SDIO pins");

    // ── 15. Capacitive Touch (FT6336G) ───────────────────────────────────────
    let touch_rst = esp_hal::gpio::Output::new(peripherals.GPIO18, esp_hal::gpio::Level::Low);
    let touch_int = esp_hal::gpio::Input::new(peripherals.GPIO17, esp_hal::gpio::Pull::Up);
    touch::spawn_touch_task(
        spawner,
        peripherals.I2C0,
        peripherals.GPIO16, // SDA
        peripherals.GPIO15, // SCL
        touch_rst,
        touch_int,
    );
    log::info!("Touch task spawned on I2C0");

    // ── 15b. Battery Sensing ─────────────────────────────────────────────────
    battery::spawn_battery_task(spawner, peripherals.ADC1, peripherals.GPIO9);
    log::info!("Battery task spawned on ADC1 IO9");

    // ── 16. Audio (I2S Speaker & Microphone) ─────────────────────────────────
    let dma_channel = peripherals.DMA_CH0;
    audio::spawn_audio_tasks(
        spawner,
        peripherals.I2S0,
        peripherals.GPIO4, // MCLK
        peripherals.GPIO5, // BCLK
        peripherals.GPIO7, // LRCK/WS
        peripherals.GPIO6, // DOUT (Speaker)
        peripherals.GPIO8, // DIN (Mic)
        peripherals.GPIO1, // Amp EN
        dma_channel,
    );
    log::info!("Audio tasks spawned on I2S0");

    // ── 17. Expansion & UART Serial Port ─────────────────────────────────────
    let _uart_rx = peripherals.GPIO43;
    let _uart_tx = peripherals.GPIO44;
    let _exp_io2 = peripherals.GPIO2;
    let _exp_io3 = peripherals.GPIO3;
    let _exp_io21 = peripherals.GPIO21;

    // ── 18. CPU usage monitor ────────────────────────────────────────────────
    cpu_usage::spawn_cpu_monitor(spawner);
    spawner.spawn(cpu_usage::core0_idle_task()).unwrap();
    log::info!("CPU usage monitor spawned");

    // ── 19. Core 0 brain task ────────────────────────────────────────────────
    spawner
        .spawn(core0::brain_task(spawner, io, wifi_init))
        .unwrap();
    log::info!("Brain task spawned");

    // Keep main alive forever so _core1_guard and other locals are not dropped.
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
