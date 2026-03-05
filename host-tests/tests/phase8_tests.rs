//! Phase 8 TDD Tests — The Dynamic Brain (Wasm Hot-Swapping & OTA Engine)
//!
//! These tests verify the Phase 8 additions:
//!
//! - **OTA session lifecycle**: `ota_begin` → `ota_receive_chunk` (×N) →
//!   `ota_finalize` transitions the state machine correctly.
//! - **Binary verification**: [`abi::crc32`] detects bit-flips; corrupted
//!   binaries are rejected by `ota_finalize`.
//! - **Hot-swap routine**: `hot_swap_wasm` executes the four-step
//!   pause→flush→instantiate→resume sequence and Core 1 motor state is
//!   unaffected throughout.
//! - **Core 1 isolation**: Motor speeds and the dead-man's switch are
//!   unchanged before and after a hot-swap.
//! - **Wasm ABI — `host_get_ota_status`**: The Wasm guest can call this
//!   function to read the current OTA state into its linear memory.
//! - **Pointer bounds checking**: Out-of-bounds pointers return `ERR_BOUNDS`.
//! - **OTA constants**: Sizes and capacities match the design document.

use abi::{crc32, OtaState, OtaStatus, OTA_MAX_BINARY_SIZE, OTA_STATUS_SIZE};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── CRC-32 utility tests ──────────────────────────────────────────────────────

#[test]
fn crc32_of_empty_is_zero() {
    assert_eq!(crc32(&[]), 0x0000_0000);
}

#[test]
fn crc32_standard_test_vector_123456789() {
    // Well-known CRC-32/ISO-HDLC test vector.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn crc32_is_nonzero_for_nonempty_input() {
    // A non-empty, non-trivial slice must not collide with the empty result.
    assert_ne!(crc32(&[0xDE, 0xAD, 0xBE, 0xEF]), 0x0000_0000);
}

#[test]
fn crc32_detects_single_bit_flip() {
    let data: Vec<u8> = (0u8..64).collect();
    let mut corrupted = data.clone();
    corrupted[32] ^= 0x01;
    assert_ne!(crc32(&data), crc32(&corrupted));
}

#[test]
fn crc32_is_consistent_across_calls() {
    let data = b"hello, SandOS OTA";
    assert_eq!(crc32(data), crc32(data));
}

// ── OTA session lifecycle tests ───────────────────────────────────────────────

#[test]
fn ota_state_idle_by_default() {
    let host = MockHost::default();
    assert_eq!(host.ota_state, OtaState::Idle);
}

#[test]
fn ota_begin_transitions_to_receiving() {
    let mut host = MockHost::default();
    let status = host.ota_begin(1024);
    assert_eq!(status, abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Receiving);
}

#[test]
fn ota_begin_sets_expected_size() {
    let mut host = MockHost::default();
    host.ota_begin(4096);
    assert_eq!(host.ota_expected_size, 4096);
}

#[test]
fn ota_begin_clears_previous_buffer() {
    let mut host = MockHost::default();
    host.ota_begin(128);
    host.ota_receive_chunk(0, &[1u8; 32]);
    // Begin a new session — buffer must reset.
    host.ota_begin(64);
    assert_eq!(host.ota_buffer.len(), 64);
    assert!(host.ota_buffer.iter().all(|&b| b == 0));
}

#[test]
fn ota_begin_rejects_zero_size() {
    let mut host = MockHost::default();
    assert_eq!(host.ota_begin(0), abi::status::ERR_INVALID_ARG);
    assert_eq!(host.ota_state, OtaState::Idle);
}

#[test]
fn ota_begin_rejects_oversized_binary() {
    let mut host = MockHost::default();
    let too_big = OTA_MAX_BINARY_SIZE as u32 + 1;
    assert_eq!(host.ota_begin(too_big), abi::status::ERR_INVALID_ARG);
    assert_eq!(host.ota_state, OtaState::Idle);
}

#[test]
fn ota_begin_accepts_max_binary_size() {
    let mut host = MockHost::default();
    assert_eq!(host.ota_begin(OTA_MAX_BINARY_SIZE as u32), abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Receiving);
}

#[test]
fn ota_begin_returns_err_busy_when_swap_in_progress() {
    let mut host = MockHost::default();
    // Manually force the Swapping state (not normally reachable from tests
    // without hot_swap_wasm, but validates the guard).
    host.ota_state = OtaState::Swapping;
    assert_eq!(host.ota_begin(128), abi::status::ERR_BUSY);
}

// ── OTA chunk reception tests ─────────────────────────────────────────────────

#[test]
fn ota_receive_chunk_writes_data_at_offset() {
    let mut host = MockHost::default();
    host.ota_begin(256);
    let chunk = [0xAB, 0xCD, 0xEF];
    host.ota_receive_chunk(10, &chunk);
    assert_eq!(&host.ota_buffer[10..13], &chunk);
}

#[test]
fn ota_receive_chunk_increments_bytes_received() {
    let mut host = MockHost::default();
    host.ota_begin(256);
    host.ota_receive_chunk(0, &[0u8; 64]);
    assert_eq!(host.ota_bytes_received, 64);
    host.ota_receive_chunk(64, &[0u8; 32]);
    assert_eq!(host.ota_bytes_received, 96);
}

#[test]
fn ota_receive_chunk_returns_err_busy_when_idle() {
    let mut host = MockHost::default();
    assert_eq!(
        host.ota_receive_chunk(0, &[1, 2, 3]),
        abi::status::ERR_BUSY
    );
}

#[test]
fn ota_receive_chunk_returns_err_bounds_when_out_of_range() {
    let mut host = MockHost::default();
    host.ota_begin(16);
    // Writing 8 bytes at offset 12 would reach offset 20 > 16 → ERR_BOUNDS.
    assert_eq!(
        host.ota_receive_chunk(12, &[0u8; 8]),
        abi::status::ERR_BOUNDS
    );
}

#[test]
fn ota_receive_chunk_returns_err_invalid_arg_for_empty_data() {
    let mut host = MockHost::default();
    host.ota_begin(64);
    assert_eq!(host.ota_receive_chunk(0, &[]), abi::status::ERR_INVALID_ARG);
}

#[test]
fn ota_receive_multiple_chunks_reassembles_correctly() {
    let mut host = MockHost::default();
    let binary: Vec<u8> = (0u8..=255).collect(); // 256 bytes
    host.ota_begin(256);

    // Send in four 64-byte chunks.
    for i in 0..4usize {
        let offset = (i * 64) as u32;
        host.ota_receive_chunk(offset, &binary[i * 64..(i + 1) * 64]);
    }
    assert_eq!(host.ota_buffer, binary);
}

// ── OTA finalize / verification tests ────────────────────────────────────────

#[test]
fn ota_finalize_accepts_correct_crc32() {
    let mut host = MockHost::default();
    let binary: Vec<u8> = (0u8..=255).collect();
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, &binary);

    let expected = crc32(&binary);
    assert_eq!(host.ota_finalize(expected), abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Ready);
}

#[test]
fn ota_finalize_rejects_wrong_crc32() {
    let mut host = MockHost::default();
    let binary = vec![0xABu8; 128];
    host.ota_begin(128);
    host.ota_receive_chunk(0, &binary);

    // Deliberately wrong CRC.
    assert_eq!(host.ota_finalize(0xDEAD_BEEF), abi::status::ERR_INVALID_ARG);
    assert_eq!(host.ota_state, OtaState::Failed);
}

#[test]
fn ota_finalize_detects_corrupted_chunk() {
    let mut host = MockHost::default();
    let binary: Vec<u8> = (0u8..128).collect();
    let good_crc = crc32(&binary);

    host.ota_begin(128);
    // Introduce a one-byte corruption.
    let mut corrupted = binary.clone();
    corrupted[64] ^= 0x01;
    host.ota_receive_chunk(0, &corrupted);

    assert_eq!(host.ota_finalize(good_crc), abi::status::ERR_INVALID_ARG);
    assert_eq!(host.ota_state, OtaState::Failed);
}

#[test]
fn ota_finalize_returns_err_busy_when_idle() {
    let mut host = MockHost::default();
    assert_eq!(host.ota_finalize(0xDEAD_BEEF), abi::status::ERR_BUSY);
}

// ── Hot-swap routine tests ────────────────────────────────────────────────────

#[test]
fn hot_swap_increments_swap_count() {
    let mut host = build_verified_host(b"fake wasm binary");
    assert_eq!(host.hot_swap_count, 0);
    host.hot_swap_wasm();
    assert_eq!(host.hot_swap_count, 1);
}

#[test]
fn hot_swap_installs_new_binary() {
    let binary = b"new wasm binary content";
    let mut host = build_verified_host(binary);
    host.hot_swap_wasm();
    assert_eq!(host.active_wasm_binary, binary.as_slice());
}

#[test]
fn hot_swap_clears_ota_buffer_after_swap() {
    let mut host = build_verified_host(b"wasm v2");
    host.hot_swap_wasm();
    assert!(host.ota_buffer.is_empty());
}

#[test]
fn hot_swap_resets_ota_state_to_idle() {
    let mut host = build_verified_host(b"wasm v3");
    host.hot_swap_wasm();
    assert_eq!(host.ota_state, OtaState::Idle);
}

#[test]
fn hot_swap_vm_is_not_paused_after_completion() {
    let mut host = build_verified_host(b"wasm v4");
    host.hot_swap_wasm();
    assert!(!host.vm_paused);
}

#[test]
fn hot_swap_returns_err_busy_when_not_ready() {
    let mut host = MockHost::default();
    // No OTA session started — state is Idle, not Ready.
    assert_eq!(host.hot_swap_wasm(), abi::status::ERR_BUSY);
}

#[test]
fn hot_swap_returns_err_busy_when_still_receiving() {
    let mut host = MockHost::default();
    host.ota_begin(64);
    // Receiving state — finalize has not been called yet.
    assert_eq!(host.hot_swap_wasm(), abi::status::ERR_BUSY);
}

#[test]
fn hot_swap_returns_err_busy_after_failed_verification() {
    let mut host = MockHost::default();
    host.ota_begin(8);
    host.ota_receive_chunk(0, &[1, 2, 3, 4, 5, 6, 7, 8]);
    host.ota_finalize(0x0000_0000); // wrong CRC → Failed state
    assert_eq!(host.hot_swap_wasm(), abi::status::ERR_BUSY);
}

// ── Core 1 isolation tests ────────────────────────────────────────────────────
//
// These tests assert that the hot-swap operation on Core 0 never modifies the
// motor state or dead-man's switch variables that Core 1 reads from.

#[test]
fn hot_swap_does_not_change_motor_speeds() {
    let mut host = MockHost::default();
    host.set_motor_speed(100, -50);

    // Record motor state before hot-swap.
    let left_before  = host.motor_left_speed;
    let right_before = host.motor_right_speed;

    // Perform the OTA + hot-swap sequence.
    let binary = b"firmware v2";
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, binary);
    host.ota_finalize(crc32(binary));
    host.hot_swap_wasm();

    // Motor state must be identical after the swap.
    assert_eq!(host.motor_left_speed, left_before);
    assert_eq!(host.motor_right_speed, right_before);
}

#[test]
fn hot_swap_does_not_affect_motors_enabled_flag() {
    let mut host = MockHost::default();
    assert!(host.motors_enabled);

    let binary = b"v2";
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, binary);
    host.ota_finalize(crc32(binary));
    host.hot_swap_wasm();

    assert!(host.motors_enabled);
}

#[test]
fn hot_swap_does_not_reset_dead_mans_switch_state() {
    let mut host = MockHost::default();
    // Simulate the dead-man's switch being active.
    host.dead_mans_active = true;
    host.motor_left_speed = 0;
    host.motor_right_speed = 0;

    let binary = b"new app";
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, binary);
    host.ota_finalize(crc32(binary));
    host.hot_swap_wasm();

    // Dead-man's switch must remain active.
    assert!(host.dead_mans_active);
}

#[test]
fn hot_swap_does_not_affect_routing_mode() {
    let mut host = MockHost::default();
    host.routing_mode = abi::RoutingMode::Distributed;

    let binary = b"v2";
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, binary);
    host.ota_finalize(crc32(binary));
    host.hot_swap_wasm();

    assert_eq!(host.routing_mode, abi::RoutingMode::Distributed);
}

// ── get_ota_status method tests ───────────────────────────────────────────────

#[test]
fn get_ota_status_returns_idle_by_default() {
    let host = MockHost::default();
    let mut out = [0u8; OTA_STATUS_SIZE as usize];
    assert_eq!(host.get_ota_status(&mut out), abi::status::OK);
    let status = OtaStatus::from_bytes(&out).expect("must decode");
    assert_eq!(status.state, OtaState::Idle);
    assert_eq!(status.bytes_received, 0);
    assert_eq!(status.total_size, 0);
    assert_eq!(status.swap_count, 0);
}

#[test]
fn get_ota_status_reflects_receiving_state() {
    let mut host = MockHost::default();
    host.ota_begin(512);
    host.ota_receive_chunk(0, &[0u8; 128]);

    let mut out = [0u8; OTA_STATUS_SIZE as usize];
    host.get_ota_status(&mut out);
    let status = OtaStatus::from_bytes(&out).unwrap();
    assert_eq!(status.state, OtaState::Receiving);
    assert_eq!(status.total_size, 512);
    assert_eq!(status.bytes_received, 128);
}

#[test]
fn get_ota_status_reflects_ready_state() {
    let binary = b"readybin";
    let host = build_verified_host(binary);

    let mut out = [0u8; OTA_STATUS_SIZE as usize];
    host.get_ota_status(&mut out);
    let status = OtaStatus::from_bytes(&out).unwrap();
    assert_eq!(status.state, OtaState::Ready);
}

#[test]
fn get_ota_status_reflects_swap_count_after_hot_swap() {
    let mut host = build_verified_host(b"v1");
    host.hot_swap_wasm();

    let mut host2 = build_verified_host(b"v2");
    host2.hot_swap_count = 1; // simulate one previous swap
    host2.hot_swap_wasm();

    let mut out = [0u8; OTA_STATUS_SIZE as usize];
    host2.get_ota_status(&mut out);
    let status = OtaStatus::from_bytes(&out).unwrap();
    assert_eq!(status.swap_count, 2);
}

#[test]
fn get_ota_status_short_buffer_returns_err_bounds() {
    let host = MockHost::default();
    let mut short = [0u8; 8]; // needs 16 bytes
    assert_eq!(host.get_ota_status(&mut short), abi::status::ERR_BOUNDS);
}

// ── Full OTA round-trip test ──────────────────────────────────────────────────

#[test]
fn ota_full_round_trip_begin_chunks_finalize_swap() {
    let binary: Vec<u8> = (0u8..=255).collect(); // 256-byte "Wasm" binary
    let expected_crc = crc32(&binary);
    let mut host = MockHost::default();

    // 1. Begin.
    assert_eq!(host.ota_begin(binary.len() as u32), abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Receiving);

    // 2. Send in 16-byte chunks.
    for (i, chunk) in binary.chunks(16).enumerate() {
        let offset = (i * 16) as u32;
        assert_eq!(host.ota_receive_chunk(offset, chunk), abi::status::OK);
    }
    assert_eq!(host.ota_buffer, binary);

    // 3. Finalize with correct CRC.
    assert_eq!(host.ota_finalize(expected_crc), abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Ready);

    // 4. Hot-swap.
    assert_eq!(host.hot_swap_wasm(), abi::status::OK);
    assert_eq!(host.ota_state, OtaState::Idle);
    assert_eq!(host.active_wasm_binary, binary);
    assert_eq!(host.hot_swap_count, 1);
    assert!(!host.vm_paused);
}

#[test]
fn ota_full_round_trip_rejected_if_crc_wrong() {
    let binary: Vec<u8> = vec![0x42u8; 64];
    let wrong_crc = crc32(&binary) ^ 0xFFFF_FFFF; // guaranteed to be wrong
    let mut host = MockHost::default();

    host.ota_begin(64);
    host.ota_receive_chunk(0, &binary);
    assert_eq!(host.ota_finalize(wrong_crc), abi::status::ERR_INVALID_ARG);
    assert_eq!(host.ota_state, OtaState::Failed);

    // Hot-swap must be rejected after failure.
    assert_eq!(host.hot_swap_wasm(), abi::status::ERR_BUSY);
}

// ── Wasm ABI integration tests ────────────────────────────────────────────────

/// The Wasm guest reads back the default (Idle, zeros) OTA status.
#[test]
fn wasm_get_ota_status_idle_by_default() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_ota_status"
                (func $ota_status (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                (call $ota_status (i32.const 0))
            )
        )
    "#);

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, abi::status::OK);

    // State field at offset 0 should be 0 (Idle).
    let host = harness.host();
    assert_eq!(host.ota_state, OtaState::Idle);
}

/// The Wasm guest reads back the OTA status after a session has started.
#[test]
fn wasm_get_ota_status_reflects_receiving_state() {
    let mut mock = MockHost::default();
    mock.ota_begin(1024);
    mock.ota_receive_chunk(0, &[0xABu8; 256]);
    let mut harness = WasmHarness::new(mock);

    // WAT: call host_get_ota_status(0), then read state at offset 0 as i32.
    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_ota_status"
                (func $ota_status (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "get_state") (result i32)
                (call $ota_status (i32.const 0))
                drop
                (i32.load (i32.const 0))
            )
            (func (export "get_bytes_received") (result i32)
                (call $ota_status (i32.const 0))
                drop
                (i32.load (i32.const 4))
            )
            (func (export "get_total_size") (result i32)
                (call $ota_status (i32.const 0))
                drop
                (i32.load (i32.const 8))
            )
        )
    "#);

    let state          = harness.call_unit_i32(&instance, "get_state");
    let bytes_received = harness.call_unit_i32(&instance, "get_bytes_received");
    let total_size     = harness.call_unit_i32(&instance, "get_total_size");

    assert_eq!(state, OtaState::Receiving as i32);
    assert_eq!(bytes_received, 256);
    assert_eq!(total_size, 1024);
}

/// An out-of-bounds `out_ptr` returns `ERR_BOUNDS`.
#[test]
fn wasm_get_ota_status_oob_ptr_returns_err_bounds() {
    let mut harness = WasmHarness::new(MockHost::default());

    // Memory is 1 page = 65536 bytes.
    // out_ptr = 65530, size = 16 → end = 65546 > 65536 → ERR_BOUNDS.
    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_ota_status"
                (func $ota_status (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                (call $ota_status (i32.const 65530))
            )
        )
    "#);

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, abi::status::ERR_BOUNDS);
}

/// End-to-end: full OTA session from Wasm's perspective — the guest polls
/// `host_get_ota_status` after each step to confirm the state machine advances.
#[test]
fn wasm_end_to_end_ota_status_lifecycle() {
    let binary: Vec<u8> = vec![0x77u8; 32];
    let crc = crc32(&binary);
    let mut mock = MockHost::default();

    mock.ota_begin(32);
    mock.ota_receive_chunk(0, &binary);
    mock.ota_finalize(crc);
    // State is now Ready.
    mock.hot_swap_wasm();
    // State is now Idle; swap_count = 1.

    let mut harness = WasmHarness::new(mock);
    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_ota_status"
                (func $ota_status (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "get_swap_count") (result i32)
                (call $ota_status (i32.const 0))
                drop
                ;; swap_count is at offset 12 as u32 LE
                (i32.load (i32.const 12))
            )
        )
    "#);

    let swap_count = harness.call_unit_i32(&instance, "get_swap_count");
    assert_eq!(swap_count, 1);
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Build a [`MockHost`] with a verified OTA binary ready for hot-swap.
fn build_verified_host(binary: &[u8]) -> MockHost {
    let mut host = MockHost::default();
    let crc = crc32(binary);
    host.ota_begin(binary.len() as u32);
    host.ota_receive_chunk(0, binary);
    let result = host.ota_finalize(crc);
    assert_eq!(result, abi::status::OK, "build_verified_host: finalize failed");
    assert_eq!(host.ota_state, OtaState::Ready);
    host
}
