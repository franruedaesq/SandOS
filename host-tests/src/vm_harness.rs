//! Wasm VM test harness.
//!
//! [`WasmHarness`] spins up a `wasmi` interpreter with all ABI host functions
//! registered against a [`MockHost`].  Tests use it to run WAT snippets and
//! assert on the resulting host state.

use std::cell::RefCell;
use std::rc::Rc;

use abi::{
    status, validate_ptr_len, ImuReading, ImuTelemetry, OdometryTelemetry, HOST_MODULE,
    FN_DEBUG_LOG, FN_DRAW_EYE, FN_EMIT_IMU_TELEMETRY, FN_EMIT_ODOM_TELEMETRY,
    FN_GET_AUDIO_AVAIL, FN_GET_PITCH_ROLL, FN_GET_TELEMETRY_QUEUE_LEN, FN_GET_UPTIME_MS,
    FN_READ_AUDIO, FN_SET_BRIGHTNESS, FN_SET_MOTOR_SPEED, FN_START_AUDIO, FN_STOP_AUDIO,
    FN_TOGGLE_LED, FN_WRITE_TEXT, MAX_AUDIO_READ, MAX_MOTOR_SPEED, MAX_TEXT_BYTES,
};
use wasmi::{Caller, Engine, Linker, Memory, Module, Store};

use crate::mock_host::MockHost;

// ── Harness ───────────────────────────────────────────────────────────────────

/// A configured wasmi engine + store with all ABI host functions registered.
pub struct WasmHarness {
    engine:  Engine,
    store:   Store<Rc<RefCell<MockHost>>>,
    linker:  Linker<Rc<RefCell<MockHost>>>,
}

impl WasmHarness {
    /// Create a new harness with the provided [`MockHost`].
    pub fn new(host: MockHost) -> Self {
        let engine = Engine::default();
        let host   = Rc::new(RefCell::new(host));
        let store  = Store::new(&engine, Rc::clone(&host));
        let linker = build_linker(&engine);
        Self { engine, store, linker }
    }

    /// Compile and instantiate a WAT snippet, returning the wasmi [`Instance`].
    pub fn load_wat(&mut self, wat_src: &str) -> wasmi::Instance {
        let wasm   = wat::parse_str(wat_src).expect("invalid WAT");
        let module = Module::new(&self.engine, &wasm[..]).expect("invalid Wasm module");
        self.linker
            .instantiate(&mut self.store, &module)
            .expect("instantiation failed")
            .start(&mut self.store)
            .expect("start failed")
    }

    /// Borrow the mock host state for assertions.
    pub fn host(&self) -> std::cell::Ref<'_, MockHost> {
        self.store.data().borrow()
    }

    /// Mutably borrow the mock host state (e.g. to feed audio data).
    pub fn host_mut(&mut self) -> std::cell::RefMut<'_, MockHost> {
        self.store.data().borrow_mut()
    }

    /// Call a typed exported function `(param: i32) -> i32`.
    pub fn call_i32_i32(&mut self, instance: &wasmi::Instance, name: &str, arg: i32) -> i32 {
        let f = instance
            .get_typed_func::<i32, i32>(&self.store, name)
            .unwrap_or_else(|_| panic!("export '{}' not found", name));
        f.call(&mut self.store, arg).unwrap_or_else(|_| panic!("call '{}' failed", name))
    }

    /// Call a typed exported function `() -> i32`.
    pub fn call_unit_i32(&mut self, instance: &wasmi::Instance, name: &str) -> i32 {
        let f = instance
            .get_typed_func::<(), i32>(&self.store, name)
            .unwrap_or_else(|_| panic!("export '{}' not found", name));
        f.call(&mut self.store, ()).unwrap_or_else(|_| panic!("call '{}' failed", name))
    }

    /// Call a typed exported function `(param: i32, param: i32) -> i32`.
    pub fn call_i32i32_i32(&mut self, instance: &wasmi::Instance, name: &str, a: i32, b: i32) -> i32 {
        let f = instance
            .get_typed_func::<(i32, i32), i32>(&self.store, name)
            .unwrap_or_else(|_| panic!("export '{}' not found", name));
        f.call(&mut self.store, (a, b)).unwrap_or_else(|_| panic!("call '{}' failed", name))
    }

    /// Invoke a WAT export that is expected to trap (e.g., `unreachable`).
    ///
    /// Returns `Ok(result)` if the call succeeded without trapping, or
    /// `Err(wasmi::Error)` if the guest trapped.  This is used by Phase 4
    /// sandbox-isolation tests to verify that a Wasm trap does *not* crash
    /// the host harness.
    pub fn try_call_unit_i32(
        &mut self,
        instance: &wasmi::Instance,
        name: &str,
    ) -> Result<i32, wasmi::Error> {
        let f = instance
            .get_typed_func::<(), i32>(&self.store, name)
            .map_err(wasmi::Error::from)?;
        f.call(&mut self.store, ()).map_err(wasmi::Error::from)
    }
}

// ── Linker construction ───────────────────────────────────────────────────────

/// Register all ABI host functions into the linker using [`MockHost`].
fn build_linker(engine: &Engine) -> Linker<Rc<RefCell<MockHost>>> {
    let mut linker: Linker<Rc<RefCell<MockHost>>> = Linker::new(engine);

    // ── Phase 1 ──────────────────────────────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_TOGGLE_LED,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i32 {
            caller.data().borrow_mut().toggle_led()
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_GET_UPTIME_MS,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i64 {
            caller.data().borrow().get_uptime_ms()
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_DEBUG_LOG,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, ptr: i32, len: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
            caller.data().borrow_mut().debug_log(&bytes)
        }
    ).unwrap();

    // ── Phase 2 — Display ─────────────────────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_DRAW_EYE,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, expression: i32| -> i32 {
            caller.data().borrow_mut().draw_eye(expression)
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_WRITE_TEXT,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, ptr: i32, len: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            if len as u32 > MAX_TEXT_BYTES {
                return status::ERR_BOUNDS;
            }
            let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
            caller.data().borrow_mut().write_text(&bytes)
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_SET_BRIGHTNESS,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, value: i32| -> i32 {
            caller.data().borrow_mut().set_brightness(value)
        }
    ).unwrap();

    // ── Phase 2 — Audio ───────────────────────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_START_AUDIO,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i32 {
            caller.data().borrow_mut().start_audio_capture()
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_STOP_AUDIO,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i32 {
            caller.data().borrow_mut().stop_audio_capture()
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_GET_AUDIO_AVAIL,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i32 {
            caller.data().borrow().get_audio_avail()
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_READ_AUDIO,
        |mut caller: Caller<'_, Rc<RefCell<MockHost>>>, ptr: i32, max_len: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(ptr as u32, max_len as u32, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            if max_len as u32 > MAX_AUDIO_READ {
                return status::ERR_BOUNDS;
            }
            let n = max_len as usize;
            let mut tmp = vec![0u8; n];
            let copied = caller.data().borrow_mut().read_audio(&mut tmp) as usize;
            mem.data_mut(&mut caller)[ptr as usize..ptr as usize + copied]
                .copy_from_slice(&tmp[..copied]);
            copied as i32
        }
    ).unwrap();

    // ── Phase 3 — Sensors ─────────────────────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_GET_PITCH_ROLL,
        |mut caller: Caller<'_, Rc<RefCell<MockHost>>>, pitch_ptr: i32, roll_ptr: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(pitch_ptr as u32, 4, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            if validate_ptr_len(roll_ptr as u32, 4, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            let ImuReading { pitch_millideg, roll_millideg } =
                caller.data().borrow().get_pitch_roll();
            let data = mem.data_mut(&mut caller);
            data[pitch_ptr as usize..pitch_ptr as usize + 4]
                .copy_from_slice(&pitch_millideg.to_le_bytes());
            data[roll_ptr as usize..roll_ptr as usize + 4]
                .copy_from_slice(&roll_millideg.to_le_bytes());
            status::OK
        }
    ).unwrap();

    // ── Phase 4 — Motors ──────────────────────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_SET_MOTOR_SPEED,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, left: i32, right: i32| -> i32 {
            if left.abs() > MAX_MOTOR_SPEED || right.abs() > MAX_MOTOR_SPEED {
                return status::ERR_INVALID_ARG;
            }
            caller.data().borrow_mut().set_motor_speed(left, right)
        }
    ).unwrap();

    // ── Phase 6 — Structured Telemetry ───────────────────────────────────────

    linker.func_wrap(HOST_MODULE, FN_EMIT_IMU_TELEMETRY,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, ptr: i32, len: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            if len as usize != ImuTelemetry::SERIALIZED_SIZE {
                return status::ERR_BOUNDS;
            }
            let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
            caller.data().borrow_mut().emit_imu_telemetry(&bytes)
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_EMIT_ODOM_TELEMETRY,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>, ptr: i32, len: i32| -> i32 {
            let mem = match get_memory(&caller) {
                Some(m) => m,
                None    => return status::ERR_BOUNDS,
            };
            let mem_size = mem.data(&caller).len() as u32;
            if validate_ptr_len(ptr as u32, len as u32, mem_size).is_err() {
                return status::ERR_BOUNDS;
            }
            if len as usize != OdometryTelemetry::SERIALIZED_SIZE {
                return status::ERR_BOUNDS;
            }
            let bytes = mem.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
            caller.data().borrow_mut().emit_odom_telemetry(&bytes)
        }
    ).unwrap();

    linker.func_wrap(HOST_MODULE, FN_GET_TELEMETRY_QUEUE_LEN,
        |caller: Caller<'_, Rc<RefCell<MockHost>>>| -> i32 {
            caller.data().borrow().get_telemetry_queue_len()
        }
    ).unwrap();

    linker
}

// ── Utility ───────────────────────────────────────────────────────────────────

fn get_memory(caller: &Caller<'_, Rc<RefCell<MockHost>>>) -> Option<Memory> {
    caller.get_export("memory").and_then(|e| e.into_memory())
}
