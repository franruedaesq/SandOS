//! Phase 5 TDD Tests — The Nervous System Expansion (Distributed Robotics)
//!
//! These tests verify the Phase 5 additions:
//!
//! - [`WorkerPacket`] encoding / decoding (motor speed, heartbeat, emergency stop)
//! - Brain Mock: `set_motor_speed` now enqueues an outgoing Worker packet
//! - Dead-man's switch logic: motors halt when no heartbeat arrives in time
//! - Wasm ABI: the full pipeline Wasm → `host_set_motor_speed` → Worker packet
//! - Worker packet decode validates magic header and payload bounds

use abi::{
    deadman_triggered, status, worker_cmd, EspNowCommand, WorkerPacket, HEARTBEAT_INTERVAL_MS,
    WORKER_TIMEOUT_MS, MAX_MOTOR_SPEED,
};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── WorkerPacket encoding / decoding ─────────────────────────────────────────

#[test]
fn worker_packet_motor_speed_has_correct_header() {
    let pkt = WorkerPacket::motor_speed(100, -50);
    assert_eq!(pkt[0], EspNowCommand::MAGIC[0], "magic byte 0 must match");
    assert_eq!(pkt[1], EspNowCommand::MAGIC[1], "magic byte 1 must match");
    assert_eq!(pkt[2], worker_cmd::MOTOR_SPEED, "cmd_id must be MOTOR_SPEED");
    assert_eq!(pkt[3], 4, "payload_len must be 4");
}

#[test]
fn worker_packet_motor_speed_encodes_values_big_endian() {
    // 100 in big-endian i16 = [0x00, 0x64]; -50 = [0xFF, 0xCE]
    let pkt = WorkerPacket::motor_speed(100, -50);
    let left  = i16::from_be_bytes([pkt[4], pkt[5]]);
    let right = i16::from_be_bytes([pkt[6], pkt[7]]);
    assert_eq!(left, 100);
    assert_eq!(right, -50);
}

#[test]
fn worker_packet_heartbeat_has_zero_payload_len() {
    let pkt = WorkerPacket::heartbeat();
    assert_eq!(pkt[2], worker_cmd::HEARTBEAT);
    assert_eq!(pkt[3], 0);
    assert_eq!(pkt.len(), 4, "heartbeat packet must be exactly 4 bytes");
}

#[test]
fn worker_packet_emergency_stop_format() {
    let pkt = WorkerPacket::emergency_stop();
    assert_eq!(pkt[2], worker_cmd::EMERGENCY_STOP);
    assert_eq!(pkt[3], 0);
}

#[test]
fn worker_packet_decode_motor_speed_roundtrip() {
    let original = WorkerPacket::motor_speed(200, -200);
    let (cmd, payload) = WorkerPacket::decode(&original).expect("decode must succeed");
    assert_eq!(cmd, worker_cmd::MOTOR_SPEED);
    let (l, r) = WorkerPacket::parse_motor_speed(payload).expect("parse must succeed");
    assert_eq!(l, 200);
    assert_eq!(r, -200);
}

#[test]
fn worker_packet_decode_heartbeat_roundtrip() {
    let pkt = WorkerPacket::heartbeat();
    let (cmd, payload) = WorkerPacket::decode(&pkt).expect("decode must succeed");
    assert_eq!(cmd, worker_cmd::HEARTBEAT);
    assert!(payload.is_empty(), "heartbeat payload must be empty");
}

#[test]
fn worker_packet_decode_rejects_too_short() {
    assert!(WorkerPacket::decode(&[]).is_none());
    assert!(WorkerPacket::decode(&[0x5A]).is_none());
    assert!(WorkerPacket::decode(&[0x5A, 0x4E, 0x30]).is_none());
}

#[test]
fn worker_packet_decode_rejects_bad_magic() {
    let bad = [0x00, 0x00, worker_cmd::MOTOR_SPEED, 0x00];
    assert!(WorkerPacket::decode(&bad).is_none());
}

#[test]
fn worker_packet_decode_rejects_payload_length_overflow() {
    // Claims 10 bytes of payload but only 2 extra bytes are present.
    let bad = [
        EspNowCommand::MAGIC[0],
        EspNowCommand::MAGIC[1],
        worker_cmd::MOTOR_SPEED,
        0x0A, // payload_len = 10
        0x00,
        0x01, // only 2 payload bytes
    ];
    assert!(WorkerPacket::decode(&bad).is_none());
}

#[test]
fn worker_packet_parse_motor_speed_rejects_short_payload() {
    assert!(WorkerPacket::parse_motor_speed(&[]).is_none());
    assert!(WorkerPacket::parse_motor_speed(&[0x00, 0x01, 0x00]).is_none()); // 3 bytes, needs 4
}

// ── Brain mock: outgoing Worker packet queue ──────────────────────────────────

#[test]
fn mock_host_outgoing_worker_cmds_starts_empty() {
    let host = MockHost::default();
    assert!(
        host.outgoing_worker_cmds.is_empty(),
        "no outgoing commands before any set_motor_speed call"
    );
}

#[test]
fn mock_host_set_motor_speed_enqueues_worker_packet() {
    let mut host = MockHost::default();
    host.set_motor_speed(120, -80);
    assert_eq!(host.outgoing_worker_cmds.len(), 1);
    let pkt = host.outgoing_worker_cmds[0];
    let (cmd, payload) = WorkerPacket::decode(&pkt).expect("must decode");
    assert_eq!(cmd, worker_cmd::MOTOR_SPEED);
    let (l, r) = WorkerPacket::parse_motor_speed(payload).unwrap();
    assert_eq!(l, 120);
    assert_eq!(r, -80);
}

#[test]
fn mock_host_set_motor_speed_no_worker_packet_on_invalid_args() {
    let mut host = MockHost::default();
    // Left speed exceeds MAX_MOTOR_SPEED — must not enqueue.
    let result = host.set_motor_speed(MAX_MOTOR_SPEED + 1, 0);
    assert_eq!(result, status::ERR_INVALID_ARG);
    assert!(
        host.outgoing_worker_cmds.is_empty(),
        "no packet must be queued when args are invalid"
    );
}

#[test]
fn mock_host_set_motor_speed_no_worker_packet_when_motors_disabled() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    let result = host.set_motor_speed(100, 100);
    assert_eq!(result, status::ERR_BUSY);
    assert!(
        host.outgoing_worker_cmds.is_empty(),
        "no packet must be queued when motors are disabled"
    );
}

#[test]
fn mock_host_worker_packets_accumulate_over_multiple_calls() {
    let mut host = MockHost::default();
    host.set_motor_speed(10, 10);
    host.set_motor_speed(20, -20);
    host.set_motor_speed(0, 0);
    assert_eq!(host.outgoing_worker_cmds.len(), 3);
}

// ── Dead-man's switch logic ───────────────────────────────────────────────────

#[test]
fn deadman_switch_not_triggered_before_timeout() {
    assert!(!deadman_triggered(0, WORKER_TIMEOUT_MS));
    assert!(!deadman_triggered(WORKER_TIMEOUT_MS - 1, WORKER_TIMEOUT_MS));
}

#[test]
fn deadman_switch_triggers_at_timeout() {
    assert!(deadman_triggered(WORKER_TIMEOUT_MS, WORKER_TIMEOUT_MS));
}

#[test]
fn deadman_switch_triggers_after_timeout() {
    assert!(deadman_triggered(WORKER_TIMEOUT_MS + 1, WORKER_TIMEOUT_MS));
    assert!(deadman_triggered(1_000, WORKER_TIMEOUT_MS));
}

#[test]
fn heartbeat_interval_keeps_deadman_switch_alive() {
    // Simulate N consecutive heartbeats arriving exactly at HEARTBEAT_INTERVAL_MS.
    // Each one resets elapsed to 0.  The switch must never trigger.
    for _ in 0..100 {
        assert!(
            !deadman_triggered(HEARTBEAT_INTERVAL_MS, WORKER_TIMEOUT_MS),
            "switch must NOT trigger when heartbeat arrives at interval boundary"
        );
    }
}

#[test]
fn single_missed_heartbeat_triggers_switch() {
    // If the Brain goes silent after exactly one heartbeat interval the elapsed
    // time jumps straight past WORKER_TIMEOUT_MS.
    assert!(
        deadman_triggered(WORKER_TIMEOUT_MS, WORKER_TIMEOUT_MS),
        "switch MUST trigger when elapsed == timeout"
    );
}

// ── Worker state machine ──────────────────────────────────────────────────────

/// Minimal simulation of the Worker's ESP-NOW receive loop for unit testing.
struct WorkerSim {
    /// Whether motors are currently enabled.
    pub enabled: bool,
    /// Current motor speeds.
    pub left: i16,
    pub right: i16,
    /// Simulated elapsed time since last packet (ms).
    pub elapsed_ms: u64,
}

impl WorkerSim {
    fn new() -> Self {
        Self { enabled: true, left: 0, right: 0, elapsed_ms: 0 }
    }

    /// Simulate receiving a valid packet from the Brain.
    fn receive_packet(&mut self, data: &[u8]) {
        let Some((cmd, payload)) = WorkerPacket::decode(data) else { return; };
        // Any valid packet resets the dead-man's switch.
        self.elapsed_ms = 0;
        self.enabled = true;
        match cmd {
            worker_cmd::MOTOR_SPEED => {
                if let Some((l, r)) = WorkerPacket::parse_motor_speed(payload) {
                    self.left  = l;
                    self.right = r;
                }
            }
            worker_cmd::HEARTBEAT => {}
            worker_cmd::EMERGENCY_STOP => {
                self.enabled = false;
                self.left  = 0;
                self.right = 0;
            }
            _ => {}
        }
    }

    /// Simulate the passage of time without a packet.  Triggers dead-man's
    /// switch if elapsed exceeds the timeout.
    fn advance_time(&mut self, ms: u64) {
        self.elapsed_ms += ms;
        if deadman_triggered(self.elapsed_ms, WORKER_TIMEOUT_MS) {
            self.enabled = false;
            self.left  = 0;
            self.right = 0;
        }
    }
}

#[test]
fn worker_receives_motor_speed_and_stores_it() {
    let mut sim = WorkerSim::new();
    let pkt = WorkerPacket::motor_speed(150, -100);
    sim.receive_packet(&pkt);
    assert!(sim.enabled);
    assert_eq!(sim.left, 150);
    assert_eq!(sim.right, -100);
}

#[test]
fn worker_heartbeat_keeps_motors_running() {
    let mut sim = WorkerSim::new();
    sim.left  = 200;
    sim.right = 200;

    // Advance time to just before the timeout using repeated heartbeats.
    for _ in 0..10 {
        let hb = WorkerPacket::heartbeat();
        sim.receive_packet(&hb);
        sim.advance_time(HEARTBEAT_INTERVAL_MS);
    }

    assert!(sim.enabled, "motors must still be enabled after regular heartbeats");
    assert_eq!(sim.left,  200, "motor speeds must be preserved");
    assert_eq!(sim.right, 200);
}

#[test]
fn worker_stops_on_brain_silence() {
    let mut sim = WorkerSim::new();
    let pkt = WorkerPacket::motor_speed(100, 100);
    sim.receive_packet(&pkt);
    assert_eq!(sim.left, 100);

    // Brain goes silent for longer than the timeout.
    sim.advance_time(WORKER_TIMEOUT_MS);

    assert!(!sim.enabled, "dead-man's switch must disable motors");
    assert_eq!(sim.left,  0, "left motor must be zeroed");
    assert_eq!(sim.right, 0, "right motor must be zeroed");
}

#[test]
fn worker_resumes_after_brain_reconnects() {
    let mut sim = WorkerSim::new();

    // Brain goes silent.
    sim.advance_time(WORKER_TIMEOUT_MS);
    assert!(!sim.enabled);

    // Brain comes back and sends a new motor command.
    let pkt = WorkerPacket::motor_speed(50, 50);
    sim.receive_packet(&pkt);
    assert!(sim.enabled, "motors must re-enable when Brain reconnects");
    assert_eq!(sim.left,  50);
    assert_eq!(sim.right, 50);
}

#[test]
fn worker_emergency_stop_disables_motors_immediately() {
    let mut sim = WorkerSim::new();
    sim.left  = 255;
    sim.right = 255;

    let e_stop = WorkerPacket::emergency_stop();
    sim.receive_packet(&e_stop);

    assert!(!sim.enabled, "emergency stop must disable motors");
    assert_eq!(sim.left,  0);
    assert_eq!(sim.right, 0);
}

#[test]
fn worker_ignores_malformed_packets() {
    let mut sim = WorkerSim::new();
    sim.receive_packet(&[]); // too short
    sim.receive_packet(&[0xFF, 0xFF, 0x00, 0x00]); // bad magic
    // elapsed stays at 0 since no time has advanced — motors still enabled.
    assert!(sim.enabled);
}

// ── Wasm ABI → Worker packet pipeline ────────────────────────────────────────

/// End-to-end: Wasm calls `host_set_motor_speed` → Brain queues a Worker packet.
#[test]
fn wasm_set_motor_speed_produces_outgoing_worker_packet() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 75
                i32.const -75
                call $set_speed
            )
        )
    "#);

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);

    // Phase 4 compat: speeds are still tracked in the mock.
    assert_eq!(harness.host().motor_left_speed, 75);
    assert_eq!(harness.host().motor_right_speed, -75);

    // Phase 5: a WorkerPacket must have been enqueued.
    let cmds = &harness.host().outgoing_worker_cmds;
    assert_eq!(cmds.len(), 1, "exactly one Worker packet must be enqueued");
    let (cmd, payload) = WorkerPacket::decode(&cmds[0]).unwrap();
    assert_eq!(cmd, worker_cmd::MOTOR_SPEED);
    let (l, r) = WorkerPacket::parse_motor_speed(payload).unwrap();
    assert_eq!(l, 75);
    assert_eq!(r, -75);
}

/// Wasm emergency stop (0, 0) still enqueues a Worker packet with zero speeds.
#[test]
fn wasm_emergency_stop_enqueues_zero_speed_worker_packet() {
    let mut host = MockHost::default();
    host.motor_left_speed  = 200;
    host.motor_right_speed = 200;
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "run") (result i32)
                i32.const 0
                i32.const 0
                call $set_speed
            )
        )
    "#);

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);

    let cmds = &harness.host().outgoing_worker_cmds;
    assert_eq!(cmds.len(), 1);
    let (_, payload) = WorkerPacket::decode(&cmds[0]).unwrap();
    let (l, r) = WorkerPacket::parse_motor_speed(payload).unwrap();
    assert_eq!(l, 0);
    assert_eq!(r, 0);
}

/// Multiple Wasm motor commands produce multiple Worker packets in order.
#[test]
fn wasm_multiple_motor_commands_produce_ordered_worker_packets() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_set_motor_speed"
                (func $set_speed (param i32 i32) (result i32)))
            (func (export "cmd1") (result i32)
                i32.const 100 i32.const 100 call $set_speed)
            (func (export "cmd2") (result i32)
                i32.const -50 i32.const 50 call $set_speed)
        )
    "#);

    harness.call_unit_i32(&instance, "cmd1");
    harness.call_unit_i32(&instance, "cmd2");

    let cmds = &harness.host().outgoing_worker_cmds;
    assert_eq!(cmds.len(), 2, "two commands → two packets");

    let (_, p1) = WorkerPacket::decode(&cmds[0]).unwrap();
    let (l1, r1) = WorkerPacket::parse_motor_speed(p1).unwrap();
    assert_eq!((l1, r1), (100, 100));

    let (_, p2) = WorkerPacket::decode(&cmds[1]).unwrap();
    let (l2, r2) = WorkerPacket::parse_motor_speed(p2).unwrap();
    assert_eq!((l2, r2), (-50, 50));
}
