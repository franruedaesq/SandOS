# Evaluation: Adapting SandOS to the Ideaspark ESP32 Development Board

This document evaluates the feasibility, difficulty, and potential impact of adapting the SandOS operating system to run on an Ideaspark ESP32 development board, compared to the original ESP32-S3-DevKitC-1-N16R8.

## 1. Hardware Comparison

| Feature | Original Board (ESP32-S3-DevKitC-1-N16R8) | Target Board (Ideaspark ESP32) | Impact on SandOS |
| :--- | :--- | :--- | :--- |
| **Microcontroller** | ESP32-S3 (Xtensa Dual-Core 32-bit LX7) | ESP32 (Xtensa Dual-Core 32-bit LX6) | **High** - Different architecture target and HAL features. |
| **SRAM** | 512KB | 520KB (16KB Cache) | **Low** - Similar internal memory for Core 1 and Host OS. |
| **PSRAM (External RAM)**| 8MB | **None** | **Critical** - SandOS relies on PSRAM for the Wasm Virtual Machine. |
| **Flash Storage** | 16MB | 4MB | **Medium** - Tighter constraints for OS, OTA partitions, and Wasm apps. |
| **Built-in Peripherals**| None | 2.44cm OLED Display (CH340 USB driver) | **Medium** - Requires new display driver (likely I2C instead of SPI DMA). |

## 2. Can the same OS work on both out of the box?

**No.** The current SandOS firmware is explicitly compiled for the `xtensa-esp32s3-none-elf` target and relies heavily on the `esp-hal/esp32s3` features. Furthermore, its memory architecture is built under the assumption that a large pool of external PSRAM is available to host the WebAssembly (`wasmi`) guest applications.

If you attempt to flash the current SandOS firmware onto the Ideaspark ESP32, it will fail to boot or immediately crash due to architectural differences (LX7 vs LX6) and missing memory regions (PSRAM).

## 3. Key Problems & Difficulties in Adapting SandOS

If you choose to adapt SandOS for the Ideaspark board, you will face the following major challenges:

### A. The Microcontroller Architecture Change (ESP32-S3 -> ESP32)
* **Target Swap:** You must change the Rust compilation target from `xtensa-esp32s3-none-elf` to `xtensa-esp32-none-elf`.
* **HAL Migration:** The `esp-hal` configuration in `Cargo.toml` must be updated from `esp32s3` to `esp32`.
* **Peripheral Mapping:** The ESP32 and ESP32-S3 have different internal peripheral mappings. DMA channels, SPI/I2C controllers, and interrupt routing will need to be reconfigured.

### B. The "PSRAM Problem" (Critical Barrier)
* **Wasm Memory Hunger:** SandOS explicitly maps the Wasm Virtual Machine (Core 0) to external PSRAM. Wasm guests, even small ones, require significant memory overhead for their linear memory space, interpreter state, and stack.
* **SRAM Constraints:** The Ideaspark board only has 520KB of internal SRAM. This memory must be shared by:
    * The Embassy async runtime.
    * The Wi-Fi/Network stack (which is very RAM-intensive).
    * Core 1's real-time motor control loops.
    * The Host OS state.
* **Difficulty:** Fitting the `wasmi` interpreter and guest applications into the remaining SRAM (after the OS and Networking take their share) will be extremely difficult, if not impossible. You would likely need to severely restrict the capabilities of the Wasm guest apps or remove the Wasm sandbox entirely, which defeats the core philosophy of SandOS.

### C. Flash Storage Constraints (16MB -> 4MB)
* **OTA Updates:** SandOS supports Over-The-Air (OTA) updates for Wasm applications. A 4MB flash must house the bootloader, partition table, the Rust firmware itself, and leave room for at least two Wasm application partitions (for active and OTA staging).
* **Difficulty:** You will need to carefully tune the partition table (`partitions.csv`) to ensure everything fits, leaving very little room for large guest logic or assets.

### D. Display Integration
* **OLED Driver:** The Ideaspark board features a built-in 2.44cm OLED display. The current SandOS documentation (Phase 2) mentions an SPI display using DMA. The Ideaspark OLED is highly likely to be an SSD1306 or SH1106 running over I2C.
* **Difficulty:** You will need to write or integrate a new Host ABI function to render the LLM/Emotions to an I2C OLED display instead of the current SPI display. Fortunately, `embedded-graphics` supports SSD1306, but the DMA strategy mentioned in the OS architecture will need to be re-evaluated for I2C.

## 4. Conclusion

Adapting SandOS to the Ideaspark ESP32 is a **high-difficulty** task primarily because the Ideaspark board lacks PSRAM. The Zero-Trust, Wasm-sandboxed architecture of SandOS relies heavily on external memory to separate unpredictable AI logic from the real-time Host OS. Running this architecture entirely within 520KB of SRAM will require severe compromises to the size and complexity of the applications you can run.
