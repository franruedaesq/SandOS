# SandOS Remote Control Integrations

This document explains two remote-control patterns for SandOS on ESP32-S3:

1. **Telegram control** from your phone
2. **Online server control** (web/app/backend)

Both are presented as architecture patterns that fit current SandOS design, using ABI and routing rules as the safety boundary.

---

## 1) Common control contract (applies to both)

No matter where a command originates (Telegram or cloud server), the final in-device flow should stay:

`Remote command` -> `Core 0 ingress` -> `ABI validated action` -> `Message bus intent` -> `Router` -> `Core 1 apply`

This preserves:

- Guest isolation
- Motor safety gates
- Dead-man switch behavior
- Deterministic control loop on Core 1

Key modules:

- `firmware/src/core0/espnow.rs`
- `firmware/src/web_server.rs`
- `firmware/src/core0/abi.rs`
- `firmware/src/message_bus.rs`
- `firmware/src/router.rs`

---

## 2) Telegram integration

## Option A — Cloud bridge (recommended for production)

### Why

- Better token security (bot token kept off device)
- Easier auth/rate limiting
- Works behind NAT without opening inbound port to device

### Flow

1. User sends Telegram message to bot (`/forward`, `/stop`, etc.).
2. Cloud bridge receives Bot API update.
3. Bridge authenticates user/chat and maps text to normalized command.
4. Bridge forwards command to SandOS endpoint (HTTP/WebSocket/MQTT bridge).
5. Device ingests command in Core 0 and maps into ABI command/intents.
6. Router/Core 1 execute with standard safety rules.

### Suggested command mapping

- `/forward 120` -> `set_motor_speed(120,120)`
- `/left 80` -> `set_motor_speed(-80,80)`
- `/stop` -> zero intent / emergency stop command
- `/status` -> query telemetry snapshot

### Required safeguards

- Chat/user allow-list
- Per-command rate limiting
- Replay protection (nonce/timestamp)
- Hard clamp to `MAX_MOTOR_SPEED`
- Idle timeout to trigger stop if command stream dies

## Option B — Direct Telegram from ESP32-S3 (possible, less ideal)

### Why teams still use it

- Fewer infrastructure components
- Quick prototype without cloud backend

### Tradeoffs

- Bot token stored on device flash
- TLS/memory constraints on embedded client stack
- Harder to rotate credentials and audit access centrally

### Flow

1. ESP32 connects to Wi-Fi (`firmware/src/wifi.rs`).
2. Core 0 polling task reads Telegram Bot API updates.
3. Parsed commands map to internal command IDs/ABI calls.
4. Router/Core 1 path remains unchanged.

### Minimum requirements

- Secure token storage strategy
- Poll interval with backoff and timeout handling
- Parser that only accepts strict command grammar

---

## 3) Online server integration (web/app/backend)

This pattern supports browser/mobile dashboards, automation rules, and multi-device fleets.

## Reference architecture

1. **Client UI** (web/mobile) sends signed control commands.
2. **Control API server** validates identity/permissions and emits normalized command events.
3. **Device session layer** (WebSocket/MQTT/HTTP long-poll) routes commands to target ESP32-S3.
4. **SandOS Core 0** ingests and validates command payload.
5. **ABI + Router + Core 1** execute command and emit telemetry back.

## Inbound transport choices

- **HTTP polling:** simplest but higher latency
- **WebSocket:** bidirectional, good for low-latency command + status
- **MQTT:** strong for fleet/pub-sub scenarios

## Device-side integration patterns

### Pattern 1: Keep current web server + add command endpoint

- Extend `web_server.rs` with command route (`POST /api/cmd`)
- Parse signed payload
- Push intent to message bus

### Pattern 2: Add outbound socket client task

- New Core 0 task holds persistent connection to cloud
- Receives command frames and sends telemetry acknowledgements
- Better for NAT/firewall-friendly remote control

---

## 4) Security and safety rules for remote control

Apply these for **both Telegram and cloud server paths**:

1. **Authenticate every control source** (chat/user/session identity).
2. **Authorize by capability** (who can move motors vs read telemetry only).
3. **Validate schema and bounds** before ABI calls.
4. **Fail closed** on malformed or expired commands.
5. **Rate limit** high-frequency commands.
6. **Keep dead-man switch active** regardless of remote source.
7. **Log command audit trail** (source, timestamp, command, result).
8. **Keep emergency stop path highest priority**.

---

## 5) End-to-end example

## Example A — Telegram phone command to motion

1. Phone: `/forward 100`
2. Telegram Bot API -> bridge parser
3. Bridge emits normalized `{cmd:"set_motor", left:100, right:100}`
4. Device receives command via chosen transport
5. Host validates limits and motor-enabled gate
6. Intent published to message bus
7. Router forwards to Core 1 (single-board mode)
8. Core 1 blends command with PID and applies outputs
9. Telemetry returned to bridge/UI

## Example B — Online server dashboard control

1. User presses virtual joystick on web app
2. Backend converts joystick vector to left/right target speeds
3. Signed command delivered to device session
4. Device executes through same ABI/routing pipeline
5. Dashboard receives telemetry stream and updates UI

---

## 6) Practical implementation roadmap in this repo

1. Define normalized remote command schema (shared with ABI constraints).
2. Implement one ingress path first (recommended: cloud bridge -> device socket).
3. Add command parser task on Core 0.
4. Reuse existing message bus and router path for actuation.
5. Add auth + audit logging before exposing to internet.
6. Add Telegram adapter as an input source to the same backend command schema.

This keeps Telegram and server control as **two front doors** into a **single internal SandOS command pipeline**, which is simpler to maintain and safer to reason about.
