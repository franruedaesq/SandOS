use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::gpio::GpioPin;
use esp_hal::peripherals::ADC1;
use portable_atomic::{AtomicU32, Ordering};

/// Latest battery voltage reading in millivolts.
pub static BATTERY_VOLTAGE_MV: AtomicU32 = AtomicU32::new(0);

pub fn spawn_battery_task(spawner: Spawner, adc1: ADC1, pin: GpioPin<9>) {
    spawner.spawn(battery_task(adc1, pin)).unwrap();
}

#[embassy_executor::task]
async fn battery_task(adc1: ADC1, pin: GpioPin<9>) {
    let mut adc_config = AdcConfig::new();
    let mut adc_pin = adc_config.enable_pin(pin, Attenuation::_11dB);
    let mut adc = Adc::new(adc1, adc_config);

    log::info!("[battery] Task started, sensing on IO9");

    loop {
        // nb::block! is safe enough here if oneshot returns immediately most of the time
        // The ADC conversion takes just a few cycles so we don't need a full async wrapper
        // unless esp-hal provides one. We yield anyway via Timer::after right after.
        if let Ok(value) = nb::block!(adc.read_oneshot(&mut adc_pin)) {
            let mv = value as u32 * 2; // Stub conversion
            BATTERY_VOLTAGE_MV.store(mv, Ordering::Relaxed);
            log::debug!("[battery] Read ADC: {} -> {} mV", value, mv);
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}
