mod driver;
use driver::TftDisplay;
// Phase 2 — OLED Display Driver.
//
// Drives a 0.96" I2C OLED panel (`SSD1306`, 128×64) on the ESP32-S3.
// Wiring used by this project:
// - `VCC` -> `3V3`
// - `GND` -> `GND`
// - `SCL` -> `GPIO9`
// - `SDA` -> `GPIO8`
//
// ## Architecture
//
// ```text
// Wasm guest calls host_draw_eye(Happy)
//      │
//      ▼
// AbiHost::draw_eye()        ← validates arg, delegates to DisplayDriver
//      │
//      ▼
// DisplayDriver::draw_eye()  ← embedded-graphics renders to framebuffer
//      │
//      ▼
// Async I2C transfer         ← DMA/interrupt-driven; CPU yields while
//                               the frame is pushed to the OLED controller
// ```
//
// ## UI Modes
//
// - **Face mode** (default): full 128×64 animated face with idle expression
//   cycling and auto-blink.
// - **Menu mode**: face shrinks to the right 64 px; a categorised menu
//   appears on the left 64 px. Triggered by the BOOT button (GPIO 0).
// - **Full-screen modes**: Pomodoro, System Monitor, Clock, Vienna Lines.
//
// ## BOOT button
//
// GPIO 0 is monitored by a dedicated async Embassy task using hardware
// edge interrupts — no polling required:
// - Short press (< 2 s): face → menu, or navigate to next menu item.
// - Long press (≥ 2 s): face → menu, or select the highlighted item.
// - 10 s of inactivity in menu/action mode → auto-return to face mode.

use abi::{status, EyeExpression};
const COLOR_PASTEL_GREEN: Rgb565 = Rgb565::new(18, 55, 18);
const COLOR_PASTEL_PEACH: Rgb565 = Rgb565::new(31, 50, 20);
const COLOR_SOFT_GRAY: Rgb565 = Rgb565::new(24, 48, 24);

use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::RgbColor;
use esp_hal::spi::master::Spi;
use esp_hal::gpio::Output;
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},

    prelude::{Drawable, Point, Primitive},
    primitives::{
        Circle, Line, PrimitiveStyle, Rectangle, RoundedRectangle, CornerRadii, Ellipse,
    },
    text::Text,
    Pixel,
};
use esp_hal::{
    gpio::Input,
    Async,
};


pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;
const DISPLAY_QUEUE_DEPTH: usize = 8;
const EXPRESSION_OVERRIDE_TIMEOUT: Duration = Duration::from_secs(8);
// OLED stability baseline (validated on device): do not change casually.
const FRAME_PERIOD: Duration = Duration::from_millis(50);
const DIAG_SKIP_FLUSH: bool = false;

// ── Tiny xorshift32 PRNG (no-std, no-alloc) ──────────────────────────────────

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Random u32 in `[lo, hi)` (inclusive low, exclusive high).
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo))
    }
}

// ── Expression schedule ───────────────────────────────────────────────────────

/// Expressions to cycle through in idle mode.
///
/// The cycle is designed to feel natural:
///   Neutral (resting) → Happy (content) → Sleepy (dozing off) →
///   Neutral (wakes) → Thinking (pondering) → Heart (affectionate) →
///   Neutral → Surprised (alert) → Happy
///
/// Contextual overrides from system events take priority (see DisplayCommand):
///   - WiFi connected       → Happy
///   - WiFi connecting      → Thinking
///   - WiFi error/timeout   → Sad
///   - Repeated failures    → Angry
///   - New ESP-NOW message  → Surprised
///   - Button press         → Heart (greeting)
///   - Long idle (>60s)     → Sleepy
const IDLE_EXPRESSIONS: [EyeExpression; 9] = [
    EyeExpression::Neutral,
    EyeExpression::Happy,
    EyeExpression::Sleepy,
    EyeExpression::Neutral,
    EyeExpression::Thinking,
    EyeExpression::Heart,
    EyeExpression::Neutral,
    EyeExpression::Surprised,
    EyeExpression::Happy,
];

static DISPLAY_CHANNEL: Channel<CriticalSectionRawMutex, DisplayCommand, DISPLAY_QUEUE_DEPTH> =
    Channel::new();

/// Channel through which the async `button_task` sends press events to the
/// display render loop.  Capacity 4 is more than enough for any realistic
/// button mashing rate.
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
    spi: Spi<'static, esp_hal::Async>,
    dc: Output<'static>,
    cs: Output<'static>,
    boot_btn: esp_hal::gpio::Input<'static>,
) {
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(spi, dc, cs)).unwrap();
}

/// Dedicated Embassy task for the BOOT button (GPIO 0).
///
/// Uses hardware edge interrupts (`wait_for_falling_edge`) instead of
/// polling so the CPU is never spinning for button state.  Press
/// classification (short vs. long) is handled here and the result is
/// forwarded to the display render loop via `BUTTON_EVENT_CHANNEL`.
#[embassy_executor::task]
async fn button_task(mut boot_btn: Input<'static>) {
    loop {
        // Wait for the active-LOW BOOT button to be pressed (falling edge).
        boot_btn.wait_for_falling_edge().await;
        let press_start = Instant::now();

        // Wait for release (rising edge) within 2 s.
        // If the timeout fires first it is a long press.
        match with_timeout(
            Duration::from_millis(2000),
            boot_btn.wait_for_rising_edge(),
        )
        .await
        {
            Ok(_) => {
                let held_ms = (Instant::now() - press_start).as_millis();
                log::info!("[button] short press (held {}ms)", held_ms);
                if BUTTON_EVENT_CHANNEL
                    .sender()
                    .try_send(ButtonEvent::ShortPress)
                    .is_err()
                {
                    log::warn!("[button] channel full — short press dropped");
                }
            }
            Err(_timeout) => {
                log::info!("[button] long press");
                if BUTTON_EVENT_CHANNEL
                    .sender()
                    .try_send(ButtonEvent::LongPress)
                    .is_err()
                {
                    log::warn!("[button] channel full — long press dropped");
                }
                // Wait for the physical release before we can detect the next press.
                boot_btn.wait_for_rising_edge().await;
            }
        }

        // Brief debounce delay before arming the next edge detection.
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[embassy_executor::task]
async fn display_task(
    spi: Spi<'static, esp_hal::Async>,
    dc: Output<'static>,
    cs: Output<'static>,
) {
    let mut oled = TftDisplay::new(spi, dc, cs);
    oled.init().await;

    let mut state = FaceState::default();

    // Startup sequence
    for i in 0..40 {
        state.expression = if i < 20 { EyeExpression::Sleepy } else { EyeExpression::Happy };
        state.force_clear = true;
        render_full_face(&mut oled, &mut state);
        let _ = oled.flush().await;
        Timer::after(Duration::from_millis(50)).await;
    }

    // Initialise Instant-based fields now that the Embassy timer driver is running.
    state.last_button_time = Instant::now();
    // Seed PRNG from uptime so the animation schedule varies between boots.
    state.rng = Rng(Instant::now().as_millis() as u32 | 1);
    let seed_ms = Instant::now().as_millis() as u64;
    state.next_blink_ms = seed_ms + state.rng.range(1500, 3500) as u64;
    state.next_expression_ms = seed_ms + state.rng.range(3000, 5000) as u64;
    state.next_look_ms = seed_ms + state.rng.range(800, 2000) as u64;
    let mut had_flush_error = false;
    let mut flush_fail_since: Option<Instant> = None;

    let receiver = DISPLAY_CHANNEL.receiver();
    let btn_receiver = BUTTON_EVENT_CHANNEL.receiver();

    // Starvation state: true while the animation is known to be frozen.
    let mut starved = false;

    loop {
        let frame_start = Instant::now();

        // 1. Drain the DisplayCommand queue from the Wasm ABI.
        while let Ok(cmd) = receiver.try_receive() {
            match cmd {
                DisplayCommand::SetExpression(expr) => {
                    state.expression = expr;
                    state.expression_override = true;
                    state.expression_override_since = Some(Instant::now());
                }
                DisplayCommand::SetText(text) => state.text = text,
                DisplayCommand::SetBrightness(value) => {
                    state.brightness = value;

                }
            }
        }

        // 2. Drain button events sent by the async button_task.
        while let Ok(btn_event) = btn_receiver.try_receive() {
            handle_button_event(btn_event, &mut state);
        }


        // 2c. Drain touch events.
        let touch_coords = crate::sensors::load_touch_coords();
        if let Some((x, y)) = touch_coords {
            let x_i32 = x as i32;
            let y_i32 = y as i32;

            // Continuously track eye direction based on touch while held
            state.eye_look_x = ((x_i32 - 120) / 30).clamp(-4, 4);
            state.eye_look_y = ((y_i32 - 160) / 30).clamp(-4, 4);

            // Update button time continuously while held for tactile visual feedback
            state.last_button_time = Instant::now();

            if !state.was_touched {
                state.was_touched = true;
                log::info!("[display] touch start at {},{}", x, y);
                // Map to virtual 128x64 display coordinates assuming the display scales up or is drawn top-left.
                // The prompt mentions physical touch, we need to know the physical mapping.
                // Assuming X is 0-240 and Y is 0-320 mapping to UI.
                // Let's assume UI is drawn at top left 128x64 but the screen is 240x320. Wait, no, the driver code shows:
                // DISPLAY_WIDTH = 240, DISPLAY_HEIGHT = 320. The draw loops use X: 0..240, Y: 0..320, but the rendering bounds in display/mod.rs are hardcoded like 128 and 64!
                // Let's map X to 0-128 and Y to 0-64 simply by scaling.
                // x is roughly 0..240 (or 240..0 if flipped? We'll see). Let's just assume simple scaling:
                let vx = (x as u32 * 128 / 240) as i32;
                let vy = (y as u32 * 64 / 320) as i32;

                let mut handled_by_swipe = false;
                if state.touch_start_x.is_none() {
                    state.touch_start_x = Some(x_i32);
                } else if let Some(start_x) = state.touch_start_x {
                    let delta = x_i32 - start_x;
                    if delta > 50 && matches!(state.ui_mode, UiMode::Face) {
                        state.ui_mode = UiMode::TopMenu;
                        state.top_menu_selected = 0;
                        state.touch_start_x = None;
                        handled_by_swipe = true;
                        crate::audio::play_blip();
                    } else if delta < -50 && !matches!(state.ui_mode, UiMode::Face) {
                        return_to_face(&mut state);
                        state.touch_start_x = None;
                        handled_by_swipe = true;
                        crate::audio::play_blip();
                    }
                }

                if handled_by_swipe {
                    continue; // Skip tap logic if a swipe triggered a state transition
                }

                match state.ui_mode {
                    UiMode::Face => {
                        // "Face should be full screen, and then if we touch on the face, i get small and to the right, and we show the menu on the left"
                        state.ui_mode = UiMode::TopMenu;
                        state.top_menu_selected = 0;
                        log::info!("[display] touch face -> TopMenu");
                    }
                    UiMode::TopMenu | UiMode::SubMenu(_) | UiMode::WebMenu => {
                        // Menu is on the left (x < 63). Check if touch is in the menu area.
                        if vx < 64 {
                            let slot = (vy / 16) as usize;
                            if let UiMode::TopMenu = state.ui_mode {
                                let items = get_menu_items(&state.ui_mode);
                                // Determine current scroll offset
                                let sel = state.top_menu_selected as usize;
                                let max_visible = 4;
                                let scroll_start = if sel >= max_visible { sel - (max_visible - 1) } else { 0 };
                                let target_idx = scroll_start + slot;
                                if target_idx < items.len() {
                                    state.top_menu_selected = target_idx as u8;
                                    execute_menu_action(items[target_idx].action, &mut state);
                                }
                            } else if let UiMode::SubMenu(cat) = state.ui_mode {
                                let items = get_menu_items(&UiMode::SubMenu(cat));
                                let sel = state.sub_menu_selected as usize;
                                let max_visible = 4;
                                let scroll_start = if sel >= max_visible { sel - (max_visible - 1) } else { 0 };
                                let target_idx = scroll_start + slot;
                                if target_idx < items.len() {
                                    state.sub_menu_selected = target_idx as u8;
                                    execute_menu_action(items[target_idx].action, &mut state);
                                }
                            } else if let UiMode::WebMenu = state.ui_mode {
                                if slot < 2 {
                                    state.web_menu_selected = slot as u8;
                                    if state.web_menu_selected == 0 {
                                        crate::web_server::enable_web_server();
                                        crate::wifi::mark_connecting();
                                        state.expression = EyeExpression::Happy;
                                    } else {
                                        crate::web_server::disable_web_server();
                                        state.expression = EyeExpression::Neutral;
                                    }
                                }
                            }
                        } else {
                            // Touch right side -> maybe go back to face?
                            return_to_face(&mut state);
                        }
                    }
                    UiMode::ViennaLines => {
                        state.ui_mode = UiMode::ViennaDetail;
                    }
                    UiMode::ViennaDetail => {
                        state.ui_mode = UiMode::ViennaLines;
                        state.vienna_scroll_x = 0;
                    }
                    _ => {
                        // Any other mode exits to face or menu. We'll exit to menu for tools.
                        if state.flashlight_on {
                            state.flashlight_on = false;
                            crate::led_state::set_led_color(0, 0, 0);
                            state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                        } else if state.party_mode_on {
                            state.party_mode_on = false;
                            crate::led_state::set_led_color(0, 0, 0);
                            state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                        } else {
                            return_to_face(&mut state);
                        }
                    }
                }
            }
        } else {
            state.was_touched = false;
            if state.touch_start_x.is_some() {
                state.touch_start_x = None;
            }
        }

        // 2b. Drive LED effects (flashlight / party mode).
        tick_led_effects(&mut state);

        // 3. Auto-return to face mode after inactivity.
        // Pause idle timeout while WiFi connecting or Pomodoro running.
        let pause_idle_timeout = (matches!(state.ui_mode, UiMode::WebMenu)
            && crate::web_server::is_web_server_enabled()
            && crate::wifi::wifi_status() == crate::wifi::WIFI_STATUS_CONNECTING)
            || (matches!(state.ui_mode, UiMode::Pomodoro)
                && state.pomodoro_start.is_some()
                && !state.pomodoro_done);
        let idle_secs: u64 = match state.ui_mode {
            UiMode::ViennaLines | UiMode::ViennaDetail
            | UiMode::SystemMonitor | UiMode::ClockView => 30,
            UiMode::Flashlight | UiMode::PartyMode => 60,
            _ => 10,
        };
        if !matches!(state.ui_mode, UiMode::Face) && !pause_idle_timeout {
            let idle = Instant::now() - state.last_button_time;
            if idle >= Duration::from_secs(idle_secs) {
                return_to_face(&mut state);
            }
        }

        // 4. Render and push the frame.
        render_frame(&mut oled, &mut state);

        // 4b. Flush framebuffer to ILI9341 via async SPI
        if !DIAG_SKIP_FLUSH {
            let flush_start = Instant::now();
            let mut flush_ok = matches!(
                with_timeout(Duration::from_millis(400), oled.flush()).await,
                Ok(Ok(()))
            );
            if !flush_ok {
                flush_ok = matches!(
                    with_timeout(Duration::from_millis(400), oled.flush()).await,
                    Ok(Ok(()))
                );
            }
            let last_flush_us = (Instant::now() - flush_start).as_micros() as u64;
            if flush_ok {
                flush_fail_since = None;
                had_flush_error = false;
            } else {
                if !had_flush_error {
                    log::error!("[display] flush failed ({}us)", last_flush_us);
                    had_flush_error = true;
                }
                // Track how long we've been failing continuously.
                let fail_start = *flush_fail_since.get_or_insert(Instant::now());
                if Instant::now() - fail_start >= Duration::from_millis(500) {
                    log::warn!("[display] flush failing >500ms — re-initialising OLED");
                    oled.init().await;
                    // Reset animation timers so the face restarts cleanly.
                    let now_ms = Instant::now().as_millis() as u64;
                    state.next_blink_ms = now_ms + state.rng.range(1500, 3500) as u64;
                    state.next_expression_ms = now_ms + state.rng.range(3000, 5000) as u64;
                    state.next_look_ms = now_ms + state.rng.range(800, 2000) as u64;
                    state.frame = 0;
                    state.transition_end_ms = 0;
                    state.blink_end_ms = 0;
                    flush_fail_since = None;
                    had_flush_error = false;
                }
            }
        }

        // 6. Sleep the remainder of the frame period.
        //    Yield at least once per frame so the executor can run other tasks
        //    (Wi-Fi, web server, button task).
        //    If we're already past the deadline (flush took too long), yield
        //    only 100µs so we don't hand off for seconds.
        {
            const POLL_INTERVAL: Duration = Duration::from_millis(10);
            let frame_deadline = frame_start + FRAME_PERIOD;
            if Instant::now() >= frame_deadline {
                // Already over budget — yield once briefly, then continue.
                let yield_start = Instant::now();
                Timer::after(Duration::from_micros(100)).await;
                let yield_us = (Instant::now() - yield_start).as_micros();
                if yield_us > 200_000 {
                    if !starved {
                        starved = true;
                        log::warn!("[display] FROZEN — executor busy (yield {}ms)",
                            yield_us / 1000);
                    }
                } else if starved {
                    starved = false;
                    log::info!("[display] RESUMED — animation running normally");
                }
            } else {
                loop {
                    let yield_start = Instant::now();
                    Timer::after(POLL_INTERVAL).await;
                    let yield_us = (Instant::now() - yield_start).as_micros();
                    // Starvation watchdog: if a 10ms sleep takes >200ms,
                    // another task is hogging the executor.
                    if yield_us > 200_000 {
                        if !starved {
                            starved = true;
                            log::warn!("[display] FROZEN — executor busy (yield {}ms)",
                                yield_us / 1000);
                        }
                    } else if starved {
                        starved = false;
                        log::info!("[display] RESUMED — animation running normally");
                    }
                    if Instant::now() >= frame_deadline {
                        break;
                    }
                }
            }
        }
    }
}

// ── UI mode ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum UiMode {
    /// Full 128×64 animated face.
    Face,
    /// Top-level category menu (split: left menu, right mini-face).
    TopMenu,
    /// Category submenu (split: left menu, right mini-face).
    SubMenu(MenuCategory),
    /// Web control submenu (ON/OFF selector + status line).
    WebMenu,
    /// Flashlight mode — RGB LED full white, status on OLED.
    Flashlight,
    /// Pomodoro timer — full-screen 25-minute countdown.
    Pomodoro,
    /// Party mode — RGB LED color cycling, status on OLED.
    PartyMode,
    /// System monitor — WiFi, IP, uptime, memory.
    SystemMonitor,
    /// Digital clock — NTP time or uptime fallback.
    ClockView,
    /// Full-screen Vienna public transport departures (station→direction list).
    ViennaLines,
    /// Detail view for a selected Vienna stop (all departure times).
    ViennaDetail,
}

#[derive(Clone, Copy, PartialEq)]
enum MenuCategory {
    Tools,
    Info,
    Transport,
    Settings,
}

// ── Menu data structures ────────────────────────────────────────────────────

struct MenuItem {
    label: &'static str,
    action: MenuItemAction,
}

#[derive(Clone, Copy, PartialEq)]
enum MenuItemAction {
    OpenSub(MenuCategory),
    OpenWebMenu,
    ToggleFlashlight,
    TogglePartyMode,
    EnterPomodoro,
    EnterSystemMonitor,
    EnterClockView,
    EnterViennaLines,
    CycleBrightness,
    Back,
}

// ── Button helpers ────────────────────────────────────────────────────────────

// ── UI state machine ──────────────────────────────────────────────────────────

const TOP_MENU: &[MenuItem] = &[
    MenuItem { label: "Tools",     action: MenuItemAction::OpenSub(MenuCategory::Tools) },
    MenuItem { label: "Info",      action: MenuItemAction::OpenSub(MenuCategory::Info) },
    MenuItem { label: "Transport", action: MenuItemAction::OpenSub(MenuCategory::Transport) },
    MenuItem { label: "Web",       action: MenuItemAction::OpenWebMenu },
    MenuItem { label: "Settings",  action: MenuItemAction::OpenSub(MenuCategory::Settings) },
];

const TOOLS_MENU: &[MenuItem] = &[
    MenuItem { label: "Flashlight", action: MenuItemAction::ToggleFlashlight },
    MenuItem { label: "Pomodoro",   action: MenuItemAction::EnterPomodoro },
    MenuItem { label: "Party Mode", action: MenuItemAction::TogglePartyMode },
    MenuItem { label: "< Back",     action: MenuItemAction::Back },
];

const INFO_MENU: &[MenuItem] = &[
    MenuItem { label: "System",  action: MenuItemAction::EnterSystemMonitor },
    MenuItem { label: "Clock",   action: MenuItemAction::EnterClockView },
    MenuItem { label: "< Back",  action: MenuItemAction::Back },
];

const TRANSPORT_MENU: &[MenuItem] = &[
    MenuItem { label: "Wiener L", action: MenuItemAction::EnterViennaLines },
    MenuItem { label: "< Back",   action: MenuItemAction::Back },
];

const SETTINGS_MENU: &[MenuItem] = &[
    MenuItem { label: "Brightness", action: MenuItemAction::CycleBrightness },
    MenuItem { label: "< Back",     action: MenuItemAction::Back },
];

const WEB_MENU_ITEMS: [&str; 2] = ["ON", "OFF"];

/// Return the menu item slice for a given UI mode.
fn get_menu_items(mode: &UiMode) -> &'static [MenuItem] {
    match mode {
        UiMode::TopMenu => TOP_MENU,
        UiMode::SubMenu(MenuCategory::Tools) => TOOLS_MENU,
        UiMode::SubMenu(MenuCategory::Info) => INFO_MENU,
        UiMode::SubMenu(MenuCategory::Transport) => TRANSPORT_MENU,
        UiMode::SubMenu(MenuCategory::Settings) => SETTINGS_MENU,
        _ => TOP_MENU,
    }
}

/// Return the selected index for the current menu mode.
fn get_menu_selected(state: &FaceState) -> u8 {
    match state.ui_mode {
        UiMode::TopMenu => state.top_menu_selected,
        UiMode::SubMenu(_) => state.sub_menu_selected,
        _ => 0,
    }
}

/// Helper to cleanly return to Face mode, resetting all transient state.
fn return_to_face(state: &mut FaceState) {
    state.ui_mode = UiMode::Face;
    state.force_clear = true;
    state.text.clear();
    state.expression = EyeExpression::Neutral;
    state.expression_override = false;
    state.expression_override_since = None;
    // Turn off LED effects when returning to face
    if state.flashlight_on {
        state.flashlight_on = false;
        crate::led_state::set_led_color(0, 0, 0);
    }
    if state.party_mode_on {
        state.party_mode_on = false;
        crate::led_state::set_led_color(0, 0, 0);
    }
    log::info!("[display] -> Face");
}

/// Execute a menu item action from a long press in TopMenu or SubMenu.
fn execute_menu_action(action: MenuItemAction, state: &mut FaceState) {
    crate::audio::play_blip();
    state.force_clear = true;
    match action {
        MenuItemAction::OpenSub(cat) => {
            state.ui_mode = UiMode::SubMenu(cat);
            state.sub_menu_selected = 0;
            log::info!("[display] -> SubMenu");
        }
        MenuItemAction::OpenWebMenu => {
            state.ui_mode = UiMode::WebMenu;
            state.web_menu_selected = if crate::web_server::is_web_server_enabled() { 1 } else { 0 };
            log::info!("[display] -> WebMenu");
        }
        MenuItemAction::ToggleFlashlight => {
            state.flashlight_on = !state.flashlight_on;
            if state.flashlight_on {
                // Flashlight and party are mutually exclusive
                state.party_mode_on = false;
                state.ui_mode = UiMode::Flashlight;
                crate::led_state::set_led_color(255, 255, 255);
            } else {
                crate::led_state::set_led_color(0, 0, 0);
                state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
            }
            log::info!("[display] flashlight = {}", state.flashlight_on);
        }
        MenuItemAction::TogglePartyMode => {
            state.party_mode_on = !state.party_mode_on;
            if state.party_mode_on {
                // Party and flashlight are mutually exclusive
                state.flashlight_on = false;
                state.party_hue = 0;
                state.ui_mode = UiMode::PartyMode;
            } else {
                crate::led_state::set_led_color(0, 0, 0);
                state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
            }
            log::info!("[display] party = {}", state.party_mode_on);
        }
        MenuItemAction::EnterPomodoro => {
            state.ui_mode = UiMode::Pomodoro;
            state.pomodoro_start = Some(Instant::now());
            state.pomodoro_paused_remaining = None;
            state.pomodoro_done = false;
            log::info!("[display] -> Pomodoro");
        }
        MenuItemAction::EnterSystemMonitor => {
            state.ui_mode = UiMode::SystemMonitor;
            log::info!("[display] -> SystemMonitor");
        }
        MenuItemAction::EnterClockView => {
            state.ui_mode = UiMode::ClockView;
            log::info!("[display] -> ClockView");
        }
        MenuItemAction::EnterViennaLines => {
            state.vienna_selected = 0;
            state.vienna_scroll_x = -20;
            state.ui_mode = UiMode::ViennaLines;
            log::info!("[display] -> ViennaLines");
        }
        MenuItemAction::CycleBrightness => {
            state.brightness_level = (state.brightness_level + 1) % 3;
            let val = match state.brightness_level {
                0 => 64u8,
                1 => 160u8,
                _ => 255u8,
            };
            state.brightness = val;
            let _ = DISPLAY_CHANNEL
                .sender()
                .try_send(DisplayCommand::SetBrightness(val));
            log::info!("[display] brightness level = {}", state.brightness_level);
        }
        MenuItemAction::Back => {
            state.ui_mode = UiMode::TopMenu;
            log::info!("[display] -> TopMenu (back)");
        }
    }
}

fn handle_button_event(ev: ButtonEvent, state: &mut FaceState) {
    match ev {
        ButtonEvent::ShortPress => {
            let now = Instant::now();

            // ── Double press detection (2 short presses within 400ms) ──
            let since_last = now - state.last_short_press_time;
            state.last_short_press_time = now;
            if since_last <= Duration::from_millis(400)
                && !matches!(state.ui_mode, UiMode::Face)
            {
                log::info!("[display] double press -> Face");
                return_to_face(state);
                state.last_button_time = now;
                return;
            }

            state.last_button_time = now;
            log::info!("[display] short press");

            match state.ui_mode {
                UiMode::Face => {
                    state.ui_mode = UiMode::TopMenu;
                    state.top_menu_selected = 0;
                }
                UiMode::TopMenu => {
                    state.top_menu_selected =
                        (state.top_menu_selected + 1) % TOP_MENU.len() as u8;
                }
                UiMode::SubMenu(cat) => {
                    let items = get_menu_items(&UiMode::SubMenu(cat));
                    state.sub_menu_selected =
                        (state.sub_menu_selected + 1) % items.len() as u8;
                }
                UiMode::WebMenu => {
                    state.web_menu_selected = (state.web_menu_selected + 1) % 2;
                }
                UiMode::Flashlight => {
                    // Any press exits flashlight
                    state.flashlight_on = false;
                    crate::led_state::set_led_color(0, 0, 0);
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                }
                UiMode::PartyMode => {
                    // Any press exits party mode
                    state.party_mode_on = false;
                    crate::led_state::set_led_color(0, 0, 0);
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                }
                UiMode::Pomodoro => {
                    // Short press: pause/resume
                    if state.pomodoro_done {
                        state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                    } else if let Some(paused_ms) = state.pomodoro_paused_remaining {
                        // Resume: set start so that elapsed = total - paused_ms
                        let total_ms = 25u64 * 60 * 1000;
                        let elapsed = total_ms.saturating_sub(paused_ms);
                        state.pomodoro_start =
                            Some(Instant::now() - Duration::from_millis(elapsed));
                        state.pomodoro_paused_remaining = None;
                    } else if let Some(start) = state.pomodoro_start {
                        // Pause: store remaining
                        let total_ms = 25u64 * 60 * 1000;
                        let elapsed = (Instant::now() - start).as_millis();
                        state.pomodoro_paused_remaining =
                            Some(total_ms.saturating_sub(elapsed));
                    }
                }
                UiMode::SystemMonitor => {
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Info);
                }
                UiMode::ClockView => {
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Info);
                }
                UiMode::ViennaLines => {
                    let data = crate::vienna_fetch::get_lines();
                    if !data.stops.is_empty() {
                        state.vienna_selected =
                            (state.vienna_selected + 1) % data.stops.len();
                        state.vienna_scroll_x = -20;
                    }
                }
                UiMode::ViennaDetail => {
                    state.ui_mode = UiMode::ViennaLines;
                    state.vienna_scroll_x = 0;
                }
            }
        }
        ButtonEvent::LongPress => {
            state.last_button_time = Instant::now();
            log::info!("[display] long press");

            match state.ui_mode {
                UiMode::Face => {
                    state.ui_mode = UiMode::TopMenu;
                    state.top_menu_selected = 0;
                }
                UiMode::TopMenu => {
                    let action = TOP_MENU[state.top_menu_selected as usize].action;
                    execute_menu_action(action, state);
                }
                UiMode::SubMenu(cat) => {
                    let items = get_menu_items(&UiMode::SubMenu(cat));
                    let action = items[state.sub_menu_selected as usize].action;
                    execute_menu_action(action, state);
                }
                UiMode::WebMenu => {
                    if state.web_menu_selected == 0 {
                        crate::web_server::enable_web_server();
                        crate::wifi::mark_connecting();
                        state.expression = EyeExpression::Happy;
                    } else {
                        crate::web_server::disable_web_server();
                        state.expression = EyeExpression::Neutral;
                    }
                    log::info!(
                        "[display] web set -> {}",
                        WEB_MENU_ITEMS[state.web_menu_selected as usize]
                    );
                }
                UiMode::Flashlight => {
                    state.flashlight_on = false;
                    crate::led_state::set_led_color(0, 0, 0);
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                }
                UiMode::PartyMode => {
                    state.party_mode_on = false;
                    crate::led_state::set_led_color(0, 0, 0);
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                }
                UiMode::Pomodoro => {
                    // Long press: cancel timer and return
                    state.pomodoro_start = None;
                    state.pomodoro_paused_remaining = None;
                    state.pomodoro_done = false;
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Tools);
                }
                UiMode::SystemMonitor => {
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Info);
                }
                UiMode::ClockView => {
                    state.ui_mode = UiMode::SubMenu(MenuCategory::Info);
                }
                UiMode::ViennaLines => {
                    state.ui_mode = UiMode::ViennaDetail;
                }
                UiMode::ViennaDetail => {
                    state.ui_mode = UiMode::ViennaLines;
                    state.vienna_scroll_x = 0;
                }
            }
        }
    }
}

// ── Face state ────────────────────────────────────────────────────────────────

struct FaceState {
    expression: EyeExpression,
    prev_expression: EyeExpression,
    prev_is_blinking: bool,
    prev_bob_y: i32,
    prev_eye_look_x: i32,
    prev_eye_look_y: i32,
    eye_look_y: i32,
    force_clear: bool,
    prev_text: heapless::String<64>,
    text: heapless::String<64>,
    brightness: u8,
    frame: u32,
    /// Set to `true` when expression was set via `DisplayCommand::SetExpression`.
    expression_override: bool,
    /// Timestamp of last ABI expression override.
    expression_override_since: Option<Instant>,
    ui_mode: UiMode,
    prev_ui_mode: UiMode,
    // ── Menu navigation ──
    top_menu_selected: u8,
    sub_menu_selected: u8,
    web_menu_selected: u8,
    vienna_selected: usize,
    prev_top_menu_selected: u8,
    prev_sub_menu_selected: u8,
    prev_web_menu_selected: u8,
    prev_vienna_selected: usize,
    /// Horizontal scroll pixel offset for the selected Vienna list item (marquee).
    vienna_scroll_x: i32,
    /// Updated on every button event; used for the inactivity timeout.
    last_button_time: Instant,
    // ── Feature state ──
    flashlight_on: bool,
    party_mode_on: bool,
    party_hue: u16,
    pomodoro_start: Option<Instant>,
    pomodoro_paused_remaining: Option<u64>,
    pomodoro_done: bool,
    brightness_level: u8,
    // ── Double press detection ──
    last_short_press_time: Instant,
    was_touched: bool,
    touch_start_x: Option<i32>,
    // ── Rich animation state ──
    rng: Rng,
    /// Millisecond timestamp when the next blink should start.
    next_blink_ms: u64,
    /// Millisecond timestamp when the current blink ends (0 = not blinking).
    blink_end_ms: u64,
    /// Millisecond timestamp when the next idle expression change should happen.
    next_expression_ms: u64,
    /// Index into IDLE_EXPRESSIONS for the *current* expression.
    idle_expr_idx: u8,
    /// Eye-pupil horizontal offset for a gentle "look-around" effect (-2..+2).
    eye_look_x: i32,
    /// Timestamp when eye_look_x should next change.
    next_look_ms: u64,
    /// Transition: when switching expressions, eyes close for 120 ms then reopen.
    transition_end_ms: u64,
}

impl Default for FaceState {
    fn default() -> Self {
        Self {
            expression: EyeExpression::Neutral,
            prev_expression: EyeExpression::Neutral,
            prev_is_blinking: false,
            prev_bob_y: 0,
            prev_eye_look_x: 0,
            prev_eye_look_y: 0,
            eye_look_y: 0,
            force_clear: true,
            prev_text: heapless::String::new(),
            text: heapless::String::new(),
            brightness: 255,
            frame: 0,
            expression_override: false,
            expression_override_since: None,
            ui_mode: UiMode::Face,
            prev_ui_mode: UiMode::Face,
            top_menu_selected: 0,
            sub_menu_selected: 0,
            web_menu_selected: 0,
            vienna_selected: 0,
            prev_top_menu_selected: 0,
            prev_sub_menu_selected: 0,
            prev_web_menu_selected: 0,
            prev_vienna_selected: 0,
            vienna_scroll_x: 0,
            last_button_time: Instant::from_ticks(0),
            flashlight_on: false,
            party_mode_on: false,
            party_hue: 0,
            pomodoro_start: None,
            pomodoro_paused_remaining: None,
            pomodoro_done: false,
            brightness_level: 2, // High (255)
            last_short_press_time: Instant::from_ticks(0),
            was_touched: false,
            touch_start_x: None,
            rng: Rng(0xDEAD_BEEF),
            next_blink_ms: 2500,
            blink_end_ms: 0,
            next_expression_ms: 4000,
            idle_expr_idx: 0,
            eye_look_x: 0,
            next_look_ms: 1500,
            transition_end_ms: 0,
        }
    }
}

// ── LED effects ──────────────────────────────────────────────────────────────

/// Integer-only HSV to RGB conversion.
/// h: 0-359 (hue degrees), s: 0-255 (saturation), v: 0-255 (value).
fn hsv_to_rgb(h: u16, s: u8, v: u8) -> (u8, u8, u8) {
    if s == 0 {
        return (v, v, v);
    }
    let region = h / 60;
    let remainder = ((h % 60) as u32 * 255) / 60;
    let p = ((v as u32) * (255 - s as u32)) / 255;
    let q = ((v as u32) * (255 - (s as u32 * remainder) / 255)) / 255;
    let t = ((v as u32) * (255 - (s as u32 * (255 - remainder)) / 255)) / 255;
    let (r, g, b) = match region {
        0 => (v as u32, t, p),
        1 => (q, v as u32, p),
        2 => (p, v as u32, t),
        3 => (p, q, v as u32),
        4 => (t, p, v as u32),
        _ => (v as u32, p, q),
    };
    (r as u8, g as u8, b as u8)
}

/// Drive LED effects each frame (called from display loop).
fn tick_led_effects(state: &mut FaceState) {
    if state.flashlight_on {
        crate::led_state::set_led_color(255, 255, 255);
        return;
    }
    if state.party_mode_on {
        state.party_hue = (state.party_hue + 7) % 360;
        let (r, g, b) = hsv_to_rgb(state.party_hue, 255, 255);
        crate::led_state::set_led_color(r, g, b);
    }
}

// ── Render dispatcher ─────────────────────────────────────────────────────────

fn render_frame(oled: &mut TftDisplay, state: &mut FaceState) {
    match state.ui_mode {
        UiMode::Face => render_full_face(oled, state),
        UiMode::ViennaLines => render_vienna_lines(oled, state),
        UiMode::ViennaDetail => render_vienna_detail(oled, state),
        UiMode::Pomodoro => render_pomodoro(oled, state),
        UiMode::SystemMonitor => render_system_monitor(oled, state),
        UiMode::ClockView => render_clock_view(oled, state),
        UiMode::Flashlight => render_flashlight(oled, state),
        UiMode::PartyMode => render_party_mode(oled, state),
        _ => render_menu_mode(oled, state), // TopMenu, SubMenu, WebMenu
    }
    state.prev_ui_mode = state.ui_mode;
}

// ── Full-screen face rendering ────────────────────────────────────────────────

/// Update all time-based animation state (blink, idle expression, eye drift).
/// This is called once per frame before rendering so the same logic drives
/// both the full-face and menu-mode mini-face.
fn tick_animation(state: &mut FaceState, now_ms: u64) {
    // --- Override timeout ---
    if state.expression_override {
        if let Some(since) = state.expression_override_since {
            if Instant::now() - since >= EXPRESSION_OVERRIDE_TIMEOUT {
                state.expression_override = false;
                state.expression_override_since = None;
            }
        } else {
            state.expression_override = false;
        }
    }

    // --- Blink scheduling (random interval 2–5 s, 300 ms duration) ---
    // 300 ms is long enough to be visible even when the frame-rate is low.
    if now_ms >= state.next_blink_ms && state.blink_end_ms <= now_ms {
        state.blink_end_ms = now_ms + 300;
        let next_gap = state.rng.range(2000, 5000) as u64;
        state.next_blink_ms = now_ms + next_gap;
    }

    // --- Idle expression cycling (random 3–6 s, brief transition) ---
    if !state.expression_override && now_ms >= state.next_expression_ms {
        state.transition_end_ms = now_ms + 120; // eyes close 120 ms
        state.idle_expr_idx = (state.idle_expr_idx + 1) % IDLE_EXPRESSIONS.len() as u8;
        state.expression = IDLE_EXPRESSIONS[state.idle_expr_idx as usize];
        let next_gap = state.rng.range(3000, 6500) as u64;
        state.next_expression_ms = now_ms + next_gap;
    }

    // --- Eye drift (gentle horizontal pupil shift every 1–3 s) ---
    if now_ms >= state.next_look_ms {
        state.eye_look_x = (state.rng.next() % 5) as i32 - 2; // -2..+2
        state.next_look_ms = now_ms + state.rng.range(1000, 3000) as u64;
    }
}

/// Smooth breathing bob (triangle wave, ±1 px — subtle).
fn breathing_offset(now_ms: u64) -> i32 {
    let phase = (now_ms % 3000) as i32;
    if phase < 1500 {
        ((phase - 750) * 2 / 750).clamp(-2, 2)
    } else {
        -((phase - 2250) * 2 / 750).clamp(-2, 2)
    }
}

/// Render the full 128×64 kawaii face (frameless — eyes and mouth only).
fn erase_face(oled: &mut TftDisplay, prev_eye_look_x: i32, prev_eye_look_y: i32, prev_bob_y: i32, scale: i32) {
    if scale > 0 {
        let cy = 22 + prev_bob_y + prev_eye_look_y;
        let cx = 38 + prev_eye_look_x; // left eye base cx
        // bounding box of eyes and mouth
        let _ = Rectangle::new(Point::new(cx - 20, cy - 20), Size::new(100, 60))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    } else {
        let cy = 22 + prev_bob_y + prev_eye_look_y;
        let cx = 80 + prev_eye_look_x / 2;
        let _ = Rectangle::new(Point::new(cx - 15, cy - 15), Size::new(50, 40))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    }
}

fn render_full_face(oled: &mut TftDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    let bob_y = breathing_offset(now_ms);
    let in_transition = now_ms < state.transition_end_ms;
    let blinking = now_ms < state.blink_end_ms || in_transition;

    let face_changed = state.expression != state.prev_expression
        || blinking != state.prev_is_blinking
        || bob_y != state.prev_bob_y
        || state.eye_look_x != state.prev_eye_look_x
        || state.eye_look_y != state.prev_eye_look_y
        || state.text != state.prev_text;

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else if face_changed {
        erase_face(oled, state.prev_eye_look_x, state.prev_eye_look_y, state.prev_bob_y, 1);
    } else {
        // No need to redraw face if nothing changed
        return;
    }

    state.prev_text = state.text.clone();
    state.prev_expression = state.expression;
    state.prev_is_blinking = blinking;
    state.prev_bob_y = bob_y;
    state.prev_eye_look_x = state.eye_look_x;
    state.prev_eye_look_y = state.eye_look_y;

    // ── Eyes ──
    // Centred on 128×64: eyes at y≈24, spread wide for kawaii proportions
    let eye_cy = 22 + bob_y + state.eye_look_y;
    let le_cx = 38 + state.eye_look_x; // left-eye centre X
    let re_cx = 90 + state.eye_look_x; // right-eye centre X
    draw_eyes(oled, state.expression, le_cx, re_cx, eye_cy, blinking, 1, in_transition, now_ms);


    // ── Cheeks (Blush) ──
    let blush_color = Rgb565::new(31, 32, 16); // light pink
    let cheek_r = 6;
    let _ = Circle::new(Point::new(le_cx - 16, eye_cy + 8), (cheek_r * 2) as u32)
        .into_styled(PrimitiveStyle::with_fill(blush_color)).draw(oled);
    let _ = Circle::new(Point::new(re_cx + 4, eye_cy + 8), (cheek_r * 2) as u32)
        .into_styled(PrimitiveStyle::with_fill(blush_color)).draw(oled);

    // ── Mouth ──
    let mouth_cy = 46 + bob_y + state.eye_look_y;
    draw_mouth(oled, state.expression, 64, mouth_cy, 1, now_ms);

    // ── Status text overlay ──
    if !state.text.is_empty() {
        let text_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
        let _ = Rectangle::new(Point::new(0, 54), Size::new(DISPLAY_WIDTH, 10))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
        let _ = Text::new(&state.text, Point::new(0, 63), text_style).draw(oled);
    }
}

/// Draw both eyes for any expression.  `scale` 1 = full (128×64), 0 = mini (64px panel).
fn draw_eyes(
    oled: &mut TftDisplay,
    expr: EyeExpression,
    le_cx: i32,
    re_cx: i32,
    eye_cy: i32,
    blinking: bool,
    scale: i32,
    in_transition: bool,
    now_ms: u64,
) {
    let fill_on = PrimitiveStyle::with_fill(Rgb565::BLACK);
    let fill_off = PrimitiveStyle::with_fill(Rgb565::WHITE);
    let stroke2 = PrimitiveStyle::with_stroke(Rgb565::BLACK, if scale > 0 { 2 } else { 1 });

    // Eye radius scales: full=12, mini=6
    let r = if scale > 0 { 12i32 } else { 6i32 };
    // Highlight radius: full=4, mini=2
    let hr = if scale > 0 { 4i32 } else { 2i32 };

    if blinking {
        if in_transition {
            // Squished eyes during transition
            let _ = Ellipse::with_center(Point::new(le_cx, eye_cy), Size::new((r * 2) as u32, r as u32))
                .into_styled(fill_on).draw(oled);
            let _ = Ellipse::with_center(Point::new(re_cx, eye_cy), Size::new((r * 2) as u32, r as u32))
                .into_styled(fill_on).draw(oled);
        } else {
            // Blink: thick horizontal lines (kawaii ─ ─)
            let hw = r + 2;
            let _ = Line::new(Point::new(le_cx - hw, eye_cy), Point::new(le_cx + hw, eye_cy))
                .into_styled(stroke2).draw(oled);
            let _ = Line::new(Point::new(re_cx - hw, eye_cy), Point::new(re_cx + hw, eye_cy))
                .into_styled(stroke2).draw(oled);
        }
        return;
    }

    match expr {
        EyeExpression::Neutral | EyeExpression::Blink => {
            // Large filled circles with highlight dot (classic kawaii)
            let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight (upper-right of each eye)
            let _ = Circle::new(Point::new(le_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
        }
        EyeExpression::Happy => {
            // Happy: filled upward arcs (＾ ＾) — draw filled circle then erase bottom half
            let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight (upper-right of each eye)
            let _ = Circle::new(Point::new(le_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            // Erase bottom 60% to create upward arc
            let cut_h = (r * 6 / 5) as u32;
            let _ = Rectangle::new(
                Point::new(le_cx - r - 1, eye_cy - 1),
                Size::new((r * 2 + 2) as u32, cut_h),
            ).into_styled(fill_off).draw(oled);
            let _ = Rectangle::new(
                Point::new(re_cx - r - 1, eye_cy - 1),
                Size::new((r * 2 + 2) as u32, cut_h),
            ).into_styled(fill_off).draw(oled);
        }
        EyeExpression::Sad => {
            // Sad: filled circles with droopy eyelid covering top 65%
            let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight (upper-right of each eye)
            let _ = Circle::new(Point::new(le_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            // Eyelid: erase top portion + angled lid line
            let lid_h = (r * 13 / 10) as u32;
            let _ = Rectangle::new(
                Point::new(le_cx - r - 1, eye_cy - r - 1),
                Size::new((r * 2 + 2) as u32, lid_h),
            ).into_styled(fill_off).draw(oled);
            let _ = Rectangle::new(
                Point::new(re_cx - r - 1, eye_cy - r - 1),
                Size::new((r * 2 + 2) as u32, lid_h),
            ).into_styled(fill_off).draw(oled);
            // Angled eyelid lines (drooping inward)
            let _ = Line::new(
                Point::new(le_cx - r, eye_cy - r / 3 - 2),
                Point::new(le_cx + r, eye_cy + 1),
            ).into_styled(stroke2).draw(oled);
            let _ = Line::new(
                Point::new(re_cx - r, eye_cy + 1),
                Point::new(re_cx + r, eye_cy - r / 3 - 2),
            ).into_styled(stroke2).draw(oled);
        }
        EyeExpression::Angry => {
            // Angry: filled circles with thick V-brows pointing inward
            let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight (upper-right of each eye)
            let _ = Circle::new(Point::new(le_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            // Cut top portion for narrowed look
            let cut = (r * 2 / 3) as u32;
            let _ = Rectangle::new(
                Point::new(le_cx - r - 1, eye_cy - r - 1),
                Size::new((r * 2 + 2) as u32, cut),
            ).into_styled(fill_off).draw(oled);
            let _ = Rectangle::new(
                Point::new(re_cx - r - 1, eye_cy - r - 1),
                Size::new((r * 2 + 2) as u32, cut),
            ).into_styled(fill_off).draw(oled);
            // Thick angry brows angling inward
            let bw = r + 4;
            let _ = Line::new(
                Point::new(le_cx - bw, eye_cy - r - 2),
                Point::new(le_cx + bw / 2, eye_cy - r / 3),
            ).into_styled(stroke2).draw(oled);
            let _ = Line::new(
                Point::new(re_cx - bw / 2, eye_cy - r / 3),
                Point::new(re_cx + bw, eye_cy - r - 2),
            ).into_styled(stroke2).draw(oled);
        }
        EyeExpression::Surprised => {
            // Surprised: large open circles with pupils + highlights
            let big_r = r + 2;
            let _ = Circle::new(Point::new(le_cx - big_r, eye_cy - big_r), (big_r * 2) as u32)
                .into_styled(stroke2).draw(oled);
            let _ = Circle::new(Point::new(re_cx - big_r, eye_cy - big_r), (big_r * 2) as u32)
                .into_styled(stroke2).draw(oled);
            // Pupils (offset slightly down-centre)
            let pr = if scale > 0 { 3i32 } else { 2i32 };
            let _ = Circle::new(Point::new(le_cx - pr, eye_cy - pr + 1), (pr * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - pr, eye_cy - pr + 1), (pr * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight
            let _ = Circle::new(Point::new(le_cx + big_r / 3, eye_cy - big_r / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + big_r / 3, eye_cy - big_r / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
        }
        EyeExpression::Thinking => {
            // Thinking: left eye normal (with look-up pupil), right eye half-closed
            let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(le_cx + r / 4, eye_cy - r / 2 - hr / 2), hr as u32)
                .into_styled(fill_off).draw(oled);
            // Right eye: thick horizontal line (half-closed)
            let hw = r + 2;
            let _ = Line::new(Point::new(re_cx - hw, eye_cy), Point::new(re_cx + hw, eye_cy))
                .into_styled(stroke2).draw(oled);
        }
        EyeExpression::Heart => {
            // Heart eyes: pixel-art hearts
            draw_heart(oled, le_cx, eye_cy, r);
            draw_heart(oled, re_cx, eye_cy, r);
        }
        EyeExpression::Sleepy => {
            // Sleepy: U-shaped closed eyes (like the image row 2 col 2)
            let hw = r;
            // Left eye: upside-down arc — draw circle, erase top half
            let _ = Circle::new(Point::new(le_cx - hw, eye_cy - hw), (hw * 2) as u32)
                .into_styled(stroke2).draw(oled);
            let _ = Rectangle::new(
                Point::new(le_cx - hw - 1, eye_cy - hw - 1),
                Size::new((hw * 2 + 2) as u32, (hw + 1) as u32),
            ).into_styled(fill_off).draw(oled);
            // Right eye
            let _ = Circle::new(Point::new(re_cx - hw, eye_cy - hw), (hw * 2) as u32)
                .into_styled(stroke2).draw(oled);
            let _ = Rectangle::new(
                Point::new(re_cx - hw - 1, eye_cy - hw - 1),
                Size::new((hw * 2 + 2) as u32, (hw + 1) as u32),
            ).into_styled(fill_off).draw(oled);
        }
        EyeExpression::Listening => {
            // Expanded eyes
            let _ = Circle::new(Point::new(le_cx - r - 2, eye_cy - r - 2), ((r + 2) * 2) as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::new(Point::new(re_cx - r - 2, eye_cy - r - 2), ((r + 2) * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Highlight shifted
            let _ = Circle::new(Point::new(le_cx + r / 4 + 1, eye_cy - r / 2 - hr / 2 - 1), hr as u32)
                .into_styled(fill_off).draw(oled);
            let _ = Circle::new(Point::new(re_cx + r / 4 + 1, eye_cy - r / 2 - hr / 2 - 1), hr as u32)
                .into_styled(fill_off).draw(oled);
        }
        EyeExpression::Processing => {
            // Two small overlapping circles that alternate sizes
            let t = (now_ms / 500) % 2;
            let r1 = if t == 0 { r } else { r / 2 };
            let r2 = if t == 1 { r } else { r / 2 };

            let _ = Circle::with_center(Point::new(le_cx - r/2, eye_cy), r1 as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::with_center(Point::new(le_cx + r/2, eye_cy), r2 as u32)
                .into_styled(fill_on).draw(oled);

            let _ = Circle::with_center(Point::new(re_cx - r/2, eye_cy), r1 as u32)
                .into_styled(fill_on).draw(oled);
            let _ = Circle::with_center(Point::new(re_cx + r/2, eye_cy), r2 as u32)
                .into_styled(fill_on).draw(oled);
        }
    }
}

/// Draw a filled pixel-art heart centred at (cx, cy) with radius `r`.
fn draw_heart(oled: &mut TftDisplay, cx: i32, cy: i32, r: i32) {
    let fill_on = PrimitiveStyle::with_fill(Rgb565::BLACK);
    // Heart = two overlapping circles on top + triangle pointing down
    let hr = r * 2 / 3; // radius of each lobe
    // Left lobe
    let _ = Circle::new(Point::new(cx - hr - hr / 2, cy - hr), (hr * 2) as u32)
        .into_styled(fill_on).draw(oled);
    // Right lobe
    let _ = Circle::new(Point::new(cx + hr / 2 - hr, cy - hr), (hr * 2) as u32)
        .into_styled(fill_on).draw(oled);
    // Bottom triangle: fill rows from the circle equator down to the tip
    for dy in 0..=hr {
        let progress = dy as u32;
        let total = hr as u32;
        // Width narrows linearly from full width to 0
        let half_w = ((hr as u32 + hr as u32 / 2) * (total - progress) / total) as i32;
        if half_w > 0 {
            let _ = Line::new(
                Point::new(cx - half_w, cy + dy),
                Point::new(cx + half_w, cy + dy),
            ).into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1)).draw(oled);
        }
    }
    // Highlight (upper-right)
    let highlight_r = if r > 10 { 4i32 } else { 2i32 };
    let _ = Circle::new(Point::new(cx + r / 4, cy - r / 2 - highlight_r / 2), highlight_r as u32)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE)).draw(oled);
}

// ── Shared mouth drawing ──────────────────────────────────────────────────────

/// Draw the mouth centred at (`cx`, `cy`).  `scale` 1 = full size, 0 = mini.
fn draw_mouth(
    oled: &mut TftDisplay,
    expr: EyeExpression,
    cx: i32,
    cy: i32,
    scale: i32,
    _now_ms: u64,
) {
    let fill_on = PrimitiveStyle::with_fill(Rgb565::BLACK);
    let fill_off = PrimitiveStyle::with_fill(Rgb565::WHITE);
    let stroke = PrimitiveStyle::with_stroke(Rgb565::BLACK, 1);
    let stroke2 = PrimitiveStyle::with_stroke(Rgb565::BLACK, if scale > 0 { 2 } else { 1 });
    // Mouth dimensions scale with mode
    let mr = if scale > 0 { 10i32 } else { 6i32 }; // mouth radius for curves

    match expr {
        EyeExpression::Happy | EyeExpression::Heart => {
            // Wide smile: filled crescent (circle + erase top half)
            let _ = Circle::new(Point::new(cx - mr, cy - mr), (mr * 2) as u32)
                .into_styled(fill_on).draw(oled);
            // Erase top half + a bit more to make a crescent smile
            let _ = Rectangle::new(
                Point::new(cx - mr - 1, cy - mr - 1),
                Size::new((mr * 2 + 2) as u32, (mr + 1) as u32),
            ).into_styled(fill_off).draw(oled);
        }
        EyeExpression::Sad => {
            // Frown: inverted crescent (circle + erase bottom half)
            let _ = Circle::new(Point::new(cx - mr, cy - mr / 2), (mr * 2) as u32)
                .into_styled(stroke2).draw(oled);
            // Erase bottom half
            let _ = Rectangle::new(
                Point::new(cx - mr - 1, cy + mr / 2),
                Size::new((mr * 2 + 2) as u32, (mr + 2) as u32),
            ).into_styled(fill_off).draw(oled);
            // Erase top portion to keep just the bottom arc
            let _ = Rectangle::new(
                Point::new(cx - mr - 1, cy - mr / 2 - 1),
                Size::new((mr * 2 + 2) as u32, (mr / 2 + 1) as u32),
            ).into_styled(fill_off).draw(oled);
        }
        EyeExpression::Angry => {
            // Gritted teeth: rectangle with vertical black bars
            let tw = mr + scale * 4;
            let th = if scale > 0 { 6i32 } else { 4i32 };
            let _ = Rectangle::new(Point::new(cx - tw, cy - th / 2), Size::new((tw * 2) as u32, th as u32))
                .into_styled(fill_on).draw(oled);
            // Teeth gaps
            let teeth = if scale > 0 { 4 } else { 3 };
            for i in 1..teeth {
                let tx = cx - tw + i * (tw * 2 / teeth);
                let _ = Rectangle::new(Point::new(tx, cy - th / 2 + 1), Size::new(1, (th - 2) as u32))
                    .into_styled(fill_off).draw(oled);
            }
        }
        EyeExpression::Surprised => {
            // Open "O" mouth — small filled circle
            let or = if scale > 0 { 5i32 } else { 3i32 };
            let _ = Circle::new(Point::new(cx - or, cy - or), (or * 2) as u32)
                .into_styled(stroke2).draw(oled);
        }
        EyeExpression::Thinking => {
            // Wavy squiggle (3-segment line)
            let seg = mr * 2 / 3;
            let _ = Line::new(Point::new(cx - seg * 2, cy), Point::new(cx - seg, cy - 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx - seg, cy - 2), Point::new(cx + seg, cy + 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx + seg, cy + 2), Point::new(cx + seg * 2, cy))
                .into_styled(stroke).draw(oled);
        }
        EyeExpression::Sleepy => {
            // Tiny "w" mouth — two small V's side by side
            let w = mr / 2;
            let _ = Line::new(Point::new(cx - w * 2, cy), Point::new(cx - w, cy + 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx - w, cy + 2), Point::new(cx, cy))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx, cy), Point::new(cx + w, cy + 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx + w, cy + 2), Point::new(cx + w * 2, cy))
                .into_styled(stroke).draw(oled);
        }
        _ => {
            // Neutral: small thick horizontal line (subtle, calm)
            let hw = if scale > 0 { 6i32 } else { 4i32 };
            let _ = Line::new(Point::new(cx - hw, cy), Point::new(cx + hw, cy))
                .into_styled(stroke2).draw(oled);
        }
    }
}

// ── Menu mode rendering ───────────────────────────────────────────────────────

/// Render the split-screen menu mode (menu left, mini-face right).
fn render_menu_mode(oled: &mut TftDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    let menu_changed = state.ui_mode != state.prev_ui_mode
        || state.top_menu_selected != state.prev_top_menu_selected
        || state.sub_menu_selected != state.prev_sub_menu_selected
        || state.web_menu_selected != state.prev_web_menu_selected;

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
        render_menu_panel(oled, state);

        let divider_style = PrimitiveStyle::with_stroke(Rgb565::BLACK, 1);
        let _ = Line::new(Point::new(63, 0), Point::new(63, 63))
            .into_styled(divider_style)
            .draw(oled);
    } else if menu_changed {
        // Erase menu area and redraw
        let _ = Rectangle::new(Point::new(0, 0), Size::new(62, DISPLAY_HEIGHT))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
        render_menu_panel(oled, state);
    }

    state.prev_ui_mode = state.ui_mode;
    state.prev_top_menu_selected = state.top_menu_selected;
    state.prev_sub_menu_selected = state.sub_menu_selected;
    state.prev_web_menu_selected = state.web_menu_selected;


    render_mini_face(oled, state);
}

/// Draw the left 63-px menu panel.
fn render_menu_panel(oled: &mut TftDisplay, state: &FaceState) {
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inverted_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    if let UiMode::WebMenu = state.ui_mode {
        render_web_menu_panel(oled, state);
        return;
    }

    // Get the menu items and selected index for the current mode.
    let items = get_menu_items(&state.ui_mode);
    let sel = get_menu_selected(state) as usize;
    let total = items.len();
    let max_visible: usize = 4;

    // Scrolling window: show 4 items at a time (4 × 16 = 64px fits).
    let scroll_start = if sel >= max_visible {
        sel - (max_visible - 1)
    } else {
        0
    };

    for slot in 0..max_visible {
        let i = scroll_start + slot;
        if i >= total {
            break;
        }
        let label = items[i].label;
        let y_top = (slot as i32) * 16;
        if i == sel {
            // Give button tactility by checking if it was just touched/pressed
            let pressed = Instant::now().duration_since(state.last_button_time).as_millis() < 150;
            let shrink = if pressed { 1 } else { 0 };
            let fill_color = if pressed { COLOR_PASTEL_PEACH } else { COLOR_SOFT_GRAY };

            let _ = RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(1 + shrink, y_top + 1 + shrink), Size::new((61 - shrink * 2) as u32, (14 - shrink * 2) as u32)), Size::new(4, 4))
                .into_styled(PrimitiveStyle::with_fill(fill_color))
                .draw(oled);
            let _ = Text::new(label, Point::new(4, y_top + 12), style).draw(oled);
        } else {
            let _ = Text::new(label, Point::new(4, y_top + 12), style).draw(oled);
        }
    }

    // Scroll indicators
    if scroll_start > 0 {
        let _ = Text::new("\x1e", Point::new(56, 8), style).draw(oled);
    }
    if scroll_start + max_visible < total {
        let _ = Text::new("\x1f", Point::new(56, 62), style).draw(oled);
    }
}

fn render_web_menu_panel(oled: &mut TftDisplay, state: &FaceState) {
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inverted_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    let _ = Text::new("Web", Point::new(2, 10), style).draw(oled);

    for (i, &label) in WEB_MENU_ITEMS.iter().enumerate() {
        let y_top = 14 + (i as i32) * 16;
        if i == state.web_menu_selected as usize {
            let _ = RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(1, y_top + 1), Size::new(61, 12)), Size::new(4, 4))
                .into_styled(PrimitiveStyle::with_fill(COLOR_SOFT_GRAY))
                .draw(oled);
            let _ = Text::new(label, Point::new(4, y_top + 11), style).draw(oled);
        } else {
            let _ = Text::new(label, Point::new(4, y_top + 11), style).draw(oled);
        }
    }

    let mut status = heapless::String::<64>::new();
    let web_enabled = crate::web_server::is_web_server_enabled();
    if !web_enabled {
        let _ = status.push_str("disconnected");
    } else {
        match crate::wifi::wifi_status() {
            crate::wifi::WIFI_STATUS_CONNECTING => {
                let _ = status.push_str("connecting");
            }
            crate::wifi::WIFI_STATUS_CONNECTED => {
                if let Some(ip) = crate::wifi::wifi_ipv4() {
                    let _ = core::fmt::write(
                        &mut status,
                        format_args!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
                    );
                } else {
                    let _ = status.push_str("connected");
                }
            }
            crate::wifi::WIFI_STATUS_ERROR => {
                let _ = status.push_str("error");
            }
            _ => {
                let _ = status.push_str("disconnected");
            }
        }
    }

    let _ = Rectangle::new(Point::new(0, 52), Size::new(63, 12))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
        .draw(oled);
    let _ = Text::new(&status, Point::new(2, 62), style).draw(oled);
}

// ── New full-screen mode renderers ───────────────────────────────────────────

/// Full-screen Pomodoro timer (25-minute countdown + progress bar).
fn render_pomodoro(oled: &mut TftDisplay, state: &mut FaceState) {
    use core::fmt::Write;

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else {
        // Just erase the specific text areas that change instead of the whole screen
        let _ = Rectangle::new(Point::new(34, 28), Size::new(60, 20))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
        let _ = Rectangle::new(Point::new(0, 48), Size::new(128, 14))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
        let _ = Rectangle::new(Point::new(0, 56), Size::new(128, 7))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    }
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    // Header bar
    let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(oled);
    let _ = Text::new("POMODORO", Point::new(34, 9), inv_style).draw(oled);

    let total_ms: u64 = 25 * 60 * 1000;
    let remaining_ms = if state.pomodoro_done {
        0u64
    } else if let Some(paused) = state.pomodoro_paused_remaining {
        paused
    } else if let Some(start) = state.pomodoro_start {
        let elapsed = (Instant::now() - start).as_millis();
        total_ms.saturating_sub(elapsed)
    } else {
        total_ms
    };

    // Check completion
    if remaining_ms == 0 && !state.pomodoro_done && state.pomodoro_start.is_some() {
        state.pomodoro_done = true;
        state.expression = EyeExpression::Surprised;
        state.expression_override = true;
        state.expression_override_since = Some(Instant::now());
    }

    let total_secs = (remaining_ms / 1000) as u32;
    let mm = total_secs / 60;
    let ss = total_secs % 60;

    // Large MM:SS display
    let mut time_str = heapless::String::<8>::new();
    let _ = write!(time_str, "{:02}:{:02}", mm, ss);
    // Double-size: draw each character as 12×20 using FONT_6X10 twice
    let _ = Text::new(&time_str, Point::new(34, 38), style).draw(oled);

    // Status line
    let status_str = if state.pomodoro_done {
        "DONE! press to exit"
    } else if state.pomodoro_paused_remaining.is_some() {
        "PAUSED  press:resume"
    } else {
        "hold:cancel press:pause"
    };
    let _ = Text::new(status_str, Point::new(2, 50), style).draw(oled);

    // Progress bar at bottom (y=54..62)
    let _ = Rectangle::new(Point::new(0, 55), Size::new(128, 9))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1))
        .draw(oled);
    let elapsed_ms = total_ms.saturating_sub(remaining_ms);
    let fill_w = ((elapsed_ms * 126) / total_ms) as u32;
    if fill_w > 0 {
        let _ = Rectangle::new(Point::new(1, 56), Size::new(fill_w, 7))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
    }
}

/// Full-screen system monitor (WiFi, IP, uptime, memory).
fn render_system_monitor(oled: &mut TftDisplay, state: &mut FaceState) {
    use core::fmt::Write;

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else {
        // Only erase the content rows
        let _ = Rectangle::new(Point::new(0, 20), Size::new(128, 44))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    }
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    // Header bar
    let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(oled);
    let _ = Text::new("SYSTEM", Point::new(40, 9), inv_style).draw(oled);

    // WiFi status
    let wifi_str = match crate::wifi::wifi_status() {
        crate::wifi::WIFI_STATUS_CONNECTED => "WiFi: Connected",
        crate::wifi::WIFI_STATUS_CONNECTING => "WiFi: Connecting",
        crate::wifi::WIFI_STATUS_ERROR => "WiFi: Error",
        _ => "WiFi: Off",
    };
    let _ = Text::new(wifi_str, Point::new(2, 24), style).draw(oled);

    // IP address
    let mut ip_str = heapless::String::<22>::new();
    let _ = ip_str.push_str("IP: ");
    if let Some(ip) = crate::wifi::wifi_ipv4() {
        let _ = write!(ip_str, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    } else {
        let _ = ip_str.push_str("--");
    }
    let _ = Text::new(&ip_str, Point::new(2, 36), style).draw(oled);

    // Uptime
    let secs = Instant::now().as_millis() / 1000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut up_str = heapless::String::<22>::new();
    let _ = write!(up_str, "Up: {}h {}m {}s", h, m, s);
    let _ = Text::new(&up_str, Point::new(2, 48), style).draw(oled);

    // Memory
    let free_kb = esp_alloc::HEAP.free() / 1024;
    let used_kb = esp_alloc::HEAP.used() / 1024;
    let mut mem_str = heapless::String::<22>::new();
    let _ = write!(mem_str, "Mem: {}k/{}k", free_kb, free_kb + used_kb);
    let _ = Text::new(&mem_str, Point::new(2, 60), style).draw(oled);
}

/// Full-screen clock view (NTP time or uptime fallback).
fn render_clock_view(oled: &mut TftDisplay, state: &mut FaceState) {
    use core::fmt::Write;

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else {
        // Only erase the content area
        let _ = Rectangle::new(Point::new(0, 20), Size::new(128, 44))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    }
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    // Header bar
    let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(oled);
    let _ = Text::new("CLOCK", Point::new(44, 9), inv_style).draw(oled);

    if let Some(unix_secs) = crate::ntp::wall_clock_secs() {
        // Convert to HH:MM:SS (UTC+1 for Vienna)
        let local_secs = unix_secs + 3600; // UTC+1
        let day_secs = (local_secs % 86400) as u32;
        let hh = day_secs / 3600;
        let mm = (day_secs % 3600) / 60;
        let ss = day_secs % 60;

        let mut time_str = heapless::String::<12>::new();
        let _ = write!(time_str, "{:02}:{:02}:{:02}", hh, mm, ss);
        let _ = Text::new(&time_str, Point::new(34, 35), style).draw(oled);

        // Date: days since Unix epoch → Y/M/D
        let total_days = (local_secs / 86400) as u32;
        let (y, m, d) = days_to_date(total_days);
        let mut date_str = heapless::String::<16>::new();
        let _ = write!(date_str, "{:04}-{:02}-{:02}", y, m, d);
        let _ = Text::new(&date_str, Point::new(28, 50), style).draw(oled);

        let _ = Text::new("NTP synced", Point::new(28, 62), style).draw(oled);
    } else {
        // Fallback: show uptime
        let secs = Instant::now().as_millis() / 1000;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        let mut time_str = heapless::String::<16>::new();
        let _ = write!(time_str, "{}h {:02}m {:02}s", h, m, s);
        let _ = Text::new("Uptime:", Point::new(36, 30), style).draw(oled);
        let _ = Text::new(&time_str, Point::new(24, 44), style).draw(oled);
        let _ = Text::new("no NTP sync", Point::new(24, 62), style).draw(oled);
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_date(days: u32) -> (u32, u32, u32) {
    // Civil calendar algorithm
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Flashlight status screen (split: status left, mini-face right).
fn render_flashlight(oled: &mut TftDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;

        let _ = Line::new(Point::new(63, 0), Point::new(63, 63))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1))
            .draw(oled);

        let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
        let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

        // Header
        let _ = Rectangle::new(Point::new(0, 0), Size::new(63, 11))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
        let _ = Text::new("LIGHT", Point::new(12, 9), inv_style).draw(oled);

        // Status
        let _ = Text::new("ON", Point::new(22, 32), style).draw(oled);

        // Sun icon (simple circle with rays)
        let _ = Circle::new(Point::new(24, 36), 10)
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1))
            .draw(oled);

        let _ = Text::new(">exit", Point::new(12, 60), style).draw(oled);
    }

    render_mini_face(oled, state);
}

/// Party mode status screen (split: status left, mini-face right).
fn render_party_mode(oled: &mut TftDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;

        let _ = Line::new(Point::new(63, 0), Point::new(63, 63))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1))
            .draw(oled);

        // Header
        let _ = Rectangle::new(Point::new(0, 0), Size::new(63, 11))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
        let _ = Text::new("PARTY", Point::new(12, 9), inv_style).draw(oled);
        let _ = Text::new(">exit", Point::new(12, 60), style).draw(oled);
    } else {
        // Erase text area
        let _ = Rectangle::new(Point::new(0, 30), Size::new(62, 30))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    }

    // Animated note symbols based on frame
    let phase = (state.frame / 4) % 3;
    let note = match phase {
        0 => "~ * ~",
        1 => "* ~ *",
        _ => "~ ~ ~",
    };
    let _ = Text::new(note, Point::new(4, 32), style).draw(oled);

    // Show current hue angle
    {
        use core::fmt::Write;
        let mut hue_str = heapless::String::<12>::new();
        let _ = write!(hue_str, "Hue: {}", state.party_hue);
        let _ = Text::new(&hue_str, Point::new(4, 46), style).draw(oled);
    }

    render_mini_face(oled, state);
}

/// Full-screen Vienna Lines list view (128×64).
///
/// Layout:
///   Rows 0-35:  3 visible station→direction items (12px each)
///               Selected row is inverted and scrolls horizontally (marquee)
///   Row 37:     separator line
///   Rows 39-63: route lines + wait minutes for the selected stop
///
/// Navigation: short press = next item, long press = detail view
fn render_vienna_lines(oled: &mut TftDisplay, state: &mut FaceState) {
    use core::fmt::Write;

    let vienna_changed = state.vienna_selected != state.prev_vienna_selected;
    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else if vienna_changed || state.frame % 2 == 0 {
        // Erase list and bottom area
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 36))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
        let _ = Rectangle::new(Point::new(0, 48), Size::new(128, 16))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    } else {
        return; // Don't redraw if not changed and not marquee frame
    }
    state.prev_vienna_selected = state.vienna_selected;

    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let data = crate::vienna_fetch::get_lines();

    // Loading / error / empty states
    if data.loading && data.stops.is_empty() {
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
        let _ = Text::new("Wiener Linien", Point::new(22, 9), inv_style).draw(oled);
        let _ = Text::new("Loading...", Point::new(28, 36), style).draw(oled);
        return;
    }
    if data.error && data.stops.is_empty() {
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
        let _ = Text::new("Wiener Linien", Point::new(22, 9), inv_style).draw(oled);
        let _ = Text::new("Fetch error", Point::new(24, 36), style).draw(oled);
        return;
    }
    if data.stops.is_empty() {
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 11))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(oled);
        let _ = Text::new("Wiener Linien", Point::new(22, 9), inv_style).draw(oled);
        let _ = Text::new("No data yet", Point::new(24, 36), style).draw(oled);
        return;
    }

    let total = data.stops.len();
    let sel = state.vienna_selected % total;

    // List area: 3 visible rows
    let max_visible: usize = 3;
    let scroll_start = if sel >= max_visible {
        sel - (max_visible - 1)
    } else {
        0
    };

    for slot in 0..max_visible {
        let idx = scroll_start + slot;
        if idx >= total {
            break;
        }
        let stop = &data.stops[idx];
        let y_top = (slot as i32) * 12;
        let is_selected = idx == sel;

        // Build display string: "STATION > DIRECTION"
        let mut label = heapless::String::<80>::new();
        let _ = write!(label, "{} > {}", stop.station, stop.direction);

        if is_selected {
            let _ = RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(1, y_top), Size::new(126, 12)), Size::new(4, 4))
                .into_styled(PrimitiveStyle::with_fill(COLOR_SOFT_GRAY))
                .draw(oled);

            let text_w = label.len() as i32 * 6;

            // Animación de marquesina si el texto excede el ancho (128px)
            if text_w > 128 {
                // Si el valor es negativo, el offset es 0 (se queda quieto).
                // Si es positivo, usamos el valor para desplazarlo.
                let scroll_offset = state.vienna_scroll_x.max(0);
                let x = 1 - scroll_offset;

                let _ = Text::new(&label, Point::new(x, y_top + 9), style).draw(oled);

                // Avanzamos el contador/scroll en cada frame
                state.vienna_scroll_x += 1;

                // Cuando el texto sale completamente por la izquierda
                if state.vienna_scroll_x > text_w {
                    // Reiniciamos a -20. Esto hará que el texto vuelva a aparecer
                    // a la izquierda y espere 20 frames (2 segundos) antes de moverse.
                    state.vienna_scroll_x = -20;
                }
        } else {
                let _ = Text::new(&label, Point::new(1, y_top + 9), style).draw(oled);
            }
        }  else {
            // Truncate non-selected rows to fit 21 chars
            let trunc: &str = if label.len() > 21 {
                &label[..21]
            } else {
                &label
            };
            let _ = Text::new(trunc, Point::new(1, y_top + 9), style).draw(oled);
        }
    }

    // Separator line
    let _ = Line::new(Point::new(0, 37), Point::new(127, 37))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::BLACK, 1))
        .draw(oled);

    // Bottom area: show routes for selected stop (up to 2 lines)
    let selected = &data.stops[sel];
    let mut y = 48i32;
    for route in selected.routes.iter() {
        if y > 62 {
            break;
        }
        let mut detail = heapless::String::<64>::new();
        let _ = write!(detail, "{}:", route.line);
        for dep in route.departures.iter() {
            let _ = write!(detail, " {}", dep.wait_minutes);
        }
        let _ = Text::new(&detail, Point::new(1, y), style).draw(oled);
        y += 12;
    }
}

/// Detail view for a selected Vienna stop — shows all departure info.
///
/// Layout:
///   Row 0:  inverted header with station name
///   Row 12: "→ Direction"
///   Rows 24+: each route line with wait times and clock times
///
/// Any button press returns to the list view.
fn render_vienna_detail(oled: &mut TftDisplay, state: &mut FaceState) {
    use core::fmt::Write;

    let vienna_changed = state.vienna_selected != state.prev_vienna_selected;
    if state.force_clear {
        oled.clear(Rgb565::WHITE);
        state.force_clear = false;
    } else if vienna_changed {
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 64))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(oled);
    } else {
        return;
    }
    state.prev_vienna_selected = state.vienna_selected;

    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
    let inv_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let data = crate::vienna_fetch::get_lines();

    if data.stops.is_empty() {
        let _ = Text::new("No data", Point::new(34, 36), style).draw(oled);
        return;
    }

    let sel = state.vienna_selected % data.stops.len();
    let stop = &data.stops[sel];

    // Header bar: station name
    let _ = Rectangle::new(Point::new(0, 0), Size::new(128, 12))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(oled);
    let hdr: &str = &stop.station;
    let hdr_x = ((128i32 - (hdr.len() as i32) * 6) / 2).max(0);
    let _ = Text::new(hdr, Point::new(hdr_x, 9), inv_style).draw(oled);

    // Direction row
    let mut dir_buf = heapless::String::<36>::new();
    let _ = dir_buf.push_str("> ");
    let _ = dir_buf.push_str(&stop.direction);
    let dir_trunc: &str = if dir_buf.len() > 21 { &dir_buf[..21] } else { &dir_buf };
    let _ = Text::new(dir_trunc, Point::new(1, 22), style).draw(oled);

    // Routes with full departure info
    let mut y = 34i32;
    for route in stop.routes.iter() {
        if y > 62 {
            break;
        }
        // Line name + wait minutes + clock times
        // e.g. "U6: 2m(09:00) 6m(09:04)"
        let mut line_buf: heapless::String<64> = heapless::String::new();
        let _ = write!(line_buf, "{}:", route.line);
        for dep in route.departures.iter() {
            if dep.time_str.is_empty() {
                let _ = write!(line_buf, " {}m", dep.wait_minutes);
            } else {
                let _ = write!(line_buf, " {}m({})", dep.wait_minutes, dep.time_str);
            }
        }
        let _ = Text::new(&line_buf, Point::new(1, y), style).draw(oled);
        y += 12;
    }
}

/// Draw the mini animated face on the right 64-px panel (x=64..127).
fn render_mini_face(oled: &mut TftDisplay, state: &mut FaceState) {
    let now_ms = Instant::now().as_millis() as u64;
    let blinking = now_ms < state.blink_end_ms || now_ms < state.transition_end_ms;

    let bob_y = breathing_offset(now_ms) / 2; // subtler in mini

    let face_changed = state.expression != state.prev_expression
        || blinking != state.prev_is_blinking
        || bob_y != state.prev_bob_y
        || state.eye_look_x != state.prev_eye_look_x
        || state.eye_look_y != state.prev_eye_look_y;

    let mode_changed = state.ui_mode != state.prev_ui_mode;
    if mode_changed || face_changed {
        if !mode_changed {
            // only erase if we didn't just clear the whole screen
            erase_face(oled, state.prev_eye_look_x, state.prev_eye_look_y, state.prev_bob_y, 0);
        }
    } else {
        return;
    }

    state.prev_expression = state.expression;
    state.prev_is_blinking = blinking;
    state.prev_bob_y = bob_y;
    state.prev_eye_look_x = state.eye_look_x;
    state.prev_eye_look_y = state.eye_look_y;

    // Eye positions centred in the right 64px panel
    let le_cx = 80 + state.eye_look_x / 2;
    let re_cx = 110 + state.eye_look_x / 2;
    let eye_cy = 22 + bob_y + state.eye_look_y;

    let in_transition = now_ms < state.transition_end_ms;
    draw_eyes(oled, state.expression, le_cx, re_cx, eye_cy, blinking, 0, in_transition, now_ms);


    // ── Cheeks (Blush) ──
    let blush_color = Rgb565::new(31, 32, 16); // light pink
    let cheek_r = 3;
    let _ = Circle::new(Point::new(le_cx - 8, eye_cy + 4), (cheek_r * 2) as u32)
        .into_styled(PrimitiveStyle::with_fill(blush_color)).draw(oled);
    let _ = Circle::new(Point::new(re_cx + 2, eye_cy + 4), (cheek_r * 2) as u32)
        .into_styled(PrimitiveStyle::with_fill(blush_color)).draw(oled);

    // Mini mouth
    draw_mouth(oled, state.expression, 95, 38 + bob_y, 0, now_ms);
}

