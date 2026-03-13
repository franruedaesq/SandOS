use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    gpio::GpioPin,
    peripherals::ADC1,
};

use crate::hardware_profile::{set_battery_state, ModuleState};

const SAMPLE_PERIOD_MS: u64 = 1000;
const EMA_ALPHA_NUM: i32 = 1;
const EMA_ALPHA_DEN: i32 = 4;
const ADC_MAX: i32 = 4095;
const ADC_REF_MV: i32 = 1100;
const ADC_ATTEN_SCALE_NUM: i32 = 3100;
const ADC_ATTEN_SCALE_DEN: i32 = 1100;
const BATTERY_DIVIDER_NUM: i32 = 2;
const BATTERY_DIVIDER_DEN: i32 = 1;
const LOW_ENTER_MV: i32 = 3480;
const LOW_EXIT_MV: i32 = 3560;
const CRITICAL_ENTER_MV: i32 = 3320;
const CRITICAL_EXIT_MV: i32 = 3400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BatteryBand {
    Healthy,
    Low,
    Critical,
}

struct Median3 {
    buf: [i32; 3],
    count: usize,
    idx: usize,
}

impl Median3 {
    const fn new() -> Self {
        Self {
            buf: [0; 3],
            count: 0,
            idx: 0,
        }
    }

    fn push(&mut self, value: i32) -> i32 {
        self.buf[self.idx] = value;
        self.idx = (self.idx + 1) % self.buf.len();
        self.count = (self.count + 1).min(self.buf.len());

        if self.count < self.buf.len() {
            return value;
        }

        let mut v = self.buf;
        v.sort_unstable();
        v[1]
    }
}

#[inline]
fn adc_raw_to_mv(raw: u16) -> i32 {
    let raw = raw as i32;
    let pin_mv = (raw * ADC_REF_MV) / ADC_MAX;
    let atten_mv = (pin_mv * ADC_ATTEN_SCALE_NUM) / ADC_ATTEN_SCALE_DEN;
    (atten_mv * BATTERY_DIVIDER_NUM) / BATTERY_DIVIDER_DEN
}

#[inline]
fn liion_percent(mv: i32) -> u8 {
    let p = if mv <= 3300 {
        0
    } else if mv <= 3600 {
        (mv - 3300) * 20 / 300
    } else if mv <= 3700 {
        20 + (mv - 3600) * 20 / 100
    } else if mv <= 3800 {
        40 + (mv - 3700) * 20 / 100
    } else if mv <= 3900 {
        60 + (mv - 3800) * 15 / 100
    } else if mv <= 4100 {
        75 + (mv - 3900) * 20 / 200
    } else if mv <= 4200 {
        95 + (mv - 4100) * 5 / 100
    } else {
        100
    };

    p.clamp(0, 100) as u8
}

#[inline]
fn ulp_mv_fallback() -> i32 {
    unsafe {
        let ptr = (0x5000_0000 + abi::ulp_mem::LAST_SUPPLY_MV) as *const u32;
        ptr.read_volatile() as i32
    }
}

fn classify(filtered_mv: i32, current: BatteryBand) -> BatteryBand {
    match current {
        BatteryBand::Healthy => {
            if filtered_mv <= CRITICAL_ENTER_MV {
                BatteryBand::Critical
            } else if filtered_mv <= LOW_ENTER_MV {
                BatteryBand::Low
            } else {
                BatteryBand::Healthy
            }
        }
        BatteryBand::Low => {
            if filtered_mv <= CRITICAL_ENTER_MV {
                BatteryBand::Critical
            } else if filtered_mv >= LOW_EXIT_MV {
                BatteryBand::Healthy
            } else {
                BatteryBand::Low
            }
        }
        BatteryBand::Critical => {
            if filtered_mv >= CRITICAL_EXIT_MV {
                if filtered_mv >= LOW_EXIT_MV {
                    BatteryBand::Healthy
                } else {
                    BatteryBand::Low
                }
            } else {
                BatteryBand::Critical
            }
        }
    }
}

#[embassy_executor::task]
pub async fn probe_task(adc1: ADC1, bat_adc: GpioPin<9>) {
    let mut adc = Adc::new(adc1, AdcConfig::new());
    let mut channel = adc.enable_pin(bat_adc, Attenuation::_11dB);

    let mut median = Median3::new();
    let mut ema_mv: Option<i32> = None;
    let mut band = BatteryBand::Healthy;

    loop {
        let (raw_mv, source) = match nb::block!(adc.read_oneshot(&mut channel)) {
            Ok(raw) => (adc_raw_to_mv(raw), "adc"),
            Err(_) => (ulp_mv_fallback(), "ulp-fallback"),
        };

        let med_mv = median.push(raw_mv);
        let filtered_mv = match ema_mv {
            None => med_mv,
            Some(prev) => {
                (EMA_ALPHA_NUM * med_mv + (EMA_ALPHA_DEN - EMA_ALPHA_NUM) * prev) / EMA_ALPHA_DEN
            }
        };
        ema_mv = Some(filtered_mv);

        let pct = liion_percent(filtered_mv);
        let next_band = classify(filtered_mv, band);

        if next_band != band {
            log::warn!(
                "[battery] state transition {:?} -> {:?} (raw={}mV filtered={}mV {}%)",
                band,
                next_band,
                raw_mv,
                filtered_mv,
                pct
            );
        }
        band = next_band;

        match band {
            BatteryBand::Healthy => set_battery_state(ModuleState::Online),
            BatteryBand::Low => set_battery_state(ModuleState::Configured),
            BatteryBand::Critical => set_battery_state(ModuleState::Fault),
        }

        log::info!(
            "[battery] src={} raw={}mV filtered={}mV charge={}%, band={:?}",
            source,
            raw_mv,
            filtered_mv,
            pct,
            band
        );

        embassy_time::Timer::after(embassy_time::Duration::from_millis(SAMPLE_PERIOD_MS)).await;
    }
}
