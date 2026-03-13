//! Phase 6 TDD Tests — The Standardized Communicator (Data Serialization Protocol)
//!
//! These tests verify the Phase 6 additions:
//!
//! - **CDR Serialization**: `ImuTelemetry` and `OdometryTelemetry` serialize and
//!   deserialize correctly through the CDR format used by ROS 2 / DDS.
//! - **Wasm ABI — `host_emit_imu_telemetry`**: The Wasm guest can construct a
//!   CDR payload in its linear memory and push it to the telemetry TX queue.
//! - **Wasm ABI — `host_emit_odom_telemetry`**: Same for odometry packets.
//! - **Wasm ABI — `host_get_telemetry_queue_len`**: The guest can query the
//!   current queue depth for flow-control purposes.
//! - **Queue back-pressure**: Attempting to enqueue beyond the capacity limit
//!   returns `ERR_BUSY` instead of blocking or panicking.
//! - **Packet validation**: Wrong payload sizes return `ERR_BOUNDS`.

use abi::{status, ImuTelemetry, OdometryTelemetry, TelemetryPacket, TELEMETRY_TX_CAPACITY};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── Direct mock-host telemetry tests ─────────────────────────────────────────

#[test]
fn telemetry_queue_is_empty_by_default() {
    let host = MockHost::default();
    assert_eq!(host.telemetry_queue.len(), 0);
    assert_eq!(host.get_telemetry_queue_len(), 0);
}

#[test]
fn emit_imu_telemetry_enqueues_packet() {
    let mut host = MockHost::default();
    let imu = ImuTelemetry {
        sequence: 1,
        pitch_millideg: 5_000,
        roll_millideg: -2_000,
        ..Default::default()
    };
    let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
    imu.to_cdr(&mut buf);
    assert_eq!(host.emit_imu_telemetry(&buf), status::OK);
    assert_eq!(host.telemetry_queue.len(), 1);
}

#[test]
fn emit_imu_telemetry_preserves_fields() {
    let mut host = MockHost::default();
    let original = ImuTelemetry {
        sequence: 99,
        timestamp_us: 123_456,
        loop_time_us: 1_980,
        pitch_millideg: 30_000,
        roll_millideg: -10_000,
        yaw_rate_millideg_s: 500,
        linear_accel_x_mm_s2: 9_800,
        linear_accel_y_mm_s2: 0,
    };
    let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
    original.to_cdr(&mut buf);
    host.emit_imu_telemetry(&buf);

    match host.telemetry_queue[0] {
        TelemetryPacket::Imu(decoded) => assert_eq!(decoded, original),
        _ => panic!("expected ImuTelemetry variant"),
    }
}

#[test]
fn emit_imu_telemetry_wrong_size_returns_err_bounds() {
    let mut host = MockHost::default();
    let short_buf = [0u8; 10];
    assert_eq!(host.emit_imu_telemetry(&short_buf), status::ERR_BOUNDS);
    assert!(host.telemetry_queue.is_empty());
}

#[test]
fn emit_odom_telemetry_enqueues_packet() {
    let mut host = MockHost::default();
    let odom = OdometryTelemetry {
        sequence: 3,
        left_speed: 100,
        right_speed: -80,
        ..Default::default()
    };
    let mut buf = [0u8; OdometryTelemetry::SERIALIZED_SIZE];
    odom.to_cdr(&mut buf);
    assert_eq!(host.emit_odom_telemetry(&buf), status::OK);
    assert_eq!(host.telemetry_queue.len(), 1);
}

#[test]
fn emit_odom_telemetry_preserves_fields() {
    let mut host = MockHost::default();
    let original = OdometryTelemetry {
        sequence: 7,
        timestamp_us: 50_000,
        loop_time_us: 2_005,
        left_speed: 200,
        right_speed: -200,
    };
    let mut buf = [0u8; OdometryTelemetry::SERIALIZED_SIZE];
    original.to_cdr(&mut buf);
    host.emit_odom_telemetry(&buf);

    match host.telemetry_queue[0] {
        TelemetryPacket::Odometry(decoded) => assert_eq!(decoded, original),
        _ => panic!("expected OdometryTelemetry variant"),
    }
}

#[test]
fn emit_odom_telemetry_wrong_size_returns_err_bounds() {
    let mut host = MockHost::default();
    let short_buf = [0u8; 5];
    assert_eq!(host.emit_odom_telemetry(&short_buf), status::ERR_BOUNDS);
    assert!(host.telemetry_queue.is_empty());
}

#[test]
fn get_telemetry_queue_len_increments_per_push() {
    let mut host = MockHost::default();
    let imu = ImuTelemetry::default();
    let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
    imu.to_cdr(&mut buf);

    for i in 1..=5 {
        host.emit_imu_telemetry(&buf);
        assert_eq!(host.get_telemetry_queue_len(), i);
    }
}

#[test]
fn telemetry_queue_back_pressure_returns_err_busy() {
    let mut host = MockHost::default();
    let imu = ImuTelemetry::default();
    let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
    imu.to_cdr(&mut buf);

    // Fill the queue to capacity.
    for _ in 0..TELEMETRY_TX_CAPACITY {
        assert_eq!(host.emit_imu_telemetry(&buf), status::OK);
    }
    // Next push must fail with ERR_BUSY.
    assert_eq!(host.emit_imu_telemetry(&buf), status::ERR_BUSY);
    assert_eq!(host.telemetry_queue.len(), TELEMETRY_TX_CAPACITY);
}

#[test]
fn mixed_telemetry_packets_are_ordered_correctly() {
    let mut host = MockHost::default();
    let imu = ImuTelemetry {
        sequence: 1,
        ..Default::default()
    };
    let odom = OdometryTelemetry {
        sequence: 2,
        ..Default::default()
    };

    let mut imu_buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
    imu.to_cdr(&mut imu_buf);
    let mut odom_buf = [0u8; OdometryTelemetry::SERIALIZED_SIZE];
    odom.to_cdr(&mut odom_buf);

    host.emit_imu_telemetry(&imu_buf);
    host.emit_odom_telemetry(&odom_buf);

    assert_eq!(host.telemetry_queue.len(), 2);
    assert!(matches!(host.telemetry_queue[0], TelemetryPacket::Imu(_)));
    assert!(matches!(
        host.telemetry_queue[1],
        TelemetryPacket::Odometry(_)
    ));
}

// ── Wasm ABI integration tests ────────────────────────────────────────────────

/// The Wasm guest builds an ImuTelemetry CDR payload in its linear memory and
/// emits it via `host_emit_imu_telemetry`.
#[test]
fn wasm_emit_imu_telemetry_enqueues_packet() {
    let mut harness = WasmHarness::new(MockHost::default());

    // Build a WAT module that:
    //  1. Declares 1 page (64 KiB) of linear memory.
    //  2. Stores an ImuTelemetry CDR payload (36 bytes) at address 0.
    //  3. Calls host_emit_imu_telemetry(0, 36) and returns the result.
    //
    // The payload encodes:
    //  sequence=1  (0x01000000)
    //  timestamp_us=0, loop_time_us=0, pitch=15000 (0x703A0000),
    //  roll=-3500 (0x14F2FFFF), all others = 0.
    //  pitch  15000 = 0x00003A98 → LE bytes 98 3A 00 00
    //  roll  -3500  = 0xFFFFF274 → LE bytes 74 F2 FF FF
    let wat_src = r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; Bytes [0..4]: sequence = 1 (u32 LE)
                (i32.store8 (i32.const 0)  (i32.const 1))
                (i32.store8 (i32.const 1)  (i32.const 0))
                (i32.store8 (i32.const 2)  (i32.const 0))
                (i32.store8 (i32.const 3)  (i32.const 0))
                ;; Bytes [4..12]: timestamp_us = 0 (u64 LE) — already zero
                ;; Bytes [12..16]: loop_time_us = 2000 (u32 LE)
                (i32.store8 (i32.const 12) (i32.const 0xD0))
                (i32.store8 (i32.const 13) (i32.const 0x07))
                (i32.store8 (i32.const 14) (i32.const 0))
                (i32.store8 (i32.const 15) (i32.const 0))
                ;; Bytes [16..20]: pitch = 15000 (i32 LE) 0x00003A98
                (i32.store8 (i32.const 16) (i32.const 0x98))
                (i32.store8 (i32.const 17) (i32.const 0x3A))
                (i32.store8 (i32.const 18) (i32.const 0))
                (i32.store8 (i32.const 19) (i32.const 0))
                ;; Bytes [20..24]: roll = -3500 (i32 LE) 0xFFFFF254
                (i32.store8 (i32.const 20) (i32.const 0x54))
                (i32.store8 (i32.const 21) (i32.const 0xF2))
                (i32.store8 (i32.const 22) (i32.const 0xFF))
                (i32.store8 (i32.const 23) (i32.const 0xFF))
                ;; Bytes [24..36]: remaining fields = 0 — already zero
                ;; Call host_publish(topic=104, ptr=0, len=36)
                (call $publish (i32.const 104) (i32.const 0) (i32.const 36))
            )
        )
    "#;

    let instance = harness.load_wat(wat_src);
    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);

    let host = harness.host();
    assert_eq!(host.telemetry_queue.len(), 1);
    match host.telemetry_queue[0] {
        TelemetryPacket::Imu(imu) => {
            assert_eq!(imu.sequence, 1);
            assert_eq!(imu.loop_time_us, 2000);
            assert_eq!(imu.pitch_millideg, 15_000);
            assert_eq!(imu.roll_millideg, -3_500);
        }
        _ => panic!("expected ImuTelemetry variant"),
    }
}

/// The Wasm guest emits an odometry packet via `host_emit_odom_telemetry`.
#[test]
fn wasm_emit_odom_telemetry_enqueues_packet() {
    let mut harness = WasmHarness::new(MockHost::default());

    // OdometryTelemetry layout (20 bytes):
    // [0..4]   sequence=2  → 02 00 00 00
    // [4..12]  timestamp_us=0
    // [12..16] loop_time_us=2000  → D0 07 00 00
    // [16..18] left_speed=120   → 78 00
    // [18..20] right_speed=-80  → B0 FF
    let wat_src = r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; sequence = 2
                (i32.store8 (i32.const 0)  (i32.const 2))
                (i32.store8 (i32.const 1)  (i32.const 0))
                (i32.store8 (i32.const 2)  (i32.const 0))
                (i32.store8 (i32.const 3)  (i32.const 0))
                ;; timestamp_us = 0 [4..12] already zeroed
                ;; loop_time_us = 2000
                (i32.store8 (i32.const 12) (i32.const 0xD0))
                (i32.store8 (i32.const 13) (i32.const 0x07))
                (i32.store8 (i32.const 14) (i32.const 0))
                (i32.store8 (i32.const 15) (i32.const 0))
                ;; left_speed = 120 (i16 LE)
                (i32.store8 (i32.const 16) (i32.const 120))
                (i32.store8 (i32.const 17) (i32.const 0))
                ;; right_speed = -80 = 0xFFB0 (i16 LE)
                (i32.store8 (i32.const 18) (i32.const 0xB0))
                (i32.store8 (i32.const 19) (i32.const 0xFF))
                ;; Call host_publish(topic=105, ptr=0, len=20)
                (call $publish (i32.const 105) (i32.const 0) (i32.const 20))
            )
        )
    "#;

    let instance = harness.load_wat(wat_src);
    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);

    let host = harness.host();
    assert_eq!(host.telemetry_queue.len(), 1);
    match host.telemetry_queue[0] {
        TelemetryPacket::Odometry(odom) => {
            assert_eq!(odom.sequence, 2);
            assert_eq!(odom.loop_time_us, 2000);
            assert_eq!(odom.left_speed, 120);
            assert_eq!(odom.right_speed, -80);
        }
        _ => panic!("expected OdometryTelemetry variant"),
    }
}

/// Wrong payload size returns ERR_BOUNDS from the Wasm ABI.
#[test]
fn wasm_emit_imu_telemetry_wrong_size_returns_err_bounds() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; Pass len=10 instead of 36 — should return ERR_BOUNDS
                (call $publish (i32.const 104) (i32.const 0) (i32.const 10))
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_BOUNDS);
    assert!(harness.host().telemetry_queue.is_empty());
}

/// `host_get_telemetry_queue_len` reports the current queue depth to the guest.
#[test]
fn wasm_get_telemetry_queue_len_reflects_push_count() {
    let mut harness = WasmHarness::new(MockHost::default());

    // First, query length before any push — must be 0.
    let query_instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_telemetry_queue_len"
                (func $qlen (result i32)))
            (func (export "run") (result i32)
                (call $qlen)
            )
        )
    "#,
    );
    let len_before = harness.call_unit_i32(&query_instance, "run");
    assert_eq!(len_before, 0);

    // Push two IMU packets via MockHost directly.
    {
        let mut host = harness.host_mut();
        let imu = ImuTelemetry::default();
        let mut buf = [0u8; ImuTelemetry::SERIALIZED_SIZE];
        imu.to_cdr(&mut buf);
        host.emit_imu_telemetry(&buf);
        host.emit_imu_telemetry(&buf);
    }

    // Re-query: must now report 2.
    let len_after = harness.call_unit_i32(&query_instance, "run");
    assert_eq!(len_after, 2);
}

/// Wasm out-of-bounds pointer for emit_imu_telemetry returns ERR_BOUNDS.
#[test]
fn wasm_emit_imu_telemetry_out_of_bounds_ptr_returns_err_bounds() {
    let mut harness = WasmHarness::new(MockHost::default());

    // Memory is 1 page = 65536 bytes.
    // ptr=65510, len=36 → end = 65546 > 65536 → should return ERR_BOUNDS.
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                (call $publish (i32.const 104) (i32.const 65510) (i32.const 36))
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::ERR_BOUNDS);
}
