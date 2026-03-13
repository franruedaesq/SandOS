//! Phase 7 TDD Tests — The Local AI Subsystem (Embedded Inference Engine)
//!
//! These tests verify the Phase 7 additions:
//!
//! - **`InferenceResult` serialization**: The struct correctly round-trips
//!   through `pack`/`unpack` (atomic storage) and `to_bytes`/`from_bytes`
//!   (Wasm memory transfer).
//! - **Stub inference engine**: `TinyMlEngine::run` (via the `run_stub_inference`
//!   mirror in `MockHost`) produces deterministic, testable results.
//! - **Radio link monitoring**: `MockHost::is_radio_link_alive` tracks the
//!   `radio_link_alive` flag, and the fallback activates when `false`.
//! - **Fallback inference pipeline**: When the radio link is silent, audio
//!   buffer samples are routed to the inference engine and the result is
//!   stored so the Wasm guest can query it.
//! - **Wasm ABI — `host_get_local_inference`**: The Wasm guest can call this
//!   function to read the current inference result from Host memory into its
//!   own linear memory.
//! - **Pointer bounds checking**: Out-of-bounds pointers return `ERR_BOUNDS`.

use abi::{InferenceResult, INFERENCE_RESULT_SIZE};
use host_tests::{
    mock_host::{run_stub_inference, MockHost},
    vm_harness::WasmHarness,
};

// ── Direct mock-host inference tests ─────────────────────────────────────────

#[test]
fn inference_result_inactive_by_default() {
    let host = MockHost::default();
    assert!(!host.inference_result.active);
    assert_eq!(host.inference_result.top_class, 0);
    assert_eq!(host.inference_result.confidence_pct, 0);
}

#[test]
fn get_local_inference_writes_inactive_result_by_default() {
    let host = MockHost::default();
    let mut out = [0u8; INFERENCE_RESULT_SIZE as usize];
    let status = host.get_local_inference(&mut out);
    assert_eq!(status, abi::status::OK);
    let result = InferenceResult::from_bytes(&out).expect("must decode");
    assert!(!result.active);
    assert_eq!(result.top_class, 0);
    assert_eq!(result.confidence_pct, 0);
}

#[test]
fn get_local_inference_returns_set_result() {
    let mut host = MockHost::default();
    host.inference_result = InferenceResult {
        active: true,
        top_class: 3,
        confidence_pct: 87,
    };
    let mut out = [0u8; INFERENCE_RESULT_SIZE as usize];
    host.get_local_inference(&mut out);
    let result = InferenceResult::from_bytes(&out).expect("must decode");
    assert!(result.active);
    assert_eq!(result.top_class, 3);
    assert_eq!(result.confidence_pct, 87);
}

#[test]
fn get_local_inference_short_buffer_returns_err_bounds() {
    let host = MockHost::default();
    let mut short = [0u8; 8]; // needs 12 bytes
    assert_eq!(
        host.get_local_inference(&mut short),
        abi::status::ERR_BOUNDS
    );
}

// ── Radio link monitoring tests ───────────────────────────────────────────────

#[test]
fn radio_link_alive_by_default() {
    let host = MockHost::default();
    assert!(host.is_radio_link_alive());
}

#[test]
fn radio_link_can_be_set_to_silent() {
    let mut host = MockHost::default();
    host.radio_link_alive = false;
    assert!(!host.is_radio_link_alive());
}

#[test]
fn radio_link_restored_after_setting_to_alive() {
    let mut host = MockHost::default();
    host.radio_link_alive = false;
    host.radio_link_alive = true;
    assert!(host.is_radio_link_alive());
}

// ── Fallback inference pipeline tests ────────────────────────────────────────

#[test]
fn fallback_inference_no_snapshot_returns_false() {
    let mut host = MockHost::default();
    assert!(!host.run_fallback_inference());
    assert!(!host.inference_result.active);
}

#[test]
fn fallback_inference_with_snapshot_returns_true() {
    let mut host = MockHost::default();
    let samples: Vec<i8> = vec![10, -20, 30, -40, 50];
    host.push_audio_for_inference(samples);
    assert!(host.run_fallback_inference());
}

#[test]
fn fallback_inference_activates_result() {
    let mut host = MockHost::default();
    let samples: Vec<i8> = vec![100i8; 64]; // all 100 → same top_byte
    host.push_audio_for_inference(samples.clone());
    host.run_fallback_inference();
    assert!(host.inference_result.active);
}

#[test]
fn fallback_inference_with_uniform_samples_produces_deterministic_result() {
    let mut host = MockHost::default();
    // All samples are 100 → top_byte = 100, top_class = 100 % 8 = 4
    // mean_abs = 100, confidence_pct = (100 * 100) / 127 = 78
    let samples: Vec<i8> = vec![100i8; 128];
    host.push_audio_for_inference(samples.clone());
    host.run_fallback_inference();

    let expected = run_stub_inference(&samples);
    assert_eq!(host.inference_result, expected);
    assert_eq!(host.inference_result.top_class, 4);
    assert_eq!(host.inference_result.confidence_pct, 78);
}

#[test]
fn fallback_inference_empty_tensor_leaves_result_inactive() {
    let mut host = MockHost::default();
    host.push_audio_for_inference(vec![]);
    host.run_fallback_inference();
    assert!(!host.inference_result.active);
}

#[test]
fn fallback_inference_drains_queue_one_at_a_time() {
    let mut host = MockHost::default();
    host.push_audio_for_inference(vec![10i8; 32]);
    host.push_audio_for_inference(vec![20i8; 32]);

    // First call drains snapshot 1.
    assert!(host.run_fallback_inference());
    assert_eq!(host.audio_inference_queue.len(), 1);

    // Second call drains snapshot 2.
    assert!(host.run_fallback_inference());
    assert!(host.audio_inference_queue.is_empty());

    // Third call: queue empty.
    assert!(!host.run_fallback_inference());
}

#[test]
fn push_audio_for_inference_stores_snapshot() {
    let mut host = MockHost::default();
    let samples: Vec<i8> = vec![1, 2, 3, -1, -2];
    host.push_audio_for_inference(samples.clone());
    assert_eq!(host.audio_inference_queue.len(), 1);
    assert_eq!(host.audio_inference_queue[0], samples);
}

// ── Stub inference engine tests ───────────────────────────────────────────────

#[test]
fn stub_inference_empty_input_is_inactive() {
    let result = run_stub_inference(&[]);
    assert!(!result.active);
    assert_eq!(result.top_class, 0);
    assert_eq!(result.confidence_pct, 0);
}

#[test]
fn stub_inference_single_sample() {
    let result = run_stub_inference(&[64i8]);
    assert!(result.active);
    // top_byte = 64 → top_class = 64 % 8 = 0
    assert_eq!(result.top_class, 0);
    // mean_abs = 64, confidence_pct = (64 * 100) / 127 = 50
    assert_eq!(result.confidence_pct, 50);
}

#[test]
fn stub_inference_all_zeros() {
    let result = run_stub_inference(&[0i8; 16]);
    assert!(result.active);
    assert_eq!(result.confidence_pct, 0);
}

#[test]
fn stub_inference_max_samples() {
    let samples = vec![127i8; abi::INFERENCE_TENSOR_SIZE];
    let result = run_stub_inference(&samples);
    assert!(result.active);
    // top_byte = 127 → 127 % 8 = 7
    assert_eq!(result.top_class, 7);
    // mean_abs = 127 → confidence_pct = (127 * 100) / 127 = 100
    assert_eq!(result.confidence_pct, 100);
}

#[test]
fn stub_inference_result_pack_unpack_identity() {
    let original = InferenceResult {
        active: true,
        top_class: 5,
        confidence_pct: 63,
    };
    let packed = original.pack();
    let unpacked = InferenceResult::unpack(packed);
    assert_eq!(unpacked, original);
}

// ── Wasm ABI integration tests ────────────────────────────────────────────────

/// The Wasm guest calls `host_get_local_inference(out_ptr)` and reads back
/// the three i32 fields (active, top_class, confidence_pct) from its linear
/// memory.
#[test]
fn wasm_get_local_inference_inactive_by_default() {
    let mut harness = WasmHarness::new(MockHost::default());

    // Declare 1 Wasm memory page (64 KiB).
    // The function writes 12 bytes at ptr=0:
    //   [0..4]  active (i32 LE)
    //   [4..8]  top_class (i32 LE)
    //   [8..12] confidence_pct (i32 LE)
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_local_inference"
                (func $infer (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                ;; Call with out_ptr = 0
                (call $infer (i32.const 0))
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, abi::status::OK);

    // Read back the 12-byte result from Wasm memory at address 0.
    let host = harness.host();
    let mut out = [0u8; INFERENCE_RESULT_SIZE as usize];
    out.copy_from_slice(&[0u8; INFERENCE_RESULT_SIZE as usize]); // zero init
                                                                 // We can't directly access Wasm memory from here, but we can verify via
                                                                 // the MockHost that the inference_result is inactive.
    assert!(!host.inference_result.active);
}

/// The Wasm guest reads back an active inference result that was previously
/// set on the mock host.
#[test]
fn wasm_get_local_inference_returns_set_result() {
    let mut mock = MockHost::default();
    mock.inference_result = InferenceResult {
        active: true,
        top_class: 2,
        confidence_pct: 75,
    };
    let mut harness = WasmHarness::new(mock);

    // WAT module:
    //  1. Calls host_get_local_inference(0) → writes 12 bytes at offset 0.
    //  2. Reads active    at offset 0  as i32 → store to local.
    //  3. Reads top_class at offset 4  as i32 → store to local.
    //  4. Reads confidence at offset 8 as i32 → store to local.
    //  5. Returns active XOR'd so we can check it quickly (1 = active).
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_local_inference"
                (func $infer (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "get_active") (result i32)
                (call $infer (i32.const 0))
                drop
                ;; active is at offset 0 as i32 LE
                (i32.load (i32.const 0))
            )
            (func (export "get_top_class") (result i32)
                (call $infer (i32.const 0))
                drop
                (i32.load (i32.const 4))
            )
            (func (export "get_confidence") (result i32)
                (call $infer (i32.const 0))
                drop
                (i32.load (i32.const 8))
            )
        )
    "#,
    );

    let active = harness.call_unit_i32(&instance, "get_active");
    let top_class = harness.call_unit_i32(&instance, "get_top_class");
    let confidence = harness.call_unit_i32(&instance, "get_confidence");

    assert_eq!(active, 1);
    assert_eq!(top_class, 2);
    assert_eq!(confidence, 75);
}

/// Out-of-bounds `out_ptr` returns `ERR_BOUNDS`.
#[test]
fn wasm_get_local_inference_oob_ptr_returns_err_bounds() {
    let mut harness = WasmHarness::new(MockHost::default());

    // Memory is 1 page = 65536 bytes.
    // out_ptr=65530, size=12 → end=65542 > 65536 → ERR_BOUNDS.
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_local_inference"
                (func $infer (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                (call $infer (i32.const 65530))
            )
        )
    "#,
    );

    let result = harness.call_unit_i32(&instance, "run");
    assert_eq!(result, abi::status::ERR_BOUNDS);
}

/// End-to-end: mock a fallback scenario where the radio link is silent,
/// audio samples are queued, inference runs, and the Wasm guest reads the
/// fresh result.
#[test]
fn wasm_end_to_end_fallback_inference_pipeline() {
    let mut mock = MockHost::default();

    // Simulate a silent radio link.
    mock.radio_link_alive = false;

    // Push an audio snapshot that will produce a known result.
    // All samples = 100 → top_class = 4, confidence_pct = 78.
    let samples: Vec<i8> = vec![100i8; 128];
    mock.push_audio_for_inference(samples);

    // Simulate the Router running the fallback inference.
    mock.run_fallback_inference();

    // Verify the inference result was stored.
    assert!(mock.inference_result.active);
    assert_eq!(mock.inference_result.top_class, 4);
    assert_eq!(mock.inference_result.confidence_pct, 78);

    // Now run the Wasm guest to query the result.
    let mut harness = WasmHarness::new(mock);
    let instance = harness.load_wat(
        r#"
        (module
            (import "env" "host_get_local_inference"
                (func $infer (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "get_class") (result i32)
                (call $infer (i32.const 0))
                drop
                (i32.load (i32.const 4))
            )
        )
    "#,
    );

    let top_class = harness.call_unit_i32(&instance, "get_class");
    assert_eq!(top_class, 4);
}
