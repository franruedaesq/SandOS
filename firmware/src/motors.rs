//! Servo motor control logic (Neck Reflexes).
//!
//! Uses `esp_hal::ledc` to generate a 50Hz PWM signal for MG90S/SG90 servos.
//! We monitor the `TOF_DISTANCE_MM` threshold to instantly trigger a "flinch" reflex.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use embassy_time::{Duration, Timer};
use esp_hal::ledc::channel::ChannelIFace;
use crate::sensors::load_tof_distance;

const PWM_FREQ_HZ: u32 = 50;
const DUTY_CENTER: u8 = 7; // Approx percent duty for center position
const DUTY_FLINCH: u8 = 10; // Approx percent duty for flinch position
const TOF_THRESHOLD_MM: u32 = 200;

pub static MOTOR_COMMAND: AtomicU32 = AtomicU32::new(DUTY_CENTER as u32);
pub static MOTOR_ENABLED: AtomicU8 = AtomicU8::new(1);

#[inline]
pub fn store_motor_command(left: i16, _right: i16) -> bool {
    // Map left value to a dummy duty cycle logic for testing
    MOTOR_COMMAND.store(left as u16 as u32, Ordering::Release);
    true
}

#[inline]
pub fn load_motor_command() -> (i16, i16) {
    (MOTOR_COMMAND.load(Ordering::Acquire) as i16, 0)
}

#[inline]
pub fn set_motor_enabled(enabled: bool) {
    MOTOR_ENABLED.store(u8::from(enabled), Ordering::Release);
}

#[inline]
pub fn is_motor_enabled() -> bool {
    MOTOR_ENABLED.load(Ordering::Acquire) != 0
}

#[embassy_executor::task]
pub async fn servo_reflex_task(channel: esp_hal::ledc::channel::Channel<'static, esp_hal::ledc::LowSpeed>) {
    log::info!("[motors] Servo reflex task started");

    loop {
        if is_motor_enabled() {
            let dist = load_tof_distance();

            let target_duty = if dist < TOF_THRESHOLD_MM {
                DUTY_FLINCH
            } else {
                DUTY_CENTER
            };

            let _ = channel.set_duty(target_duty);
        }

        Timer::after(Duration::from_millis(20)).await;
    }
}
