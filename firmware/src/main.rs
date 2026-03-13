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
use alloc::boxed::Box;
use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation, AdcPin},
    cpu_control::{CpuControl, Stack},
    gpio::{Io, GpioPin},
    i2c::master::{Config as I2cConfig, I2c},
    i2s::master::{DataFormat, I2s, Standard},
    peripheral::Peripheral,
    peripherals::ADC1,
    rmt::{TxChannelConfig, TxChannelCreator},
    time::RateExtU32,
    timer::timg::TimerGroup,
};
use esp_wifi::EspWifiController;
use static_cell::StaticCell;

mod core0;
mod core1;
mod display;
mod inference;
mod led_state;
mod message_bus;
mod motors;
mod ntp;
mod rgb_led;
mod router;
mod sensors;
mod telemetry;
mod touch;
mod ulp;
mod vienna_fetch;
mod audio;
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
#[embassy_executor::task]
async fn battery_task(mut adc: Adc<'static, ADC1>, mut pin: AdcPin<GpioPin<9>, ADC1>) {
    loop {
        if let Ok(value) = adc.read_oneshot(&mut pin) {
            // Rough conversion for proof of concept (12-bit ADC, ~3.3V ref with 11dB)
            let mv = (value as u32 * 3300 / 4095) as u16;
            crate::sensors::store_battery_mv(mv);
        }
        Timer::after(Duration::from_millis(1000)).await;
    }
}

#[embassy_executor::task]
async fn touch_task(i2c: I2c<'static, esp_hal::Async>) {
    let mut touch = touch::Ft6336::new(i2c);
    loop {
        if let Ok(Some((x, y))) = touch.read_touch().await {
            crate::sensors::store_touch_coords(x, y);
        } else {
            // Touch released or error
            crate::sensors::clear_touch_coords();
        }
        Timer::after(Duration::from_millis(20)).await;
    }
}

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

    // ── Battery ADC init ──────────────────────────────────────────────────────
    let mut adc_config = AdcConfig::new();
    let battery_pin = adc_config.enable_pin(peripherals.GPIO9, Attenuation::_11dB);
    let adc = Adc::new(peripherals.ADC1, adc_config);
    spawner.spawn(battery_task(adc, battery_pin)).unwrap();
    log::info!("Battery ADC task spawned");

    // ── Touchscreen I2C init ──────────────────────────────────────────────────
    let i2c = I2c::new(peripherals.I2C1, I2cConfig::default())
        .unwrap()
        .with_sda(peripherals.GPIO16)
        .with_scl(peripherals.GPIO15)
        .into_async();
    spawner.spawn(touch_task(i2c)).unwrap();
    log::info!("Touch I2C task spawned");

    // ── 8. Audio I2S + DMA init ──────────────────────────────────────────────
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = esp_hal::dma_buffers!(12288, 12288);

    let i2s = I2s::new(
        peripherals.I2S0,
        Standard::Philips,
        DataFormat::Data16Channel16,
        16000.Hz(),
        peripherals.DMA_CH0,
        rx_descriptors,
        tx_descriptors,
    );

    let i2s = i2s.with_mclk(peripherals.GPIO4).into_async();

    // GPIO bindings need to be split if the same pin is used for RX and TX,
    // but esp-hal 0.23 requires taking ownership. For I2S bclk/ws, we can
    // configure them on either rx or tx, and the peripheral shares them internally
    // if it's the same port.
    // Wait, let's look at how esp-hal does it. If we use with_bclk on rx, we can't use it on tx.
    // However, I2S0 shares signals. We can just build rx and tx without re-assigning bclk and ws
    // to tx if they are already on rx, or vice versa? No, they have separate setters.
    // Let's create dummy pins or `unsafe` clone if needed?
    // Wait, the standard pattern in esp-hal 0.23 for I2S is to call with_bclk and with_ws on i2s_tx OR i2s_rx
    // and the hardware handles it since it's the same peripheral? Actually, esp-hal has `TxCreator` and `RxCreator`.
    // Let's use `unsafe { peripherals.GPIO5.clone_unchecked() }` to bypass the borrow checker for the shared BCLK/WS pins.
    let bclk_tx = unsafe { peripherals.GPIO5.clone_unchecked() };
    let ws_tx = unsafe { peripherals.GPIO7.clone_unchecked() };

    let mut i2s_rx = i2s
        .i2s_rx
        .with_bclk(peripherals.GPIO5)
        .with_ws(peripherals.GPIO7)
        .with_din(peripherals.GPIO8)
        .build();

    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(bclk_tx)
        .with_ws(ws_tx)
        .with_dout(peripherals.GPIO6)
        .build();

    // Enable speaker amplifier
    let mut speaker_en = esp_hal::gpio::Output::new(peripherals.GPIO1, esp_hal::gpio::Level::High);
    // Setting to low enables audio output based on user input
    speaker_en.set_low();

    // We need to pass static mutable references to the DMA buffers to the async tasks
    // Leak the rx_buffer and tx_buffer to make them 'static.
    // dma_buffers returns `&mut [u8; N]` types, which are actually references to static memory
    // created by the macro. We can safely transmute them to static slices.
    let rx_buffer_static: &'static mut [u8] = unsafe {
        core::slice::from_raw_parts_mut(rx_buffer.as_mut_ptr(), rx_buffer.len())
    };
    let tx_buffer_static: &'static mut [u8] = unsafe {
        core::slice::from_raw_parts_mut(tx_buffer.as_mut_ptr(), tx_buffer.len())
    };

    spawner.spawn(audio::audio_rx_task(i2s_rx, rx_buffer_static)).unwrap();
    spawner.spawn(audio::audio_tx_task(i2s_tx, tx_buffer_static)).unwrap();
    log::info!("Audio I2S tasks spawned");

    // ── 8. Display + button tasks ────────────────────────────────────────────
    let spi = esp_hal::spi::master::Spi::new(peripherals.SPI2, esp_hal::spi::master::Config::default()).unwrap()
        .with_sck(peripherals.GPIO12)
        .with_mosi(peripherals.GPIO11)
        .with_miso(peripherals.GPIO13)
        .into_async();

    let cs = esp_hal::gpio::Output::new(peripherals.GPIO10, esp_hal::gpio::Level::High);
    let dc = esp_hal::gpio::Output::new(peripherals.GPIO46, esp_hal::gpio::Level::Low);
    let mut blk = esp_hal::gpio::Output::new(peripherals.GPIO45, esp_hal::gpio::Level::High);

    display::spawn_display_task(
        spawner,
        spi,
        dc,
        cs,
        boot_btn,
    );
    log::info!("Display + button tasks spawned");

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
    let (wifi_interface, wifi_controller) = esp_wifi::wifi::new_with_mode(
        wifi_init,
        peripherals.WIFI,
        esp_wifi::wifi::WifiStaDevice,
    )
    .expect("Failed to create WiFi STA interface");
    // ── 11. Embassy-net stack ────────────────────────────────────────────────
    let (stack, runner) = wifi::make_stack(wifi_interface);

    // ── 12. Network tasks ────────────────────────────────────────────────────
    //
    // net_task: drives the embassy-net packet processor (must run alongside wifi_task).
    // wifi_task: manages WiFi association, DHCP, and reconnect.
    spawner.spawn(wifi::net_task(runner)).unwrap();
    spawner.spawn(wifi::wifi_task(wifi_controller, stack)).unwrap();
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

    // ── 14. Core 0 brain task ────────────────────────────────────────────────
    //
    // Spawns the ESP-NOW receiver, the Wasm engine, and the OS router tasks.
    #[cfg(feature = "espnow")]
    let esp_now_token = unsafe { core::mem::transmute::<(), esp_wifi::esp_now::EspNowWithWifiCreateToken>(()) };

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
