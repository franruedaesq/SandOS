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
    spi_cfg.frequency = 40.MHz();
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

    // Draw something basic to prove the screen works (Phase 2 requirement)
    display.clear(Rgb565::BLACK).unwrap();
    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::GREEN);
    Text::new("SandOS Base", Point::new(20, 30), style).draw(&mut display).unwrap();

    let rect_style = PrimitiveStyle::with_fill(Rgb565::BLUE);
    Rectangle::new(Point::new(20, 50), Size::new(100, 50))
        .into_styled(rect_style)
        .draw(&mut display)
        .unwrap();

    let receiver = DISPLAY_CHANNEL.receiver();

    loop {
        // Drain display commands
        while let Ok(cmd) = receiver.try_receive() {
            match cmd {
                DisplayCommand::SetExpression(_) => {
                    log::info!("[display] expression updated");
                }
                DisplayCommand::SetText(_) => {
                    log::info!("[display] text updated");
                }
                DisplayCommand::SetBrightness(_) => {
                    log::info!("[display] brightness updated");
                }
            }
        }

        // 4b. Advance rlvgl ticks and render frame
        // This ensures LVGL gets CPU time every 10ms for smooth 3D/Lottie rendering
        // without starving other async networking tasks. We do this in a "Think-Wait"
        // non-blocking manner.
        // Note: Full rlvgl::tick() or task_handler() goes here when initialized.

        // 5. Flush framebuffer via async SPI (conceptually, though currently mock flush).
        if !DIAG_SKIP_FLUSH {
            // let _ = display.flush().await; // Usually required when double buffering
        }

        // 6. Sleep for a short interval (e.g. 10ms) to unblock the network/Wasm logic
        Timer::after(Duration::from_millis(10)).await;
    }
}
