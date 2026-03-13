# SandOS — Wasm Robotics OS for ESP32-S3

A fault-tolerant, low-latency, WebAssembly-sandboxed operating system for
robotics built on the ESP32-S3 with the Embassy async framework.

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                        ESP32-S3                          │
│                                                          │
│  ┌──────────────────────┐  ┌──────────────────────────┐  │
│  │  Core 0 — The Brain  │  │  Core 1 — The Muscle     │  │
│  │  ─────────────────── │  │  ─────────────────────── │  │
│  │  • Wasm VM (wasmi)   │  │  • Hard real-time loop   │  │
│  │  • ESP-NOW radio     │  │  • Motor / balance ctrl  │  │
│  │  • Host-Guest ABI    │  │  • GPIO / PWM / I2C      │  │
│  └──────────┬───────────┘  └──────────────────────────┘  │
│             │  inter-core channel                         │
│  ┌──────────▼───────────┐                                 │
│  │  ULP — The Paramedic │                                 │
│  │  ─────────────────── │                                 │
│  │  • Voltage monitor   │                                 │
│  │  • Temp. threshold   │                                 │
│  └──────────────────────┘                                 │
└──────────────────────────────────────────────────────────┘
```

### Host-Guest ABI (Zero Trust)

The Wasm Sandbox is blind to the hardware. Every hardware interaction goes
through a validated ABI call:

```
Wasm guest calls host_toggle_led()
    → VM pauses
    → Rust Host validates args
    → Host executes hardware action
    → Result returned to guest
    → VM resumes
```

---

## Workspace Crates

| Crate        | Target                    | Purpose                     |
| ------------ | ------------------------- | --------------------------- |
| `abi`        | `no_std` + `std`          | Shared Host-Guest ABI types |
| `firmware`   | `xtensa-esp32s3-none-elf` | ESP32-S3 OS firmware        |
| `wasm-apps`  | `wasm32-unknown-unknown`  | Guest Wasm applications     |
| `host-tests` | x86_64 (std)              | TDD host-side test suite    |

## Documentation

- [OS Architecture](docs/os-architecture.md)
- [Incremental Component Flow](docs/component-incremental-flow.md)
- [Remote Control Integrations](docs/remote-control-integrations.md)

---

## Quick Start

### Host-side TDD (no hardware required)

```bash
cargo test -p host-tests
```

### Firmware (requires ESP toolchain)

```bash
# Install Espressif's Rust toolchain
cargo install espup
espup install
. $HOME/export-esp.sh          # or %USERPROFILE%\export-esp.ps1 on Windows

# Build the firmware
cd firmware
cargo build --release

# Flash to real hardware
espflash flash --monitor target/xtensa-esp32s3-none-elf/release/firmware
```

### Emulation — `cargo run` (no hardware required)

`cargo run` automatically chains three steps:

1. **Build** — compiles the firmware for `xtensa-esp32s3-none-elf`.
2. **Validate** — converts the ELF to a flashable `.bin` via `espflash save-image`
   (proves the bootloader + partition table structure is correct).
3. **Boot** — starts the image in Espressif's `qemu-system-xtensa` fork.

#### Prerequisites

| Tool                                                | Minimum version | How to install           |
| --------------------------------------------------- | --------------- | ------------------------ |
| [espflash](https://github.com/esp-rs/espflash)      | 3.0             | `cargo install espflash` |
| [Espressif QEMU](https://github.com/espressif/qemu) | any recent      | See below                |

**Installing Espressif QEMU**

> The upstream QEMU project does not support ESP32-S3. You need Espressif's
> own fork.

Option A — Pre-built release (recommended):

```bash
# Visit https://github.com/espressif/qemu/releases and download the
# 'xtensa' archive for your OS, e.g.:
#   qemu-esp-develop-9.2.2+esp-20250228-x86_64-linux-gnu.tar.xz  (Linux x86-64)
#   qemu-esp-develop-9.2.2+esp-20250228-aarch64-apple-darwin.tar.xz (macOS M-series)
tar -xf <downloaded-archive>.tar.xz
cd qemu-esp-develop-*
export PATH="$PWD/bin:$PATH"   # add to ~/.bashrc / ~/.zshrc permanently
qemu-system-xtensa --version   # verify
```

Option B — Build from source:

```bash
git clone --depth 1 https://github.com/espressif/qemu
cd qemu
./configure --target-list=xtensa-softmmu \
            --enable-gcrypt \
            --disable-werror
make -j$(nproc)
sudo make install
```

#### Running

```bash
cd firmware
cargo run --release
# Press Ctrl-A then X to exit QEMU.
```

#### CI / validate-only mode

Set `SANDOS_VALIDATE_ONLY=1` to skip QEMU and only verify that the ELF
converts to a valid flash image (useful in headless CI pipelines):

```bash
cd firmware
SANDOS_VALIDATE_ONLY=1 cargo run --release
```

### Guest Wasm Apps

```bash
rustup target add wasm32-unknown-unknown
cd wasm-apps
cargo build --release --target wasm32-unknown-unknown
# Output: target/wasm32-unknown-unknown/release/wasm_apps.wasm
```

---

## Phase 1 — The Bare-Metal Brain

**Hardware:** ESP32-S3 only (built-in LED, Wi-Fi antenna)

- [x] Dual-core Embassy boot (Core 0 + Core 1)
- [x] ULP Paramedic (internal temperature monitoring)
- [x] ESP-NOW wireless (broadcast + receive)
- [x] Wasm Sandbox (wasmi interpreter on Core 0)
- [x] Host-Guest ABI: `host_toggle_led()`

**Success Criterion:** PC sends a wireless command → Core 0 passes it to the
Wasm app → Wasm calls `host_toggle_led()` → onboard LED blinks. Core 1 runs
its real-time loop uninterrupted throughout.

## Phase 2 — The Face & Voice

**Hardware:** ESP32-S3 + 0.96" I2C OLED + I2S microphone

### OLED wiring used in this project

| OLED Pin | ESP32-S3 Pin | Suggested wire color |
| -------- | ------------ | -------------------- |
| VCC      | 3V3          | Red                  |
| GND      | GND          | Black                |
| SCL      | GPIO 9       | Yellow               |
| SDA      | GPIO 8       | Green                |

- [x] DMA display driver (SPI2 with DMA, no CPU blocking)
- [x] `embedded-graphics` drawing canvas
- [x] ABI: `host_draw_eye(expression)`, `host_write_text(ptr, len)`
- [x] ABI: `host_start_audio_capture()`, `host_read_audio(ptr, max_len)`
- [x] LLM pipeline (mic → ESP-NOW → PC → intent → ESP32)

**Success Criterion:** Speak into mic → PC LLM processes → sends text back →
Wasm calls `host_draw_eye()` + `host_write_text()` → 60 FPS robot face on
screen. Core 1 never halted.

## Phase 3 — IMU Polling

**Hardware:** ESP32-S3 + IMU (MPU-6050)

- [x] Configure Core 1 to poll the IMU via I2C/SPI
- [x] Push deterministic sensor data from Core 1 to Core 0 safely
- [x] Expose `host_get_pitch_roll` to Wasm via ABI

**Success Criterion:** Physical tilt of the board is instantly read by Core 1 and exposed to the Wasm app.

## Phase 4 — Motor Control Pipeline

**Hardware:** ESP32-S3 + Motor Drivers + Motors

- [x] PWM driver integration on Core 1
- [x] Real-time PID balance loop strictly on Core 1
- [x] ULP-driven dead-man's switch / voltage cut-off

**Success Criterion:** Closed-loop balancing using PID math on Core 1, with Watchdog resets on Core 0 acting as a fault-tolerance chaos test.

## Phase 5 — The Flexible Nervous System (Unified Message Router)

**Hardware:** ESP32-S3

- [x] OS Message Bus for `MovementIntent` abstraction
- [x] Routing Engine (`RoutingMode::SingleBoard` / `Distributed`)
- [x] Dead-Man's Switch (timeout > 50ms)

**Success Criterion:** Intents are routed successfully, with missing commands resulting in safe motor stops.

## Phase 6 — Async Telemetry TX

**Hardware:** ESP32-S3

- [x] Non-blocking `TELEMETRY_TX_CHANNEL`
- [x] Structured IMU and odometry packet serialization
- [x] Integration with ESP-NOW RX task for opportunistic drain

**Success Criterion:** High-frequency (100 pps) telemetry stream broadcast over ESP-NOW without starving the Wasm engine loop.

## Phase 7 — Radio Link Monitoring

**Hardware:** ESP32-S3

- [x] RX path records timestamp of valid command packets (`RADIO_LAST_RX_MS`)
- [x] Link vitality checks (`is_radio_link_alive`)

**Success Criterion:** Allows fallback routing/intelligence to be activated if the ESP-NOW link remains silent past the threshold.

## Phase 8 — The Dynamic Brain (Wasm Hot-Swapping & OTA Engine)

**Hardware:** ESP32-S3

- [x] PSRAM-backed chunked binary receiver (`OtaReceiver`)
- [x] CRC-32 validation
- [x] 4-step hot-swap logic: Pause → Flush → Instantiate → Resume
- [x] Wasm ABI for `host_get_ota_status`

**Success Criterion:** Over-The-Air application swapping occurs dynamically without resetting the OS, interrupting Core 1 motor loops, or causing jitter.

## WiFi & Web Dashboard

WiFi runs alongside ESP-NOW using `esp-wifi` coexistence mode. Once
connected the device serves an HTTP dashboard on port **80**.

### How it works

| Component        | Description                                                 |
| ---------------- | ----------------------------------------------------------- |
| `wifi.rs`        | Manages WiFi STA association and DHCP reconnect loop        |
| `web_server.rs`  | Lightweight HTTP/1.0 server; starts **disabled** by default |
| `display/mod.rs` | BOOT button → Menu → "Web" item toggles the server on/off   |

The web server is **disabled at boot** to avoid DHCP delays starving the
display task. Enable it from the BOOT-button menu (item 3 — "Web") and
the IP address is printed to the serial console.

### Starvation prevention

- The display driver uses **async I2C** — the CPU yields during each frame
  flush, giving the WiFi stack and web server regular executor time.
- The BOOT-button task uses hardware **edge interrupts** (no polling).
- The web server rate-limits requests with a 20 ms yield between connections.
- The web server sleeps while disabled, imposing zero overhead.

### WiFi credentials

Set at compile time via environment variables (defaults to Wokwi built-in AP):

```bash
WIFI_SSID="MyNetwork" WIFI_PASSWORD="secret" cargo run --release
```

### Erasing flash before first use (real hardware)

```bash
espflash erase-flash -p /dev/ttyUSB0   # adjust port as needed
```

espflash erase-flash -p /dev/cu.usbmodem5B5F1229581
. $HOME/export-esp.sh
cargo run --release
