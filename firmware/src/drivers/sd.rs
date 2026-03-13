use esp_hal::gpio::{GpioPin, Input, Pull};

use crate::hardware_profile::{set_sd_state, ModuleState};

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

    loop {
        let all_high = clk_in.is_high()
            && cmd_in.is_high()
            && d0_in.is_high()
            && d1_in.is_high()
            && d2_in.is_high()
            && d3_in.is_high();

        if all_high {
            set_sd_state(ModuleState::Online);
            log::debug!("[sd] SDIO pull-ups present; card/bus lines look sane");
        } else {
            set_sd_state(ModuleState::Fault);
            log::warn!("[sd] SDIO line check failed (one or more lines stuck low)");
        }

        embassy_time::Timer::after(embassy_time::Duration::from_secs(4)).await;
    }
}
