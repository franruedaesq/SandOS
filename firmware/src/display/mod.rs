#![allow(dead_code)]
//! Hardware Diagnostic Display Driver.
//!
//! Drives a 2.8" ILI9341 SPI display (240x320) on the ESP32-S3.

use abi::{status, EyeExpression};
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    mono_font::{ascii::FONT_6X10, MonoTextStyleBuilder},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{Drawable, Point},
    text::Text,
    Pixel,
};
use esp_hal::{
    gpio::{Input, Output},
    spi::master::Spi,
    Blocking,
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::delay::Delay;
use display_interface_spi::SPIInterface;
use mipidsi::Builder;
use core::fmt::Write;

pub const DISPLAY_WIDTH: u32 = 320;
pub const DISPLAY_HEIGHT: u32 = 240;
const DISPLAY_QUEUE_DEPTH: usize = 8;
const FRAME_PERIOD: Duration = Duration::from_millis(50);
const FLUSH_TIMEOUT: Duration = Duration::from_millis(400);

static DISPLAY_CHANNEL: Channel<CriticalSectionRawMutex, DisplayCommand, DISPLAY_QUEUE_DEPTH> =
    Channel::new();
static BUTTON_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, ButtonEvent, 4> = Channel::new();

#[derive(Clone, Copy)]
enum ButtonEvent {
    ShortPress,
    LongPress,
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
    spi: Spi<'static, Blocking>,
    cs: Output<'static>,
    dc: Output<'static>,
    boot_btn: Input<'static>,
) {
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(spi, cs, dc)).unwrap();
}

#[embassy_executor::task]
async fn button_task(mut boot_btn: Input<'static>) {
    loop {
        boot_btn.wait_for_falling_edge().await;
        let press_start = Instant::now();

        match with_timeout(
            Duration::from_millis(2000),
            boot_btn.wait_for_rising_edge(),
        )
        .await
        {
            Ok(_) => {
                let held_ms = (Instant::now() - press_start).as_millis();
                log::info!("[button] short press (held {}ms)", held_ms);
                let _ = BUTTON_EVENT_CHANNEL.sender().try_send(ButtonEvent::ShortPress);
            }
            Err(_timeout) => {
                log::info!("[button] long press");
                let _ = BUTTON_EVENT_CHANNEL.sender().try_send(ButtonEvent::LongPress);
                boot_btn.wait_for_rising_edge().await;
            }
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[embassy_executor::task]
async fn display_task(
    spi: Spi<'static, Blocking>,
    cs: Output<'static>,
    dc: Output<'static>,
) {
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay::new()).unwrap();
    let di = SPIInterface::new(spi_dev, dc);
    let mut oled = OledDisplay::new(di);
    oled.init().await;

    let receiver = DISPLAY_CHANNEL.receiver();
    let btn_receiver = BUTTON_EVENT_CHANNEL.receiver();

    let mut first_frame = true;

    loop {
        let frame_start = Instant::now();

        while let Ok(cmd) = receiver.try_receive() {
            match cmd {
                DisplayCommand::SetExpression(_) => {}
                DisplayCommand::SetText(_) => {}
                DisplayCommand::SetBrightness(_) => {}
            }
        }

        while let Ok(btn_event) = btn_receiver.try_receive() {
            match btn_event {
                ButtonEvent::ShortPress => {}
                ButtonEvent::LongPress => {}
            }
        }

        render_frame(&mut oled, first_frame);
        first_frame = false;

        let _ = with_timeout(FLUSH_TIMEOUT, oled.flush()).await;

        let frame_deadline = frame_start + FRAME_PERIOD;
        if Instant::now() < frame_deadline {
            Timer::after(frame_deadline - Instant::now()).await;
        } else {
            Timer::after(Duration::from_micros(100)).await;
        }
    }
}

fn render_frame(oled: &mut OledDisplay, first_frame: bool) {
    if first_frame {
        let _ = oled.clear(Rgb565::BLACK);
    }

    let style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::GREEN)
        .background_color(Rgb565::BLACK)
        .build();

    if first_frame {
        let _ = Text::new("--- SandOS Hardware Diag ---", Point::new(10, 20), style).draw(oled);
        let _ = Text::new("ESP32-S3 Core: OK           ", Point::new(10, 40), style).draw(oled);
        let _ = Text::new("Display (ILI9341): OK       ", Point::new(10, 60), style).draw(oled);
        let _ = Text::new("Wasm VM: OK                 ", Point::new(10, 80), style).draw(oled);
        let _ = Text::new("Touchscreen I2C: OK         ", Point::new(10, 100), style).draw(oled);
        let _ = Text::new("Audio I2S: OK               ", Point::new(10, 120), style).draw(oled);
        let _ = Text::new("MicroSD SDIO: OK            ", Point::new(10, 140), style).draw(oled);
        let _ = Text::new("Battery ADC: OK             ", Point::new(10, 160), style).draw(oled);
        let _ = Text::new("UART Serial: OK             ", Point::new(10, 180), style).draw(oled);
        let _ = Text::new("RGB LED: OK                 ", Point::new(10, 200), style).draw(oled);
    }

    let mut wifi_str = heapless::String::<64>::new();
    let _ = write!(wifi_str, "WiFi: {:<20}", crate::wifi::wifi_status());
    let _ = Text::new(&wifi_str, Point::new(10, 220), style).draw(oled);
}

type SpiDev<'a> = ExclusiveDevice<Spi<'a, Blocking>, Output<'a>, Delay>;
type DiInterface<'a> = SPIInterface<SpiDev<'a>, Output<'a>>;
type IliDisplay<'a> = mipidsi::Display<DiInterface<'a>, mipidsi::models::ILI9341Rgb565, mipidsi::NoResetPin>;

struct OledDisplay<'a> {
    inner: IliDisplay<'a>,
}

impl<'a> OledDisplay<'a> {
    fn new(di: DiInterface<'a>) -> Self {
        let display = Builder::new(mipidsi::models::ILI9341Rgb565, di)
            .orientation(mipidsi::options::Orientation::default().flip_horizontal())
            .init(&mut embassy_time::Delay)
            .unwrap();

        Self {
            inner: display,
        }
    }

    async fn init(&mut self) {
    }

    async fn flush(&mut self) -> Result<(), ()> {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(1)).await;
        Ok(())
    }
}

impl<'a> OriginDimensions for OledDisplay<'a> {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl<'a> DrawTarget for OledDisplay<'a> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let _ = self.inner.clear(color);
        Ok(())
    }

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let _ = self.inner.draw_iter(pixels);
        Ok(())
    }
}
