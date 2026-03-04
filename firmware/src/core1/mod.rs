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
//! ## Phase 4 implementation
//!
//! Core 1 now runs the full motor control pipeline each tick:
//!
//! 1. Read pitch/roll from the IMU.
//! 2. Check the ULP voltage flag — disable motors on critical voltage.
//! 3. Run the PID balance controller to compute a PWM output.
//! 4. Blend the PID output with any steering command from Core 0 (Wasm ABI).
//! 5. Apply the blended duty cycles to the motor PWM hardware.
//!
//! The Wasm sandbox on Core 0 can only influence the *steering offset*; it
//! cannot override the balancing loop or exceed the PWM clamps.
mod pid;

use abi::ImuReading;
use embassy_time::{Duration, Ticker};
use pid::PidController;

use crate::{motors, sensors, ulp};

// ── Real-time loop period ─────────────────────────────────────────────────────

/// Core 1 loop period: 2 ms = 500 Hz — matches the IMU polling rate.
const RT_LOOP_PERIOD_MS: u64 = 2;

/// Loop period expressed in seconds (used by the PID controller).
const DT_S: f32 = RT_LOOP_PERIOD_MS as f32 / 1000.0;

// ── PID tuning constants ──────────────────────────────────────────────────────
//
// These are starting values to be tuned with real hardware.
// Increasing Kp makes the response faster but can cause oscillation.
// Increasing Ki reduces steady-state error.
// Increasing Kd damps oscillation but amplifies noise.

/// Proportional gain.
const PID_KP: f32 = 10.0;
/// Integral gain.
const PID_KI: f32 = 0.5;
/// Derivative gain.
const PID_KD: f32 = 0.1;
/// Minimum PWM duty cycle output (full reverse).
const PID_OUT_MIN: f32 = -255.0;
/// Maximum PWM duty cycle output (full forward).
const PID_OUT_MAX: f32 = 255.0;

// ── Tasks ─────────────────────────────────────────────────────────────────────

/// Core 1 entry-point task: IMU polling + PID balance loop + motor control.
///
/// Every [`RT_LOOP_PERIOD_MS`] this task:
/// 1. Reads pitch and roll from the IMU over I2C/SPI.
/// 2. Publishes the result to [`crate::sensors::IMU_DATA`].
/// 3. Checks the ULP low-voltage flag — disables motors on critical voltage.
/// 4. Runs the PID controller to compute a balance correction.
/// 5. Blends the PID output with the steering command from Core 0.
/// 6. Applies the result to the motor PWM hardware.
#[embassy_executor::task]
pub async fn realtime_task() {
    let mut ticker = Ticker::every(Duration::from_millis(RT_LOOP_PERIOD_MS));
    let mut tick_count: u64 = 0;
    let mut pid = PidController::new(PID_KP, PID_KI, PID_KD, PID_OUT_MIN, PID_OUT_MAX);

    loop {
        // ── 1. IMU read ───────────────────────────────────────────────────────
        // Stub: synthesised oscillation until the real sensor is wired up.
        let reading = simulate_imu(tick_count);
        sensors::store_imu(reading);

        // ── 2. Voltage safety check ───────────────────────────────────────────
        // If the ULP paramedic signals a critical voltage drop, cut power
        // immediately and reset the PID state so there is no jerk on recovery.
        if ulp::is_voltage_critical() {
            motors::set_motor_enabled(false);
            pid.reset();
        }

        // ── 3. PID balance loop ───────────────────────────────────────────────
        if motors::is_motor_enabled() {
            // Convert millidegrees → degrees for the PID (setpoint = 0 = upright).
            let pitch_deg = reading.pitch_millideg as f32 / 1000.0;
            let balance_output = pid.update(0.0, pitch_deg, DT_S);

            // ── 4. Blend with Wasm steering command ───────────────────────────
            // The Wasm guest on Core 0 can set a differential steering offset
            // (e.g., +50 left, -50 right to turn right).  The PID correction
            // is applied on top of this so the robot never stops balancing.
            let (cmd_left, cmd_right) = motors::load_motor_command();
            // Use saturating addition before clamping to i16 to guard against
            // the (unlikely) case where the PID output and command are both
            // at their extreme values simultaneously.
            let left = clamp_i16(
                (balance_output as i32).saturating_add(cmd_left as i32),
            );
            let right = clamp_i16(
                (balance_output as i32).saturating_add(cmd_right as i32),
            );

            // ── 5. Apply PWM ──────────────────────────────────────────────────
            // In production: write `left` and `right` to the LEDC / MCPWM
            // peripheral duty registers here.
            //
            // Example:
            //   ledc_left.set_duty(left.unsigned_abs() as u32).ok();
            //   dir_left.set_level(left >= 0).ok();
            //   ledc_right.set_duty(right.unsigned_abs() as u32).ok();
            //   dir_right.set_level(right >= 0).ok();
            let _ = (left, right); // suppress unused-variable warning in stub
        } else {
            // Motors are disabled — ensure hardware PWM outputs are zeroed.
            // In production: ledc_left.set_duty(0).ok(); ledc_right.set_duty(0).ok();
        }

        tick_count = tick_count.wrapping_add(1);
        ticker.next().await;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Saturate an `i32` to the `i16` range (used for PWM duty clamping).
#[inline]
fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
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

