//! Phase 4 TDD Tests — The Muscle & Survival (Motors & Fault Tolerance)
//!
//! These tests verify the Phase 4 Host-Guest ABI additions and safety logic:
//!
//! - Direct [`MockHost::set_motor_speed`] unit tests (range validation, safe shutdown)
//! - `host_set_motor_speed` Wasm ABI — happy path and bounds validation
//! - Sandbox isolation: Wasm trap (`unreachable`) does not crash the Host OS
//! - Safe shutdown: motor commands are rejected when `motors_enabled` is false
//! - Watchdog feed counter increments on successful motor commands
//! - End-to-end: Wasm guest reads IMU, sets motor speed based on pitch

use abi::{status, ImuReading, MAX_MOTOR_SPEED};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── Direct MockHost unit tests ────────────────────────────────────────────────

#[test]
fn motor_speeds_default_to_zero() {
    let host = MockHost::default();
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

#[test]
fn motors_enabled_by_default() {
    let host = MockHost::default();
    assert!(host.motors_enabled);
}

#[test]
fn set_motor_speed_stores_speeds() {
    let mut host = MockHost::default();
    let result = host.set_motor_speed(100, -150);
    assert_eq!(result, status::OK);
    assert_eq!(host.motor_left_speed, 100);
    assert_eq!(host.motor_right_speed, -150);
}

#[test]
fn set_motor_speed_max_values_accepted() {
    let mut host = MockHost::default();
    assert_eq!(
        host.set_motor_speed(MAX_MOTOR_SPEED, MAX_MOTOR_SPEED),
        status::OK
    );
    assert_eq!(host.motor_left_speed, MAX_MOTOR_SPEED);
    assert_eq!(host.motor_right_speed, MAX_MOTOR_SPEED);
}

#[test]
fn set_motor_speed_min_values_accepted() {
    let mut host = MockHost::default();
    assert_eq!(
        host.set_motor_speed(-MAX_MOTOR_SPEED, -MAX_MOTOR_SPEED),
        status::OK
    );
    assert_eq!(host.motor_left_speed, -MAX_MOTOR_SPEED);
    assert_eq!(host.motor_right_speed, -MAX_MOTOR_SPEED);
}

#[test]
fn set_motor_speed_zero_values_accepted() {
    let mut host = MockHost::default();
    // Start with non-zero speeds.
    host.set_motor_speed(50, -50);
    // Emergency stop.
    let result = host.set_motor_speed(0, 0);
    assert_eq!(result, status::OK);
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

#[test]
fn set_motor_speed_left_exceeds_max_returns_error() {
    let mut host = MockHost::default();
    let result = host.set_motor_speed(MAX_MOTOR_SPEED + 1, 0);
    assert_eq!(result, status::ERR_INVALID_ARG);
    // Speeds must not have changed.
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

#[test]
fn set_motor_speed_right_exceeds_min_returns_error() {
    let mut host = MockHost::default();
    let result = host.set_motor_speed(0, -(MAX_MOTOR_SPEED + 1));
    assert_eq!(result, status::ERR_INVALID_ARG);
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

// ── Safe-shutdown / motor enable flag ────────────────────────────────────────

#[test]
fn set_motor_speed_blocked_when_motors_disabled() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    let result = host.set_motor_speed(100, 100);
    assert_eq!(result, status::ERR_BUSY);
    // Speeds must remain unchanged (0).
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

#[test]
fn set_motor_speed_accepted_after_motors_re_enabled() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    host.set_motor_speed(100, 100); // rejected
    host.motors_enabled = true;
    let result = host.set_motor_speed(80, -80); // should be accepted now
    assert_eq!(result, status::OK);
    assert_eq!(host.motor_left_speed, 80);
    assert_eq!(host.motor_right_speed, -80);
}

// ── Watchdog feed counter ─────────────────────────────────────────────────────

#[test]
fn watchdog_feed_count_starts_at_zero() {
    let host = MockHost::default();
    assert_eq!(host.watchdog_feed_count, 0);
}

#[test]
fn watchdog_fed_on_successful_motor_command() {
    let mut host = MockHost::default();
    host.set_motor_speed(50, 50);
    assert_eq!(host.watchdog_feed_count, 1);
}

#[test]
fn watchdog_not_fed_on_invalid_motor_command() {
    let mut host = MockHost::default();
    host.set_motor_speed(1000, 0); // Invalid — exceeds MAX_MOTOR_SPEED
    assert_eq!(host.watchdog_feed_count, 0);
}

#[test]
fn watchdog_not_fed_when_motors_disabled() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    host.set_motor_speed(50, 50); // Rejected — safe shutdown active
    assert_eq!(host.watchdog_feed_count, 0);
}

#[test]
fn watchdog_feed_count_accumulates_over_multiple_commands() {
    let mut host = MockHost::default();
    for _ in 0..5 {
        host.set_motor_speed(10, -10);
    }
    assert_eq!(host.watchdog_feed_count, 5);
}

// ── Wasm ABI integration tests ────────────────────────────────────────────────

/// The Wasm guest calls `host_set_motor_speed` with valid speeds and receives OK.
#[test]
fn wasm_set_motor_speed_valid() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 120   ;; left speed
                i32.const -80   ;; right speed
                call $set_speed
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    assert_eq!(harness.host().motor_left_speed, 120);
    assert_eq!(harness.host().motor_right_speed, -80);
}

/// The Wasm guest calls `host_set_motor_speed` with MAX/MIN boundary values.
#[test]
fn wasm_set_motor_speed_boundary_values() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 255   ;; MAX_MOTOR_SPEED
                i32.const -255  ;; -MAX_MOTOR_SPEED
                call $set_speed
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    assert_eq!(harness.host().motor_left_speed, 255);
    assert_eq!(harness.host().motor_right_speed, -255);
}

/// `host_set_motor_speed` with an out-of-range left speed returns ERR_INVALID_ARG.
#[test]
fn wasm_set_motor_speed_left_out_of_range_returns_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 256   ;; Exceeds MAX_MOTOR_SPEED by 1
                i32.const 0
                call $set_speed
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_INVALID_ARG);
    assert_eq!(
        harness.host().motor_left_speed,
        0,
        "left speed must not change on error"
    );
    assert_eq!(
        harness.host().motor_right_speed,
        0,
        "right speed must not change on error"
    );
}

/// `host_set_motor_speed` with an out-of-range right speed returns ERR_INVALID_ARG.
#[test]
fn wasm_set_motor_speed_right_out_of_range_returns_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 0
                i32.const -300  ;; Exceeds -MAX_MOTOR_SPEED
                call $set_speed
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_INVALID_ARG);
}

/// When `motors_enabled = false` (ULP safe-shutdown) the ABI returns ERR_BUSY.
#[test]
fn wasm_set_motor_speed_blocked_by_safe_shutdown() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 100
                i32.const 100
                call $set_speed
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_BUSY);
    assert_eq!(
        harness.host().motor_left_speed,
        0,
        "left speed must remain 0 during shutdown"
    );
    assert_eq!(
        harness.host().motor_right_speed,
        0,
        "right speed must remain 0 during shutdown"
    );
}

// ── Sandbox isolation (Chaos Test) ────────────────────────────────────────────

/// **Chaos Test part 1 — Wasm trap.**
///
/// A malicious (or buggy) Wasm guest that hits an `unreachable` instruction
/// triggers a trap.  The Host OS must not crash; `wasmi` returns an error and
/// the sandbox stays alive to handle the next command.
///
/// On real hardware this is complemented by the WDT: an infinite loop would
/// prevent the post-command feed, causing a Core 0 reset that leaves the
/// Core 1 balance loop untouched.
#[test]
fn wasm_trap_does_not_crash_host() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (func (export "crash") (result i32)
                unreachable
            )
        )
    "#,
    );

    // The trap must be caught by `wasmi` and returned as an Err.
    let result = harness.try_call_unit_i32(&instance, "crash");
    assert!(
        result.is_err(),
        "a Wasm unreachable must produce a trap error"
    );

    // The harness (and by extension the Host OS) must still be usable.
    // Verify by issuing a valid ABI call after the trap.
    let toggle_instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_toggle_led" (func $led (result i32)))
            (func (export "run") (result i32) call $led)
        )
    "#,
    );
    let led_result = harness.call_unit_i32(&toggle_instance, "run");
    assert_eq!(led_result, status::OK);
    assert!(
        harness.host().led_on,
        "LED should toggle after a preceding Wasm trap"
    );
}

/// **Chaos Test part 2 — Motor state is preserved across a Wasm trap.**
///
/// Core 1 had already received a motor command before the trap.  After the
/// trap the last valid command must still be stored in the host state.
#[test]
fn motor_state_preserved_after_wasm_trap() {
    let mut harness = WasmHarness::new(MockHost::default());

    // 1. Issue a valid motor command.
    let motor_instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 200
                i32.const 200
                call $set_speed
            )
        )
    "#,
    );
    harness.call_unit_i32(&motor_instance, "run");
    assert_eq!(harness.host().motor_left_speed, 200);

    // 2. Load a malicious module that traps immediately.
    let crash_instance = harness.load_wat(
        r#"
        (module
            (func (export "crash") (result i32)
                unreachable
            )
        )
    "#,
    );
    let _ = harness.try_call_unit_i32(&crash_instance, "crash");

    // 3. Motor speed must still reflect the last *valid* command.
    assert_eq!(
        harness.host().motor_left_speed,
        200,
        "motor speed must be preserved after a Wasm trap"
    );
}

// ── End-to-end: IMU → PID → motor ─────────────────────────────────────────────

/// End-to-end: Wasm guest reads pitch from the IMU and sets motor speed
/// proportional to tilt.  Simulates a simplified balance controller in Wasm.
#[test]
fn wasm_imu_to_motor_speed_pipeline() {
    let mut host = MockHost::default();
    // Simulate 10° of forward tilt (10 000 millideg).
    host.imu_reading = ImuReading {
        pitch_millideg: 10_000,
        roll_millideg: 0,
    };
    let mut harness = WasmHarness::new(host);

    // A simplified Wasm "balance controller":
    // - reads pitch via host_get_pitch_roll
    // - divides by 100 to get a PWM duty in the ±100 range
    // - sets both motors to that duty (symmetric for pure balance)
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $imu (param i32 i32) (result i32)))
            (import "env" "host_set_motor_speed"
                (func $motors (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; pitch at offset 0, roll at offset 4
            (func (export "run") (result i32)
                (local $duty i32)
                i32.const 0
                i32.const 4
                call $imu
                drop
                ;; load pitch (i32), divide by 100 → duty
                i32.const 0
                i32.load
                i32.const 100
                i32.div_s
                ;; set same duty for both motors
                local.tee $duty
                local.get $duty
                call $motors
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    // 10 000 / 100 = 100
    assert_eq!(harness.host().motor_left_speed, 100);
    assert_eq!(harness.host().motor_right_speed, 100);
}

/// Wasm guest calls emergency stop (both speeds = 0) after detecting level IMU.
#[test]
fn wasm_emergency_stop_on_level_imu() {
    let mut host = MockHost::default();
    // Pre-set non-zero speeds.
    host.motor_left_speed = 50;
    host.motor_right_speed = 50;
    host.imu_reading = ImuReading {
        pitch_millideg: 0,
        roll_millideg: 0,
    };
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $motors (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 0
                i32.const 0
                call $motors
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    assert_eq!(harness.host().motor_left_speed, 0);
    assert_eq!(harness.host().motor_right_speed, 0);
}
