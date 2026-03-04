//! OS Message Bus — Core 0 ↔ routing layer.
//!
//! This module is the "nervous system" described in Phase 5: a hardware-agnostic
//! routing layer inside the Host OS that lets the Wasm Sandbox publish movement
//! intents without knowing whether the target is a local Core 1 loop or a remote
//! ESP-NOW radio.
//!
//! ## Architecture
//!
//! ```text
//! Wasm ABI (host_set_motor_speed)
//!       │
//!       ▼  publish_intent()
//! OS Message Bus ──► INTENT_CHANNEL  ──► router_task
//!                                              │
//!                         ┌───────────────────┤
//!                         │ SingleBoard        │ Distributed
//!                         ▼                    ▼
//!              Core 1 motor bridge       ESP-NOW radio
//! ```
//!
//! ## Dead-Man's Switch
//!
//! [`router_task`](crate::router) monitors the channel with a
//! [`DEAD_MANS_SWITCH_MS`]-millisecond timeout.  If no intent arrives within
//! that window the Router zeroes all control loops, preventing a runaway robot
//! when the Wasm app stops or the radio link drops.

use core::sync::atomic::{AtomicU8, Ordering};

use abi::{MovementIntent, RoutingMode};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

// ── Channel ───────────────────────────────────────────────────────────────────

/// Depth of the movement-intent queue.
const INTENT_QUEUE_DEPTH: usize = 8;

/// Lock-free channel: ABI (Core 0) → Router task.
///
/// The Wasm ABI pushes [`MovementIntent`]s here; the Router task drains them
/// and dispatches to the correct backend.
pub static INTENT_CHANNEL: Channel<CriticalSectionRawMutex, MovementIntent, INTENT_QUEUE_DEPTH> =
    Channel::new();

// ── Routing Mode ──────────────────────────────────────────────────────────────

/// Atomic storage for the current [`RoutingMode`] (0 = SingleBoard, 1 = Distributed).
static ROUTING_MODE: AtomicU8 = AtomicU8::new(RoutingMode::SingleBoard as u8);

/// Switch the routing mode at runtime.
///
/// Safe to call from any core or task; the change is visible to the Router
/// task on the next iteration via `Acquire` ordering.
#[inline]
pub fn set_routing_mode(mode: RoutingMode) {
    ROUTING_MODE.store(mode as u8, Ordering::Release);
}

/// Read the current routing mode.
#[inline]
pub fn get_routing_mode() -> RoutingMode {
    match ROUTING_MODE.load(Ordering::Acquire) {
        1 => RoutingMode::Distributed,
        _ => RoutingMode::SingleBoard,
    }
}

// ── Publisher (ABI / Core 0) ──────────────────────────────────────────────────

/// Publish a validated [`MovementIntent`] to the OS Message Bus.
///
/// Returns `true` if the intent was enqueued successfully, `false` if the
/// queue was full (back-pressure: the ABI should return [`abi::status::ERR_BUSY`]).
///
/// The caller is responsible for pinging the dead-man's switch timestamp via
/// [`embassy_time::Instant::now`] so the Router can detect stale flow.
#[inline]
pub fn publish_intent(intent: MovementIntent) -> bool {
    INTENT_CHANNEL.try_send(intent).is_ok()
}
