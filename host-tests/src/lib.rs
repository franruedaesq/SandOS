//! SandOS host-side TDD helpers.
//!
//! This crate provides a [`MockHost`] — a software simulation of the
//! ESP32-S3 hardware that runs on the developer's PC (x86_64 + std).
//!
//! It is used by the test modules to verify the Host-Guest ABI logic and
//! the Wasm sandbox behaviour **without** requiring physical hardware.
//!
//! ## Running the tests
//!
//! ```sh
//! cargo test -p host-tests
//! ```

pub mod mock_host;
pub mod vm_harness;
