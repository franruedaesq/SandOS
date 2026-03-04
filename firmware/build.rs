//! Build script for the SandOS firmware.
//!
//! Validates that we are targeting the correct chip and links the custom
//! memory layout required by esp-hal.
fn main() {
    // Ensure we are always re-run when the linker script changes.
    println!("cargo:rerun-if-changed=build.rs");

    // esp-build embeds the correct linker arguments for the target chip.
    esp_build::assert_unique_used_features!("esp32s3");
}
