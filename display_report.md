# Display Rendering Report

This report evaluates the current display rendering implementation (`firmware/src/display/driver.rs` and `firmware/src/display/mod.rs`) against the provided best-practice criteria for memory efficiency, fluid UI experience, and hardware utilization on the ESP32-S3.

## 1. Analysis of Current Implementation

### Stick to RGB565
*   **Status:** **Applied**
*   **Details:** The `embedded-graphics` `Rgb565` color format is correctly used as the main display format, allowing 65K colors with 2 bytes per pixel. This ensures an optimal balance between visual quality and memory usage.

### Implement Partial Screen Updates
*   **Status:** **Partially Applied**
*   **Details:** The UI logic in `firmware/src/display/mod.rs` tracks previous states (`prev_expression`, `prev_ui_mode`, etc.) using the `FaceState` struct. It successfully avoids redrawing the *entire* screen every frame during idle animations. For instance, when the face blinks or bobs, it only erases the bounding boxes of the eyes and mouth (`erase_face`), leaving the rest untouched. A full clear (`force_clear = true`) is only triggered upon major UI transitions (like moving from Face to Menu). However, the underlying hardware driver (`TftDisplay::flush()`) ignores this optimization because it always transmits the entire 153.6 KB buffer over SPI, regardless of whether only a small region changed.

### Skip Full-Screen Double Buffering
*   **Status:** **Not Applied**
*   **Details:** The ESP32-S3 chip has limited SRAM (512KB). The current `TftDisplay` structure allocates a full `DISPLAY_BUF_SIZE` (`320 * 240`) array of `u16` dynamically on the heap (PSRAM via `alloc::alloc::alloc_zeroed`). This single buffer consumes 153.6 KB (320 * 240 * 2 bytes). While it's single-buffering rather than double-buffering, it is still storing a full framebuffer, which takes up a massive chunk of available memory.

### Utilize DMA (Direct Memory Access)
*   **Status:** **Implicitly Applied / Requires Optimization**
*   **Details:** The `esp-hal` `Spi` driver used in `Async` mode typically utilizes DMA under the hood for large transfers. In `flush()`, the buffer is written in chunks of 8192 bytes: `self.spi.write(chunk).await`. This ensures the CPU yields during the SPI transfer. However, as noted in memory constraints for ESP32-S3 `esp-hal`, a single DMA descriptor supports max 4092 bytes. While the async HAL handles breaking it down, the constant pushing of 153.6 KB per frame saturates the bus and DMA engine unnecessarily.

### Render in Chunks or Scanlines
*   **Status:** **Not Applied**
*   **Details:** Instead of allocating a small horizontal slice (e.g., 10 KB buffer) and rendering just that slice, the codebase renders everything into the massive 153.6 KB full-screen buffer first, and then flushes it all at once. This defeats the purpose of chunked rendering, which is designed specifically to eliminate the need for large memory allocations.

---

## 2. Important Configurable Values

The following are crucial variables and constants in the codebase that affect screen fluidity, framerate, and memory footprint. Modifying these is key to tweaking the experience:

*   **Resolution:**
    *   `DISPLAY_WIDTH`: `320`
    *   `DISPLAY_HEIGHT`: `240`
    *   *Modifying these requires a hardware change, but they dictate the total memory footprint.*

*   **Memory & Transfer:**
    *   `DISPLAY_BUF_SIZE`: `320 * 240` (153.6 KB total buffer size). *Can be drastically reduced if moving to scanline rendering.*
    *   `SPI Chunk Size (in flush)`: `8192` bytes. *This size is passed to the async SPI write. Adjusting this might affect DMA descriptor overhead.*

*   **Framerate & UI:**
    *   `FRAME_PERIOD`: `Duration::from_millis(33)` (~30 FPS). *Reducing this value attempts a higher framerate, but it might bottleneck if the SPI bus takes longer than the period to flush the 153.6 KB buffer.*
    *   `DISPLAY_QUEUE_DEPTH`: `8`. *Command queue capacity for Wasm-to-Display intents.*

---

## 3. Diagnosis and Points to Improve

### Diagnosis
The current UI loop successfully employs state-tracking logic to minimize drawing CPU cycles. However, this optimization is completely negated by the hardware flush layer, which brute-forces the transmission of the entire 153.6 KB framebuffer over the SPI bus every single frame (~30 times a second). This results in high bus utilization, potential framerate drops if the async executor is starved, and an unnecessary 153.6 KB constant memory overhead.

### Suggestions for Improvement

1.  **Refactor to Partial DMA Updates (Dirty Rectangles):**
    Since the UI already tracks what changes, update the `TftDisplay` driver to accept a bounding box `(x, y, width, height)` when flushing. Instead of sending the full 153.6 KB array, set the ILI9341 column address (`0x2A`) and page address (`0x2B`) commands to exactly match the dirty rectangle, and only send the pixel data for that specific region. This will drastically reduce SPI bus traffic and eliminate visual tearing.

2.  **Transition to Scanline/Chunked Rendering:**
    If dirty rectangles are too complex, eliminate the massive 153.6 KB `buffer` in `TftDisplay`. Instead, allocate two small ~10 KB buffers (`dma_buffers!`).
    *   Set the ILI9341 window to full screen.
    *   Loop through the screen vertically (e.g., 20 scanlines at a time).
    *   `embedded-graphics` can draw just the primitives that intersect the current 20-scanline chunk.
    *   Initiate an async DMA transfer for chunk A, while the CPU simultaneously renders the next 20 scanlines into chunk B.

3.  **Optimize SPI DMA Chunking:**
    The current `flush()` splits the buffer into `8192` byte chunks manually before awaiting the SPI write. Ensure this aligns well with the `esp-hal` DMA descriptor limits (4092 bytes). Utilizing the `dma_buffers!` macro natively provided by `esp-hal` for I2S/SPI might provide better throughput than the manual slice iteration currently used.

4.  **Increase SPI Clock Speed:**
    Ensure the SPI frequency is configured to the maximum stable speed supported by the ILI9341 panel (often around 40 MHz - 80 MHz, depending on wiring). This drastically reduces the time it takes to push frames.