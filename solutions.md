# Solutions for SandOS Freezing with Wi-Fi, Web Server, and Display

## The Root Cause: Core 0 Starvation

The architecture of SandOS is not fundamentally broken, and **yes, it is absolutely possible to have all these features working together simultaneously.**

The freezing you are experiencing is a classic symptom of **cooperative multitasking starvation** on Core 0.

Here is what is happening under the hood:
1.  **Shared Core:** The Wi-Fi driver (`net_task`), the Web Server (`web_server_task`), and the Display driver (`display_task`) are all currently assigned to run on **Core 0** (The Brain) within the exact same Embassy async executor.
2.  **Blocking I2C Transfers:** The display driver currently uses a `Blocking` I2C implementation (`esp_hal::i2c::master::I2c<'static, Blocking>`). Pushing a full 1024-byte framebuffer to the OLED screen over I2C at 400kHz takes around 25-40 milliseconds. During this time, the display task completely holds the Core 0 CPU and prevents any other tasks from running.
3.  **Heavy Network Traffic:** When the Wi-Fi stack and Web Server are active, they generate a lot of CPU-intensive network interrupts and data processing (e.g., DHCP negotiation can take up to 30 seconds).
4.  **The Freeze:** Because the display is using blocking I2C calls, it either starves the network stack (causing timeouts), or the network stack’s heavy processing starves the display (causing the animations to stop and the button polling to miss your presses). This is why the code currently mentions that the `net_task` can starve the display, and has a "display-only mode" workaround.

To fix this, we need to allow Core 0 to multiplex these tasks more efficiently without one blocking the others. Here are the proposed solutions, ordered from easiest to most robust.

---

## Solution 1: Use Async I2C with DMA for the Display (Recommended)

This is the most "correct" solution according to the SandOS architecture guidelines ("Phase 2" specifically mentions DMA display drivers).

**How it works:**
Instead of using `Blocking` I2C, we switch the `esp-hal` I2C driver to its `Async` version and attach a Direct Memory Access (DMA) channel to it.

**Why it fixes the problem:**
When it's time to draw a frame, Core 0 tells the DMA hardware "Here is the memory address of the 1024-byte framebuffer, please send it to the I2C pins." Core 0 then immediately yields (`.await`). The DMA hardware handles sending the data in the background, freeing up Core 0 to immediately switch over and process Wi-Fi packets, web server requests, and poll the BOOT button. When the frame is finished sending, the DMA triggers an interrupt, and the display task wakes up to draw the next frame.

**Effort Level:** Medium. Requires changing the `I2c` setup in `firmware/src/display/mod.rs` to use `I2c::new_async` with a DMA channel, and changing the `flush()` methods to use `.await`.

## Solution 2: Hardware Interrupts for the BOOT Button

Currently, the BOOT button is read by *polling* it every 10ms-40ms inside the display loop. If the display is frozen by network traffic, the poll never happens, and your button presses are ignored.

**How it works:**
Configure the BOOT button (GPIO 0) to trigger a hardware interrupt on the falling edge (when pressed).

**Why it fixes the problem:**
Hardware interrupts preempt the currently running task. Even if the network task is aggressively using the CPU, pressing the button will instantly pause the network task, record the button press in a tiny interrupt handler, and resume. The display task can then read this saved state whenever it next gets a chance to run.

**Effort Level:** Low-Medium. Requires changing `esp_hal::gpio::Input` to use async `wait_for_falling_edge().await` in a dedicated tiny Embassy task, rather than polling it manually inside the display's render loop.

## Solution 3: Prioritized Executors on Core 0

The Embassy framework allows you to run multiple executors on a single core, assigning them different hardware interrupt priority levels.

**How it works:**
You can create a low-priority executor for the `net_task` and `web_server_task`, and a high-priority executor for the `display_task` and button polling.

**Why it fixes the problem:**
If the network task is running, and the display task's 100ms timer expires, the high-priority executor will literally interrupt the low-priority executor, run the display code, and then hand control back to the network. This guarantees the display always gets its time slice.

**Effort Level:** Medium. Requires modifying `firmware/src/main.rs` to set up Software Interrupts (SWI) for multiple Embassy executors.

---

## Summary

The architecture is sound, but the current implementation of the display driver is "hogging" Core 0.

I strongly recommend implementing **Solution 1** (Async DMA I2C) combined with **Solution 2** (Async Button Interrupts). This will make the UI silky smooth, drastically reduce Core 0 CPU usage, and allow the Web Server, Wi-Fi, and Display to run simultaneously without any freezing.