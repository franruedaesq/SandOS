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

| Crate | Target | Purpose |
|-------|--------|---------|
| `abi` | `no_std` + `std` | Shared Host-Guest ABI types |
| `firmware` | `xtensa-esp32s3-none-elf` | ESP32-S3 OS firmware |
| `wasm-apps` | `wasm32-unknown-unknown` | Guest Wasm applications |
| `host-tests` | x86_64 (std) | TDD host-side test suite |

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

# Flash to ESP32-S3
espflash flash --monitor target/xtensa-esp32s3-none-elf/release/firmware
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

**Hardware:** ESP32-S3 + SPI/I2C screen + I2S microphone

- [x] DMA display driver (SPI2 with DMA, no CPU blocking)
- [x] `embedded-graphics` drawing canvas
- [x] ABI: `host_draw_eye(expression)`, `host_write_text(ptr, len)`
- [x] ABI: `host_start_audio_capture()`, `host_read_audio(ptr, max_len)`
- [x] LLM pipeline (mic → ESP-NOW → PC → intent → ESP32)

**Success Criterion:** Speak into mic → PC LLM processes → sends text back →
Wasm calls `host_draw_eye()` + `host_write_text()` → 60 FPS robot face on
screen. Core 1 never halted.
