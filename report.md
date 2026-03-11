# Investigation Report: Fixing Core 0 Task Starvation and Enabling WiFi/Web Server

## Overview

The user asked to investigate an issue in the SandOS firmware on ESP32-S3 where `net_task`, `web_server_task`, and `display_task` all run on Core 0. When WiFi is enabled, heavy network processing starves the display task because the display task pushes the 1024-byte framebuffer via I2C at 400kHz. The network interrupts could also be delayed. The code currently disables network tasks ("Display-only mode") to maintain smooth UI and button polling.

The goal is to analyze the required changes to enable WiFi and the Web Server while ensuring that the animations and the BOOT button menu work smoothly without starvation.

## Current Situation

1. **Display Task:** According to `firmware/src/display.rs` and the issue description, the display task has already been converted to use `esp_hal::i2c::master::I2c<'static, Async>` which yields to the executor during the flush. `display::spawn_display_task` now uses `into_async()` for the I2C driver.
2. **Web Server Task:** The `web_server_task` yields and sleeps when disabled, and it rate-limits connections to 20ms between requests to avoid starving the display loop.
3. **Button Task:** The BOOT button uses hardware edge interrupts (`wait_for_falling_edge`), avoiding CPU spin-polling.
4. **Main Setup:** The WiFi initialization, networking tasks, ESP-NOW, and `core0::brain_task` are currently disabled and commented out in `firmware/src/main.rs`.

## How to have everything working smoothly

Since `display.rs` has already been moved to **Async I2C** (`Async` I2C yields to other tasks instead of blocking Core 0), the most robust solution without touching the code is to re-enable the networking components in `main.rs`.

Here are the required steps and changes (though the prompt asks not to change the code):

### 1. Re-enable WiFi and Network Tasks in `main.rs`
Uncomment the `WIFI_INIT` static and initialization code. We need to initialize the WiFi hardware, configure the stack, and spawn the `wifi::net_task` and `wifi::wifi_task`.

* Initialize `esp_wifi` components:
  * `esp_wifi::init` with the TIMG1 timer and RNG.
  * Initialize the `WIFI_INIT` StaticCell.
* Create the network stack and spawn the network tasks (`net_task` and `wifi_task`).

### 2. Spawn Web Server Task
Pass the `Stack` instance to `web_server_task` and spawn it using Embassy. The web server starts disabled by default, meaning it sleeps until the user enables it via the display menu (holding the BOOT button -> Web).

### 3. Spawn Brain Task
We must re-enable the spawning of `core0::brain_task`, passing the initialized WiFi resources (`wifi_init`, `espnow_token`), and other dependencies.

### 4. Handling Starvation (Why Async I2C fixes it)
Because the I2C display driver uses the `Async` trait, whenever `oled.flush().await` is called, the Embassy executor on Core 0 pauses `display_task` and switches to another ready task (e.g., `net_task` processing network traffic, or `web_server_task`). Once the DMA I2C transfer is complete, it triggers an interrupt that wakes `display_task` back up.

Since the network task runs quickly, and I2C yields during the ~25ms it takes to flush the display frame, Core 0 is effectively multiplexed. The Web UI and DHCP network stack will get the CPU time they need without causing the display animation to visibly freeze or the button edges to be missed.

### 5. Web Server Toggling Workflow
1. At boot, `web_server::is_web_server_enabled()` is false.
2. The `web_server_task` sleeps and checks every 500ms.
3. The user short presses BOOT to enter Menu Mode.
4. The user navigates to "Web" and long presses BOOT.
5. The display toggles the state, and `web_server::enable_web_server()` is called.
6. `web_server_task` detects the enabled state, waits for the network DHCP lease (IP assignment), and starts listening on port 80.
7. The Dashboard is served on the IP address. This does not block Core 0 due to the `Timer::after(Duration::from_millis(20)).await` added in the web server task loop, ensuring the display task is not starved when there are back-to-back requests.

### Conclusion
The architecture to fix the starvation is already structurally present in `display.rs` via `esp_hal::i2c::master::I2c<'static, Async>`. The next step is simply to uncomment the commented WiFi and ESP-NOW initialization in `main.rs` and spawn the `wifi_task`, `net_task`, `web_server_task`, and `brain_task`. Because I2C transfers are async, they yield the CPU to the WiFi stack during display updates, meaning all processes will share Core 0 smoothly.
