# SandOS OS Architecture

## 1) What SandOS is

SandOS is a dual-core, `no_std` Rust operating system for ESP32-S3 robotics. It is built around one principle: **separate unpredictable logic from real-time physical control**.

- **Core 0 (The Brain)** runs networking, Wasm sandbox applications, routing, OTA, and integration logic.
- **Core 1 (The Muscle)** runs deterministic control loops (e.g., IMU polling, balancing, motor loop) at fixed timing.
- **ULP (The Paramedic)** monitors low-power safety signals (voltage/temp) independently.

This split is visible in the firmware boot flow and task layout:

- `firmware/src/main.rs`
- `firmware/src/core0/mod.rs`
- `firmware/src/core1/mod.rs`

---

## 2) Ultimate goal of the OS

The ultimate goal is to make robotics behavior:

1. **Safe** — guest code cannot directly manipulate hardware.
2. **Fault-tolerant** — guest crashes should not destabilize motor control.
3. **Hot-swappable** — behavior can be updated (Wasm OTA) without reflashing full firmware.
4. **Composable** — sensors, actuators, and network/control surfaces can be added incrementally.
5. **Portable** — ABI contracts are stable and shared between firmware, guest apps, and tests.

---

## 3) Core responsibilities

## Core 0 — The Brain

Core 0 owns high-level orchestration and non-deterministic workloads:

- Wasm VM execution (`core0/wasm_vm.rs`)
- Host-side ABI validation and function binding (`core0/abi.rs`)
- ESP-NOW receive/transmit path (`core0/espnow.rs`)
- OTA chunk receiver and hot-swap coordination (`core0/ota.rs`)
- Router task for movement intents (`router.rs`)
- Wi-Fi + HTTP dashboard/API (`wifi.rs`, `web_server.rs`)

### Why this belongs on Core 0

These tasks have variable latency (radio, parsing, VM, HTTP), so they must not block real-time motor control.

## Core 1 — The Muscle

Core 1 owns deterministic control loops:

- Fixed-period realtime loop (2ms / 500Hz)
- IMU polling and atomic sensor bridge writes (`sensors.rs`)
- PID/balance output generation (`core1/mod.rs`, `core1/pid.rs`)
- Safe motor command application with enable gate (`motors.rs`)
- Structured telemetry production (`telemetry.rs`)

### Why this belongs on Core 1

Physical control requires bounded timing and jitter control, independent from VM/radio/web workloads.

## ULP — The Paramedic

ULP provides low-power safety monitoring used by the main cores:

- Critical voltage flagging
- Temperature/health-style checks
- Fast safety gating for motor disable behavior

---

## 4) Sandbox model

SandOS uses a **zero-trust host-guest model**:

- Guest app = Wasm binary (untrusted by default).
- Host OS = Rust firmware (trusted).
- Guest cannot access GPIO/PWM/I2C/SPI/radio memory directly.
- Every hardware action must pass through host ABI imports.

Host-side guards include:

- Argument validation (`ERR_INVALID_ARG`)
- Pointer/length bounds checking (`ERR_BOUNDS`)
- Resource/busy protection (`ERR_BUSY`)
- Explicit status-code return path (no hidden hardware side effects)

Reference:

- `abi/src/lib.rs`
- `firmware/src/core0/abi.rs`
- `wasm-apps/src/lib.rs`

---

## 5) Application Binary Interface (ABI)

The ABI crate (`abi`) is the shared contract for:

- Function names and command IDs
- Error/status codes
- Limits and constraints
- Shared packet and telemetry structures
- Routing mode and movement intent data

### ABI purpose

The ABI makes all participants speak one protocol:

- Firmware (Host)
- Wasm guest apps
- PC tools/bridges/tests

### ABI examples already present

- `host_toggle_led`
- `host_draw_eye`, `host_write_text`
- `host_get_pitch_roll`
- `host_set_motor_speed`
- `host_emit_imu_telemetry`, `host_emit_odom_telemetry`
- `host_get_local_inference`
- OTA command IDs (`OTA_BEGIN`, `OTA_CHUNK`, `OTA_FINALIZE`)

---

## 6) SandOS rules (operating rules)

These are the practical system rules SandOS follows today.

1. **Real-time first:** Core 1 loop timing has priority over app/network features.
2. **Guest isolation:** no direct hardware access from Wasm.
3. **Validate before action:** host checks args and memory before touching hardware.
4. **Safety gate on motors:** motor commands are rejected/zeroed when disabled.
5. **Dead-man switch:** no fresh movement intent within timeout => stop motors.
6. **Back-pressure aware:** queues are non-blocking; overload drops/returns busy instead of stalling control loops.
7. **OTA integrity first:** binary chunks must pass CRC before swap.
8. **Distributed mode is explicit:** routing mode controls local vs remote execution path.

---

## 7) How all parts work together

## High-level flow

1. Device boots and initializes heap/radio/tasks.
2. Core 1 starts realtime loop first.
3. Core 0 starts Wasm/radio/router/web stack.
4. Commands arrive over ESP-NOW or local interfaces.
5. Host validates command and dispatches through ABI/message bus.
6. Router sends movement intents to local Core 1 (single-board) or remote path (distributed roadmap).
7. Core 1 applies control loop logic and updates sensors/telemetry.
8. Telemetry is transmitted asynchronously to external consumers.

## Command path (single-board current implementation)

`Remote Command` -> `ESP-NOW RX (Core 0)` -> `Wasm command dispatch` -> `host_set_motor_speed()` -> `MovementIntent channel` -> `Router` -> `Core 1 motor bridge` -> `PID blend + PWM apply`

## Telemetry path

`Core 1 realtime loop` -> `Telemetry channel` -> `Core 0 ESP-NOW TX` -> `PC / observer / bridge`

## OTA path

`OTA_BEGIN/CHUNK/FINALIZE packets` -> `Core 0 OTA receiver (PSRAM staging)` -> `CRC verify` -> `swap signal` -> `Wasm VM hot-swap`

---

## 8) Current vs roadmap

## Implemented now (code present)

- Dual-core boot and task split
- Wasm host-guest import model
- Message bus and routing mode abstraction
- Dead-man switch logic in router
- Wi-Fi + web dashboard/API
- ESP-NOW command + telemetry path
- OTA receiver state machine and hot-swap signaling

## Roadmap / partial areas

- Full distributed motor TX handoff in router (`Distributed` path still marked TODO for TX queue integration)
- Hardware-specific display/audio/sensor final drivers for every target board
- Full production command/auth layers for internet-facing control

---

## 9) Design summary

SandOS is a **safety-first robotics OS** where Core 1 preserves deterministic control and Core 0 provides flexible intelligence and connectivity through a strict ABI/sandbox boundary. The combination of routing abstraction, queue-based decoupling, and OTA-capable Wasm execution enables iterative growth from a single board to richer, remotely managed robotic systems.
