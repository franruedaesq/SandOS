use abi::{status, EyeExpression};
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use embassy_time::{Duration, Timer};
use esp_hal::{
    gpio::{GpioPin, Input, Output, Level},
    peripherals::SPI2,
    spi::master::{Config as SpiConfig, Spi, SpiDmaBus},
    time::RateExtU32,
    Async,
    dma::{DmaChannelFor, DmaDescriptor, DmaRxBuf, DmaTxBuf},
};
use embedded_graphics::pixelcolor::Rgb565;

pub mod strip;
pub mod ui;

pub const DISPLAY_WIDTH: u32 = 320;  // landscape
pub const DISPLAY_HEIGHT: u32 = 240;
const DISPLAY_QUEUE_DEPTH: usize = 8;

/// Strip height in scanlines. 40 lines × 320px × 2 bytes = 25,600 bytes per strip.
// This is a tradeoff between CPU overhead (lower is better) and DMA buffer size (higher is better).
// IMPORTANT FOR UI: This value affects the animation smoothness and touch response latency. Too high and the UI will feel sluggish, too low and CPU overhead may cause frame drops. 40 is a good balance for this application. Adjust with care.
const STRIP_HEIGHT: i32 = 30;

// ILI9341 commands
const CMD_SOFTWARE_RESET: u8 = 0x01;
const CMD_SLEEP_OFF: u8 = 0x11;
const CMD_INVERT_ON: u8 = 0x21;
const CMD_DISPLAY_ON: u8 = 0x29;
const CMD_COLUMN_ADDRESS_SET: u8 = 0x2A;
const CMD_PAGE_ADDRESS_SET: u8 = 0x2B;
const CMD_MEMORY_WRITE: u8 = 0x2C;
const CMD_MEMORY_ACCESS_CONTROL: u8 = 0x36;
const CMD_PIXEL_FORMAT_SET: u8 = 0x3A;

// DMA buffers — must be in internal SRAM ('static)
const STRIP_BYTES: usize = (STRIP_HEIGHT as usize) * (DISPLAY_WIDTH as usize) * 2;
static mut SPI_TX_DESC: [DmaDescriptor; 10] = [DmaDescriptor::EMPTY; 10];
static mut SPI_TX_BUF: [u8; STRIP_BYTES] = [0u8; STRIP_BYTES];
static mut SPI_RX_DESC: [DmaDescriptor; 1] = [DmaDescriptor::EMPTY; 1];
static mut SPI_RX_BUF: [u8; 4] = [0u8; 4];

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

/// DMA-backed ILI9341 display driver.
///
/// Manages DC and CS pins manually alongside an async SPI DMA bus
/// for bulk pixel transfers.
struct DmaDisplay {
    spi: SpiDmaBus<'static, Async>,
    dc: Output<'static>,
    cs: Output<'static>,
}

impl DmaDisplay {
    /// Send a command byte followed by optional data bytes (blocking, for rare commands).
    fn send_command(&mut self, cmd: u8, data: &[u8]) {
        self.cs.set_low();
        self.dc.set_low();
        let _ = self.spi.write(&[cmd]);
        if !data.is_empty() {
            self.dc.set_high();
            let _ = self.spi.write(data);
        }
        self.cs.set_high();
    }

    /// Set the drawing window for subsequent pixel writes.
    fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
        self.send_command(CMD_COLUMN_ADDRESS_SET, &[
            (x0 >> 8) as u8, x0 as u8,
            (x1 >> 8) as u8, x1 as u8,
        ]);
        self.send_command(CMD_PAGE_ADDRESS_SET, &[
            (y0 >> 8) as u8, y0 as u8,
            (y1 >> 8) as u8, y1 as u8,
        ]);
    }

    /// DMA-send pixel data to the display (async hot path).
    async fn write_pixels(&mut self, data: &[u8]) {
        self.cs.set_low();
        // Send MemoryWrite command (blocking — single byte)
        self.dc.set_low();
        let _ = self.spi.write(&[CMD_MEMORY_WRITE]);
        // Send pixel data via async DMA
        self.dc.set_high();
        let _ = self.spi.write_async(data).await;
        self.cs.set_high();
    }
}

/// Set up DMA SPI and hardware, then spawn display + button tasks.
///
/// All generic DMA channel handling happens here (not in the embassy task,
/// which cannot be generic).
pub fn spawn_display_task<CH: DmaChannelFor<esp_hal::spi::AnySpi> + 'static>(
    spawner: Spawner,
    spi2: SPI2,
    mosi: GpioPin<11>,
    sck: GpioPin<12>,
    cs: GpioPin<10>,
    dc: GpioPin<46>,
    rst: GpioPin<45>,
    boot_btn: Input<'static>,
    dma_channel: CH,
) {
    // ── SPI + DMA setup ────────────────────────────────────────────────────
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

    let tx_descriptors = unsafe { &mut *core::ptr::addr_of_mut!(SPI_TX_DESC) };
    let tx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(SPI_TX_BUF) };
    let rx_descriptors = unsafe { &mut *core::ptr::addr_of_mut!(SPI_RX_DESC) };
    let rx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(SPI_RX_BUF) };

    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).expect("Failed to create DMA TX buf");
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).expect("Failed to create DMA RX buf");

    let spi_dma = spi.with_dma(dma_channel);
    let spi_dma_bus = spi_dma.with_buffers(dma_rx_buf, dma_tx_buf).into_async();

    // ── Spawn tasks ────────────────────────────────────────────────────────
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(spi_dma_bus, cs_out, dc_out, rst_out)).unwrap();
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
    spi: SpiDmaBus<'static, Async>,
    cs: Output<'static>,
    dc: Output<'static>,
    mut rst: Output<'static>,
) {
    // ── Hardware reset ──────────────────────────────────────────────────────
    rst.set_low();
    Timer::after(Duration::from_millis(1)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(5)).await;

    let mut dma_display = DmaDisplay { spi, dc, cs };

    // ── ILI9341 software init ───────────────────────────────────────────────
    dma_display.send_command(CMD_SOFTWARE_RESET, &[]);
    Timer::after(Duration::from_millis(120)).await;

    // Memory Access Control: Landscape (MV=1, BGR=1) = 0x28
    dma_display.send_command(CMD_MEMORY_ACCESS_CONTROL, &[0x28]);

    // Pixel format: 16-bit RGB565
    dma_display.send_command(CMD_PIXEL_FORMAT_SET, &[0x55]);

    // Sleep off
    dma_display.send_command(CMD_SLEEP_OFF, &[]);
    Timer::after(Duration::from_millis(5)).await;

    // Display on
    dma_display.send_command(CMD_DISPLAY_ON, &[]);

    // Invert colors (matches previous behavior)
    dma_display.send_command(CMD_INVERT_ON, &[]);

    log::info!("[display] ILI9341 initialized (DMA SPI, strip rendering)");

    // ── Display loop ────────────────────────────────────────────────────────
    let receiver = DISPLAY_CHANNEL.receiver();
    let touch_receiver = crate::touch::TOUCH_EVENTS.receiver();

    let mut ui_manager = ui::UiManager::new();
    let bg = Rgb565::new(31, 62, 29);
    static mut STRIP_BUF_ALLOC: core::mem::MaybeUninit<strip::StripBuffer> = core::mem::MaybeUninit::uninit();
    let strip_buf = unsafe {
        STRIP_BUF_ALLOC.write(strip::StripBuffer::new(DISPLAY_WIDTH as i32, STRIP_HEIGHT, bg));
        STRIP_BUF_ALLOC.assume_init_mut()
    };

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

        // Drain touch actions (swipe/tap detection and coord remapping done in touch task)
        while let Ok(action) = touch_receiver.try_receive() {
            ui_manager.handle_touch_action(action);
        }

        // Drain button events
        let btn_receiver = BUTTON_EVENT_CHANNEL.receiver();
        while let Ok(evt) = btn_receiver.try_receive() {
            match evt {
                ButtonEvent::ShortPress => {
                    ui_manager.last_interaction_time = Some(embassy_time::Instant::now());
                    match ui_manager.state {
                        ui::UiState::Idle => {
                            ui_manager.state = ui::UiState::Menu;
                        }
                        ui::UiState::Menu => {
                            ui_manager.selected_menu_item = (ui_manager.selected_menu_item + 1) % 2;
                        }
                        ui::UiState::SettingsMenu | ui::UiState::Metrics | ui::UiState::ToolsMenu | ui::UiState::InfoMenu | ui::UiState::Clock | ui::UiState::Pomodoro => {
                            // Button press goes back
                            ui_manager.state = ui::UiState::Menu;
                            ui_manager.force_redraw = true;
                        }
                    }
                }
            }
        }

        // Advance UI logic
        ui_manager.update();

        // Render via strip pipeline (only if something changed)
        if ui_manager.needs_draw() {
            let screen_h = DISPLAY_HEIGHT as i32;
            let mut y = 0i32;
            while y < screen_h {
                let h = STRIP_HEIGHT.min(screen_h - y);
                strip_buf.begin_strip(y, h);

                // Render UI into this strip (pixels outside the strip range are discarded)
                let _ = ui_manager.draw(strip_buf);

                // DMA-send the strip to the display
                dma_display.set_window(0, y as u16, (DISPLAY_WIDTH - 1) as u16, (y + h - 1) as u16);
                dma_display.write_pixels(strip_buf.as_bytes()).await;

                y += STRIP_HEIGHT;
            }
            ui_manager.save_state();
        }

        // ~30 fps — leaves CPU headroom for WiFi/Wasm on Core 0
        // this value is relevant for the idle animation speed and touch ripple effect, so it shouldn't be too high or low
        // IMPORTANT FOR UI
        Timer::after(Duration::from_millis(25)).await;
    }
}
