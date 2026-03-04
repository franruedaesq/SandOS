//! Phase 2 TDD Tests — The Face & Voice
//!
//! These tests verify the extended Host-Guest ABI for Phase 2 features:
//! - Eye expression rendering (`host_draw_eye`)
//! - Display text writing (`host_write_text`)
//! - Brightness control (`host_set_brightness`)
//! - Audio capture pipeline (`host_start_audio_capture`, `host_read_audio`)
//! - Argument bounds validation for all Phase 2 functions
//! - LLM text → display rendering end-to-end

use abi::{status, EyeExpression, MAX_AUDIO_READ, MAX_BRIGHTNESS, MAX_TEXT_BYTES};
use host_tests::{mock_host::MockHost, vm_harness::WasmHarness};

// ── Direct MockHost unit tests ────────────────────────────────────────────────

// ── Display ───────────────────────────────────────────────────────────────────

#[test]
fn draw_eye_neutral_expression() {
    let mut host = MockHost::default();
    let result = host.draw_eye(EyeExpression::Neutral as i32);
    assert_eq!(result, status::OK);
    assert_eq!(host.display.current_expression, Some(EyeExpression::Neutral));
}

#[test]
fn draw_eye_all_valid_expressions() {
    let mut host = MockHost::default();
    for i in 0..=6i32 {
        let result = host.draw_eye(i);
        assert_eq!(result, status::OK, "expression {} should be valid", i);
        let expr = EyeExpression::from_i32(i).unwrap();
        assert_eq!(host.display.current_expression, Some(expr));
    }
}

#[test]
fn draw_eye_invalid_expression_returns_error() {
    let mut host = MockHost::default();
    assert_eq!(host.draw_eye(7),   status::ERR_INVALID_ARG);
    assert_eq!(host.draw_eye(-1),  status::ERR_INVALID_ARG);
    assert_eq!(host.draw_eye(255), status::ERR_INVALID_ARG);
    // Expression must not have changed.
    assert_eq!(host.display.current_expression, None);
}

#[test]
fn write_text_stores_string() {
    let mut host = MockHost::default();
    let result = host.write_text(b"Hello, Robot!");
    assert_eq!(result, status::OK);
    assert_eq!(host.display.display_text, "Hello, Robot!");
}

#[test]
fn write_text_empty_string() {
    let mut host = MockHost::default();
    let result = host.write_text(b"");
    assert_eq!(result, status::OK);
    assert_eq!(host.display.display_text, "");
}

#[test]
fn write_text_max_length_is_accepted() {
    let mut host = MockHost::default();
    let text = vec![b'x'; MAX_TEXT_BYTES as usize];
    let result = host.write_text(&text);
    assert_eq!(result, status::OK);
}

#[test]
fn write_text_exceeding_max_length_returns_bounds_error() {
    let mut host = MockHost::default();
    let text = vec![b'x'; MAX_TEXT_BYTES as usize + 1];
    let result = host.write_text(&text);
    assert_eq!(result, status::ERR_BOUNDS);
    // Display text must not have been updated.
    assert!(host.display.display_text.is_empty());
}

#[test]
fn write_text_invalid_utf8_returns_error() {
    let mut host = MockHost::default();
    let result = host.write_text(&[0xFF, 0xFE, 0xFD]);
    assert_eq!(result, status::ERR_INVALID_ARG);
}

#[test]
fn set_brightness_valid_range() {
    let mut host = MockHost::default();
    assert_eq!(host.set_brightness(0),               status::OK);
    assert_eq!(host.display.brightness, 0);
    assert_eq!(host.set_brightness(128),              status::OK);
    assert_eq!(host.display.brightness, 128);
    assert_eq!(host.set_brightness(MAX_BRIGHTNESS),   status::OK);
    assert_eq!(host.display.brightness, MAX_BRIGHTNESS as u8);
}

#[test]
fn set_brightness_out_of_range_returns_error() {
    let mut host = MockHost::default();
    assert_eq!(host.set_brightness(-1),                  status::ERR_INVALID_ARG);
    assert_eq!(host.set_brightness(MAX_BRIGHTNESS + 1),  status::ERR_INVALID_ARG);
}

// ── Audio ─────────────────────────────────────────────────────────────────────

#[test]
fn audio_inactive_by_default() {
    let host = MockHost::default();
    assert!(!host.audio_active);
    assert_eq!(host.get_audio_avail(), 0);
}

#[test]
fn start_audio_sets_active_flag() {
    let mut host = MockHost::default();
    let result = host.start_audio_capture();
    assert_eq!(result, status::OK);
    assert!(host.audio_active);
}

#[test]
fn stop_audio_clears_active_flag() {
    let mut host = MockHost::default();
    host.start_audio_capture();
    let result = host.stop_audio_capture();
    assert_eq!(result, status::OK);
    assert!(!host.audio_active);
}

#[test]
fn read_audio_returns_bytes() {
    let mut host = MockHost::default();
    host.feed_audio(&[1, 2, 3, 4, 5]);
    let mut buf = [0u8; 5];
    let n = host.read_audio(&mut buf);
    assert_eq!(n, 5);
    assert_eq!(&buf, &[1, 2, 3, 4, 5]);
    assert_eq!(host.get_audio_avail(), 0);
}

#[test]
fn read_audio_partial_read() {
    let mut host = MockHost::default();
    host.feed_audio(&[10, 20, 30, 40, 50]);
    let mut buf = [0u8; 3];
    let n = host.read_audio(&mut buf);
    assert_eq!(n, 3);
    assert_eq!(&buf, &[10, 20, 30]);
    assert_eq!(host.get_audio_avail(), 2);
}

#[test]
fn read_audio_empty_buffer_returns_zero() {
    let mut host = MockHost::default();
    let mut buf = [0u8; 10];
    let n = host.read_audio(&mut buf);
    assert_eq!(n, 0);
}

#[test]
fn read_audio_exceeding_max_size_returns_bounds_error() {
    let mut host = MockHost::default();
    let large_buf_len = MAX_AUDIO_READ as usize + 1;
    let mut buf = vec![0u8; large_buf_len];
    let result = host.read_audio(&mut buf);
    assert_eq!(result, status::ERR_BOUNDS);
}

#[test]
fn start_audio_clears_previous_buffer() {
    let mut host = MockHost::default();
    host.feed_audio(&[1, 2, 3]);
    host.start_audio_capture();
    // Buffer should be cleared on (re-)start.
    assert_eq!(host.get_audio_avail(), 0);
}

// ── Wasm-level integration tests ──────────────────────────────────────────────

#[test]
fn wasm_draw_eye_happy() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_draw_eye" (func $draw (param i32) (result i32)))
            (func (export "run_command") (param i32) (result i32)
                i32.const 1   ;; Happy
                call $draw
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0x10);
    assert_eq!(result, status::OK);
    assert_eq!(
        harness.host().display.current_expression,
        Some(EyeExpression::Happy)
    );
}

#[test]
fn wasm_draw_eye_invalid_expression_returns_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_draw_eye" (func $draw (param i32) (result i32)))
            (func (export "run_command") (param i32) (result i32)
                i32.const 99  ;; Invalid
                call $draw
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0);
    assert_eq!(result, status::ERR_INVALID_ARG);
    // Expression must not have changed.
    assert_eq!(harness.host().display.current_expression, None);
}

#[test]
fn wasm_write_text_via_linear_memory() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_write_text" (func $write (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 200) "Hello, Robot!")
            (func (export "run_command") (param i32) (result i32)
                i32.const 200
                i32.const 13
                call $write
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0x11);
    assert_eq!(result, status::OK);
    assert_eq!(harness.host().display.display_text, "Hello, Robot!");
}

#[test]
fn wasm_write_text_oob_returns_bounds_error() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_write_text" (func $write (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; ptr=65530, len=100 → end=65630 > 65536
            (func (export "run_command") (param i32) (result i32)
                i32.const 65530
                i32.const 100
                call $write
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0x11);
    assert_eq!(result, status::ERR_BOUNDS);
    assert!(harness.host().display.display_text.is_empty());
}

#[test]
fn wasm_set_brightness() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_set_brightness" (func $bright (param i32) (result i32)))
            (func (export "run_command") (param i32) (result i32)
                i32.const 200
                call $bright
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0);
    assert_eq!(result, status::OK);
    assert_eq!(harness.host().display.brightness, 200);
}

#[test]
fn wasm_audio_capture_pipeline() {
    let mut harness = WasmHarness::new(MockHost::default());

    // This Wasm module reads audio data without starting a fresh capture,
    // modelling the steady-state read path (capture was started at boot).
    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_get_audio_avail" (func $avail (result i32)))
            (import "env" "host_read_audio"      (func $read  (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; Returns number of bytes read (stored at offset 500 in Wasm memory).
            (func (export "run_command") (param i32) (result i32)
                i32.const 500
                i32.const 8
                call $read
            )
        )
    "#);

    // Simulate: audio capture was started and data has arrived.
    harness.host_mut().start_audio_capture();
    harness
        .host_mut()
        .feed_audio(&[0xA0, 0xB1, 0xC2, 0xD3, 0xE4, 0xF5, 0x06, 0x17]);

    let bytes_read = harness.call_i32_i32(&instance, "run_command", 0x20);
    assert_eq!(bytes_read, 8, "should have read all 8 injected bytes");
    assert!(harness.host().audio_active, "audio should still be active");
    assert_eq!(harness.host().get_audio_avail(), 0, "buffer should be drained");
}

#[test]
fn wasm_audio_start_stop_pipeline() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_start_audio_capture" (func $start (result i32)))
            (import "env" "host_stop_audio_capture"  (func $stop  (result i32)))
            (func (export "run_start") (result i32) call $start)
            (func (export "run_stop")  (result i32) call $stop)
        )
    "#);

    assert!(!harness.host().audio_active);

    let r = harness.call_unit_i32(&instance, "run_start");
    assert_eq!(r, status::OK);
    assert!(harness.host().audio_active);

    let r = harness.call_unit_i32(&instance, "run_stop");
    assert_eq!(r, status::OK);
    assert!(!harness.host().audio_active);
}

/// End-to-end LLM pipeline test:
/// PC sends a text response → Wasm draws happy eyes + writes the text.
#[test]
fn wasm_llm_response_renders_face_and_text() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_draw_eye"   (func $draw (param i32) (result i32)))
            (import "env" "host_write_text" (func $write (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; Simulate handle_llm_response(ptr=300, len=9, mood=Happy=1)
            (data (i32.const 300) "I'm happy!")
            (func (export "run_command") (param i32) (result i32)
                i32.const 1     ;; Happy expression
                call $draw
                drop
                i32.const 300   ;; ptr to text
                i32.const 10    ;; len("I'm happy!")
                call $write
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0);
    assert_eq!(result, status::OK);
    assert_eq!(
        harness.host().display.current_expression,
        Some(EyeExpression::Happy)
    );
    assert_eq!(harness.host().display.display_text, "I'm happy!");
}

/// Phase 2 isolation: sending multiple ABI calls in a sequence must not
/// corrupt shared state between calls.
#[test]
fn wasm_sequential_abi_calls_maintain_independent_state() {
    let mut harness = WasmHarness::new(MockHost::default());

    let instance = harness.load_wat(r#"
        (module
            (import "env" "host_toggle_led"  (func $led   (result i32)))
            (import "env" "host_draw_eye"    (func $draw  (param i32) (result i32)))
            (import "env" "host_write_text"  (func $write (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "OK")
            (func (export "run_command") (param i32) (result i32)
                ;; Call all three ABI functions in a row.
                call $led
                drop
                i32.const 2    ;; Sad
                call $draw
                drop
                i32.const 0
                i32.const 2
                call $write
            )
        )
    "#);

    let result = harness.call_i32_i32(&instance, "run_command", 0);
    assert_eq!(result, status::OK);
    assert!(harness.host().led_on);
    assert_eq!(
        harness.host().display.current_expression,
        Some(EyeExpression::Sad)
    );
    assert_eq!(harness.host().display.display_text, "OK");
}
