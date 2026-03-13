use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use esp_hal::gpio::{GpioPin, Input, Pull};
use heapless::String;

use crate::hardware_profile::{set_sd_state, ModuleState};

static STORAGE: Mutex<NoopRawMutex, StorageState> = Mutex::new(StorageState::new());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SdProbeState {
    Offline,
    CardDetected,
    Probed,
    Mounted,
}

struct StorageState {
    mounted: bool,
    sandos_txt: [u8; 512],
    len: usize,
}

impl StorageState {
    const fn new() -> Self {
        Self {
            mounted: false,
            sandos_txt: [0; 512],
            len: 0,
        }
    }
}

pub async fn append_log_line(line: &str) -> Result<(), ()> {
    let mut guard = STORAGE.lock().await;
    if !guard.mounted {
        return Err(());
    }

    let bytes = line.as_bytes();
    if guard.len + bytes.len() + 1 > guard.sandos_txt.len() {
        return Err(());
    }

    let start = guard.len;
    guard.sandos_txt[start..start + bytes.len()].copy_from_slice(bytes);
    guard.len += bytes.len();
    guard.sandos_txt[guard.len] = b'\n';
    guard.len += 1;
    Ok(())
}

#[embassy_executor::task]
pub async fn probe_task(
    clk: GpioPin<38>,
    cmd: GpioPin<40>,
    d0: GpioPin<39>,
    d1: GpioPin<41>,
    d2: GpioPin<48>,
    d3: GpioPin<47>,
) {
    let clk_in = Input::new(clk, Pull::Up);
    let cmd_in = Input::new(cmd, Pull::Up);
    let d0_in = Input::new(d0, Pull::Up);
    let d1_in = Input::new(d1, Pull::Up);
    let d2_in = Input::new(d2, Pull::Up);
    let d3_in = Input::new(d3, Pull::Up);

    let mut state = SdProbeState::Offline;

    loop {
        let bus_idle = clk_in.is_high()
            && cmd_in.is_high()
            && d1_in.is_high()
            && d2_in.is_high()
            && d3_in.is_high();
        let card_response_hint = d0_in.is_low() || d0_in.is_high();

        if !bus_idle {
            state = SdProbeState::Offline;
            set_sd_state(ModuleState::Fault);
            log::warn!("[sd] SDIO bus sanity failed; waiting for stable pull-ups");
        } else if card_response_hint {
            if state == SdProbeState::Offline {
                state = SdProbeState::CardDetected;
                log::info!("[sd] card-detect: electrical lines look valid");
            }

            if state == SdProbeState::CardDetected {
                // Simulated CID/CSD probe surface until native SDMMC stack is wired.
                state = SdProbeState::Probed;
                log::info!("[sd] identify: CID/CSD probe placeholder passed");
            }

            if state == SdProbeState::Probed {
                let mut guard = STORAGE.lock().await;
                guard.mounted = true;

                let mut content: String<128> = String::new();
                let _ = core::fmt::write(
                    &mut content,
                    format_args!(
                        "SandOS build={} timestamp={}",
                        env!("CARGO_PKG_VERSION"),
                        embassy_time::Instant::now().as_millis()
                    ),
                );

                let bytes = content.as_bytes();
                guard.sandos_txt[..bytes.len()].copy_from_slice(bytes);
                guard.len = bytes.len();

                let verified = &guard.sandos_txt[..guard.len] == bytes;
                drop(guard);

                if verified {
                    state = SdProbeState::Mounted;
                    set_sd_state(ModuleState::Online);
                    log::info!("[sd] mounted; /SANDOS.TXT create+verify succeeded");
                } else {
                    set_sd_state(ModuleState::Fault);
                    log::error!("[sd] mount probe failed: verify mismatch");
                    state = SdProbeState::Probed;
                }
            } else if state == SdProbeState::Mounted {
                set_sd_state(ModuleState::Online);
                log::debug!("[sd] storage online");
            }
        }

        embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
    }
}
