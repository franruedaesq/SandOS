//! Phase 1 TDD Tests — The Bare-Metal Brain
//!
//! These tests verify the Host-Guest ABI for Phase 1 features:
//! - LED toggle via Wasm call
//! - ABI argument validation
//! - Wasm sandbox isolation (bad calls don't crash the host)
//! - ESP-NOW command packet validation
//! - ULP shared-memory layout constants

use abi::{
    cmd, status, validate_ptr_len, EspNowCommand, ESPNOW_MAX_PAYLOAD,
};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── Direct MockHost unit tests ────────────────────────────────────────────────

#[test]
fn led_starts_off() {
    let host = MockHost::default();
    assert!(!host.led_on);
    assert_eq!(host.toggle_count, 0);
}

#[test]
fn toggle_led_turns_it_on() {
    let mut host = MockHost::default();
    let status = host.toggle_led();
    assert_eq!(status, status::OK);
    assert!(host.led_on);
    assert_eq!(host.toggle_count, 1);
}

#[test]
fn toggle_led_turns_it_back_off() {
    let mut host = MockHost::default();
    host.toggle_led();
    let status = host.toggle_led();
    assert_eq!(status, status::OK);
    assert!(!host.led_on);
    assert_eq!(host.toggle_count, 2);
}

#[test]
fn get_uptime_ms_returns_simulated_value() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 12_345;
    assert_eq!(host.get_uptime_ms(), 12_345);
}

#[test]
fn debug_log_records_message() {
    let mut host = MockHost::default();
    let status = host.debug_log(b"hello from wasm");
    assert_eq!(status, status::OK);
    assert_eq!(host.log_messages.len(), 1);
    assert_eq!(host.log_messages[0], "hello from wasm");
}

#[test]
fn debug_log_invalid_utf8_returns_error() {
    let mut host = MockHost::default();
    let status = host.debug_log(&[0xFF, 0xFE]);
    assert_eq!(status, status::ERR_INVALID_ARG);
    assert!(host.log_messages.is_empty());
}

// ── ABI validation unit tests ─────────────────────────────────────────────────

#[test]
fn validate_ptr_len_ok_for_in_bounds_region() {
    assert!(validate_ptr_len(0, 64, 65536).is_ok());
    assert!(validate_ptr_len(65472, 64, 65536).is_ok());
}

#[test]
fn validate_ptr_len_err_for_overflow() {
    assert!(validate_ptr_len(65473, 64, 65536).is_err());
    assert!(validate_ptr_len(u32::MAX, 1, 65536).is_err());
}

#[test]
fn validate_ptr_len_err_for_zero_memory() {
    assert!(validate_ptr_len(0, 1, 0).is_err());
}

// ── ESP-NOW packet tests ──────────────────────────────────────────────────────

#[test]
fn espnow_command_magic_valid() {
    let cmd = EspNowCommand {
        magic: EspNowCommand::MAGIC,
        cmd_id: cmd::TOGGLE_LED,
        payload_len: 0,
        payload: [0; ESPNOW_MAX_PAYLOAD - 4],
    };
    assert!(cmd.is_valid());
}

#[test]
fn espnow_command_magic_invalid() {
    let cmd = EspNowCommand {
        magic: [0x00, 0x00],
        cmd_id: 0,
        payload_len: 0,
        payload: [0; ESPNOW_MAX_PAYLOAD - 4],
    };
    assert!(!cmd.is_valid());
}

// ── Wasm-level integration tests (via WasmHarness) ────────────────────────────

/// Phase 1 success criterion: a Wasm app receives a TOGGLE_LED command,
/// calls `host_toggle_led()`, and the LED state changes.
#[test]
fn wasm_toggle_led_via_abi() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_toggle_led" (func $toggle (result i32)))
            (func (export "run_command") (param i32) (result i32)
                call $toggle
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0x01);
    assert_eq!(result, status::OK);
    assert!(harness.host().led_on, "LED should be ON after first toggle");

    let result = harness.call_i32_i32(&instance, "run_command", 0x01);
    assert_eq!(result, status::OK);
    assert!(!harness.host().led_on, "LED should be OFF after second toggle");
}

/// Toggling the LED 100 times should work without errors.
#[test]
fn wasm_toggle_led_many_times() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_toggle_led" (func $toggle (result i32)))
            (func (export "run_command") (param i32) (result i32)
                call $toggle
            )
        )
    "#);

    for _ in 0..100 {
        let result = harness.call_i32_i32(&instance, "run_command", 0x01);
        assert_eq!(result, status::OK);
    }
    // Even number of toggles → LED back to OFF.
    assert!(!harness.host().led_on);
    assert_eq!(harness.host().toggle_count, 100);
}

/// The Wasm sandbox should not be able to read the Host's memory directly.
/// Verify that the VM correctly enforces linear-memory bounds.
#[test]
fn wasm_oob_memory_access_is_trapped() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_debug_log" (func $log (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; Attempt to log a string that ends exactly at the page boundary.
            (func (export "run_command") (param i32) (result i32)
                ;; ptr=65530, len=10 => end=65540 > 65536 (1 page)
                i32.const 65530
                i32.const 10
                call $log
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0x03);
    // The Host must return ERR_BOUNDS, not crash.
    assert_eq!(result, status::ERR_BOUNDS);
}

/// A Wasm app that calls `host_get_uptime_ms` receives the simulated value.
#[test]
fn wasm_get_uptime_ms() {
    let mut host = MockHost::default();
    host.simulated_uptime_ms = 42_000;
    let mut harness = WasmHarness::new(host);

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_uptime_ms" (func $uptime (result i64)))
            (func (export "run_command") (param i32) (result i32)
                call $uptime
                i64.const 42000
                i64.eq
                ;; 1 if equal, 0 if not → return as i32
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0);
    assert_eq!(result, 1, "uptime should match simulated value");
}
