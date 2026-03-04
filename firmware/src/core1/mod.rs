//! Core 1 — The Muscle.
//!
//! Runs hard real-time Embassy tasks for motor control, sensor polling, and
//! balancing.  This core operates **completely independently** of Core 0: it
//! does not share any mutable state with the Wasm engine or the ESP-NOW radio.
//!
//! ## Current Phase 1 implementation
//!
//! Phase 1 does not have physical motors, so Core 1 runs a high-frequency
//! "heartbeat" task that proves the real-time loop is alive and uninterrupted.
//! The loop period is chosen to match a future 500 Hz IMU polling rate.
//!
//! ## Future phases
//!
//! - Phase 3: Replace `dummy_rt_loop` with `imu_poll_task` (2 ms deadline).
//! - Phase 4: Add `motor_pwm_task` and `pid_balance_task`.
use embassy_time::{Duration, Ticker};

// ── Real-time loop period ─────────────────────────────────────────────────────

/// Core 1 loop period (2 ms = 500 Hz) — matches IMU polling in Phase 3.
const RT_LOOP_PERIOD_MS: u64 = 2;

// ── Tasks ─────────────────────────────────────────────────────────────────────

/// Core 1 entry-point task: the hard real-time loop.
///
/// In Phase 1 this is a dummy loop that simply counts iterations, proving
/// Core 1 is alive.  The same task will be extended in later phases to
/// drive motors and read sensors.
#[embassy_executor::task]
pub async fn realtime_task() {
    let mut ticker = Ticker::every(Duration::from_millis(RT_LOOP_PERIOD_MS));
    let mut tick_count: u64 = 0;

    loop {
        // ── Phase 3+ : IMU read will go here ─────────────────────────────────
        // let (pitch, roll) = imu.read().await;
        // IMU_DATA.store(encode(pitch, roll), Ordering::Release);

        // ── Phase 4+ : PID balance loop will go here ─────────────────────────
        // let output = pid.update(pitch, roll);
        // motor_left.set_duty(output.left);
        // motor_right.set_duty(output.right);

        tick_count = tick_count.wrapping_add(1);
        ticker.next().await;
    }
}
