# Product Requirements Document (PRD): Custom Wasm Robotics OS

## 1. Executive Summary

The objective is to build a highly fault-tolerant, low-latency, WebAssembly-sandboxed operating system for robotics using the ESP32-S3. The system relies on an asymmetric dual-core architecture written in `no_std` Rust (Embassy framework) to isolate hard real-time physical processes (motors/balance) from unpredictable high-level AI logic (LLM processing/ROS 2 translation).

## 2. Test-Driven Development (TDD) Strategy

**Can we use TDD?** Absolutely, and it is highly recommended. Rust’s ecosystem is uniquely suited for embedded TDD.

* **Host-Side Testing (`cargo test`):** You will write tests for your state machines, ABI logic, and Wasm translation layer on your PC. Because the Wasm runtime and ABI are purely logical, you can verify they work before ever flashing the ESP32-S3.
* **Hardware-in-the-Loop (HIL) Testing:** Using frameworks like `defmt-test`, you can run automated tests directly on the ESP32-S3 to verify that Core 0 and Core 1 are communicating correctly and that the ULP is successfully reading internal voltages.

## 3. Core Architecture

* **Core 0 (The Brain):** Executes the Wasm Virtual Machine (Guest) and manages ESP-NOW wireless communication.
* **Core 1 (The Muscle):** Executes hard real-time Embassy tasks (Host) for motor control and balancing.
* **ULP Coprocessor (The Paramedic):** Monitors system vitals (voltage) independently.
* **Host-Guest ABI:** A strictly defined set of virtual functions allowing the Sandbox to request hardware actions from the Host without direct memory access.

---

## 4. Phased Implementation Plan

*Note: Every phase is designed to culminate in a fully bootable, working version of the OS. As you add features, the OS simply becomes more capable.*

### Phase 1: The Bare-Metal Brain (Core OS Foundation)

**Hardware Required:** ESP32-S3 Board only (no external modules).
**Objective:** Establish the dual-core architecture, the Wasm sandbox, and wireless communication using only the onboard components (built-in LED, internal temperature sensor, and Wi-Fi antenna).

* **Establish the Host OS:** Configure the Rust Embassy framework to boot successfully, allocating distinct tasks to Core 0 and Core 1.
* **Implement the ULP Paramedic:** Write a tiny routine for the ULP coprocessor to monitor the internal chip temperature or supply voltage, setting a flag if it exceeds a threshold.
* **Establish ESP-NOW:** Configure Core 0 to broadcast and receive raw wireless packets to/from your PC.
* **Integrate the Wasm Sandbox:** Embed a lightweight WebAssembly interpreter onto Core 0.
* **Define the First ABI:** Create a simple Host Function bridge (e.g., `host_toggle_onboard_led()`).
* **Phase 1 Success Criteria:** You can send a wireless command from your PC to the ESP32-S3. Core 0 receives it, passes it to the Wasm app, and the Wasm app successfully calls the Host Function to blink the onboard LED. Meanwhile, Core 1 is successfully running a high-speed dummy loop without being interrupted by the Wasm execution.

### Phase 2: The Face & Voice (UI & Audio)

**Hardware Required:** ESP32-S3, SPI/I2C Screen (e.g., OLED/LCD), Microphone (I2S).
**Objective:** Implement the Command-Based UI architecture and audio streaming without lagging the operating system.

* **Implement DMA Display Drivers:** Configure the Host OS to communicate with the screen using Direct Memory Access (DMA) so the CPU is not blocked during screen refreshes.
* **Integrate `embedded-graphics`:** Set up the Rust drawing canvas in the Host memory.
* **Expand the ABI:** Add Host Functions for UI rendering (e.g., `host_draw_eye(expression)` or `host_write_text(string)`) and audio capture.
* **Establish LLM Pipeline:** Stream raw microphone data via ESP-NOW to the PC, and accept high-level "intent" commands back from the PC.
* **Phase 2 Success Criteria:** You can speak into the microphone, the PC's LLM processes it, and sends back a text response. The Wasm app on the ESP32 reads the text, calls the UI Host Functions, and the screen renders a blinking robot face with the text response at a smooth 60 FPS, all without halting Core 1.

### Phase 3: The Senses (Sensors & Memory Mapping)

**Hardware Required:** External sensors (e.g., IMU/Gyroscope, LIDAR).
**Objective:** Introduce external data gathering into the hard real-time loop and share it safely with the Wasm Sandbox.

* **Configure SRAM vs. PSRAM:** Formally map the Wasm engine to external PSRAM, ensuring the ultra-fast internal SRAM is reserved for the IMU and Core 1 operations.
* **Implement Real-Time Polling:** Configure Core 1 to poll the IMU via I2C/SPI using strict async timers, ensuring perfect determinism (e.g., exactly every 2ms).
* **Build the Safe Data Bridge:** Create a thread-safe, non-blocking memory structure (like an Atomic variable or an async channel) so Core 1 can dump sensor data that Core 0 can read.
* **Expand the ABI:** Add Host Functions for the Wasm app to request the latest sensor data (e.g., `host_get_pitch_roll()`).
* **Phase 3 Success Criteria:** You can physically tilt the ESP32-S3. Core 1 reads the tilt instantly. The Wasm app on Core 0 successfully queries this data via the ABI and transmits the telemetry to the PC via ESP-NOW, rendering a 3D simulation of the board moving in real-time.

### Phase 4: The Muscle & Survival (Motors & Fault Tolerance)

**Hardware Required:** Motor Drivers, Physical Motors, Battery system.
**Objective:** Close the loop. Drive physical hardware safely and prove the fault tolerance of the sandbox.

* **Implement Motor PWM:** Configure the Host OS to generate highly precise Pulse Width Modulation (PWM) signals on Core 1 to drive the motor controllers.
* **Build the PID Controller:** Implement the balancing/movement mathematical loop strictly on Core 1.
* **Engage the Watchdog:** Attach the ESP32-S3's hardware Watchdog Timer to the Wasm execution thread on Core 0.
* **Implement Safe Shutdown Protocol:** Program the Host OS to automatically cut motor power if the ULP paramedic detects a critical voltage drop.
* **Phase 4 Success Criteria (The Chaos Test):** The fully assembled robot is balancing itself using Core 1. You intentionally send a malicious Wasm script from the PC that contains an infinite loop or a fatal crash. Core 0 catches the crash, the Watchdog resets the Sandbox, and the Wasm app reboots—**all while the robot continues to balance perfectly without twitching or falling over.**

### Phase 5: The Nervous System Expansion (Distributed Robotics)

**Hardware Required:** A second ESP32-S3 board (The "Worker").
**Objective:** Scale the OS from a single chip to a distributed architecture. Move the motor control to the Worker chip so the Brain chip can dedicate 100% of its real-time loops to complex tasks like LIDAR mapping.

* **Abstract the Hardware:** Modify the ABI on the Brain so that a Wasm command like `host_set_motor_speed()` no longer controls local pins, but instead queues an ESP-NOW packet.
* **Build the Worker Firmware:** Flash the second ESP32-S3 with a stripped-down, purely `no_std` Embassy firmware (no Wasm, no UI). It only listens for ESP-NOW packets and translates them to motor PWM.
* **Implement the Dead-Man's Switch:** Program the Worker to automatically halt all motors if it doesn't receive a heartbeat packet from the Brain every 50ms.
* **Phase 5 Success Criteria:** The Brain ESP32 (running the Wasm AI) sends a movement command over the air. The Worker ESP32 receives it in <2ms and drives the motors. You then unplug the Brain's power; the Worker detects the missing heartbeat and stops the motors instantly, preventing a runaway robot.

### Phase 6: The Digital Twin (ROS 2 & RVIZ Integration)

**Hardware Required:** PC running Linux (Ubuntu) or a ROS 2 Docker container.
**Objective:** Bridge your custom ESP-NOW protocol into the professional ROS 2 ecosystem, allowing you to simulate and visualize the robot in real-time.

* **Build the PC Agent (The Bridge):** Write a Python or Rust script on your PC that listens to the raw UDP/ESP-NOW packets coming from the robot.
* **Map to ROS Topics:** Translate your custom telemetry packets (e.g., IMU tilt, wheel encoder speed) into standard ROS 2 message types (like `sensor_msgs/Imu` and `nav_msgs/Odometry`).
* **Establish the URDF:** Create a Unified Robot Description Format (URDF) file on your PC—a 3D XML model of your robot's physical dimensions.
* **Phase 6 Success Criteria:** You boot up RVIZ on your PC. You physically pick up and tilt the real ESP32-S3 robot with your hands. On the PC monitor, the 3D 3D model of your robot perfectly mirrors your physical movements in real-time, proving the telemetry pipeline is mathematically sound.

### Phase 7: Local AI Fallback (Edge Machine Learning)

**Hardware Required:** ESP32-S3 Camera (e.g., OV2640).
**Objective:** Utilize the ESP32-S3's specialized AI vector instructions. Give the robot a "survival instinct" to recognize basic objects or wake-words locally if the Wi-Fi connection to your PC's powerful LLM drops.

* **Train a TinyML Model:** Train a heavily quantized (int8) neural network on your PC (e.g., a simple wake-word detector or a stop-sign recognizer).
* **Integrate ESP-NN:** Map Espressif's neural network acceleration libraries into your Rust Host.
* **Create the Fallback Logic:** Program Core 0 to route microphone/camera data to the local TinyML model *only* if the ESP-NOW connection to the PC fails.
* **Expand the ABI:** Add `host_get_local_inference()` so the Wasm Sandbox can ask the Host what the local AI sees.
* **Phase 7 Success Criteria:** You disconnect the PC/Router. You say the wake-word ("Hey Robot, stop!"). The ESP32-S3 processes the audio locally using its vector instructions, the Wasm app reads the local inference, and the robot halts.

### Phase 8: The "App Store" (Dynamic Wasm OTA)

**Hardware Required:** Just the robot and your PC.
**Objective:** Achieve the ultimate goal of the Wasm Sandbox architecture: Over-The-Air (OTA) application swapping without ever flashing the core OS or rebooting the hardware.

* **Implement a Wasm Loader:** Write logic in the Rust Host to receive a binary `.wasm` file in chunks over Wi-Fi and store it in the external PSRAM.
* **Build the Sandbox Hot-Swap:** Program the Host to gracefully pause the current Wasm app, detach it from memory, and load the newly downloaded `.wasm` file into the interpreter.
* **Phase 8 Success Criteria:** The robot is actively balancing on your desk running "App A" (which makes the screen show Happy Eyes). From your PC, you compile "App B" (Angry Eyes) and send it via Wi-Fi. The robot catches the file, swaps the Sandbox brain in a fraction of a second, and the eyes turn Angry—**the robot never reboots, and the motors never stop balancing during the brain transplant.**

---

### The Journey Ahead

With these 8 phases, you go from an empty piece of silicon to a professional-grade, fault-tolerant, ROS 2-compatible, hot-swappable robotics platform.

Since your first board arrives tomorrow, **Phase 1** is your immediate target. Would you like me to map out the exact folder structure and the `Cargo.toml` dependencies you should set up on your computer right now so you are ready to compile the moment you plug it in?

