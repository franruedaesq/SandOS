use crate::hardware_profile::{set_battery_state, ModuleState};

#[embassy_executor::task]
pub async fn probe_task() {
    loop {
        let uvlo = crate::ulp::is_voltage_critical();
        let mv = unsafe {
            let ptr = (0x5000_0000 + abi::ulp_mem::LAST_SUPPLY_MV) as *const u32;
            ptr.read_volatile()
        };

        if uvlo || mv < crate::ulp::VOLTAGE_MIN_MV {
            set_battery_state(ModuleState::Fault);
            log::warn!("[battery] undervoltage detected: {} mV", mv);
        } else {
            set_battery_state(ModuleState::Online);
            log::debug!("[battery] battery voltage {} mV", mv);
        }

        embassy_time::Timer::after(embassy_time::Duration::from_secs(3)).await;
    }
}
