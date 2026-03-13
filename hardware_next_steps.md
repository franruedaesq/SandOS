# SandOS Hardware Next Steps

This document details the specific technical steps required to bring the partially connected or uninitialized hardware components from a "Connected" or "Stubbed" state to a fully "Connected & Functional" state within the SandOS ecosystem.

## 1. Capacitive Touchscreen (D-FT6336G)
**Current Status:** I2C bus initialized (IO15/IO16). Interrupt (IO17) and Reset (IO18) pins acquired.
**Next Steps for Full Functionality:**
1.  **Add Driver Crate:** Add an existing I2C FT6336G driver crate (e.g., `ft6336`) to `firmware/Cargo.toml` or write a minimal `no_std` driver locally.
2.  **Initialize Driver:** In `firmware/src/main.rs`, pass the initialized `_i2c` bus and the `_touch_rst` / `_touch_int` pins into the driver instance.
3.  **Create Touch Task:** Spawn a new async Embassy task (e.g., `touch_task`) that awaits falling edges on the `_touch_int` pin using `.wait_for_falling_edge().await`.
4.  **Read and Route:** Upon interrupt, read the X/Y coordinates from the FT6336G registers via I2C and route them into the SandOS `router` via a new channel (e.g., `TOUCH_EVENT_CHANNEL`) to be consumed by the UI or Wasm VM.

## 2. Battery Support Socket
**Current Status:** ADC pin (IO9) acquired.
**Next Steps for Full Functionality:**
1.  **Initialize ADC:** In `firmware/src/main.rs`, instantiate the ESP32-S3 ADC1 peripheral and configure IO9 as an analog reading channel with the appropriate attenuation (likely 11dB for 0-3.3V range).
2.  **Create Battery Task / ULP Logic:** Since SandOS uses the ULP for background monitoring, integrate the ADC reading into the `ulp::start()` logic, or spawn a low-frequency Embassy task (`battery_task`) that periodically triggers an ADC read.
3.  **Voltage Calculation:** Apply the correct voltage divider math to convert the raw ADC value to the actual battery voltage (e.g., 3.7V - 4.2V).
4.  **Telemetry Integration:** Add the battery voltage to the `TelemetryPacket` in `abi.rs` so it is broadcast over ESP-NOW and accessible to the Wasm VM via a host ABI call.

## 3. MicroSD Storage Slot
**Current Status:** SDIO pins (IO38, 39, 40, 41, 47, 48) acquired. Memory memory notes `embedded-sdmmc` crate over SPI, but pins map to SDIO.
**Next Steps for Full Functionality:**
1.  **Determine Bus Mode:** While standard SDIO is faster, the `embedded-sdmmc` crate mentioned in architecture notes is SPI-based. If using SDIO, a dedicated ESP-IDF or custom raw SDIO MAC driver is required. If falling back to SPI mode for simplicity/compatibility, re-initialize IO40 (CMD) as MOSI, IO38 (CLK) as SCK, IO39 (D0) as MISO, and IO41 (D1) as CS.
2.  **Initialize File System:** Instantiate the `embedded_sdmmc::SdMmcSpi` block device and wrap it in a `VolumeManager`.
3.  **Wasm Loader Integration:** Implement logic in `wasm_vm.rs` to open the root directory of the SD card, find `guest.wasm`, and read it into the allocated PSRAM buffer instead of (or as a fallback to) the hardcoded firmware bytes.

## 4. Built-in Microphone & Audio Speaker Interface (I2S)
**Current Status:** I2S pins (IO4, 5, 6, 7, 8) acquired. Audio enable pin (IO1) set low.
**Next Steps for Full Functionality:**
1.  **Initialize I2S Peripheral:** In `firmware/src/main.rs`, instantiate the `esp_hal::i2s::I2s` peripheral in Standard or Philips mode depending on the specific codec hardware on the board.
2.  **Configure Duplex or Split:** Configure the I2S peripheral for simultaneous RX (Microphone on IO8) and TX (Speaker on IO6) using the shared clocks (IO4 MCLK, IO5 BCLK, IO7 LRCK).
3.  **DMA Buffers:** Set up DMA descriptors for continuous asynchronous I2S reading and writing to prevent CPU blocking.
4.  **Connect to ABI:** Hook the incoming DMA microphone buffers into the `AUDIO_INFERENCE_CHANNEL` in `router.rs` so the Host can pass audio data to the OpenAI API or local fallback inference. Create a corresponding task to stream ABI output audio bytes back out to the I2S TX DMA buffer.

## 5. Physical Buttons
**Current Status:** Boot button (IO0) is connected and functional (toggles Web Server). EN pin acts as hard reset.
**Next Steps for Full Functionality:**
*   **Status:** The physical buttons are actually fully functional as described. The Boot button is successfully monitored by `button_task` in `display/mod.rs` via hardware edge interrupts, and the EN pin requires no software intervention. No further "next steps" are required for this specific component.