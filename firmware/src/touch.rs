use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::{GpioPin, Input};
use esp_hal::i2c::master::{I2c, Config as I2cConfig, BusTimeout};
use esp_hal::peripherals::I2C0;
use esp_hal::time::RateExtU32;
use ft6x36::Ft6x36;

// Shared channel to pass touch events to the display loop
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub static TOUCH_EVENTS: Channel<CriticalSectionRawMutex, (u16, u16), 16> = Channel::new();

pub fn spawn_touch_task(
    spawner: Spawner,
    i2c0: I2C0,
    sda: GpioPin<16>,
    scl: GpioPin<15>,
    mut rst: esp_hal::gpio::Output<'static>,
    interrupt: Input<'static>,
) {
    // Reset the FT6336G
    rst.set_low();
    // In a real async fn we would Timer::after(Duration::from_millis(20)).await;
    // but here we just block briefly before starting the task
    esp_hal::delay::Delay::new().delay_millis(20);
    rst.set_high();
    esp_hal::delay::Delay::new().delay_millis(200);

    let mut cfg = I2cConfig::default();
    cfg.frequency = 400.kHz();
    cfg.timeout = BusTimeout::BusCycles(100_000);

    let i2c = match I2c::new(i2c0, cfg) {
        Ok(bus) => bus.with_sda(sda).with_scl(scl), // no into_async() for FT6336G compatibility
        Err(err) => {
            log::error!("[touch] I2C init failed: {:?}", err);
            return;
        }
    };

    spawner.spawn(touch_task(i2c, interrupt)).unwrap();
}

#[embassy_executor::task]
async fn touch_task(
    i2c: I2c<'static, esp_hal::Blocking>,
    mut interrupt: Input<'static>,
) {
    // Initialize the FT6336G driver
    let mut touch = Ft6x36::new(i2c, ft6x36::Dimension(240, 320));

    if let Err(e) = touch.init() {
        log::error!("[touch] Failed to initialize FT6336G: {:?}", e);
        return;
    }

    log::info!("[touch] FT6336G initialized");

    loop {
        // Wait for the interrupt pin to go low (active low)
        interrupt.wait_for_falling_edge().await;

        if let Ok(touch_event) = touch.get_touch_event() {
            if let Some(p1) = touch_event.p1 {
                log::debug!("[touch] Got touch at {}, {}", p1.x, p1.y);
                let _ = TOUCH_EVENTS.try_send((p1.x, p1.y));
            }
        }


        Timer::after(Duration::from_millis(2)).await; // debounce
    }
}
