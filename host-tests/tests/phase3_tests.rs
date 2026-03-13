//! Phase 3 TDD Tests — The Senses (Sensors & Memory Mapping)
//!
//! These tests verify the Phase 3 Host-Guest ABI additions:
//! - Direct [`MockHost::get_pitch_roll`] unit tests
//! - Sensor data bridge: Core 1 writes → Core 0 reads (via [`ImuReading`])
//! - `host_get_pitch_roll` Wasm ABI — happy path and bounds validation
//! - End-to-end: Wasm guest reads sensor data and makes a decision based on it

use abi::{status, ImuReading};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── Direct MockHost unit tests ────────────────────────────────────────────────

#[test]
fn imu_reading_defaults_to_zero() {
    let host = MockHost::default();
    let reading = host.get_pitch_roll();
    assert_eq!(reading.pitch_millideg, 0);
    assert_eq!(reading.roll_millideg, 0);
}

#[test]
fn get_pitch_roll_returns_injected_value() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: 15_000,
        roll_millideg: -8_500,
    };
    let r = host.get_pitch_roll();
    assert_eq!(r.pitch_millideg, 15_000);
    assert_eq!(r.roll_millideg, -8_500);
}

#[test]
fn get_pitch_roll_extreme_values() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: i32::MAX,
        roll_millideg: i32::MIN,
    };
    let r = host.get_pitch_roll();
    assert_eq!(r.pitch_millideg, i32::MAX);
    assert_eq!(r.roll_millideg, i32::MIN);
}

// ── ImuReading encode/decode (ABI contract) ───────────────────────────────────

#[test]
fn imu_reading_encode_decode_roundtrip_positive() {
    let reading = ImuReading {
        pitch_millideg: 45_000,
        roll_millideg: 30_000,
    };
    assert_eq!(ImuReading::decode(reading.encode()), reading);
}

#[test]
fn imu_reading_encode_decode_roundtrip_negative() {
    let reading = ImuReading {
        pitch_millideg: -90_000,
        roll_millideg: -180_000,
    };
    assert_eq!(ImuReading::decode(reading.encode()), reading);
}

#[test]
fn imu_reading_encode_decode_zero() {
    let reading = ImuReading::default();
    assert_eq!(ImuReading::decode(reading.encode()), reading);
}

// ── Wasm-level integration tests ──────────────────────────────────────────────

/// The Wasm guest calls `host_get_pitch_roll` and the values are written into
/// its linear memory at the specified offsets.
#[test]
fn wasm_get_pitch_roll_writes_to_memory() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: 20_000,
        roll_millideg: -5_000,
    };
    let mut harness = WasmHarness::new(host);

    // The Wasm module writes pitch at offset 200 and roll at offset 204,
    // then loads pitch back as the return value so the test can assert on it.
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                i32.const 200
                i32.const 204
                call $get_imu
                drop
                i32.const 200
                i32.load
            )
        )
    "#,
    );

    let pitch_read = harness.call_unit_i32(&instance, "run");
    assert_eq!(pitch_read, 20_000, "pitch should be 20 000 millideg");
}

/// Roll value is correctly written to its pointer.
#[test]
fn wasm_get_pitch_roll_roll_value_correct() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: 1_000,
        roll_millideg: -7_777,
    };
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; Write pitch at 300, roll at 304; load roll back.
            (func (export "run") (result i32)
                i32.const 300
                i32.const 304
                call $get_imu
                drop
                i32.const 304
                i32.load
            )
        )
    "#,
    );

    let roll_read = harness.call_unit_i32(&instance, "run");
    assert_eq!(roll_read, -7_777, "roll should be -7 777 millideg");
}

/// `host_get_pitch_roll` with an out-of-bounds pitch pointer returns ERR_BOUNDS.
#[test]
fn wasm_get_pitch_roll_oob_pitch_ptr_returns_bounds_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; pitch_ptr near the end of memory so 4-byte write overflows.
            (func (export "run") (result i32)
                i32.const 65534
                i32.const 100
                call $get_imu
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_BOUNDS);
}

/// `host_get_pitch_roll` with an out-of-bounds roll pointer returns ERR_BOUNDS.
#[test]
fn wasm_get_pitch_roll_oob_roll_ptr_returns_bounds_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; roll_ptr overflows the page.
            (func (export "run") (result i32)
                i32.const 100
                i32.const 65534
                call $get_imu
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_BOUNDS);
}

/// End-to-end: Wasm guest reads pitch/roll and toggles the LED if pitch > 10°.
#[test]
fn wasm_sensor_triggered_led_toggle() {
    let mut host = MockHost::default();
    // 15° pitch — above the 10° threshold the Wasm module checks.
    host.imu_reading = ImuReading {
        pitch_millideg: 15_000,
        roll_millideg: 0,
    };
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (import "env" "host_toggle_led"
                (func $led (result i32)))
            (memory (export "memory") 1)
            ;; If pitch_millideg > 10000, toggle the LED.
            (func (export "run") (result i32)
                i32.const 400   ;; pitch_ptr
                i32.const 404   ;; roll_ptr
                call $get_imu
                drop
                i32.const 400
                i32.load        ;; load pitch
                i32.const 10000
                i32.gt_s        ;; pitch > 10 000?
                if
                    call $led
                    drop
                end
                i32.const 0     ;; return OK
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    assert!(
        harness.host().led_on,
        "LED should be toggled when pitch > 10°"
    );
}

/// Wasm guest does NOT toggle the LED when pitch is below the threshold.
#[test]
fn wasm_sensor_no_led_toggle_when_pitch_is_low() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: 5_000, // only 5° — below the 10° threshold
        roll_millideg: 0,
    };
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (import "env" "host_toggle_led"
                (func $led (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                i32.const 400
                i32.const 404
                call $get_imu
                drop
                i32.const 400
                i32.load
                i32.const 10000
                i32.gt_s
                if
                    call $led
                    drop
                end
                i32.const 0
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    assert!(
        !harness.host().led_on,
        "LED should remain OFF when pitch <= 10°"
    );
}

/// `host_get_pitch_roll` called back-to-back returns the same (latest) value.
#[test]
fn wasm_get_pitch_roll_multiple_calls_return_same_value() {
    let mut host = MockHost::default();
    host.imu_reading = ImuReading {
        pitch_millideg: 3_000,
        roll_millideg: 1_500,
    };
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_pitch_roll"
                (func $get_imu (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; First call
                i32.const 0
                i32.const 4
                call $get_imu
                drop
                ;; Second call — overwrite same slots
                i32.const 0
                i32.const 4
                call $get_imu
                drop
                ;; Return pitch from second call
                i32.const 0
                i32.load
            )
        )
    "#,
    );

    let pitch = harness.call_unit_i32(&instance, "run");
    assert_eq!(pitch, 3_000);
}
