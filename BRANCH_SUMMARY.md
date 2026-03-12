# Branch Summary: Face Companion, Web Server & Architecture

This document outlines the current state of implementation on this branch, detailing the features built for SandOS, a highly fault-tolerant, low-latency WebAssembly-sandboxed robotics operating system running on the ESP32-S3.

## Overall Architecture

The system utilizes an asymmetric dual-core architecture to strictly isolate hardware control and unpredictable high-level AI logic, ensuring a crash-proof core loop.

*   **Core 0 (The Brain):** Executes the WebAssembly (Wasm) Virtual Machine and manages Wi-Fi/ESP-NOW wireless communication. It handles high-level intents and UI drawing logic.
*   **Core 1 (The Muscle):** Dedicated to hard real-time processes. It executes pure Embassy Rust tasks for polling sensors (IMU) and running PID balancing loops to directly control motors. It operates entirely independent of Core 0.
*   **ULP Coprocessor (The Paramedic):** A low-power RISC-V core that runs independently in the background, continuously monitoring critical hardware metrics (such as battery voltage). It has the authority to trigger a safe motor shutdown in an emergency.
*   **Wasm Sandbox & Host-Guest ABI:** The system uses a strict "Zero Trust" model. The Wasm Guest cannot access hardware directly. It must request actions through a validated Application Binary Interface (ABI) (e.g., `host_draw_eye()`). Core 0's Host code validates these requests before interacting with the hardware. A hardware Watchdog Timer is attached to the Wasm task; if the AI logic hangs, the Watchdog silently restarts the sandbox without halting Core 1's motor loops.

## Current Implementations

### 1. The Face Companion & OLED Screen
The robot features an expressive face and status UI rendered on a 0.96-inch 128x64 I2C OLED display (SSD1306 controller).

*   **Screen Hardware:** The display is connected via I2C (`SDA` -> `GPIO8`, `SCL` -> `GPIO9`) using the `embedded-graphics` crate for rendering primitives and text.
*   **Display Driver:** A custom asynchronous display driver uses DMA/interrupt-driven I2C transfers. This allows the CPU to yield during frame flushes, ensuring that UI updates do not starve the network stack or other tasks.
*   **Face Mode:** By default, the screen renders a 60 FPS animated kawaii face. The face features idle expression cycling (Neutral, Happy, Sleepy, Thinking, Surprised, Heart, etc.), auto-blinking, and an "eye drift" breathing effect. Expressions can be overridden via ABI calls.

### 2. Menu Navigation
The system implements a hardware-interrupt-driven menu system to interact with the device locally.

*   **BOOT Button:** Menu navigation is driven entirely by the ESP32-S3's built-in BOOT button (GPIO 0).
*   **Interaction Model:**
    *   **Short Press:** Cycles to the next menu item or switches from the Face mode to the Menu mode.
    *   **Long Press:** Selects the currently highlighted menu item or toggles the selected setting.
    *   **Double Press:** Acts as a quick shortcut to return to the default Face mode.
*   **Menu Features:** The menu shrinks the face to a mini-version on the right half of the screen and displays a scrolling list on the left. It provides access to tools (Flashlight, Party Mode, Pomodoro timer), information (System Monitor, Clock), transit times (Vienna Lines departure board), and settings (Brightness, Web Server toggle).

### 3. Wi-Fi & Web Server Dashboard
The robot can join a local Wi-Fi network and serve a diagnostic dashboard.

*   **Wi-Fi Coexistence:** The ESP32-S3 runs the Wi-Fi Station (STA) stack alongside the low-latency ESP-NOW radio.
*   **On-Demand Web Server:** To prevent DHCP negotiations from starving the display driver at boot, the HTTP/1.0 web server starts in a **disabled** state.
*   **Toggling:** The user can enable the web server via the physical display menu (`Menu > Web > ON`). A long press of the BOOT button toggles this state.
*   **Dashboard Features:** Once enabled, the server listens on port 80. Accessing the device's IP address returns a modern HTML/CSS (glassmorphism style) dashboard. The dashboard provides:
    *   Live system metrics (Uptime, PSRAM usage, Wasm Hot-swaps).
    *   Wi-Fi connection status and IP address.
    *   Interactive RGB LED color controls (sending POST requests to the device).
    *   An endpoint `/api/stats` to fetch system metrics via JSON.
