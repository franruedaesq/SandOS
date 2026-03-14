# Head OS Phase-by-Phase PRD

This Product Requirements Document details the phase-by-phase implementation plan required to finalize the Head OS and prepare it for connection to the Base OS. The goal is to establish a fault-tolerant, fluidly animating, and locally responsive subsystem.

## Phase 1: Core OS and Hardware Polling Initialization
**Objective:** Establish the foundational Embassy async execution environment, ensuring all peripherals (Display, ToF Sensor, Servos) initialize correctly without blocking each other.
*   **Action Items:**
    *   Verify the dual-core or single-core multitasking setup in Embassy.
    *   Ensure the I2C/SPI display driver initializes the OLED properly.
    *   Implement async polling for the VL53L0X ToF sensor to verify distance readings can be acquired consistently.
    *   Initialize PWM for servo control via `esp_hal::ledc`.
*   **Success Criteria:** The firmware compiles (`cargo build --release`) and runs (`cargo run --release`), successfully initializing all hardware and logging sensor data to the console.

## Phase 2: DMA Display & Fluid Animations
**Objective:** Guarantee a stutter-free 60FPS UI rendering experience using DMA.
*   **Action Items:**
    *   Finalize the SPI2/I2C asynchronous display driver using DMA.
    *   Verify that `embedded-graphics` primitives are drawn to the frame buffer and transferred via DMA without blocking the CPU.
    *   Implement basic idle animations (e.g., blinking, breathing) for the UI face.
*   **Success Criteria:** The OLED display renders a fluid, animated face that does not freeze or lag, even when dummy intensive tasks are simulated in the background.

## Phase 3: Hardware Reflexes (ToF + Servo)
**Objective:** Implement the local hardware reflex loop, entirely independent of the network or Base OS.
*   **Action Items:**
    *   Connect the VL53L0X async polling task to the servo control logic.
    *   Define the `TOF_THRESHOLD_MM` and the corresponding servo states (`DUTY_CENTER`, `DUTY_FLINCH`).
    *   Ensure the transition between states occurs instantly upon threshold crossing.
*   **Success Criteria:** When an object is placed within the threshold distance of the ToF sensor, the servo instantly snaps to the flinch position, and returns to center when the object is removed. This must happen concurrently with fluid UI animations.

## Phase 4: Intent Listener & Networking
**Objective:** Enable the Head OS to receive high-level state commands from the Base OS via ESP-NOW or UART.
*   **Action Items:**
    *   Configure the ESP-NOW receiver task.
    *   Define the struct/JSON payload format for incoming intents (e.g., target emotion, text to display).
    *   Implement a thread-safe state update mechanism (e.g., atomic variables or Embassy channels) to pass received intents to the display task.
*   **Success Criteria:** The Head OS successfully receives simulated ESP-NOW packets and updates the on-screen facial expression accordingly without interrupting the DMA rendering or the local ToF reflexes.

## Phase 5: Base OS Integration & Hardening
**Objective:** Finalize the integration with the Base OS, ensuring robust error handling and fault tolerance.
*   **Action Items:**
    *   Test the end-to-end connection between the Base OS and the Head OS.
    *   Implement fallback states: if the Base OS connection times out (heartbeat failure), the Head OS should revert to a default "Neutral" or "Searching" animation.
    *   Ensure the firmware builds flawlessly for the target architecture (`cargo build --release --target xtensa-esp32s3-none-elf`).
*   **Success Criteria:** The Head OS seamlessly reacts to commands from the Base OS, handles connection loss gracefully, and maintains local ToF reflexes and 60FPS animations under all conditions. `cargo run --release` successfully simulates or deploys the final, integrated firmware.