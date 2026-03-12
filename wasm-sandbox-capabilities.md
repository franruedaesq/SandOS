# SandOS Wasm Sandbox Capabilities

## What is the Wasm Sandbox used for?

In SandOS, the WebAssembly (Wasm) Sandbox is the environment for high-level intelligence, orchestration, and unpredictable logic. It serves as the "Brain" of the robotics operating system, separating these non-deterministic tasks from the hard real-time physical control (the "Muscle").

The sandbox allows you to:
- **Run Untrusted Guest Code Safely**: Execute third-party or experimental logic without the risk of destabilizing the robot's core systems or crashing the motor control loops.
- **Enable Hot-Swappable Behavior**: Update the robot's high-level logic on the fly (via Wasm OTA) without needing to reflash the entire firmware.
- **Handle Variable-Latency Tasks**: Manage networking, LLM integration, user interfaces, and routing without jittering the deterministic timing required for physical stability.

## How does it work?

The Wasm Sandbox operates on a **zero-trust Host-Guest model** utilizing an asymmetric dual-core setup on the ESP32-S3:

1. **Execution on Core 0**: The Wasm virtual machine (using the `wasmi` interpreter) runs entirely on Core 0. This core primarily operates using PSRAM and is dedicated to tasks like networking (ESP-NOW, Wi-Fi, Web Server), display updates, and VM execution.
2. **Real-time Isolation**: While Core 0 runs the sandbox, Core 1 executes the hard real-time loops (e.g., IMU polling, PID balancing, motor control) uninterrupted on fast internal SRAM.
3. **The Host-Guest ABI**: The Wasm guest application is completely blind to the underlying hardware. It has no direct access to memory, GPIO, I2C, or radios.
4. **Validated Hardware Access**: Every action must pass through a strict Application Binary Interface (ABI). The flow is as follows:
   - *Wasm guest calls an ABI function (e.g., `host_toggle_led()`)*
   - *VM pauses execution*
   - *Rust Host firmware validates arguments and checks permissions/bounds*
   - *Host safely executes the requested hardware action*
   - *Result/status code is returned to the guest*
   - *VM resumes execution*

## 5 Examples of What We Can Do With It

1. **Remote Control & Routing Logic**
   Process movement commands arriving over ESP-NOW or Wi-Fi, validate them, and convert them into movement intents. The Wasm app can safely dispatch these via ABI calls like `host_set_motor_speed()`, which the host then routes to Core 1's motor bridge.

2. **Interactive UI & Display Management**
   Drive an interactive OLED display (e.g., a robot face) dynamically. The Wasm app can request drawings using `host_draw_eye(expression)` or `host_write_text(ptr, len)` to render 60 FPS graphics while yielding CPU time to prevent starvation.

3. **Voice & AI Assistant Integration**
   Act as the intelligent bridge for voice interactions. The sandbox can initiate audio capture with `host_start_audio_capture()`, stream the audio to an external LLM pipeline, and react to the returned intents by updating the robot's expression or movement.

4. **Dynamic Over-The-Air (OTA) Updates**
   Change the robot's "personality" or behavioral logic entirely on the fly. Because the Wasm app runs in a sandbox, a new Wasm binary can be uploaded via Wi-Fi or ESP-NOW, verified with a CRC, and hot-swapped without rebooting the robot's real-time physical core.

5. **Telemetry Aggregation & Observation**
   Poll high-level sensor data safely using functions like `host_get_pitch_roll()` and format it for transmission. The Wasm app can act as a telemetry broker, reading IMU or odometry data from the host and broadcasting it to an external web dashboard or PC observer.