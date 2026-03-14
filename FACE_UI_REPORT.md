# Face UI Architecture Report

## 1. High-Level Overview of Current State

The current Face UI in SandOS is built around a non-blocking `UiManager` state machine found in `firmware/src/display/ui.rs`.

**Key Components:**
* **`UiManager` State Machine:** Centralizes the UI state using the `UiState` enum (`Idle`, `Menu`, `SettingsMenu`, `Metrics`) and manages animation timings via delta time (`dt_ms`).
* **Graphics Rendering:** Instead of loading heavy image assets (like `.png` or `.jpg` sprites), the UI draws the companion ("R-Kun") entirely using `embedded-graphics` primitives like `Ellipse`, `Arc`, `Line`, and `RoundedRectangle`. This is extremely memory-efficient.
* **Micro-animations:** The system implements smooth, math-based animations. Examples include:
  * A sine-wave lookup table (`SINE_LUT`) for idle breathing bounces.
  * Time-based periodic blinking (166ms every 10 seconds).
  * Tactile touch feedback via expanding ripple rings.
  * Easing functions for menus sliding in and out.
* **Hardware Independence:** The UI state machine is decoupled from direct hardware blocks. It accepts high-level `TouchAction` intents (Taps, Swipes) and pushes completed frames to the screen buffer.
* **Host-Guest ABI Integration:** The current expression states are defined by the `EyeExpression` enum provided via the `abi` crate, which bridges the gap between the Rust host and Wasm guest applications. The UI parses these ABI values to determine how to draw the primitives (e.g., ellipses vs. arcs).

---

## 2. Proposed Scalable Architecture for Expressions

To easily add new expressions (like the requested Neutral, Happy, Sad, etc.) and ensure future expandability without bloating memory, the following architecture is recommended:

### A. State Representation
Keep using the `EyeExpression` enum from the ABI as the single source of truth for the current expression.

```rust
// Proposed expansion of the EyeExpression enum in the ABI crate
pub enum EyeExpression {
    Neutral,
    Happy,
    Sad,
    Angry,
    Surprised,
    Thinking,
    Blink,
    Heart,
    Sleepy,
    // ... scalable for future states (e.g., Loading, Processing)
}
```

### B. Event-Driven Expression Triggers
Currently, expressions seem mostly hardcoded or manipulated linearly. To scale triggers efficiently without tightly coupling the UI to system modules (like WiFi or the Pomodoro timer), the OS should implement an **Event Bus** or a **Message Queue** (leveraging the existing SandBus/PubSub architecture).

System services will publish "intent" events, and the `UiManager` will subscribe to them, mapping them to expressions.

**Target Trigger Mappings:**
* **Neutral:** Default fallback state.
* **Happy:** Triggered by an event from the `wifi` module when IP is successfully leased.
* **Sad:** Triggered by an event from the `wifi` module upon connection timeout/error.
* **Angry:** Triggered by an internal state counter in the network manager tracking repeated, consecutive WiFi failures.
* **Surprised:** Triggered by the `router/message_bus` when a new ESP-NOW packet arrives, or from the Pomodoro app module upon completion.
* **Thinking:** Displayed while the `wifi` state is "Connecting..." or during Wasm loading.
* **Blink:** Handled internally by `UiManager` via a periodic delta-time timer during `Idle` state.
* **Heart:** Triggered by a hardware interrupt/button press event dispatched to the `UiManager`.
* **Sleepy:** Triggered internally by `UiManager` when the `last_interaction_time` delta exceeds 60,000ms.

### C. Procedural Drawing Pipeline (Memory Efficient)
To ensure adding expressions doesn't consume too much memory or resources, **do not introduce bitmap images or sprite sheets**. Continue using procedural `embedded-graphics` primitives.

To make this scalable and maintainable, refactor the massive `draw_r_kun` monolithic function into a cleaner pattern:

1. **Parameter Struct:** Create a localized struct that defines the eye shape, width, height, mouth angle, and blush visibility.
2. **Expression Matcher:** Use a `match self.expression` block strictly to populate the parameter struct.
3. **Single Render Pass:** Pass the populated struct into a unified rendering block that draws the shapes.

**Example Conceptual Pattern:**
```rust
struct FaceParams {
    eye_type: EyeType, // Ellipse, Arc, Line, HeartPolygon
    eye_width: u32,
    eye_height: u32,
    mouth_type: MouthType, // Arc, Circle, Squiggle
    mouth_angle: f32,
}

// Inside update loop:
let params = match self.expression {
    EyeExpression::Happy => FaceParams {
        eye_type: Arc, mouth_type: Arc, mouth_angle: 180.0, ...
    },
    EyeExpression::Sleepy => FaceParams {
        eye_type: Line, mouth_type: Squiggle, ...
    },
    // ...
};

// Then draw based on params...
```

### Summary of Benefits
1. **Low Memory Footprint:** Relying entirely on procedural math/primitives requires practically zero ROM space compared to storing image assets.
2. **Decoupled Architecture:** Using an Event Bus to trigger expressions means the WiFi code doesn't need to know the UI exists, preventing spaghetti code.
3. **Easy Expansion:** Adding a new expression simply requires adding an enum variant and defining its primitive shapes in the match block.