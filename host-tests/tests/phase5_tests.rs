//! Phase 5 TDD Tests — The Flexible Nervous System (Unified Message Router)
//!
//! These tests verify the Phase 5 additions:
//!
//! - **OS Message Bus**: `host_set_motor_speed()` publishes a `MovementIntent`
//!   to the bus instead of touching pins directly.
//! - **Routing Engine**: A `RoutingMode` toggle switches between Single-Board
//!   (intent forwarded to Core 1) and Distributed (intent sent via ESP-NOW).
//! - **Dead-Man's Switch**: If no valid intent arrives for >50 ms the Router
//!   zeroes all motor control loops regardless of operating mode.

use abi::{status, MovementIntent, RoutingMode, DEAD_MANS_SWITCH_MS, MAX_MOTOR_SPEED};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── OS Message Bus — intent publication ──────────────────────────────────────

#[test]
fn set_motor_speed_publishes_intent_to_bus() {
    let mut host = MockHost::default();
    host.set_motor_speed(100, -50);
    assert_eq!(host.intent_log.len(), 1);
    assert_eq!(host.intent_log[0], MovementIntent::new(100, -50));
}

#[test]
fn multiple_motor_commands_accumulate_in_intent_log() {
    let mut host = MockHost::default();
    host.set_motor_speed(50, 50);
    host.set_motor_speed(100, -100);
    host.set_motor_speed(0, 0);
    assert_eq!(host.intent_log.len(), 3);
    assert_eq!(host.intent_log[0], MovementIntent::new(50, 50));
    assert_eq!(host.intent_log[1], MovementIntent::new(100, -100));
    assert_eq!(host.intent_log[2], MovementIntent::zero());
}

#[test]
fn invalid_motor_command_does_not_publish_intent() {
    let mut host = MockHost::default();
    host.set_motor_speed(MAX_MOTOR_SPEED + 1, 0); // Invalid
    assert!(host.intent_log.is_empty());
}

#[test]
fn motor_command_blocked_when_disabled_does_not_publish_intent() {
    let mut host = MockHost::default();
    host.motors_enabled = false;
    host.set_motor_speed(100, 100); // Blocked
    assert!(host.intent_log.is_empty());
}

// ── Routing Engine — Single-Board mode ───────────────────────────────────────

#[test]
fn single_board_mode_is_default() {
    let host = MockHost::default();
    assert_eq!(host.routing_mode, RoutingMode::SingleBoard);
}

#[test]
fn single_board_mode_updates_local_motor_speeds() {
    let mut host = MockHost::default();
    // Explicitly set mode (even though it is the default) for clarity.
    host.routing_mode = RoutingMode::SingleBoard;
    host.set_motor_speed(120, -80);
    assert_eq!(host.motor_left_speed, 120);
    assert_eq!(host.motor_right_speed, -80);
}

#[test]
fn single_board_mode_does_not_populate_distributed_intents() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::SingleBoard;
    host.set_motor_speed(120, -80);
    assert!(host.distributed_intents.is_empty());
}

// ── Routing Engine — Distributed mode ────────────────────────────────────────

#[test]
fn distributed_mode_routes_to_distributed_intents() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::Distributed;
    host.set_motor_speed(120, -80);
    assert_eq!(host.distributed_intents.len(), 1);
    assert_eq!(host.distributed_intents[0], MovementIntent::new(120, -80));
}

#[test]
fn distributed_mode_does_not_update_local_motor_speeds() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::Distributed;
    host.set_motor_speed(120, -80);
    // Local speeds remain at their initial values.
    assert_eq!(host.motor_left_speed, 0);
    assert_eq!(host.motor_right_speed, 0);
}

#[test]
fn distributed_mode_still_logs_intent() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::Distributed;
    host.set_motor_speed(50, 50);
    // The intent_log captures every published intent regardless of mode.
    assert_eq!(host.intent_log.len(), 1);
    assert_eq!(host.distributed_intents.len(), 1);
}

#[test]
fn switching_from_distributed_to_single_board_updates_local_speeds() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::Distributed;
    host.set_motor_speed(100, 100);
    assert_eq!(
        host.motor_left_speed, 0,
        "distributed mode must not update local speeds"
    );

    // Switch back to Single-Board.
    host.routing_mode = RoutingMode::SingleBoard;
    host.set_motor_speed(75, -75);
    assert_eq!(host.motor_left_speed, 75);
    assert_eq!(host.motor_right_speed, -75);
}

// ── Dead-Man's Switch ─────────────────────────────────────────────────────────

#[test]
fn dead_mans_switch_inactive_by_default() {
    let host = MockHost::default();
    assert!(!host.dead_mans_active);
}

#[test]
fn dead_mans_switch_does_not_trip_before_timeout() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 0;
    host.set_motor_speed(50, 50); // intent at t=0

    // Check at exactly the timeout boundary: gap = 50ms, condition is `> 50`,
    // so exactly 50ms must NOT trip the switch (per spec: "intents for >50ms").
    host.check_dead_mans_switch(DEAD_MANS_SWITCH_MS);
    assert!(!host.dead_mans_active);
}

#[test]
fn dead_mans_switch_trips_after_timeout() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 0;
    host.set_motor_speed(50, 50); // intent at t=0

    // Check 51 ms later — switch should trip.
    host.check_dead_mans_switch(DEAD_MANS_SWITCH_MS + 1);
    assert!(host.dead_mans_active);
}

#[test]
fn dead_mans_switch_zeroes_motors_on_trip() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 0;
    host.set_motor_speed(200, -100); // set non-zero speeds

    // Simulate 51 ms elapsing without a new intent.
    host.check_dead_mans_switch(DEAD_MANS_SWITCH_MS + 1);

    assert!(host.dead_mans_active);
    assert_eq!(host.motor_left_speed, 0, "left motor must be zeroed by DMS");
    assert_eq!(
        host.motor_right_speed, 0,
        "right motor must be zeroed by DMS"
    );
}

#[test]
fn dead_mans_switch_clears_after_fresh_intent() {
    let mut host = MockHost::default();
    // Trip the switch first.
    host.simulated_uptime_ms = 0;
    host.set_motor_speed(100, 100);
    host.check_dead_mans_switch(DEAD_MANS_SWITCH_MS + 1);
    assert!(host.dead_mans_active);

    // Receive a fresh intent — DMS resets.
    host.simulated_uptime_ms = DEAD_MANS_SWITCH_MS + 1; // advance clock
    host.set_motor_speed(50, 50);
    assert!(!host.dead_mans_active);
}

#[test]
fn dead_mans_switch_trips_without_any_prior_intent() {
    let mut host = MockHost::default();
    // No intent has ever been published; last_intent_ms defaults to 0.
    // If current time is beyond the timeout, the switch should trip.
    host.check_dead_mans_switch(DEAD_MANS_SWITCH_MS + 1);
    assert!(host.dead_mans_active);
}

#[test]
fn last_intent_ms_updated_by_set_motor_speed() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 42;
    host.set_motor_speed(30, 30);
    assert_eq!(host.last_intent_ms, 42);
}

// ── Wasm ABI — message bus integration tests ─────────────────────────────────

/// The Wasm guest calls `host_publish` with a MOVEMENT_INTENT payload; the host logs a MovementIntent.
#[test]
fn wasm_set_motor_speed_publishes_intent() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; Write [80, 0, -60, 255] which is 80 and -60 in i16 LE to memory at offset 0
                i32.const 0
                i32.const 80
                i32.store16
                i32.const 2
                i32.const -60
                i32.store16
                ;; Call host_publish(topic=100, ptr=0, len=4)
                i32.const 100
                i32.const 0
                i32.const 4
                call $publish
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, status::OK);
    let host = harness.host();
    assert_eq!(host.intent_log.len(), 1);
    assert_eq!(host.intent_log[0], MovementIntent::new(80, -60));
}

/// In Single-Board mode the Wasm command reaches Core 1's motor bridge via host_publish.
#[test]
fn wasm_single_board_mode_updates_local_speeds() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::SingleBoard;
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                i32.const 0
                i32.const 150
                i32.store16
                i32.const 2
                i32.const -150
                i32.store16

                i32.const 100
                i32.const 0
                i32.const 4
                call $publish
            )
        )
    "#,
    );

    harness.call_unit_i32(&instance, "run");
    let host = harness.host();
    assert_eq!(host.motor_left_speed, 150);
    assert_eq!(host.motor_right_speed, -150);
    assert!(host.distributed_intents.is_empty());
}

/// In Distributed mode the Wasm publish is routed to ESP-NOW, not Core 1.
#[test]
fn wasm_distributed_mode_routes_to_espnow() {
    let mut host = MockHost::default();
    host.routing_mode = RoutingMode::Distributed;
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                i32.const 0
                i32.const 200
                i32.store16
                i32.const 2
                i32.const 200
                i32.store16

                i32.const 100
                i32.const 0
                i32.const 4
                call $publish
            )
        )
    "#,
    );

    harness.call_unit_i32(&instance, "run");
    let host = harness.host();
    // Local speeds must be untouched in distributed mode.
    assert_eq!(
        host.motor_left_speed, 0,
        "local speeds must not change in distributed mode"
    );
    assert_eq!(host.motor_right_speed, 0);
    // The intent must appear in the distributed queue.
    assert_eq!(host.distributed_intents.len(), 1);
    assert_eq!(host.distributed_intents[0], MovementIntent::new(200, 200));
}

/// Dead-man's switch integration: after simulated timeout, motors are zeroed.
#[test]
fn dead_mans_switch_integration_with_wasm_pipeline() {
    let mut harness = WasmHarness::new(MockHost::default());

    // 1. Send a valid motor command from Wasm via host_publish.
    let motor_instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_publish"
                (func $publish (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                i32.const 0
                i32.const 100
                i32.store16
                i32.const 2
                i32.const 100
                i32.store16

                i32.const 100
                i32.const 0
                i32.const 4
                call $publish
            )
        )
    "#,
    );
    harness.call_unit_i32(&motor_instance, "run");
    assert_eq!(harness.host().motor_left_speed, 100);

    // 2. Advance simulated time by more than 50 ms without a new intent.
    harness
        .host_mut()
        .check_dead_mans_switch(DEAD_MANS_SWITCH_MS + 1);

    // 3. Motors must be zeroed and the DMS flag must be set.
    assert!(harness.host().dead_mans_active);
    assert_eq!(harness.host().motor_left_speed, 0);
    assert_eq!(harness.host().motor_right_speed, 0);
}
