//! Core 1 — The Muscle.
//!
//! Runs hard real-time Embassy tasks for motor control, sensor polling, and
//! balancing.  This core operates **completely independently** of Core 0: it
//! does not share any mutable state with the Wasm engine or the ESP-NOW radio.
//!
//! ## Phase 3 implementation
//!
//! Core 1 polls the IMU every [`RT_LOOP_PERIOD_MS`] (2 ms = 500 Hz) and
//! publishes the result to [`crate::sensors::IMU_DATA`] via a single atomic
//! store.  Core 0 reads this value through the ABI without ever touching a
//! mutex or suspending.
//!
//! The simulated IMU values below will be replaced by real I2C/SPI reads
//! once the physical MPU-6050 (or equivalent) is wired up.
//!
//! ## Future phases
//!
//! - Phase 4: Add `motor_pwm_task` and `pid_balance_task`.
use abi::ImuReading;
use embassy_time::{Duration, Ticker};

use crate::sensors;

// ── Real-time loop period ─────────────────────────────────────────────────────

/// Core 1 loop period: 2 ms = 500 Hz — matches the IMU polling rate.
const RT_LOOP_PERIOD_MS: u64 = 2;

// ── Tasks ─────────────────────────────────────────────────────────────────────

/// Core 1 entry-point task: the hard real-time IMU polling loop.
///
/// Every [`RT_LOOP_PERIOD_MS`] this task:
/// 1. Reads pitch and roll from the IMU over I2C/SPI.
/// 2. Publishes the result to [`crate::sensors::IMU_DATA`] via a single
///    `AtomicU64::store(Release)` — no locks, no blocking.
///
/// In Phase 3 the I2C read is simulated with a counter-based stub.  The
/// stub will be replaced with a real `esp_hal::i2c` transfer once the
/// physical sensor is connected.
#[embassy_executor::task]
pub async fn realtime_task() {
    let mut ticker = Ticker::every(Duration::from_millis(RT_LOOP_PERIOD_MS));
    let mut tick_count: u64 = 0;

    loop {
        // ── IMU read (Phase 3) ────────────────────────────────────────────────
        // Stub: synthesise a gentle sine-like oscillation so the Wasm guest
        // can observe changing sensor data without real hardware.
        // Replace this block with an actual esp_hal::i2c / spi transfer.
        let reading = simulate_imu(tick_count);
        sensors::store_imu(reading);

        // ── Phase 4+ : PID balance loop will go here ─────────────────────────
        // let output = pid.update(reading.pitch_millideg, reading.roll_millideg);
        // motor_left.set_duty(output.left);
        // motor_right.set_duty(output.right);

        tick_count = tick_count.wrapping_add(1);
        ticker.next().await;
    }
}

// ── IMU stub ──────────────────────────────────────────────────────────────────

/// Synthesise an [`ImuReading`] from a tick counter.
///
/// Produces a slowly-oscillating pitch/roll pair so that the Wasm ABI test
/// can observe non-zero, changing values before real hardware is attached.
///
/// The oscillation period is ~500 ticks × 2 ms = ~1 second.
#[inline]
fn simulate_imu(tick: u64) -> ImuReading {
    // Use a simple triangle wave (±30 °) in millidegrees.
    const PERIOD: i64 = 500;
    const AMP: i64 = 30_000; // 30 000 millideg = 30°

    let phase = (tick as i64) % PERIOD;
    let half = PERIOD / 2;
    let pitch_millideg = if phase < half {
        (phase * AMP / half) as i32
    } else {
        ((PERIOD - phase) * AMP / half) as i32
    };
    // Roll oscillates at the same frequency as pitch but with a quarter-period
    // phase offset, so the two axes move independently.
    let roll_phase = (tick as i64 + PERIOD / 4) % PERIOD;
    let roll_millideg = if roll_phase < half {
        (roll_phase * AMP / half) as i32
    } else {
        ((PERIOD - roll_phase) * AMP / half) as i32
    };

    ImuReading {
        pitch_millideg,
        roll_millideg,
    }
}

