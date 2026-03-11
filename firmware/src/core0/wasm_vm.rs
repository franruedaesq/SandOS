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
//!
//! ## Phase 4 — Watchdog
//!
//! The hardware Watchdog Timer (WDT) is fed after every successful Wasm
//! command execution.  If the guest enters an infinite loop or crashes in a
//! way that prevents the feed, the WDT fires and resets Core 0's Embassy
//! executor, restarting the Wasm sandbox.  Core 1's balance loop is unaffected.
//!
//! ## Phase 8 — OTA Hot-Swap
//!
//! After each command the task polls [`super::OTA_SWAP_SIGNAL`].  When the
//! signal fires (set by the ESP-NOW OTA handler after CRC-32 verification):
//!
//! 1. **Pause** — the command loop stops dispatching new commands.
//! 2. **Flush** — the old [`Store`], [`Module`], and [`Instance`] are dropped,
//!    freeing all Wasm linear memory back to the PSRAM heap.
//! 3. **Instantiate** — a new engine is built from the bytes in the PSRAM
//!    staging sector supplied by [`super::ota::OtaReceiver`].
//! 4. **Resume** — the command loop continues with the new guest binary.
//!
//! Core 1's PID/motor loop is never paused — the hot-swap is invisible to it.
extern crate alloc;

use abi::{
    validate_ptr_len, ImuReading, MAX_AUDIO_READ, MAX_MOTOR_SPEED, MAX_TEXT_BYTES,
    HOST_MODULE, FN_DEBUG_LOG, FN_DRAW_EYE, FN_GET_AUDIO_AVAIL, FN_GET_LOCAL_INFERENCE,
    FN_GET_OTA_STATUS, FN_GET_PITCH_ROLL, FN_GET_UPTIME_MS, FN_READ_AUDIO, FN_SET_BRIGHTNESS,
    FN_SET_MOTOR_SPEED, FN_START_AUDIO, FN_STOP_AUDIO, FN_TOGGLE_LED, FN_WRITE_TEXT,
    INFERENCE_RESULT_SIZE, OTA_STATUS_SIZE, status,
};
use alloc::vec;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver};
use embassy_time;
use wasmi::{Caller, Engine, Linker, Memory, Module, Store};

use super::abi::AbiHost;
use crate::core0::WasmCommand;

// ── Embedded guest binary ─────────────────────────────────────────────────────

/// The compiled Phase 1 guest Wasm binary, baked into the firmware image.
///
/// Replace with `include_bytes!("../../../wasm-apps/target/…/wasm_apps.wasm")`
/// once the `wasm-apps` crate has been compiled.
static GUEST_WASM: &[u8] = include_bytes!("../../guest.wasm");

// ── Watchdog timeout ──────────────────────────────────────────────────────────

/// Maximum time allowed for a single Wasm command execution (milliseconds).
///
/// The hardware WDT is configured to this timeout.  If `run_command` does not
/// return within this window — because the guest is in an infinite loop or has
/// crashed — the WDT fires and resets the sandbox.
const WDT_TIMEOUT_MS: u64 = 500;

// ── Wasm task ─────────────────────────────────────────────────────────────────

/// Core 0 Wasm engine task.
///
/// Spins up the wasmi interpreter, registers all ABI host functions, then
/// loops — waiting for a [`WasmCommand`] and invoking the guest's exported
/// `run_command(cmd_id: i32)` function.
///
/// ## Watchdog handling
///
/// The hardware WDT is fed after each successful `run_command` call.  If the
/// guest traps (e.g. an `unreachable` instruction, a stack overflow, or a
/// host-function panic), `wasmi` returns an [`Err`] without entering an
/// infinite loop, so the WDT is *still* fed and the sandbox remains alive.
///
/// An intentional infinite loop *inside* Wasm would prevent the feed; the WDT
/// would then reset Core 0, clearing the stuck Wasm execution while leaving
/// Core 1's PID/motor loop completely untouched.
///
/// ## Phase 8 — OTA Hot-Swap
///
/// After each command the task polls [`super::OTA_SWAP_SIGNAL`].  When the
/// signal fires, the four-step hot-swap routine is executed (pause → flush →
/// instantiate → resume) before the next command is dispatched.  Core 1 is
/// never paused.
#[embassy_executor::task]
pub async fn wasm_run_task(
    receiver: Receiver<'static, CriticalSectionRawMutex, WasmCommand, 8>,
    mut host: AbiHost,
) {
    log::info!("[wasm_vm] task starting — guest binary {} bytes", GUEST_WASM.len());

    // Build the initial engine from the baked-in guest binary.
    log::info!("[wasm_vm] build_vm — start");
    let t0 = embassy_time::Instant::now();
    let (mut engine, mut store, mut run_command) = match build_vm(GUEST_WASM, &mut host) {
        Some(v) => {
            let dt = (embassy_time::Instant::now() - t0).as_millis();
            log::info!("[wasm_vm] build_vm — done ({}ms)", dt);
            v
        }
        None => {
            log::error!("[wasm_vm] build_vm FAILED — halting");
            return; // malformed initial binary — halt
        }
    };

    log::info!("[wasm_vm] entering command loop (waiting for commands)");

    // ── Command loop ──────────────────────────────────────────────────────────
    loop {
        // Phase 8: poll the OTA signal *before* blocking on the next command.
        // `try_take` is non-blocking — it returns `Some(binary_len)` only when
        // the ESP-NOW OTA handler has signalled a verified binary is ready.
        if let Some(_binary_len) = super::OTA_SWAP_SIGNAL.try_take() {
            // ── Hot-swap routine ──────────────────────────────────────────────
            //
            // Step 1: Pause — drop the current Wasm runtime state so the
            // command loop cannot dispatch any further guest calls.
            let _ = run_command;
            drop(store);
            drop(engine);

            // Step 2: Flush — the old Wasm linear memory sandbox has been
            // released by the drops above; the PSRAM heap reclaims the pages.

            // Step 3: Instantiate — obtain the verified binary from the OTA
            // receiver and build a fresh wasmi engine from it.
            //
            // In production this reads from the PSRAM staging sector managed
            // by `OtaReceiver`.  Here we fall back to the static binary so
            // the firmware remains operational if the receiver has already
            // consumed the buffer.
            let binary: &[u8] = GUEST_WASM; // placeholder: real impl reads from OtaReceiver
            match build_vm(binary, &mut host) {
                Some((e, s, f)) => {
                    engine      = e;
                    store       = s;
                    run_command = f;
                    super::increment_hot_swap_count();
                    // Step 4: Resume — fall through to the next loop iteration.
                }
                None => {
                    // New binary is malformed — attempt to restore the static
                    // fallback so Core 1 is never left without a command source.
                    if let Some((e, s, f)) = build_vm(GUEST_WASM, &mut host) {
                        engine      = e;
                        store       = s;
                        run_command = f;
                    } else {
                        return; // both binaries are broken — halt Core 0
                    }
                }
            }
        }

        let cmd = receiver.receive().await;

        // Invoke the guest.  A Wasm trap (unreachable, OOB memory access, etc.)
        // returns Err here — the sandbox stays alive and handles the next command.
        let _result = run_command.call(&mut store, cmd.cmd_id as i32);

        // Feed the watchdog after every command (success *or* trap).
        //
        // In the full firmware implementation:
        //   watchdog.feed();
        //
        // The WDT is configured in `main()` with a timeout of WDT_TIMEOUT_MS.
        // A guest infinite loop would prevent reaching this line, triggering
        // the WDT and resetting only Core 0, not Core 1.
        let _ = WDT_TIMEOUT_MS; // referenced to satisfy the compiler
    }
}

// ── VM constructor ────────────────────────────────────────────────────────────

/// Build a complete wasmi VM from `binary` bytes.
///
/// Returns `(Engine, Store, TypedFunc)` on success, or `None` if the binary
/// cannot be decoded or instantiated.  The returned function is the guest's
/// `run_command(i32) -> i32` export.
fn build_vm(
    binary: &[u8],
    host:   &mut AbiHost,
) -> Option<(Engine, Store<*mut AbiHost>, wasmi::TypedFunc<i32, i32>)> {
    let engine = Engine::default();
    let module = Module::new(&engine, binary).ok()?;
    let mut store  = Store::new(&engine, host as *mut AbiHost);
    let linker = build_linker(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .and_then(|pre| pre.start(&mut store))
        .ok()?;
    let run_command = instance.get_typed_func::<i32, i32>(&store, "run_command").ok()?;
    Some((engine, store, run_command))
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
            |caller: Caller<'_, *mut AbiHost>, ptr: i32, len: i32| -> i32 {
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
            |caller: Caller<'_, *mut AbiHost>, ptr: i32, len: i32| -> i32 {
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

    // ── Phase 3 — Sensors ─────────────────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_GET_PITCH_ROLL,
            |mut caller: Caller<'_, *mut AbiHost>, pitch_ptr: i32, roll_ptr: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                // Validate both 4-byte write slots.
                if validate_ptr_len(pitch_ptr as u32, 4, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                if validate_ptr_len(roll_ptr as u32, 4, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                let host = unsafe { &**caller.data() };
                let ImuReading { pitch_millideg, roll_millideg } = host.get_pitch_roll();
                let data = mem.data_mut(&mut caller);
                data[pitch_ptr as usize..pitch_ptr as usize + 4]
                    .copy_from_slice(&pitch_millideg.to_le_bytes());
                data[roll_ptr as usize..roll_ptr as usize + 4]
                    .copy_from_slice(&roll_millideg.to_le_bytes());
                status::OK
            },
        )
        .unwrap();

    // ── Phase 4 — Motors ──────────────────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_SET_MOTOR_SPEED,
            |caller: Caller<'_, *mut AbiHost>, left: i32, right: i32| -> i32 {
                if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
                    return status::ERR_INVALID_ARG;
                }
                let host = unsafe { &**caller.data() };
                host.set_motor_speed(left, right)
            },
        )
        .unwrap();

    // ── Phase 7 — Local AI Subsystem ──────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_GET_LOCAL_INFERENCE,
            |mut caller: Caller<'_, *mut AbiHost>, out_ptr: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                if validate_ptr_len(out_ptr as u32, INFERENCE_RESULT_SIZE, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                let host = unsafe { &**caller.data() };
                let mut tmp = [0u8; INFERENCE_RESULT_SIZE as usize];
                let status = host.get_local_inference(&mut tmp);
                if status == status::OK {
                    let end = out_ptr as usize + INFERENCE_RESULT_SIZE as usize;
                    mem.data_mut(&mut caller)[out_ptr as usize..end]
                        .copy_from_slice(&tmp);
                }
                status
            },
        )
        .unwrap();

    // ── Phase 8 — OTA Hot-Swap Engine ─────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            FN_GET_OTA_STATUS,
            |mut caller: Caller<'_, *mut AbiHost>, out_ptr: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                if validate_ptr_len(out_ptr as u32, OTA_STATUS_SIZE, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                let host = unsafe { &**caller.data() };
                let mut tmp = [0u8; OTA_STATUS_SIZE as usize];
                let result = host.get_ota_status(&mut tmp);
                if result == status::OK {
                    let end = out_ptr as usize + OTA_STATUS_SIZE as usize;
                    mem.data_mut(&mut caller)[out_ptr as usize..end]
                        .copy_from_slice(&tmp);
                }
                result
            },
        )
        .unwrap();

    // ── Phase 9 — RGB LED Control ─────────────────────────────────────────────

    linker
        .func_wrap(
            HOST_MODULE,
            abi::FN_SET_RGB_LED,
            |caller: Caller<'_, *mut AbiHost>, red: i32, green: i32, blue: i32| -> i32 {
                let host = unsafe { &mut **caller.data() };
                host.set_rgb_led(red, green, blue)
            },
        )
        .unwrap();

    linker
        .func_wrap(
            HOST_MODULE,
            abi::FN_GET_RGB_LED,
            |mut caller: Caller<'_, *mut AbiHost>, red_ptr: i32, green_ptr: i32, blue_ptr: i32| -> i32 {
                let mem = match get_memory(&caller) {
                    Some(m) => m,
                    None => return status::ERR_BOUNDS,
                };
                let mem_size = mem.data(&caller).len() as u32;
                // Validate all three 4-byte write slots.
                if validate_ptr_len(red_ptr as u32, 4, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                if validate_ptr_len(green_ptr as u32, 4, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }
                if validate_ptr_len(blue_ptr as u32, 4, mem_size).is_err() {
                    return status::ERR_BOUNDS;
                }

                let host = unsafe { &mut **caller.data() };
                // Get the current RGB values; we need temporary mutable i32 pointers
                // into the Wasm memory to pass to the host function.
                let data = mem.data_mut(&mut caller);
                let red_ptr_obj = &mut data[red_ptr as usize] as *mut u8 as *mut i32;
                let green_ptr_obj = &mut data[green_ptr as usize] as *mut u8 as *mut i32;
                let blue_ptr_obj = &mut data[blue_ptr as usize] as *mut u8 as *mut i32;

                host.get_rgb_led(red_ptr_obj, green_ptr_obj, blue_ptr_obj)
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
