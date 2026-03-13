//! Board hardware profile for the ESP32-S3 2.8" LCD + touch variant.
//!
//! Central pin map + runtime module health snapshots for the on-device monitor.

use portable_atomic::{AtomicU8, Ordering};

pub const CHIP_NAME: &str = "ESP32-S3 (dual-core LX7 @240MHz)";
pub const ROM_SIZE_KB: u32 = 384;
pub const SRAM_SIZE_KB: u32 = 512;
pub const FLASH_SIZE_MB: u32 = 16;

// LCD (ILI9341V, SPI)
pub const LCD_CS: u8 = 10;
pub const LCD_DC: u8 = 46;
pub const LCD_SCK: u8 = 12;
pub const LCD_MOSI: u8 = 11;
pub const LCD_MISO: u8 = 13;
pub const LCD_BL: u8 = 45;

// Capacitive touch (FT6336 family, I2C)
pub const TOUCH_SDA: u8 = 16;
pub const TOUCH_SCL: u8 = 15;
pub const TOUCH_RST: u8 = 18;
pub const TOUCH_INT: u8 = 17;
// Audio (I2S)
pub const AUDIO_EN: u8 = 1;
pub const AUDIO_MCLK: u8 = 4;
pub const AUDIO_BCLK: u8 = 5;
pub const AUDIO_DOUT: u8 = 6;
pub const AUDIO_WS: u8 = 7;
pub const AUDIO_DIN: u8 = 8;

// Storage / misc
pub const SD_CLK: u8 = 38;
pub const SD_CMD: u8 = 40;
pub const SD_DATA0: u8 = 39;
pub const SD_DATA1: u8 = 41;
pub const SD_DATA2: u8 = 48;
pub const SD_DATA3: u8 = 47;
pub const RGB_LED: u8 = 42;
pub const UART0_RX: u8 = 43;
pub const UART0_TX: u8 = 44;
pub const BATTERY_ADC: u8 = 9;
pub const BOOT_BUTTON: u8 = 0;
pub const RESET_SIGNAL: &str = "EN";

// Expansion socket
pub const EXP_GPIO2: u8 = 2;
pub const EXP_GPIO3: u8 = 3;
pub const EXP_GPIO14: u8 = 14;
pub const EXP_GPIO21: u8 = 21;

// USB-C / UART bridge
pub const USB_UART_RX: u8 = UART0_RX;
pub const USB_UART_TX: u8 = UART0_TX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ModuleState {
    Unknown = 0,
    Configured = 1,
    Online = 2,
    Fault = 3,
}

impl ModuleState {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Configured,
            2 => Self::Online,
            3 => Self::Fault,
            _ => Self::Unknown,
        }
    }

    pub const fn short(self) -> &'static str {
        match self {
            Self::Unknown => "UNK",
            Self::Configured => "CFG",
            Self::Online => "OK",
            Self::Fault => "ERR",
        }
    }
}

static DISPLAY_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static TOUCH_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static AUDIO_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static SD_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static BATTERY_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static RGB_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);
static UART_STATE: AtomicU8 = AtomicU8::new(ModuleState::Unknown as u8);

pub fn set_display_state(state: ModuleState) {
    DISPLAY_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_touch_state(state: ModuleState) {
    TOUCH_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_audio_state(state: ModuleState) {
    AUDIO_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_sd_state(state: ModuleState) {
    SD_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_battery_state(state: ModuleState) {
    BATTERY_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_rgb_state(state: ModuleState) {
    RGB_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn set_uart_state(state: ModuleState) {
    UART_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn init_module_states() {
    // Display starts UNKNOWN and turns ONLINE once the init sequence succeeds.
    set_display_state(ModuleState::Unknown);

    // Touch starts UNKNOWN until FT6336 init and event path validation succeed.
    set_touch_state(ModuleState::Unknown);

    // Audio/SD/Battery drivers are not probed yet, but their pins are routed and ready.
    set_audio_state(ModuleState::Configured);
    set_sd_state(ModuleState::Configured);
    set_battery_state(ModuleState::Configured);

    // RGB + UART are initialized during early boot.
    set_rgb_state(ModuleState::Online);
    set_uart_state(ModuleState::Online);
}

pub struct ModuleSnapshot {
    pub display: ModuleState,
    pub touch: ModuleState,
    pub audio: ModuleState,
    pub sd: ModuleState,
    pub battery: ModuleState,
    pub rgb: ModuleState,
    pub uart: ModuleState,
}

pub fn snapshot() -> ModuleSnapshot {
    ModuleSnapshot {
        display: ModuleState::from_u8(DISPLAY_STATE.load(Ordering::Relaxed)),
        touch: ModuleState::from_u8(TOUCH_STATE.load(Ordering::Relaxed)),
        audio: ModuleState::from_u8(AUDIO_STATE.load(Ordering::Relaxed)),
        sd: ModuleState::from_u8(SD_STATE.load(Ordering::Relaxed)),
        battery: ModuleState::from_u8(BATTERY_STATE.load(Ordering::Relaxed)),
        rgb: ModuleState::from_u8(RGB_STATE.load(Ordering::Relaxed)),
        uart: ModuleState::from_u8(UART_STATE.load(Ordering::Relaxed)),
    }
}

pub fn boot_status_line() -> heapless::String<64> {
    let snap = snapshot();
    let mut s = heapless::String::<64>::new();
    let _ = core::fmt::write(
        &mut s,
        format_args!(
            "D:{} T:{} A:{} SD:{} B:{} R:{} U:{}",
            snap.display.short(),
            snap.touch.short(),
            snap.audio.short(),
            snap.sd.short(),
            snap.battery.short(),
            snap.rgb.short(),
            snap.uart.short(),
        ),
    );
    s
}

#[embassy_executor::task]
pub async fn diagnostics_log_task() {
    loop {
        let snap = snapshot();
        let wifi = match crate::wifi::wifi_status() {
            crate::wifi::WIFI_STATUS_CONNECTED => "OK",
            crate::wifi::WIFI_STATUS_CONNECTING => "CONN",
            crate::wifi::WIFI_STATUS_ERROR => "ERR",
            _ => "OFF",
        };
        log::info!(
            "[hw] wifi={} disp={} touch={} audio={} sd={} bat={} rgb={} uart={}",
            wifi,
            snap.display.short(),
            snap.touch.short(),
            snap.audio.short(),
            snap.sd.short(),
            snap.battery.short(),
            snap.rgb.short(),
            snap.uart.short()
        );

        embassy_time::Timer::after(embassy_time::Duration::from_secs(10)).await;
    }
}
