# SandOS Wasm Migration Review

This document reviews the current SandOS architecture and identifies components currently running on the Host (Rust firmware) that could be migrated into the Wasm Sandbox, explaining how and why such a migration should occur.

## Current State Overview

Currently, the Host OS handles both hardware abstraction and significant high-level logic:
- **Core 1 (The Muscle)**: Dedicated to hard real-time physical control loops (IMU, PID, Motor Bridge). This is working exactly as intended and should never be moved to Wasm.
- **Core 0 (The Brain)**: Runs the Wasm VM and handles networking, radio (ESP-NOW, Wi-Fi), and display rendering.

While the split between Core 1 and Core 0 is sound, **Core 0 currently implements too much application-level logic** in Rust. The Wasm Sandbox is currently primarily used to dispatch simple commands, but it should be the primary owner of "behavior."

## Candidates for Wasm Migration

### 1. The HTTP Dashboard & Web API
- **Current State**: The web server (`web_server.rs`) and Wi-Fi logic are compiled statically into the Rust firmware. The HTML payload is hardcoded into the binary.
- **Why Move It**: The web dashboard is the definition of "high-level UI." It requires frequent updates, variable latency handling, and complex string formatting—all tasks that are perfectly suited for Wasm. By moving the dashboard logic to Wasm, the robot's user interface could be updated dynamically via OTA without reflashing the OS.
- **How It Would Work**:
  - **New ABI**: We would need to introduce socket-like or request/response ABIs (e.g., `host_accept_http_request()`, `host_send_http_response()`).
  - **Host Duty**: The Host OS would manage the underlying TCP/IP stack (via Embassy/esp-wifi) and pass raw or lightly-parsed HTTP requests up to the Wasm guest.
  - **Guest Duty**: The Wasm app would generate the HTML string dynamically based on internal state and return it via the ABI.

### 2. ESP-NOW Command Parsing and Validation
- **Current State**: The Host firmware listens for ESP-NOW packets, parses the `EspNowCommand` struct, checks the magic bytes, and maps command IDs directly to hardware actions or routing intents.
- **Why Move It**: Command processing is "business logic." If we want to add a new command (e.g., a "dance" command), we currently have to modify the Rust host firmware. Moving the parsing to Wasm allows the command protocol to evolve dynamically.
- **How It Would Work**:
  - **New ABI**: The Host exposes a polling function `host_get_next_packet(ptr, max_len)` or registers an interrupt callback to the Wasm VM.
  - **Host Duty**: The Host OS only maintains the raw ESP-NOW radio driver and queues incoming raw byte arrays.
  - **Guest Duty**: The Wasm app reads the byte array from its linear memory, parses the SandOS protocol headers, determines the intent, and then calls existing ABI functions (like `host_set_motor_speed()` or `host_draw_eye()`) to enact the command.

### 3. Display Menu Logic (BOOT Button State Machine)
- **Current State**: The display menu (which allows toggling the Web server) is driven by an interrupt on the BOOT button, managing state natively in `display/mod.rs`.
- **Why Move It**: UI state machines are notoriously prone to changing requirements. Baking the menu hierarchy into the firmware limits customization.
- **How It Would Work**:
  - **New ABI**: The Host exposes a function to read the button state `host_get_button_state()` or pushes an event to the Wasm app.
  - **Host Duty**: The Host maintains the DMA display driver (rendering the actual pixels) and the hardware GPIO interrupt for the button.
  - **Guest Duty**: The Wasm app maintains the state machine (e.g., `MenuState::MainMenu -> MenuState::ToggleWeb`). It determines what text to show and calls `host_write_text()` to update the OLED.

### 4. Telemetry Aggregation & Filtering
- **Current State**: Core 1 generates structured telemetry (IMU, Odometry) and pushes it to a queue. The Host (Core 0) serializes this into CDR format and transmits it via ESP-NOW.
- **Why Move It**: The structure of telemetry data (what fields are included, how often they are sent, how they are filtered/smoothed) is highly application-specific. If we want to integrate with a new external system (e.g., a custom ROS2 node vs. a simple Python script), we need different serialization formats.
- **How It Would Work**:
  - **New ABI**: Utilize the existing `host_get_pitch_roll()` and add new functions for odometry. Add a transmission ABI `host_send_radio_packet(ptr, len)`.
  - **Host Duty**: Provide raw, unfiltered sensor access to the guest via ABI. Provide a raw packet transmission pipeline.
  - **Guest Duty**: The Wasm app pulls raw data, applies low-pass filters or aggregations, serializes the data into the desired format (CDR, JSON, Protobuf), and calls the transmission ABI.

### 5. Dead-Man Switch & Routing Logic
- **Current State**: The OS Message Bus Router (`router.rs`) enforces the dead-man switch (stopping motors if no intent is received in 50ms) and handles switching between Single-Board and Distributed modes.
- **Why Move It**: While the dead-man switch is a critical safety feature, the *definition* of safety might change based on the application (e.g., should it stop immediately, or perform a controlled deceleration?). Routing logic is also highly behavioral.
- **How It Would Work**:
  - **New ABI**: The Wasm app needs access to a high-resolution timer (`host_get_uptime_us()`). The host must provide an override or heartbeat expectation.
  - **Host Duty**: The Host OS still owns the *ultimate* safety gate (e.g., `motors.rs` will not apply PWM if disabled). However, the Host delegates the timeout logic to the guest.
  - **Guest Duty**: The Wasm app implements the 50ms timer. If it doesn't receive a network packet within the window, the Wasm app explicitly calls `host_set_motor_speed(0, 0)`.

## Summary
The goal of SandOS is **Zero-Trust separation of physical control and high-level logic**. To fully realize this, the Rust firmware should act only as a strict HAL (Hardware Abstraction Layer) and networking driver, pushing as much parsing, state management, and HTML generation as possible into the hot-swappable Wasm Sandbox.