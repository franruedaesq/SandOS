# Audio Subsystem Status in SandOS

## Current Status of Microphone and Speaker

The audio subsystem in SandOS leverages the ESP32-S3's I2S0 peripheral and DMA to provide both audio input (microphone) and output (speaker) capabilities without blocking the main Embassy executor. The system allows the Wasm guest application to interact with hardware safely via a defined Host-Guest ABI.

### Microphone (Input) Status
- **Hardware Binding:** The microphone is connected to the I2S0 peripheral using `DIN` on GPIO 8, `BCLK` on GPIO 5, and `WS` (LRCK) on GPIO 7.
- **Data Capture:** An async Embassy task (`mic_rx_task`) reads audio data using `read_dma_circular_async` from the `I2sRx` stream into a 4096-byte circular DMA buffer.
- **ABI Integration:**
  - The Wasm guest can start and stop capture via `FN_START_AUDIO` and `FN_STOP_AUDIO`.
  - `FN_GET_AUDIO_AVAIL` returns the number of bytes available.
  - `FN_READ_AUDIO` reads up to 8192 bytes from a ring buffer managed in the Host state (`AbiHost::audio_buf`).
- **Inference Pipeline:** Captured audio can be routed to the fallback local inference engine if the radio link goes silent.
- **Current Limitations:** The data read from `mic_rx_task` is currently a placeholder where audio bytes are just discarded (`// In a real implementation we would route this audio to inference or wifi`). The audio ring buffer in `AbiHost` isn't actually being filled by `mic_rx_task` right now. This is a critical gap. The `AbiHost` methods like `start_audio_capture_openai` are also stubs.

### Speaker (Output) Status
- **Hardware Binding:** The speaker (via I2S amplifier) uses `DOUT` on GPIO 6, `BCLK` on GPIO 5, and `WS` on GPIO 7. `AMP_EN` on GPIO 1 must be pulled low to enable the amp.
- **Data Playback:** An async Embassy task (`speaker_tx_task`) pulls audio chunks from a lock-free channel (`AUDIO_TX_CHANNEL`) and pushes them to `write_dma_circular_async`.
- **Tactile UI Audio:** Currently, the system plays simple synthetic square waves ("blips") via `play_blip()` when UI interactions occur.
- **Current Limitations:** There is no ABI function currently exposed for the Wasm guest to *send* arbitrary PCM audio to the speaker. The system only plays predefined blips.

---

## Implementing a UI for Recording and Listening to Audio

To implement a feature to record a short audio clip and listen to it later via the UI, we would need to bridge the UI thread, the audio subsystem, and potentially the Wasm layer.

### Step-by-Step Implementation Guide

**1. Connecting the Microphone to the Buffer:**
Currently, `mic_rx_task` discards audio data. We need to pass data from `mic_rx_task` to the shared `AbiHost` or a dedicated audio recorder buffer.
- Create a global `Channel` or atomic structure to pass chunks from `mic_rx_task` to the main system.

**2. State Machine for Recording:**
We need to allocate a buffer in PSRAM to hold the recorded audio.
- Add new `UiState` enum variants: `UiState::RecordAudio`, `UiState::PlaybackAudio`.
- Add recording state variables (e.g., `is_recording`, `record_buffer`, `record_len`).

**3. UI Modifications (`firmware/src/display/ui.rs`):**
- **Menu Addition:** Add a "Record" and "Play" button to the existing menu or settings menu.
- **Visual Feedback:** When recording, draw a red recording indicator or waveform on the screen.
- **Touch Actions:** Tap "Record" to start/stop. Tap "Play" to route the recorded buffer to the `AUDIO_TX_CHANNEL`.

**4. Routing Playback:**
- Create a helper function similar to `play_blip()` that takes a slice of the recorded buffer and pushes it into `AUDIO_TX_CHANNEL` in chunks so `speaker_tx_task` can play it.

### Required File Modifications

- `firmware/src/audio.rs`: Modify `mic_rx_task` to push data to a global recording buffer when recording is active. Add a playback function to chunk the buffer into `AUDIO_TX_CHANNEL`.
- `firmware/src/display/ui.rs`: Add new states, buttons, and visual drawing logic for the recording UI. Handle touch events to trigger recording and playback.
- `firmware/src/main.rs`: Ensure memory allocation for the recording buffer is appropriately handled (likely overflowing to PSRAM).

### Impact on Resources and Performance

1. **Memory:** Recording audio requires significant memory. 1 second of 16kHz, 16-bit mono audio is 32 KB. A 5-second clip requires 160 KB. Since Core 0 internal SRAM is limited (~72 KB, mostly used by WiFi), this buffer *must* be allocated on the PSRAM heap (`alloc::vec::Vec` or a large `Box`). If allocated on the stack or in internal SRAM, it will cause an overflow or OOM panic.
2. **CPU Performance:** Passing large audio buffers between Embassy tasks can create overhead. Copying 32KB/sec around requires CPU cycles. The UI might drop frames (FPS decrease) during active recording or playback if the data copying blocks the executor.
3. **I2S DMA:** The `read_dma_circular_async` and `write_dma_circular_async` tasks are already non-blocking, but filling/draining large buffers quickly enough is critical to prevent audio tearing or dropping samples.
4. **Executor Starvation:** If the Wasm task or UI task spends too much time copying audio bytes without awaiting or yielding (`Timer::after`), it will starve the network and display tasks.
