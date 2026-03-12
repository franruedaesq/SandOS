//! Shared sensor data including IMU and VL53L0X ToF sensor.
use core::sync::atomic::{AtomicU32, Ordering};

// ── ToF Sensor ───────────────────────────────────────────────────────────────

/// Latest Distance Reading from VL53L0X in millimeters.
/// Shared between the ToF polling task (writer) and motor reflexes (reader).
pub static TOF_DISTANCE_MM: AtomicU32 = AtomicU32::new(9999);

#[inline]
pub fn store_tof_distance(mm: u32) {
    TOF_DISTANCE_MM.store(mm, Ordering::Release);
}

#[inline]
pub fn load_tof_distance() -> u32 {
    TOF_DISTANCE_MM.load(Ordering::Acquire)
}

// ── Embassy Task for ToF ──────────────────────────────────────────────────────

use embassy_time::{Duration, Timer};
use esp_hal::{
    i2c::master::I2c,
    Blocking,
};
use vl53l0x::VL53L0x;

#[embassy_executor::task]
pub async fn tof_task(i2c_bus: I2c<'static, Blocking>) {
    log::info!("[tof] Initializing VL53L0X...");

    // Initialize the ToF sensor
    let mut tof = match VL53L0x::new(i2c_bus) {
        Ok(t) => t,
        Err(_e) => {
            log::error!("[tof] Failed to construct VL53L0x driver!");
            return;
        }
    };

    // According to vl53l0x crate docs, `read_range_mm` gets the distance.
    // It's blocking, but I2C operations are quick enough to not completely stall.
    loop {
        if let Ok(dist) = tof.read_range_mm() {
            store_tof_distance(dist as u32);
        } else {
            // If failed, keep last valid distance but maybe log occasionally
        }

        // Yield to other Embassy tasks
        Timer::after(Duration::from_millis(30)).await;
    }
}

// ── Dummy IMU stubs so compilation doesn't fail ─────────────────────────────
pub fn store_imu(_reading: abi::ImuReading) {
    // Stub
}

pub fn load_imu() -> abi::ImuReading {
    abi::ImuReading { pitch_millideg: 0, roll_millideg: 0 }
}
