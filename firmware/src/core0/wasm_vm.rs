//! Wasm Virtual Machine — Core 0 task.
//!
//! Embeds a [`wasmi`] interpreter, registers all Host-Guest ABI functions,
//! and drives the guest application in response to commands received from
//! the ESP-NOW radio.
//!
//! ## Memory layout
//!
//! The Wasm engine (engine + store + module) is allocated on the PSRAM heap
//! so it does not consume the fast SRAM needed by Core 1.  The `#[global_allocator]`
//! in `main.rs` handles this transparently.
//!
//! ## Sandbox isolation
//!
//! All host functions validate their arguments *before* touching hardware.
//! If the Wasm guest passes an out-of-bounds pointer, an invalid expression
//! ID, or any other malformed argument, the Host returns an error code and
//! the guest continues running — it cannot crash the Host OS.
extern crate alloc;

use abi::{
    validate_ptr_len, EyeExpression, MAX_AUDIO_READ, MAX_TEXT_BYTES, HOST_MODULE,
    FN_DEBUG_LOG, FN_DRAW_EYE, FN_GET_AUDIO_AVAIL, FN_GET_UPTIME_MS, FN_READ_AUDIO,
    FN_SET_BRIGHTNESS, FN_START_AUDIO, FN_STOP_AUDIO, FN_TOGGLE_LED, FN_WRITE_TEXT,
    status,
};
use alloc::vec;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver};
use wasmi::{Caller, Engine, Func, Instance, Linker, Memory, Module, Store};

use super::abi::AbiHost;
use crate::core0::WasmCommand;

// ── Embedded guest binary ─────────────────────────────────────────────────────

/// The compiled Phase 1 guest Wasm binary, baked into the firmware image.
///
/// Replace with `include_bytes!("../../../wasm-apps/target/…/wasm_apps.wasm")`
/// once the `wasm-apps` crate has been compiled.
static GUEST_WASM: &[u8] = include_bytes!("../../guest.wasm");

// ── Wasm task ─────────────────────────────────────────────────────────────────

/// Core 0 Wasm engine task.
///
/// Spins up the wasmi interpreter, registers all ABI host functions, then
/// loops — waiting for a [`WasmCommand`] and invoking the guest's exported
/// `run_command(cmd_id: i32)` function.
#[embassy_executor::task]
pub async fn wasm_run_task(
    receiver: Receiver<'static, CriticalSectionRawMutex, WasmCommand, 8>,
    mut host: AbiHost,
) {
    // Build the engine once; it is reused for the lifetime of the firmware.
    let engine = Engine::default();

    let module = match Module::new(&engine, GUEST_WASM) {
        Ok(m) => m,
        Err(_) => {
            // Malformed Wasm binary — halt this task.
            return;
        }
    };

    let mut store = Store::new(&engine, &mut host as *mut AbiHost);
    let linker = build_linker(&engine);

    let instance = match linker
        .instantiate(&mut store, &module)
        .and_then(|pre| pre.start(&mut store))
    {
        Ok(i) => i,
        Err(_) => return,
    };

    // Look up the guest's `run_command` export once.
    let run_command = match instance.get_typed_func::<i32, i32>(&store, "run_command") {
        Ok(f) => f,
        Err(_) => return,
    };

    // ── Command loop ──────────────────────────────────────────────────────────
    loop {
        let cmd = receiver.receive().await;
        // Invoke the guest; ignore the return value (it's the ABI status).
        run_command.call(&mut store, cmd.cmd_id as i32).ok();
    }
}

// ── Linker construction ───────────────────────────────────────────────────────

/// Register every ABI host function with the wasmi [`Linker`].
///
/// The closures capture `*mut AbiHost` through the Store data pointer.
/// This is safe because:
/// - Only one Wasm task runs on Core 0 at a time.
/// - The `AbiHost` outlives the Store (it is owned by `brain_task`).
fn build_linker(engine: &Engine) -> Linker<*mut AbiHost> {
    let mut linker: Linker<*mut AbiHost> = Linker::new(engine);

    // ── Phase 1 ──────────────────────────────────────────────────────────────

    linker
        .func_wrap(HOST_MODULE, FN_TOGGLE_LED, |caller: Caller<'_, *mut AbiHost>| -> i32 {
            let host = unsafe { &mut **caller.data() };
            host.toggle_led()
        })
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_GET_UPTIME_MS,
            |caller: Caller<'_, *mut AbiHost>| -> i64 {
                let host = unsafe { &**caller.data() };
                host.get_uptime_ms() as i64
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_DEBUG_LOG,
            |mut caller: Caller<'_, *mut AbiHost>, ptr: i32, len: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
                let host = unsafe { &mut **caller.data() };
                host.debug_log(&bytes)
            },
        )
        .unwrap();

    // ── Phase 2 — Display ─────────────────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_DRAW_EYE,
            |caller: Caller<'_, *mut AbiHost>, expression: i32| -> i32 {
                let host = unsafe { &mut **caller.data() };
                host.draw_eye(expression)
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_WRITE_TEXT,
            |mut caller: Caller<'_, *mut AbiHost>, ptr: i32, len: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                if len as u32 > MAX_TEXT_BYTES {
                    return status::ERR_BOUNDS;
                }
                let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
                let host = unsafe { &mut **caller.data() };
                host.write_text(&bytes)
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_SET_BRIGHTNESS,
            |caller: Caller<'_, *mut AbiHost>, value: i32| -> i32 {
                let host = unsafe { &mut **caller.data() };
                host.set_brightness(value)
            },
        )
        .unwrap();

    // ── Phase 2 — Audio ───────────────────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_START_AUDIO,
            |caller: Caller<'_, *mut AbiHost>| -> i32 {
                let host = unsafe { &mut **caller.data() };
                host.start_audio_capture()
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_STOP_AUDIO,
            |caller: Caller<'_, *mut AbiHost>| -> i32 {
                let host = unsafe { &mut **caller.data() };
                host.stop_audio_capture()
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_GET_AUDIO_AVAIL,
            |caller: Caller<'_, *mut AbiHost>| -> i32 {
                let host = unsafe { &**caller.data() };
                host.get_audio_avail()
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            FN_READ_AUDIO,
            |mut caller: Caller<'_, *mut AbiHost>, ptr: i32, max_len: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                if validate_ptr_len(ptr as u32, max_len as u32, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                if max_len as u32 > MAX_AUDIO_READ {
                    return status::ERR_BOUNDS;
                }
                // Temporarily extract audio into a scratch buffer, then
                // write it into Wasm memory.
                let n = max_len as usize;
                let mut tmp = vec![0u8; n];
                let host = unsafe { &mut **caller.data() };
                let copied = host.read_audio(&mut tmp) as usize;
                mem.data_mut(&mut caller)[ptr as usize..ptr as usize + copied]
                    .copy_from_slice(&tmp[..copied]);
                copied as i32
            },
        )
        .unwrap();

    linker
}

// ── Utility ───────────────────────────────────────────────────────────────────

/// Retrieve the exported `"memory"` from the caller's instance, if present.
fn get_memory(caller: &Caller<'_, *mut AbiHost>) -> Option<Memory> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
}
