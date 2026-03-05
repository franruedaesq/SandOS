//! OTA (Over-The-Air) Hot-Swap Engine — Core 0.
//!
//! Receives a new Wasm binary over the ESP-NOW radio in chunked packets,
//! verifies its CRC-32 checksum, and performs a live hot-swap of the Wasm
//! Virtual Machine **without** rebooting the hardware or interrupting Core 1.
//!
//! ## Protocol
//!
//! The PC-side sender transmits three ESP-NOW command types in order:
//!
//! 1. [`cmd::OTA_BEGIN`] — declares the total binary size.
//! 2. [`cmd::OTA_CHUNK`] — one chunk of binary data at a given byte offset.
//! 3. [`cmd::OTA_FINALIZE`] — triggers CRC-32 verification and, on success,
//!    signals the Wasm task to perform the hot-swap.
//!
//! ## Memory layout
//!
//! The staging buffer lives entirely in PSRAM (allocated from the PSRAM heap
//! via `alloc::vec`).  It is independent of the Wasm engine's allocation
//! space, preventing heap fragmentation during the transfer.
//!
//! ## Core 1 isolation guarantee
//!
//! The hot-swap routine runs exclusively on Core 0.  Core 1's hard real-time
//! PID/motor loop shares only the atomic variables in
//! `crate::message_bus::MOTOR_CMD` — those are never modified by this module.
//! Core 1 therefore sees zero jitter from the hot-swap operation.
extern crate alloc;

use alloc::vec::Vec;

use abi::{crc32, cmd, OtaState, OTA_MAX_BINARY_SIZE};

// ── OTA state machine ─────────────────────────────────────────────────────────

/// The PSRAM-backed OTA receiver and binary staging area.
///
/// One instance lives on Core 0's Embassy task heap, owned by the ESP-NOW
/// receiver task.  All methods are synchronous and non-blocking so they
/// compose naturally with the Embassy `async` executor.
pub struct OtaReceiver {
    /// Current protocol state.
    state: OtaState,
    /// PSRAM staging buffer — pre-allocated on `begin`, overwritten by chunks.
    buffer: Vec<u8>,
    /// Total expected binary size declared in `OTA_BEGIN`.
    total_size: u32,
    /// Running count of payload bytes written (for progress reporting).
    bytes_received: u32,
    /// Number of successful hot-swaps completed since firmware boot.
    swap_count: u32,
}

impl OtaReceiver {
    /// Create a new, idle OTA receiver.
    pub const fn new() -> Self {
        Self {
            state:          OtaState::Idle,
            buffer:         Vec::new(),
            total_size:     0,
            bytes_received: 0,
            swap_count:     0,
        }
    }

    /// Current OTA state.
    #[inline]
    pub fn state(&self) -> OtaState { self.state }

    /// Bytes written to the staging buffer so far.
    #[inline]
    pub fn bytes_received(&self) -> u32 { self.bytes_received }

    /// Expected total size declared in `OTA_BEGIN`.
    #[inline]
    pub fn total_size(&self) -> u32 { self.total_size }

    /// Number of completed hot-swaps.
    #[inline]
    pub fn swap_count(&self) -> u32 { self.swap_count }

    /// Return an immutable reference to the verified binary, if ready.
    ///
    /// Returns `Some` only when `state == OtaState::Ready`; `None` otherwise.
    pub fn ready_binary(&self) -> Option<&[u8]> {
        if self.state == OtaState::Ready {
            Some(&self.buffer)
        } else {
            None
        }
    }

    // ── Protocol handlers ─────────────────────────────────────────────────────

    /// Handle an `OTA_BEGIN` command payload.
    ///
    /// Payload layout: `[total_size: u32 LE]`.
    ///
    /// An in-progress `Receiving` session is silently cancelled and replaced.
    /// Resets the staging buffer and transitions to [`OtaState::Receiving`].
    /// Returns `false` and transitions to [`OtaState::Failed`] on error.
    pub fn handle_begin(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 4 {
            self.state = OtaState::Failed;
            return false;
        }
        let total = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        if total == 0 || total as usize > OTA_MAX_BINARY_SIZE {
            self.state = OtaState::Failed;
            return false;
        }
        // A hot-swap already in progress cannot be interrupted.
        if self.state == OtaState::Swapping {
            return false;
        }
        // Any in-progress session is discarded.
        self.buffer.clear();
        // Allocate the staging area; fill with zeroes so sparse writes are safe.
        self.buffer.resize(total as usize, 0u8);
        self.total_size     = total;
        self.bytes_received = 0;
        self.state          = OtaState::Receiving;
        true
    }

    /// Handle an `OTA_CHUNK` command payload.
    ///
    /// Payload layout: `[offset: u32 LE][data …]`.
    ///
    /// Writes `data` at `offset` in the staging buffer.  Returns `false` if
    /// the session is not active or the chunk would overflow the buffer.
    pub fn handle_chunk(&mut self, payload: &[u8]) -> bool {
        if self.state != OtaState::Receiving {
            return false;
        }
        if payload.len() < 5 {
            // Need at least 4 bytes of offset + 1 byte of data.
            return false;
        }
        let offset = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let data   = &payload[4..];
        let end    = (offset as usize).saturating_add(data.len());
        if end > self.total_size as usize {
            self.state = OtaState::Failed;
            return false;
        }
        self.buffer[offset as usize..end].copy_from_slice(data);
        self.bytes_received += data.len() as u32;
        true
    }

    /// Handle an `OTA_FINALIZE` command payload.
    ///
    /// Payload layout: `[expected_crc32: u32 LE]`.
    ///
    /// Verifies the CRC-32 of the entire staging buffer.  On success,
    /// transitions to [`OtaState::Ready`]; on failure to [`OtaState::Failed`].
    pub fn handle_finalize(&mut self, payload: &[u8]) -> bool {
        if self.state != OtaState::Receiving {
            return false;
        }
        if payload.len() < 4 {
            self.state = OtaState::Failed;
            return false;
        }
        let expected = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let actual   = crc32(&self.buffer);
        if actual != expected {
            self.state = OtaState::Failed;
            return false;
        }
        self.state = OtaState::Ready;
        true
    }

    /// Dispatch an incoming OTA command to the appropriate handler.
    ///
    /// Returns `true` if the command was handled (regardless of outcome),
    /// `false` if `cmd_id` is not an OTA command.
    pub fn handle_command(&mut self, cmd_id: u8, payload: &[u8]) -> bool {
        match cmd_id {
            cmd::OTA_BEGIN    => { self.handle_begin(payload); true }
            cmd::OTA_CHUNK    => { self.handle_chunk(payload); true }
            cmd::OTA_FINALIZE => { self.handle_finalize(payload); true }
            _                 => false,
        }
    }

    // ── Hot-swap routine ──────────────────────────────────────────────────────

    /// Perform the Wasm hot-swap.
    ///
    /// Must be called **only** when `state == OtaState::Ready`.  This method
    /// embodies the four-step critical section described in the Phase 8 design:
    ///
    /// 1. **Pause** — callers must have already signalled the Wasm VM to stop.
    /// 2. **Flush** — drops the staging buffer after the binary has been handed
    ///    to the caller via the returned `Vec<u8>`.
    /// 3. **Instantiate** — the caller receives the binary and rebuilds the
    ///    wasmi `Engine` + `Store` + `Module` + `Instance` chain.
    /// 4. **Resume** — the caller unblocks the Wasm command loop.
    ///
    /// Returns `Some(binary)` with the verified Wasm bytes, or `None` if the
    /// binary is not yet ready.
    ///
    /// ## Core 1 isolation
    ///
    /// This function only touches Core 0 heap memory (PSRAM-allocated `Vec`).
    /// The motor-command atomics shared with Core 1 are not accessed, so the
    /// real-time balance loop experiences zero jitter from the hot-swap.
    pub fn take_verified_binary(&mut self) -> Option<Vec<u8>> {
        if self.state != OtaState::Ready {
            return None;
        }
        self.state = OtaState::Swapping;
        // Drain the buffer without reallocating — zero-copy hand-off.
        let binary = core::mem::take(&mut self.buffer);
        self.swap_count     += 1;
        self.state           = OtaState::Idle;
        self.total_size      = 0;
        self.bytes_received  = 0;
        Some(binary)
    }

    /// Reset a `Failed` session so that a new `OTA_BEGIN` can be accepted.
    pub fn reset_failed(&mut self) {
        if self.state == OtaState::Failed {
            self.buffer.clear();
            self.total_size     = 0;
            self.bytes_received = 0;
            self.state          = OtaState::Idle;
        }
    }
}
