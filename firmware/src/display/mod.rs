use abi::{status, EyeExpression};
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::{GpioPin, Input, Output, Level},
    peripherals::SPI2,
    spi::master::{Config as SpiConfig, Spi},
    Blocking,
    time::RateExtU32,
    Async,
};
use display_interface_spi::SPIInterface;
use ili9341::{DisplaySize240x320, Ili9341, Orientation};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
    mono_font::{ascii::FONT_10X20, MonoTextStyle},
};

pub mod ui;

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;
const DISPLAY_QUEUE_DEPTH: usize = 8;
const DIAG_SKIP_FLUSH: bool = false;

pub type Ili9341Display = Ili9341<
    SPIInterface<ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, esp_hal::delay::Delay>, Output<'static>>,
    Output<'static>,
>;

static DISPLAY_CHANNEL: Channel<CriticalSectionRawMutex, DisplayCommand, DISPLAY_QUEUE_DEPTH> =
    Channel::new();
static BUTTON_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, ButtonEvent, 4> = Channel::new();

#[derive(Clone, Copy)]
enum ButtonEvent {
    ShortPress,
}

#[derive(Clone)]
enum DisplayCommand {
    SetExpression(EyeExpression),
    SetText(heapless::String<64>),
    SetBrightness(u8),
}

pub struct DisplayDriver {
    current_expression: EyeExpression,
    brightness: u8,
    last_text: heapless::String<64>,
}

impl DisplayDriver {
    pub fn new() -> Self {
        Self {
            current_expression: EyeExpression::Neutral,
            brightness: 255,
            last_text: heapless::String::new(),
        }
    }

    pub fn draw_eye(&mut self, expression: EyeExpression) -> i32 {
        self.current_expression = expression;
        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetExpression(expression));
        status::OK
    }

    pub fn write_text(&mut self, bytes: &[u8]) -> i32 {
        let text = match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return status::ERR_INVALID_ARG,
        };

        self.last_text.clear();
        for ch in text.chars().take(64) {
            let _ = self.last_text.push(ch);
        }

        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetText(self.last_text.clone()));
        status::OK
    }

    pub fn set_brightness(&mut self, value: u8) -> i32 {
        self.brightness = value;
        let _ = DISPLAY_CHANNEL
            .sender()
            .try_send(DisplayCommand::SetBrightness(value));
        status::OK
    }
}

impl Default for DisplayDriver {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn_display_task(
    spawner: Spawner,
    spi2: SPI2,
    mosi: GpioPin<11>,
    sck: GpioPin<12>,
    cs: GpioPin<10>,
    dc: GpioPin<46>,
    rst: GpioPin<45>,
    boot_btn: Input<'static>,
) {
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(spi2, mosi, sck, cs, dc, rst)).unwrap();
}

#[embassy_executor::task]
async fn button_task(mut boot_btn: Input<'static>) {
    loop {
        boot_btn.wait_for_falling_edge().await;
        let _ = BUTTON_EVENT_CHANNEL.sender().try_send(ButtonEvent::ShortPress);
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[embassy_executor::task]
async fn display_task(
    spi2: SPI2,
    mosi: GpioPin<11>,
    sck: GpioPin<12>,
    cs: GpioPin<10>,
    dc: GpioPin<46>,
    rst: GpioPin<45>,
) {
    let mut spi_cfg = SpiConfig::default();
    spi_cfg.frequency = 80.MHz();
    spi_cfg.mode = esp_hal::spi::master::Config::default().mode;

    let spi = Spi::new(spi2, spi_cfg)
        .expect("Failed to create SPI")
        .with_mosi(mosi)
        .with_sck(sck);

    let cs_out = Output::new(cs, Level::High);
    let dc_out = Output::new(dc, Level::High);
    let rst_out = Output::new(rst, Level::High);

    let delay = esp_hal::delay::Delay::new();
    let spi_device = ExclusiveDevice::new(spi, cs_out, delay).expect("Failed to create SPI device");
    let di = SPIInterface::new(spi_device, dc_out);

    let mut display = Ili9341::new(
        di,
        rst_out,
        &mut esp_hal::delay::Delay::new(),
        Orientation::Landscape,
        DisplaySize240x320,
    ).expect("Failed to initialize ILI9341");
    let _ = display.invert_mode(ili9341::ModeState::On);

    let receiver = DISPLAY_CHANNEL.receiver();
    let touch_receiver = crate::touch::TOUCH_EVENTS.receiver();

    let mut ui_manager = ui::UiManager::new();

    loop {
        // Drain display commands
        while let Ok(cmd) = receiver.try_receive() {
            match cmd {
                DisplayCommand::SetExpression(expr) => {
                    log::info!("[display] expression updated");
                    ui_manager.expression = expr;
                }
                DisplayCommand::SetText(text) => {
                    log::info!("[display] text updated");
                    ui_manager.text = text;
                }
                DisplayCommand::SetBrightness(_) => {
                    log::info!("[display] brightness updated");
                }
            }
        }

        // Drain touch events — remap portrait touch coords (240×320) to landscape display (320×240)
        while let Ok((tx, ty)) = touch_receiver.try_receive() {
            let x = ty as i32;           // portrait Y → landscape X
            let y = 240 - tx as i32;     // portrait X → landscape Y (inverted)
            ui_manager.handle_touch(x, y);
        }

        // Drain button events
        let btn_receiver = BUTTON_EVENT_CHANNEL.receiver();
        while let Ok(evt) = btn_receiver.try_receive() {
            match evt {
                ButtonEvent::ShortPress => {
                    if ui_manager.state == ui::UiState::Idle {
                        ui_manager.state = ui::UiState::Menu;
                        ui_manager.last_interaction_frame = ui_manager.frame_count;
                    } else {
                        ui_manager.selected_menu_item = (ui_manager.selected_menu_item + 1) % 4;
                        ui_manager.last_interaction_frame = ui_manager.frame_count;
                    }
                }
            }
        }

        // Advance UI logic and render
        ui_manager.update();
        let _ = ui_manager.render(&mut display);

        // 5. Flush framebuffer via async SPI (conceptually, though currently mock flush).
        if !DIAG_SKIP_FLUSH {
            // let _ = display.flush().await; // Usually required when double buffering
        }

        // ~30 fps — leaves CPU headroom for WiFi/Wasm on Core 0
        Timer::after(Duration::from_millis(33)).await;
    }
}
