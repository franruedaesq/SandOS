# Proposed UI Expression Interactions

This document outlines proposed expansions for when specific facial expressions could be triggered by the system. Building upon the current logic, these suggestions aim to make the companion interface feel more alive, responsive to its hardware state, and reactive to user interactions.

## 1. Neutral
**The default, calm state.**
* **Current:** Default idle expression.
* **Proposed Interactions:**
  * Reverts to Neutral after successfully processing a voice command.
  * Reverts to Neutral 3 seconds after the user dismisses a notification.
  * Used as a baseline when transitioning between menus or different system tasks.

## 2. Happy
**Wide, curved crescent eyes.**
* **Current:** Used when WiFi connects.
* **Proposed Interactions:**
  * **System Updates:** Displayed upon a successful Over-The-Air (OTA) firmware hot-swap.
  * **Power:** Triggered when the device is plugged into a charger or reaches 100% battery.
  * **Task Completion:** Shown when the user successfully starts a Pomodoro session or toggles a tool (like Flashlight) on.
  * **AI Companion:** Displayed when the AI agent successfully resolves a complex intent or gives a positive text response.

## 3. Sad
**Downturned, inverted crescent eyes.**
* **Current:** Displayed on WiFi errors or timeouts.
* **Proposed Interactions:**
  * **Battery:** Triggered when the battery level drops below 15% (Low Battery Warning).
  * **Network:** Displayed if the device unexpectedly loses its ESP-NOW link or routing connection.
  * **Errors:** Shown when a requested Wasm application fails to load from the SD card.
  * **AI Companion:** Used when an API request (e.g., to OpenAI) fails or times out.

## 4. Angry
**Focused expression with gritted teeth.**
* **Current:** Appears after repeated connection failures.
* **Proposed Interactions:**
  * **Hardware Faults:** Triggered if a connected sensor (like the IMU) fails to initialize or stops reporting data.
  * **Motor Errors:** Displayed if the Dead Man's Switch trips, forcing an emergency stop due to lack of communication.
  * **Overheating:** Shown if internal device temperature sensors exceed a safe threshold.
  * **Invalid Input:** Displayed if the user repeatedly inputs an invalid combination (like holding the button during an invalid state).

## 5. Surprised
**Eyes wide open with an "O"-shaped mouth.**
* **Current:** Triggered when a new ESP-NOW message is received, or when the Pomodoro timer completes.
* **Proposed Interactions:**
  * **Sensors:** Triggered by a sudden change in pitch/roll from the IMU (e.g., if the robot/device is picked up suddenly).
  * **Interaction:** Shown immediately when the capacitive touch screen detects a tap after a long idle period.
  * **System Events:** Displayed when a new, unexpected device joins the ESP-NOW network cluster.

## 6. Thinking
**Eyes accompanied by a wavy, squiggly mouth.**
* **Current:** Displayed while attempting to connect to WiFi.
* **Proposed Interactions:**
  * **AI Integration:** Displayed while waiting for an HTTP response from the OpenAI API (or other network services).
  * **System Loading:** Shown while reading the SD card directory to find and load a Wasm application.
  * **Processing:** Displayed while parsing a large Over-The-Air (OTA) chunk or verifying the CRC32 of a new binary.

## 7. Blink
**A transitional state showing closed eyes.**
* **Current:** Happens randomly during idle.
* **Proposed Interactions:**
  * **Transition:** Forced blink immediately before transitioning into a full-screen menu or returning to the face mode.
  * **Audio Cues:** Quick blink in sync with UI audio feedback (e.g., the `play_blip` sound).

## 8. Heart
**Pixel-art heart-shaped eyes.**
* **Current:** Used as a greeting when a button is pressed.
* **Proposed Interactions:**
  * **Milestones:** Displayed when reaching a high score or milestone (e.g., completing 4 Pomodoro sessions in a row).
  * **AI Companion:** Used when the AI agent detects a positive sentiment in a received voice command.
  * **Charging Complete:** Can be pulsed once when the device hits a fully charged state.

## 9. Sleepy
**Downward U-shaped closed eyes with a tiny "w"-shaped mouth.**
* **Current:** Triggered after a long idle period (>60s).
* **Proposed Interactions:**
  * **Power Saving:** Displayed right before the device automatically dims its OLED brightness to conserve power.
  * **Time of Day:** If NTP time is available, automatically transition to sleepy mode during late night hours (e.g., 11 PM to 6 AM) when inactive.
  * **Battery Critical:** Displayed as the final expression right before the system enters deep sleep due to critically low battery.
