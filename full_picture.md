### 1. The Core Philosophy: "Host vs. Guest"

This OS is built on a strict "Zero Trust" model. The hardware doesn't trust the AI, and the motors don't trust the radio.

* **The Host (The Base OS):** Written entirely in `no_std` Rust using the **Embassy** async framework. It has absolute, "god-like" control over the ESP32-S3's memory, pins, and radios. It is designed to be lean, blazing fast, and mathematically proven not to crash.
* **The Guest (The Sandbox):** A WebAssembly (Wasm) Virtual Machine running *inside* the Host. This is a safe, padded room where your unpredictable AI logic, LLM text parsing, and ROS 2 translations live. It cannot touch the hardware directly.

---

### 2. The Hardware Blueprint (The Architect's Checklist)

We are explicitly mapping tasks to the ESP32-S3’s silicon to guarantee fault tolerance.

* **Core 0 (The Brain):** Runs the Wasm Interpreter and handles the **ESP-NOW Radio**. It talks to your PC, receives LLM outputs, and runs the high-level logic. If it crashes, only this core is affected.
* **Core 1 (The Muscle):** Runs pure Embassy Rust (No Wasm). It manages the **Hard Real-Time** motor control loops, reading gyroscopes, and keeping the robot balanced. It operates completely independently of Core 0.
* **The ULP Coprocessor (The Paramedic):** The tiny RISC-V core runs in the background. It constantly monitors battery voltage and hardware faults using almost zero power. If the battery gets dangerously low, the ULP can trigger a safe shutdown before the robot collapses.
* **Survival (The Watchdog):** We attach the Hardware Watchdog Timer *specifically* to the Wasm task on Core 0. If your AI logic gets stuck in an infinite loop, the Watchdog silently kills the Wasm Virtual Machine, flushes its RAM, and restarts it in milliseconds—all while Core 1 keeps the motors running perfectly.

---

### 3. Peripheral Communication: Host Functions (ABI)

Because the Wasm Sandbox is blind to the hardware, we use an **Application Binary Interface (ABI)**. This is a list of safe, pre-approved commands the Sandbox is allowed to ask the Host to perform.

**The Execution Flow:**

1. **Call:** The Wasm app decides it needs to update the screen and calls a virtual function (e.g., `host_draw_eye()`).
2. **Pause:** The Wasm Virtual Machine pauses the app and passes the coordinates to the Rust Host.
3. **Verify:** The Rust Host checks the coordinates. If the Wasm app asks to draw a pixel at `x: 9999` on a `320px` screen, the Host catches the error, preventing a memory crash.
4. **Execute:** The Rust Host safely sends the pixel data to the screen via the SPI hardware bus.
5. **Return:** Control is handed back to the Wasm app to continue thinking.

---

### 4. User Interface Strategy

Since your LLM on the PC is doing the heavy audio processing and generating text/emotions, the ESP32-S3 just needs to render it efficiently.

* **The Golden Rule:** *Never send pixels through the Sandbox.* It will choke the CPU.
* **The Solution:** The Wasm app only sends high-level intents (like "Mood: Happy, Text: 'Hello'").
* **The Engine (`embedded-graphics`):** The pure Rust Host receives these intents and uses the `embedded-graphics` crate to do the actual drawing.
* **What you can show:** You can program the Host to draw retro, blinking robot eyes that react to the LLM's mood, display scrolling text of what the LLM is saying, or render battery/status dashboards—all using math instead of heavy image files, saving massive amounts of RAM.

---

### 5. What Were You Missing? (The Final Pieces)

To make this architecture actually work in the real world, you need these two final concepts:

1. **Direct Memory Access (DMA):** When the Host sends the generated UI to the screen, or reads from a sensor, it uses DMA. This is a hardware feature that moves data directly from RAM to the SPI/I2C pins *without using the CPU*. This is how you get 60 FPS on the screen while Core 0 is busy translating Wasm logic.
2. **Memory Mapping (SRAM vs. PSRAM):** The ESP32-S3 has internal memory (fast SRAM) and external memory (slower PSRAM). You must map **Core 1 (Motors)** and the **Host OS** to the fast SRAM so the robot's reflexes are instant. You will map the **Wasm Virtual Machine** to the external PSRAM, giving your AI apps megabytes of space to run without interfering with the robot's physical stability.


