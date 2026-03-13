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
    i2c::master::{Config as I2cConfig, I2c},
    ledc::{Ledc, channel::{self, ChannelIFace}, timer::{self, TimerIFace}, LowSpeed},
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
mod rgb_led;
mod router;
mod sensors;
mod telemetry;
mod ulp;

use esp_backtrace as _;

static mut APP_CORE_STACK: Stack<32768> = Stack::new();
static CORE1_EXECUTOR: StaticCell<esp_hal_embassy::Executor> = StaticCell::new();
static WIFI_INIT: StaticCell<EspWifiController<'static>> = StaticCell::new();

fn core1_entry() {
    let executor = CORE1_EXECUTOR.init(esp_hal_embassy::Executor::new());
    executor.run(|spawner| {
        spawner.spawn(core1::realtime_task()).unwrap();
    });
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    esp_println::println!("\n\n=== SandOS Head OS Starting ===");

    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_println::logger::init_logger(log::LevelFilter::Info);
    log::info!("SandOS v{} booting on ESP32-S3…", env!("CARGO_PKG_VERSION"));

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    let boot_btn = esp_hal::gpio::Input::new(peripherals.GPIO0, esp_hal::gpio::Pull::Up);
    let io = Io::new(peripherals.IO_MUX);

    // RGB LED
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, 80_u32.MHz()).expect("Failed to initialize RMT");
    let tx_channel_48 = rmt.channel1.configure(
        peripherals.GPIO48,
        TxChannelConfig {
            clk_divider: 4,
            idle_output_level: false,
            idle_output: true,
            ..Default::default()
        },
    ).expect("Failed to configure RMT");

    unsafe {
        rgb_led::RGB_LED = Some(rgb_led::RgbLedDriver::new());
        if let Some(led) = (*core::ptr::addr_of_mut!(rgb_led::RGB_LED)).as_mut() {
            led.attach_tx_channel_gpio48(tx_channel_48);
            led.off();
        }
    }

    // LEDC Servo Init
    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(esp_hal::ledc::LSGlobalClkSource::APBClk);

    static TIMER0: StaticCell<esp_hal::ledc::timer::Timer<'static, LowSpeed>> = StaticCell::new();
    let lstimer0 = TIMER0.init(ledc.timer::<LowSpeed>(timer::Number::Timer0));

    lstimer0.configure(timer::config::Config {
        duty: timer::config::Duty::Duty14Bit,
        clock_source: timer::LSClockSource::APBClk,
        frequency: 50.Hz(),
    }).unwrap();
    let mut channel0 = ledc.channel(channel::Number::Channel0, peripherals.GPIO1); // Using GPIO1 for Servo
    channel0.configure(channel::config::Config {
        timer: lstimer0,
        duty_pct: 7, // 7% is center
        pin_config: channel::config::PinConfig::PushPull,
    }).unwrap();

    // ToF I2C Init
    let i2c_bus = I2c::new(peripherals.I2C0, I2cConfig::default())
        .expect("I2C Init Failed")
        .with_sda(peripherals.GPIO4)
        .with_scl(peripherals.GPIO5);

    // Start Core 1 with ToF and Servo Tasks
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _core1_guard = cpu_control.start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, core1_entry).unwrap();
    log::info!("Core 1 started");

    // Spawn ToF and Servo manually to our spawner for now since we are sharing everything on Core0 while debugging
    // Wait, Core 1 executor only runs `core1::realtime_task()`.
    // To cleanly deploy them, let's just spawn them here on Core 0 for now since Embassy Executor handles async correctly anyway,
    // or we can add them to `core1_entry`. Let's just spawn them here for simplicity.
    spawner.spawn(motors::servo_reflex_task(channel0)).unwrap();
    spawner.spawn(sensors::tof_task(i2c_bus)).unwrap();

    ulp::start(peripherals.LPWR);

    // Display Init
    let spi2 = peripherals.SPI2;
    let sck = peripherals.GPIO9;
    let mosi = peripherals.GPIO8;
    let dc = peripherals.GPIO10;
    let cs = peripherals.GPIO11;
    let rst = peripherals.GPIO12;

    display::spawn_display_task(spawner, spi2, sck, mosi, dc, cs, rst, boot_btn);

    // WiFi ESP-NOW Init
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    let wifi_controller_init = esp_wifi::init(
        timg1.timer0,
        esp_hal::rng::Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    ).expect("Failed to init esp-wifi");
    let wifi_init: &'static EspWifiController<'static> = WIFI_INIT.init(wifi_controller_init);

    spawner.spawn(core0::brain_task(spawner, io, wifi_init)).unwrap();

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
