use esp_hal::gpio::{GpioPin, Output};

use crate::hardware_profile::{set_audio_state, ModuleState, AUDIO_EN};

#[embassy_executor::task]
pub async fn probe_task(audio_en: GpioPin<1>) {
    let mut amp_en = Output::new(audio_en, esp_hal::gpio::Level::Low);
    amp_en.set_high();
    set_audio_state(ModuleState::Online);
    log::info!("[audio] speaker amplifier enabled on GPIO{}", AUDIO_EN);

    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(5)).await;
        amp_en.set_high();
    }
}
