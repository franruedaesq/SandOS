# Proposal: New Features for ESP32-S3 Mini OLED Screen

This document outlines several feature ideas that can be added to our current ESP32-S3 setup. These features utilize the existing hardware (0.96" 128x64 OLED screen, ESP32-S3, and BOOT button) and software architecture (Embassy async framework, embedded-graphics) without requiring extra hardware modules or massive overhauls to the existing code.

The goal is to provide additional functionality via the BOOT button menu while maintaining the current fault-tolerant OS design. All proposed features carry minimal risk to the core operating system.

---

## 1. OLED Flashlight Mode
**Description:**
Turns the entire 0.96" OLED screen completely white (all pixels ON) and sets the screen brightness/contrast to maximum to act as a makeshift mini flashlight.

**Implementation Details:**
- **UI Mode:** Add a `Flashlight` variant to `enum UiMode` in `firmware/src/display/mod.rs`.
- **Drawing:** Use `embedded_graphics` to draw a solid `BinaryColor::On` rectangle filling the 128x64 display, or simply call `oled.clear(BinaryColor::On)`.
- **Brightness:** Temporarily adjust the SSD1306 contrast to maximum (`0xFF`) using the existing `set_contrast` I2C command. Restore normal contrast upon exiting.
- **Interaction:** Add "Flashlight" as an item in the BOOT button menu. A short press inside the flashlight mode could exit back to the menu or face mode.

**Risk/Impact:**
*Minimal.* Only involves a new UI state and basic OLED drawing commands. No impact on the core Wasm loop or real-time tasks.

---

## 2. System Vitals Dashboard (The Paramedic View)
**Description:**
A simple diagnostic screen showing the current system status, such as Wi-Fi connection state, current IP address (if Web server is ON), ULP-monitored voltage/temperature (if available), and memory stats if accessible.

**Implementation Details:**
- **UI Mode:** Add a `SystemStats` variant to `enum UiMode`.
- **Drawing:** Render text using `MonoTextStyle` displaying simple text metrics.
- **Data Source:** Pull data from existing atomic variables or global state (e.g., `WIFI_STATE`, `WEB_SERVER_STATE`) that the Wasm brain or Paramedic already monitor.
- **Interaction:** Accessible via the BOOT button menu.

**Risk/Impact:**
*Minimal.* Only reads existing state data and draws text. Does not interfere with the core async loops.

---

## 3. Screen Saver / Idle Animation
**Description:**
If the device is inactive (no Wi-Fi requests, no ESP-NOW commands, no button presses) for a certain duration, switch to a low-power screen saver. This could be a bouncing logo (like the classic DVD logo) or a starry sky animation, which helps prevent OLED burn-in.

**Implementation Details:**
- **Logic:** We already have an inactivity timeout (`10 s of inactivity in menu/action mode → auto-return to face mode`). We can extend this to trigger an `Idle` UI mode after, say, 60 seconds of zero external interaction.
- **Drawing:** A simple bouncing 10x10 square or moving dots using `embedded_graphics`.
- **Interaction:** Any BOOT button press or incoming command instantly wakes the screen back to `Face` mode.

**Risk/Impact:**
*Low.* Prevents OLED burn-in. Requires a simple timeout check in the display task loop.

---

## 4. Pomodoro / Focus Timer
**Description:**
A simple 25-minute countdown timer displayed on the screen, useful for productivity.

**Implementation Details:**
- **UI Mode:** Add a `Pomodoro` variant to `enum UiMode`.
- **Logic:** When selected from the menu, record the start `Instant::now()` and calculate the remaining time in the display loop.
- **Drawing:** Display large text showing `MM:SS`. We can use the existing `MonoTextStyle`. When the timer reaches 00:00, the robot face could blink rapidly or show a "Done!" message.
- **Interaction:** A short press pauses/resumes, a long press resets or exits.

**Risk/Impact:**
*Minimal.* Purely visual math in the display driver. Non-blocking and relies on the existing Embassy `Instant` timekeeping.

---

## 5. One-Button "Dinosaur" Game
**Description:**
A simple endless runner game (similar to the Chrome Dinosaur game) played entirely using the BOOT button. The player controls a small sprite that jumps over obstacles.

**Implementation Details:**
- **UI Mode:** Add a `Game` variant to `enum UiMode`.
- **Logic:** Maintain game state (player Y position, obstacle X position, score). The display task loop updates positions based on `FRAME_PERIOD`.
- **Interaction:** A short press of the BOOT button triggers a "jump" (adjusts player Y velocity).
- **Drawing:** Draw simple rectangles for the player and obstacles.

**Risk/Impact:**
*Low to Moderate.* The game loop logic must run inside the async display task. Since the display task already yields during I2C flushes, it shouldn't starve other tasks, but the game logic must be kept extremely simple and non-blocking to maintain the async architecture.

---

## 6. Wasm App Switcher (Local Demo)
**Description:**
A menu to swap between basic hardcoded expressions or local Wasm apps (if Phase 8 OTA is not fully ready but we have multiple binaries in flash). For now, it could simply change the default face expression permanently.

**Implementation Details:**
- **UI Mode:** Add `AppMenu` to `enum UiMode`.
- **Interaction:** Select "Happy", "Angry", "Sad" from the menu, which updates a global `EyeExpression` state override.

**Risk/Impact:**
*Minimal.* Already partially supported by the `host_draw_eye(expression)` ABI.

---

## Summary
All of these features leverage the existing `embassy` async task structure, the `embedded_graphics` crate, and the SSD1306 OLED driver we already wrote. None of them require new hardware, and they only require adding new states to the `UiMode` enum in the display driver, meaning they will safely run on Core 0 without interfering with Core 1's real-time loops. The "OLED Flashlight" and "System Vitals" are the easiest and most immediately useful features to implement.
