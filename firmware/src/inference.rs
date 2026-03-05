//! Phase 7 — Local AI Subsystem: embedded inference engine.
//!
//! This module provides a memory-safe Rust wrapper around the embedded
//! inference pipeline.  In production the `TinyMlEngine` struct drives
//! Espressif's **ESP-NN** vector-acceleration library through a thin FFI
//! shim (see the *ESP-NN integration notes* below).  In the current build
//! it contains a lightweight stub that performs a deterministic calculation
//! on the input tensor so that the rest of the OS can be developed and
//! tested without a fully trained model.
//!
//! ## ESP-NN integration notes
//!
//! ESP-NN (<https://github.com/espressif/esp-nn>) is a collection of
//! architecture-optimised SIMD kernels for ESP32-S3's dual-core Xtensa LX7
//! with optional vector-extension (AI-extension) hardware.  The recommended
//! path for production use is:
//!
//! 1. Add `esp-nn` as a C library in `firmware/build.rs` via
//!    `println!("cargo:rustc-link-lib=esp_nn")` and include the Espressif
//!    IDF component header via `bindgen`.
//! 2. Alternatively, integrate via
//!    [TensorFlow Lite Micro](https://github.com/tensorflow/tflite-micro)
//!    which calls ESP-NN automatically on ESP32-S3 targets.
//! 3. Expose the model weights as a `static` byte slice from Flash memory
//!    (zero-copy, no heap allocation):
//!    ```rust,ignore
//!    static MODEL_TFLITE: &[u8] = include_bytes!("../../model/keyword.tflite");
//!    ```
//! 4. Pass `MODEL_TFLITE` to `TinyMlEngine::new()` and the engine will load
//!    it through the TFLite Micro interpreter, which in turn calls ESP-NN
//!    kernels for convolution and pooling operations.
//!
//! ## Thread safety
//!
//! The latest inference result is stored in [`INFERENCE_RESULT`] as a packed
//! `AtomicU32` using the same pattern as the IMU bridge in `sensors.rs`:
//! Core 0 (Wasm engine task) writes; any other code can read atomically.

use core::sync::atomic::{AtomicU32, Ordering};

use abi::{InferenceResult, INFERENCE_TENSOR_SIZE};

// ── Global inference result ───────────────────────────────────────────────────

/// Latest result from the embedded inference engine, packed as a `u32`.
///
/// Written by the inference pipeline (Core 0 task) after every inference pass.
/// Read by [`AbiHost::get_local_inference`] in response to Wasm guest queries.
///
/// Uses `AtomicU32` for lock-free inter-task sharing — the same pattern as
/// `sensors::IMU_DATA`.
pub static INFERENCE_RESULT: AtomicU32 = AtomicU32::new(0);

// ── Public API ────────────────────────────────────────────────────────────────

/// Store a fresh inference result so it can be queried by the Wasm guest.
#[inline]
pub fn store_inference_result(result: InferenceResult) {
    INFERENCE_RESULT.store(result.pack(), Ordering::Release);
}

/// Retrieve the most recent inference result.
#[inline]
pub fn load_inference_result() -> InferenceResult {
    InferenceResult::unpack(INFERENCE_RESULT.load(Ordering::Acquire))
}

// ── Inference Engine ──────────────────────────────────────────────────────────

/// A lightweight wrapper around the embedded neural-network inference pipeline.
///
/// ### Production
///
/// Replace the `run` method body with an FFI call into TFLite Micro / ESP-NN:
///
/// ```rust,ignore
/// unsafe {
///     tflite_micro_set_input(tensor.as_ptr(), tensor.len());
///     tflite_micro_invoke();
///     let class = tflite_micro_get_output_class();
///     let conf  = tflite_micro_get_output_confidence();
///     InferenceResult { active: true, top_class: class, confidence_pct: conf }
/// }
/// ```
///
/// ### Stub (current)
///
/// Performs a simple deterministic calculation on the input samples so that
/// the OS pipeline can be exercised without a trained model binary:
///
/// * `top_class` = most common byte value (mod 8) in the input tensor.
/// * `confidence_pct` = saturation-clamped average absolute sample value,
///   scaled to 0 – 100 percent.
pub struct TinyMlEngine;

impl TinyMlEngine {
    /// Construct a new engine instance.
    ///
    /// In the production build this would accept a `&'static [u8]` model
    /// slice and initialise the TFLite Micro interpreter.
    pub const fn new() -> Self {
        Self
    }

    /// Run a single inference pass on `tensor`.
    ///
    /// `tensor` must contain signed 8-bit samples in the range `[-128, 127]`,
    /// matching the quantised input format expected by TFLite Micro INT8 models.
    ///
    /// Returns an [`InferenceResult`] with `active = true` when `tensor` is
    /// non-empty, or `active = false` when there are no input samples.
    pub fn run(&self, tensor: &[i8]) -> InferenceResult {
        if tensor.is_empty() {
            return InferenceResult::default();
        }

        // ── Stub computation ─────────────────────────────────────────────────
        // In the real implementation this delegates to TFLite Micro / ESP-NN.
        // The stub produces a deterministic, testable result from the samples.

        // top_class: most-frequent byte value, mapped to [0, 7].
        let mut freq = [0u32; 256];
        for &s in tensor.iter().take(INFERENCE_TENSOR_SIZE) {
            freq[s as u8 as usize] += 1;
        }
        let top_byte = freq
            .iter()
            .enumerate()
            .max_by_key(|&(_, &count)| count)
            .map(|(idx, _)| idx)
            .unwrap_or(0) as u8;
        let top_class = top_byte % 8;

        // confidence_pct: mean absolute value of the samples, clamped to 100.
        let sum: u32 = tensor
            .iter()
            .take(INFERENCE_TENSOR_SIZE)
            .map(|&s| s.unsigned_abs() as u32)
            .sum();
        let n = tensor.len().min(INFERENCE_TENSOR_SIZE) as u32;
        let mean_abs = sum / n; // 0 – 127 range
        let confidence_pct = ((mean_abs * 100) / 127).min(100) as u8;

        InferenceResult { active: true, top_class, confidence_pct }
    }
}

/// Module-level singleton engine.
///
/// Allocated as a `const`-constructible zero-sized type so it costs nothing
/// at runtime and does not require heap allocation.
pub static ENGINE: TinyMlEngine = TinyMlEngine::new();
