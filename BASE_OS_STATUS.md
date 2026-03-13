# Base OS Status

This document summarizes the transition of SandOS from a standalone "Head OS" (which handles hard real-time motor control) into an async-first master controller ("Base OS"). The Base OS serves as the central nervous system, focused on UI, network I/O, AI communication, and application management.

## What Works

1. **Dual-Core Architecture Foundation**:
   - Core 0 runs the Wasm VM, Embassy async executor, and network loops.
   - Core 1 has been successfully stripped of hard real-time motor/balance control, leaving it available for high-throughput or complex tasks like continuous SD-Card streaming without interrupting the main OS loop.

2. **Display Infrastructure (ILI9341 & SPI)**:
   - The former I2C `SSD1306` + `embedded-graphics` setup has been removed.
   - Replaced with a faster SPI-driven `ILI9341` configuration (via `display-interface-spi` and `embedded-hal-bus`).
   - The OS now includes a "Think-Wait" animation loop to ensure LVGL renders (`rlvgl`) get consistent CPU time without being starved by network or Wasm execution pauses.

3. **SD Card Integration & Wasm Loading**:
   - `embedded-sdmmc` has been fully integrated.
   - The Wasm Engine on Core 0 automatically attempts to read the initial `app.wasm` binary from the SD card (`firmware/src/sd_card.rs`), falling back to a baked-in default if missing.
   - A chunked streaming stub (`stream_asset_chunk`) exists for future dynamic loading of 3D assets or textures directly into the LVGL engine, conserving valuable SRAM.

4. **rclaw Internal Agent Logic**:
   - A `no_std` `rclaw` workspace crate has been stubbed out to map High-Level AI intents (e.g., `{"intent": "look happy"}`) into physical Actions and Expressions.

5. **OpenAI Voice Integration Stubs**:
   - ABI Host Functions (`FN_START_AUDIO_OPENAI`, `FN_DISPATCH_INTENT`, `FN_LVGL_LABEL_SET_TEXT`, `FN_SET_AVATAR_EXPRESSION`) are fully registered.
   - The foundation to bridge microphone audio via HTTP POST to the OpenAI API and capture TTS responses is in place via `firmware/src/openai.rs`.

## What Won't Work Yet

- **Actual UI Rendering**: While `rlvgl` and the display drivers are configured, the exact LVGL UI widgets, Lottie files, and touch inputs (`XPT2046`) have not been implemented or mapped to the screen buffer. The display loop is currently mocked to "flush" cleanly without crashing.
- **Real OpenAI Network Calls**: The `openai.rs` methods currently log actions but do not initiate actual `embassy-net` TCP/HTTP transactions.
- **Physical Head OS Link**: The dispatch intent function successfully catches actions but doesn't yet serialize and broadcast them over ESP-NOW to the remote Head OS.

## Next Steps for Flashing

The firmware is technically ready to flash, but it acts primarily as an invisible command-router in its current state.

1. To run host-side simulation:
   ```bash
   cargo test -p host-tests
   ```
2. To compile and validate for the ESP32-S3 target:
   ```bash
   cd firmware
   cargo build --release
   ```
3. To flash hardware (requires Espressif toolchain):
   ```bash
   cd firmware
   cargo run --release
   ```

Before flashing for a full visual demo, the next immediate priority is initializing the `Rlvgl` core in `main.rs` and drawing the initial Glassmorphism dashboard on the 2.8" screen.
