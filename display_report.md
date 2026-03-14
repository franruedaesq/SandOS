# SandOS Display Optimization Report

## Current Implementation vs Optimizations

This report compares the current implementation of the SandOS display driver against the suggested optimizations for the ILI9341 screen.

### 1. Stick to RGB565

**Status: Applied**
The codebase already uses `RGB565` color format.
* In `firmware/src/display/mod.rs`, `Rgb565` is imported from `embedded_graphics::pixelcolor::Rgb565`.
* In `firmware/src/display/ui.rs`, methods implementing rendering trait bounds like `D: DrawTarget<Color = Rgb565>` strictly enforce the use of RGB565 format (2 bytes per pixel), which optimally balances visual quality and memory usage for the screen's 65K color capabilities.

### 2. Implement partial screen updates

**Status: Partially Applied / Implemented Custom Logic**
Instead of avoiding redrawing the entire screen every frame, the project has already partially implemented a solution that tracks the moved areas and selectively clears the background.
* In `firmware/src/display/ui.rs`, variables track the previous positions of elements (e.g., `prev_r_kun_x`, `prev_menu_offset`).
* A `force_redraw` flag dictates if a full `display.clear(bg_color)` is required.
* For typical movements (like breathing bounce or UI sliding), partial screen rectangles (`Rectangle::new`) are drawn over the previous positions with the background color (`bg_color`), before redrawing the elements at their new positions.
* **Diagnosis:** While this mimics partial updates, using native partial bounds setting on the ILI9341 hardware might further reduce SPI bandwidth compared to overwriting shapes, but the current implementation avoids a massive 240x320 screen buffer.

### 3. Skip full-screen double buffering

**Status: Applied**
The application avoids allocating a full frame buffer (which would be ~153.6 KB per buffer at 240x320 RGB565).
* Rendering is done using primitives from `embedded-graphics` drawing directly to the ILI9341 display wrapper.
* The system saves significant RAM for OS-level tasks.

### 4. Utilize DMA (Direct Memory Access)

**Status: Not Applied**
Currently, the application initializes the SPI interface using the default `Blocking` mode.
* In `firmware/src/display/mod.rs`: `Spi::new(...)` is used instead of setting up asynchronous DMA channels (`dma_buffers!` and `SpiDma`).
* `display.flush().await` is commented out.
* The current setup forces the CPU to wait on SPI transfers.

### 5. Render in chunks or scanlines

**Status: Not Applied**
The rendering currently delegates directly to the `ili9341` crate which translates `embedded-graphics` primitives into blocking SPI commands pixel-by-pixel or shape-by-shape.
* To achieve scanline rendering with DMA, the rendering pipeline needs to be modified to render the `embedded-graphics` objects into a smaller off-screen chunk buffer (e.g., `[u8; 10240]`), and then flushed chunk-by-chunk over DMA to the screen.

---

## Important Display Parameters

The following variables/constants dictate the visual experience and can be tweaked:

- **`DISPLAY_WIDTH` & `DISPLAY_HEIGHT`**: `240` and `320`.
- **`FRAME_PERIOD` equivalent**: Controlled currently by `Timer::after(Duration::from_millis(10)).await;` in `display_task`. Adjusting this wait time manages the framerate versus CPU/SPI yielding.
- **`SPI frequency`**: `spi_cfg.frequency = 40.MHz();`. Can be tweaked to 80 MHz, although 40 MHz is often the safe max for stable ILI9341 operations over jumper wires.
- **Animations:**
  - `bounce_period = 120` (frames) for the breathing effect.
  - `magnitude = 3` (pixel shift) for the breathing effect.
  - Blink chance: `self.frame_count % 300 < 5`
  - Touch interaction debounce/ripple radius: `self.ripple_radius += 4` maxing at `40`.

---

## Diagnosis and Improvements

1.  **Adopt DMA with SPI**: Move from blocking SPI to `SpiDma::new` using async channels. This is the most crucial step as writing shapes to the screen blocking the CPU hinders the rest of the OS tasks.
2.  **Chunked Framebuffer Render (`embedded-graphics` -> DMA buffer)**: Implement an adapter that renders chunks of the display (e.g., 240x20 slices) into a small SRAM buffer, and send these chunks via SPI DMA asynchronously. This allows the CPU to calculate the next slice while the current one is transmitted, improving both framerate and freeing the CPU.
3.  **Refine Partial Erasures**: The current partial erasure draws a larger bounding box to cover previous states. If moving to chunked DMA rendering, you might maintain dirty rectangles at the system level and only render the chunks intersecting those dirty areas.
4.  **SPI Bus Contention**: Verify if the `embedded-sdmmc` crate sharing SPI requires arbitration. If the display gets DMA, ensure the SD card accesses do not collide during active transmissions. (Currently, SD is on SPI3, and Display is on SPI2, avoiding collision, but good to keep in mind).
