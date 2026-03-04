//! Discrete PID controller for the Core 1 balance loop.
//!
//! Uses `f32` arithmetic — the ESP32-S3's Xtensa LX7 core has a single-
//! precision FPU so floating-point is as cheap as integer math here.
//!
//! ## Usage (Core 1 real-time task)
//!
//! ```rust,ignore
//! let mut pid = PidController::new(10.0, 0.5, 0.1, -255.0, 255.0);
//!
//! loop {
//!     let pitch = imu_read_pitch();           // degrees
//!     let output = pid.update(0.0, pitch, DT_S); // setpoint = 0 (upright)
//!     motor_set_duty(output as i16);
//! }
//! ```

/// A discrete proportional-integral-derivative (PID) controller.
///
/// All coefficients and state variables use `f32` to take advantage of the
/// ESP32-S3 hardware FPU.  The integral is clamped to `[output_min / ki,
/// output_max / ki]` to prevent integrator windup.
pub struct PidController {
    /// Proportional gain.
    kp: f32,
    /// Integral gain.
    ki: f32,
    /// Derivative gain.
    kd: f32,
    /// Accumulated error × dt sum (integral term).
    integral: f32,
    /// Error value from the previous [`update`] call (for the derivative term).
    prev_error: f32,
    /// Minimum allowed output value.
    output_min: f32,
    /// Maximum allowed output value.
    output_max: f32,
}

impl PidController {
    /// Create a new PID controller.
    ///
    /// # Arguments
    ///
    /// * `kp`         — Proportional gain.
    /// * `ki`         — Integral gain.
    /// * `kd`         — Derivative gain.
    /// * `output_min` — Minimum clamped output (e.g. `-255.0` for PWM duty).
    /// * `output_max` — Maximum clamped output (e.g. `+255.0` for PWM duty).
    pub const fn new(
        kp: f32,
        ki: f32,
        kd: f32,
        output_min: f32,
        output_max: f32,
    ) -> Self {
        Self {
            kp,
            ki,
            kd,
            integral: 0.0,
            prev_error: 0.0,
            output_min,
            output_max,
        }
    }

    /// Compute one PID step and return the control output.
    ///
    /// # Arguments
    ///
    /// * `setpoint` — Desired value (0.0 = upright for a balancing robot).
    /// * `measured` — Current measured value (e.g. pitch in degrees).
    /// * `dt_s`     — Time elapsed since the last call, in seconds.
    ///
    /// Returns the control output clamped to `[output_min, output_max]`.
    pub fn update(&mut self, setpoint: f32, measured: f32, dt_s: f32) -> f32 {
        let error = setpoint - measured;

        // Integrate with anti-windup clamping.
        self.integral += error * dt_s;
        if self.ki.abs() > f32::EPSILON {
            let windup_limit = self.output_max / self.ki;
            self.integral = self.integral.clamp(-windup_limit, windup_limit);
        }

        // Derivative on measurement to avoid "derivative kick" on setpoint jumps.
        let derivative = if dt_s > f32::EPSILON {
            (error - self.prev_error) / dt_s
        } else {
            0.0
        };
        self.prev_error = error;

        let output = self.kp * error + self.ki * self.integral + self.kd * derivative;
        output.clamp(self.output_min, self.output_max)
    }

    /// Reset the integral accumulator and the previous-error register.
    ///
    /// Call this whenever the controller is disabled (e.g., after a safe
    /// shutdown) to prevent a large integral from causing a jerk when
    /// control resumes.
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
    }
}
