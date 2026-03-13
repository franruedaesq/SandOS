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

mod core0;
mod core1;
mod display;
mod hardware_profile;
mod inference;
mod led_state;
mod message_bus;
mod motors;
mod ntp;
mod rgb_led;
mod router;
mod sensors;
mod telemetry;
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
    // 72 KB leaves headroom after WiFi creation for the scan channel list.
    esp_alloc::heap_allocator!(72 * 1024);
    // Add external PSRAM (octal/quad) as a large, lower-priority region.
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // ── 3. Logger over UART ──────────────────────────────────────────────────
    esp_println::logger::init_logger(log::LevelFilter::Info);
    log::info!("SandOS v{} booting on ESP32-S3…", env!("CARGO_PKG_VERSION"));

    // ── 4. Embassy time driver (TIMG0) ───────────────────────────────────────
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    // ── 5. GPIO ──────────────────────────────────────────────────────────────
    // BOOT button (GPIO 0, active-LOW with hardware pull-up) — must be taken
    // from peripherals before Io::new() consumes IO_MUX.
    let boot_btn = esp_hal::gpio::Input::new(peripherals.GPIO0, esp_hal::gpio::Pull::Up);

    let io = Io::new(peripherals.IO_MUX);

    // ── RGB LED init (GPIO 42 via RMT) ────────────────────────────────────────
    let rmt =
        esp_hal::rmt::Rmt::new(peripherals.RMT, 80_u32.MHz()).expect("Failed to initialize RMT");

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
            led.attach_tx_channel_gpio42(tx_channel_42);
            led.off();
        }
    }
    log::info!("RGB LED initialized on GPIO42 with RMT");

    // ── 6. Core 1 — start before Wasm VM ────────────────────────────────────
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, core1_entry)
        .unwrap();
    log::info!("Core 1 started");

    // ── 7. ULP paramedic ─────────────────────────────────────────────────────
    ulp::start(peripherals.LPWR);

    // ── 8. Display + button tasks ────────────────────────────────────────────
    //
    // spawn_display_task spawns two Embassy tasks:
    //   • display_task  — renders frames via async I2C (CPU yields during flush)
    //   • button_task   — monitors GPIO0 with hardware edge interrupts
    //
    // With async I2C the display no longer blocks Core 0, so it is safe to
    // run the Wi-Fi stack and web server alongside the display.
    display::spawn_display_task(
        spawner,
        peripherals.I2C0,
        peripherals.GPIO8,
        peripherals.GPIO9,
        boot_btn,
    );
    log::info!("Display + button tasks spawned");

    log::warn!(
        "DISPLAY BACKEND: current build still uses SSD1306(I2C0) task; ILI9341 SPI migration pending"
    );
    // Runtime module health starts as UNKNOWN unless actively verified.
    // The display task upgrades display state to ONLINE once the panel init succeeds.
    hardware_profile::set_display_state(hardware_profile::ModuleState::Unknown);
    hardware_profile::set_touch_state(hardware_profile::ModuleState::Unknown);
    hardware_profile::set_audio_state(hardware_profile::ModuleState::Unknown);
    hardware_profile::set_sd_state(hardware_profile::ModuleState::Unknown);
    hardware_profile::set_battery_state(hardware_profile::ModuleState::Unknown);
    hardware_profile::set_rgb_state(hardware_profile::ModuleState::Online);
    hardware_profile::set_uart_state(hardware_profile::ModuleState::Online);
    // ── Hardware profile boot diagnostics (target board pin map) ─────────────
    log::info!("Board profile: {}", hardware_profile::CHIP_NAME);
    log::info!("External flash: {}MB QSPI", hardware_profile::FLASH_SIZE_MB);
    log::info!(
        "LCD ILI9341 SPI pins: CS={} DC={} SCK={} MOSI={} MISO={} BL={}",
        hardware_profile::LCD_CS,
        hardware_profile::LCD_DC,
        hardware_profile::LCD_SCK,
        hardware_profile::LCD_MOSI,
        hardware_profile::LCD_MISO,
        hardware_profile::LCD_BL
    );
    log::info!(
        "Touch I2C pins: SDA={} SCL={} RST={} INT={}",
        hardware_profile::TOUCH_SDA,
        hardware_profile::TOUCH_SCL,
        hardware_profile::TOUCH_RST,
        hardware_profile::TOUCH_INT
    );
    log::info!(
        "Audio I2S pins: EN={} MCLK={} BCLK={} DOUT={} WS={} DIN={}",
        hardware_profile::AUDIO_EN,
        hardware_profile::AUDIO_MCLK,
        hardware_profile::AUDIO_BCLK,
        hardware_profile::AUDIO_DOUT,
        hardware_profile::AUDIO_WS,
        hardware_profile::AUDIO_DIN
    );
    log::info!(
        "SDIO pins: CLK={} CMD={} D0={} D1={} D2={} D3={}",
        hardware_profile::SD_CLK,
        hardware_profile::SD_CMD,
        hardware_profile::SD_DATA0,
        hardware_profile::SD_DATA1,
        hardware_profile::SD_DATA2,
        hardware_profile::SD_DATA3
    );
    log::info!(
        "Misc pins: RGB={} UART0_RX={} UART0_TX={} BAT_ADC={}",
        hardware_profile::RGB_LED,
        hardware_profile::UART0_RX,
        hardware_profile::UART0_TX,
        hardware_profile::BATTERY_ADC
    );
    let diag_line = hardware_profile::boot_status_line();
    display::set_status_text(diag_line.as_str());

    // ── 9. WiFi radio init ───────────────────────────────────────────────────
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

    // ── 10. WiFi STA interface + ESP-NOW coexistence token ───────────────────
    //
    // Create the WiFi STA interface directly (without enable_esp_now_with_wifi,
    // which was found to break STA connections on this hardware).
    // The ESP-NOW token is a zero-sized type — we transmute () into it since
    // WiFi is already initialized above.
    let (wifi_interface, wifi_controller) =
        esp_wifi::wifi::new_with_mode(wifi_init, peripherals.WIFI, esp_wifi::wifi::WifiStaDevice)
            .expect("Failed to create WiFi STA interface");
    // ── 11. Embassy-net stack ────────────────────────────────────────────────
    let (stack, runner) = wifi::make_stack(wifi_interface);

    // ── 12. Network tasks ────────────────────────────────────────────────────
    //
    // net_task: drives the embassy-net packet processor (must run alongside wifi_task).
    // wifi_task: manages WiFi association, DHCP, and reconnect.
    spawner.spawn(wifi::net_task(runner)).unwrap();
    spawner
        .spawn(wifi::wifi_task(wifi_controller, stack))
        .unwrap();
    log::info!("WiFi tasks spawned");

    // ── 13. Web server task (starts disabled) ────────────────────────────────
    //
    // Sleeps until the user enables it via the display menu.  Once enabled it
    // waits for a DHCP lease and then serves the dashboard on port 80.
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
    spawner
        .spawn(hardware_profile::diagnostics_log_task())
        .unwrap();
    log::info!("Hardware diagnostics logger task spawned");
    spawner
        .spawn(hardware_profile::touch_probe_task(
            peripherals.I2C1,
            peripherals.GPIO16,
            peripherals.GPIO15,
        ))
        .unwrap();
    log::info!("Touch probe task spawned (FT6336 on I2C1)");

    // ── 14. Core 0 brain task ────────────────────────────────────────────────
    //
    // Spawns the ESP-NOW receiver, the Wasm engine, and the OS router tasks.
    #[cfg(feature = "espnow")]
    let esp_now_token =
        unsafe { core::mem::transmute::<(), esp_wifi::esp_now::EspNowWithWifiCreateToken>(()) };

    #[cfg(feature = "espnow")]
    spawner
        .spawn(core0::brain_task(spawner, io, wifi_init, esp_now_token))
        .unwrap();

    #[cfg(not(feature = "espnow"))]
    spawner
        .spawn(core0::brain_task(spawner, io, wifi_init))
        .unwrap();
    log::info!("Brain task spawned");

    // Keep main alive forever so _core1_guard and other locals are not dropped.
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
