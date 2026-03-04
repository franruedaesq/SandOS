# Code Improvement Report for SandOS

## 1. Memory Management Optimisations

### Current State
SandOS correctly separates memory pools based on deterministic requirements:
- **Fast internal SRAM:** Reserved for Core 1 (motor control, balance loop) to guarantee hard real-time execution.
- **External PSRAM:** Handles the large, latency-tolerant Wasm Virtual Machine and its heap allocations.

However, during Phase 1 & 2 development, certain operations within the Host-Guest ABI bridge have introduced dynamic memory allocations on the hot path.

### Areas for Improvement: Dynamic Allocations in the ABI
In the implementation of ABI functions (such as `FN_READ_AUDIO`, `FN_DEBUG_LOG`, and `FN_WRITE_TEXT`) within both `firmware/src/core0/wasm_vm.rs` and `host-tests/src/vm_harness.rs`, there are occurrences of dynamic `Vec` allocation:

```rust
// Example from FN_READ_AUDIO
let mut tmp = vec![0u8; n];
```

**Why this is an issue:**
1. **Heap Fragmentation:** Repeatedly allocating and freeing memory for temporary buffers on the PSRAM heap can lead to fragmentation over time, eventually causing allocation failures and system crashes in long-running robotic applications.
2. **Performance Overhead:** Dynamic allocation is non-deterministic and slower than stack allocation, increasing the latency of the host-guest transition.

**Proposed Fix:**
Since the maximum buffer sizes for these operations are bounded and relatively small (e.g., `MAX_AUDIO_READ` is 1024 bytes, `MAX_TEXT_BYTES` is 256 bytes), we should replace dynamic `Vec` allocations with fixed-size stack arrays.

For example, in `FN_READ_AUDIO`:
```rust
let mut tmp = [0u8; MAX_AUDIO_READ as usize];
let copied = host.read_audio(&mut tmp[..n]) as usize;
```
For `FN_WRITE_TEXT` and `FN_DEBUG_LOG`, we can access the memory directly instead of cloning it into a `Vec`, or use a statically sized buffer if a copy is strictly necessary.

## 2. Hardware Stub Cleanups

### Current State
To support early-stage Test-Driven Development (TDD) before all hardware components are integrated, several modules use software stubs.

### Areas for Improvement: Transition to Physical Hardware
As the project moves through its phases (Phase 3 for sensors, Phase 4 for motors), the following stubs must be replaced with actual peripheral drivers:
- **`firmware/src/core1/mod.rs`**: The `simulate_imu` function provides a synthesised oscillating pitch/roll. This must be replaced with I2C/SPI reads from the physical MPU-6050 (or equivalent).
- **`firmware/src/display/mod.rs`**: The display driver logic is currently descriptive. It needs to wrap `mipidsi::Display` and configure the SPI2 peripheral with DMA for actual framebuffer transfers.
- **`firmware/src/core1/mod.rs`**: The PWM application logic is currently commented out (`ledc_left.set_duty(...)`). This must be bound to the ESP32-S3's LEDC or MCPWM peripherals.

## 3. Concurrency and Synchronization

### Current State
Core 1 and Core 0 operate independently without shared mutable state that requires locking. Sensor data from Core 1 is passed to Core 0 via a single atomic `u64` store (`IMU_DATA`).

### Areas for Improvement: Ensuring Atomicity under Load
The current approach using atomic operations is excellent for preventing latency spikes in the hard real-time loop.
We must ensure that any future shared state (e.g., motor target speeds from Wasm to Core 1) also adheres to this lock-free, non-blocking design pattern to prevent the Brain from accidentally starving the Muscle.

## Summary of Actionable Items
1. **[Immediate]** Refactor `wasm_vm.rs` and `vm_harness.rs` to remove dynamic `vec![0u8; n]` allocations inside the ABI functions, replacing them with fixed-size stack arrays.
2. **[Phase 3/4]** Replace the simulated IMU readings with real I2C driver code.
3. **[Phase 4]** Implement the actual LEDC/MCPWM configuration for motor control output.
