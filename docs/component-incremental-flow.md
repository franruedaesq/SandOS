# SandOS Incremental Component Flow

This guide shows a practical staged flow to grow a SandOS robot from a **bare ESP32-S3** into a richer system (screen, button, sensors, motors, networking), while preserving safety and architecture boundaries.

It describes:

- What to add physically
- What to add in firmware/ABI
- What to validate before moving to the next stage

---

## Stage 0 — Bare ESP32-S3 baseline

### Goal

Boot dual-core SandOS with no external modules.

### Hardware

- ESP32-S3 board only

### Software focus

- Confirm boot split and task startup (`firmware/src/main.rs`)
- Confirm Core 1 realtime task alive (`firmware/src/core1/mod.rs`)
- Confirm Core 0 brain task alive (`firmware/src/core0/mod.rs`)
- Confirm command path can toggle onboard LED through Wasm ABI

### Done criteria

- Device boots consistently
- `host_toggle_led` path works
- No watchdog instability

---

## Stage 1 — Add a screen (display module)

### Goal

Enable UI rendering through ABI without breaking realtime behavior.

### Hardware

- SPI display (e.g., ST7789/ILI9341)

### Software touchpoints

- Display driver scaffold: `firmware/src/display/mod.rs`
- ABI methods: `host_draw_eye`, `host_write_text`, `host_set_brightness`
- Guest usage: `wasm-apps/src/lib.rs`

### Integration flow

1. Wire display pins and power rails.
2. Bring up display init path in `DisplayDriver::new`.
3. Validate expression rendering (`host_draw_eye`).
4. Validate text writes with bounded length.
5. Confirm Core 1 loop remains stable while frequent draws happen.

### Done criteria

- UI updates from guest work
- No control-loop slowdown side effects

---

## Stage 2 — Add a button (GPIO input module)

### Goal

Add user input with a safe ABI extension pattern.

### Important note

There is no finalized built-in button ABI in current code, so this stage is an **extension pattern** aligned with SandOS design.

### Hardware

- One digital push button (with pull-up/pull-down strategy)

### Software pattern

1. Add a button driver module in firmware (e.g., `firmware/src/input/button.rs`).
2. Expose state via a safe shared bridge (atomic/queue, similar to `sensors.rs`).
3. Extend ABI constants and function name in `abi/src/lib.rs` (example: `host_get_button_state`).
4. Implement host validation logic in `firmware/src/core0/abi.rs`.
5. Consume the new ABI import in `wasm-apps/src/lib.rs`.

### Rules for this stage

- No direct GPIO reads from guest.
- Debounce in host layer, not guest.
- Return explicit status codes for invalid pointers/args.

### Done criteria

- Button state is visible to Wasm app via ABI only
- Debounced behavior is stable
- No regressions in existing command path

---

## Stage 3 — Add sensors (IMU and others)

### Goal

Feed deterministic sensor data from Core 1 to Core 0/guest safely.

### Hardware

- IMU (e.g., MPU-class)

### Software touchpoints

- Sensor bridge: `firmware/src/sensors.rs`
- Core 1 poll/update loop: `firmware/src/core1/mod.rs`
- ABI accessor: `host_get_pitch_roll` in `firmware/src/core0/abi.rs`

### Done criteria

- Sensor updates at target loop interval
- Guest reads current values through ABI
- No lock-contention/jitter introduced

---

## Stage 4 — Add motors and actuation

### Goal

Close control loop with safety gates.

### Hardware

- Motor driver + motors

### Software touchpoints

- Motor bridge and enable gate: `firmware/src/motors.rs`
- Router path: `firmware/src/router.rs`
- Intent channel: `firmware/src/message_bus.rs`
- ABI command entry: `host_set_motor_speed`

### Safety checks

- Enforce speed limits (`MAX_MOTOR_SPEED`)
- Enforce motor enabled flag
- Verify dead-man switch stop behavior

### Done criteria

- Commanded movement reaches motor layer predictably
- Timeout/loss of commands leads to stop

---

## Stage 5 — Add networking surfaces

### Goal

Enable observability/control interfaces without crossing safety boundaries.

### Software touchpoints

- Wi-Fi STA: `firmware/src/wifi.rs`
- HTTP dashboard/API: `firmware/src/web_server.rs`
- ESP-NOW command/telemetry: `firmware/src/core0/espnow.rs`
- Telemetry queue: `firmware/src/telemetry.rs`

### Done criteria

- Device can be discovered/reached over Wi-Fi
- Telemetry is flowing and non-blocking
- Control path remains ABI/routing mediated

---

## Stage 6 — Distributed mode (roadmap-aware)

### Goal

Move from one board to brain+worker style control routing.

### Current state

Routing abstraction is implemented, but distributed TX integration in router is partially TODO.

### Flow

1. Set `RoutingMode::Distributed`.
2. Serialize movement intent for remote worker.
3. Send over radio transport.
4. Worker applies local PWM.
5. Worker dead-man switch halts if heartbeat is lost.

### Done criteria

- Remote board receives and applies intents
- Heartbeat loss safely stops worker motors

---

## Stage 7 — Radio Link Monitoring

### Goal

Track ESP-NOW link vitality and allow fallback behaviors.

### Software touchpoints

- `RADIO_LAST_RX_MS` atomic counter
- Timeout verification in command validation

### Done criteria

- Inbound ESP-NOW traffic successfully marks last valid timestamp.
- Link loss > threshold triggers fallback behaviors (inference activation).

---

## Stage 8 — The Dynamic Brain (OTA Engine)

### Goal

Hot-swap the Wasm VM application over-the-air without halting the OS.

### Software touchpoints

- `OtaReceiver` PSRAM buffer engine
- Wasm execution control signal: `OTA_SWAP_SIGNAL`
- ABI export: `host_get_ota_status`

### Integration flow

1. Start `OTA_BEGIN` session with file size.
2. Send sequentially chunked binaries (`OTA_CHUNK`).
3. CRC-32 verify via `OTA_FINALIZE`.
4. Run hot-swap via `hot_swap_wasm`.

### Done criteria

- Successfully swaps applications mid-run.
- Motor/Balance control loops on Core 1 experience 0ms drop in execution.

---

## Stage checklist (quick quality gate)

Before advancing any stage:

1. Verify Core 1 realtime loop timing still holds.
2. Verify all new hardware access is host-side only.
3. Verify ABI boundaries and status codes are explicit.
4. Verify stop/failsafe behavior under link loss.
5. Verify no queue/blocking pattern can stall control loop.

---

## Minimal example path requested

If you start from zero modules:

1. ESP32-S3 only (boot + LED path)
2. Add screen (expressions/text)
3. Add button (new ABI function pattern)
4. Add IMU (sensor bridge)
5. Add motors (intent + router + safety)
6. Add network control/observability surfaces
7. Add Radio Link Monitoring (link vitality tracking)
8. Add The Dynamic Brain (OTA Hot-Swapping)

That sequence keeps SandOS stable because each step preserves the same architecture contract: **Core 1 deterministic control, Core 0 orchestration, ABI-mediated hardware access**.
