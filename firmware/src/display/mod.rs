//! Phase 2 — OLED Display Driver.
//!
//! Drives a 0.96" I2C OLED panel (`SSD1306`, 128×64) on the ESP32-S3.
//! Wiring used by this project:
//! - `VCC` -> `3V3`
//! - `GND` -> `GND`
//! - `SCL` -> `GPIO9`
//! - `SDA` -> `GPIO8`
//!
//! ## Architecture
//!
//! ```text
//! Wasm guest calls host_draw_eye(Happy)
//!      │
//!      ▼
//! AbiHost::draw_eye()        ← validates arg, delegates to DisplayDriver
//!      │
//!      ▼
//! DisplayDriver::draw_eye()  ← embedded-graphics renders to framebuffer
//!      │
//!      ▼
//! Async I2C transfer         ← DMA/interrupt-driven; CPU yields while
//!                               the frame is pushed to the OLED controller
//! ```
//!
//! ## UI Modes
//!
//! - **Face mode** (default): full 128×64 animated face with idle expression
//!   cycling and auto-blink.
//! - **Menu mode**: face shrinks to the right 64 px; a 4-item menu appears
//!   on the left 64 px. Triggered by the BOOT button (GPIO 0).
//! - **MenuAction mode**: left panel shows the selected action result.
//!
//! ## BOOT button
//!
//! GPIO 0 is monitored by a dedicated async Embassy task using hardware
//! edge interrupts — no polling required:
//! - Short press (< 2 s): face → menu, or navigate to next menu item.
//! - Long press (≥ 2 s): face → menu, or select the highlighted item.
//! - 10 s of inactivity in menu/action mode → auto-return to face mode.

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
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive},
    primitives::{
        Circle, CornerRadii, Line, PrimitiveStyle, Rectangle, RoundedRectangle,
    },
    text::Text,
    Pixel,
};
use esp_hal::{
    gpio::{GpioPin, Input},
    i2c::master::{BusTimeout, Config as I2cConfig, I2c},
    peripherals::I2C0,
    time::RateExtU32,
    Async,
};

const SSD1306_ADDR: u8 = 0x3C;
pub const DISPLAY_WIDTH: u32 = 128;
pub const DISPLAY_HEIGHT: u32 = 64;
const DISPLAY_BUF_SIZE: usize = (DISPLAY_WIDTH as usize) * (DISPLAY_HEIGHT as usize / 8);
const DISPLAY_QUEUE_DEPTH: usize = 8;
const EXPRESSION_OVERRIDE_TIMEOUT: Duration = Duration::from_secs(8);
// 100 ms (10 fps) — async I2C yields the CPU during each flush so other
// tasks (WiFi, web server) continue to run.  The frame budget is generous
// enough to keep animations smooth at this rate.
const FRAME_PERIOD: Duration = Duration::from_millis(100);
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

/// Expressions to cycle through in idle mode — every consecutive pair
/// differs so that each transition is visually distinct.
const IDLE_EXPRESSIONS: [EyeExpression; 7] = [
    EyeExpression::Neutral,
    EyeExpression::Happy,
    EyeExpression::Thinking,
    EyeExpression::Neutral,
    EyeExpression::Surprised,
    EyeExpression::Happy,
    EyeExpression::Sad,
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
    i2c0: I2C0,
    sda: GpioPin<8>,
    scl: GpioPin<9>,
    boot_btn: Input<'static>,
) {
    // Spawn the dedicated button interrupt task first so it can catch presses
    // that occur during the display splash screen.
    spawner.spawn(button_task(boot_btn)).unwrap();
    spawner.spawn(display_task(i2c0, sda, scl)).unwrap();
}

/// Dedicated Embassy task for the BOOT button (GPIO 0).
///
/// Uses hardware edge interrupts (`wait_for_falling_edge`) instead of
/// polling so the CPU is never spinning for button state.  Press
/// classification (short vs. long) is handled here and the result is
/// forwarded to the display render loop via `BUTTON_EVENT_CHANNEL`.
#[embassy_executor::task]
async fn button_task(mut boot_btn: Input<'static>) {
    log::info!("[button] task starting — async edge interrupts on GPIO0");
    loop {
        // Wait for the active-LOW BOOT button to be pressed (falling edge).
        boot_btn.wait_for_falling_edge().await;
        let press_start = Instant::now();
        log::info!("[button] pressed");

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
    i2c0: I2C0,
    sda: GpioPin<8>,
    scl: GpioPin<9>,
) {
    log::info!("[display] task starting (I2C0 SDA=GPIO8 SCL=GPIO9 BOOT=GPIO0)");

    // 400 kHz async I2C.  WiFi interrupt jitter is no longer a concern
    // because the CPU yields during transfers; BusCycles(800) at 400 kHz
    // ≈ 2 ms still catches genuine bus errors quickly.
    let mut cfg = I2cConfig::default();
    cfg.frequency = 400.kHz();
    cfg.timeout = BusTimeout::BusCycles(800);

    let i2c = match I2c::new(i2c0, cfg) {
        Ok(bus) => bus.with_sda(sda).with_scl(scl).into_async(),
        Err(err) => {
            log::error!("[display] I2C init failed: {:?}", err);
            return;
        }
    };
    log::info!("[display] I2C init OK");

    let mut oled = OledDisplay::new(i2c);
    oled.init().await;
    log::info!("[display] OLED init sequence sent");

    let mut state = FaceState::default();

    oled.clear(BinaryColor::Off);
    let title_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::new("SandOS", Point::new(34, 20), title_style).draw(&mut oled);
    let _ = Text::new("OLED OK", Point::new(34, 36), title_style).draw(&mut oled);
    log::info!("[display] splash drawn, flushing…");
    match oled.flush().await {
        Ok(()) => log::info!("[display] splash flush OK"),
        Err(()) => log::error!("[display] splash flush FAILED (continuing)"),
    }
    Timer::after(Duration::from_millis(900)).await;

    // Initialise Instant-based fields now that the Embassy timer driver is running.
    state.last_button_time = Instant::now();
    // Seed PRNG from uptime so the animation schedule varies between boots.
    state.rng = Rng(Instant::now().as_millis() as u32 | 1);
    let seed_ms = Instant::now().as_millis() as u64;
    state.next_blink_ms = seed_ms + state.rng.range(1500, 3500) as u64;
    state.next_expression_ms = seed_ms + state.rng.range(3000, 5000) as u64;
    state.next_look_ms = seed_ms + state.rng.range(800, 2000) as u64;
    log::info!("[display] entering render loop");
    let mut loop_frames: u16 = 0;
    let mut last_loop_log = Instant::now();
    let mut had_flush_error = false;
    let mut last_flush_us: u64 = 0;
    let mut last_page_errors: u8 = 0;
    let mut flush_fail_since: Option<Instant> = None;
    let mut max_frame_us: u64 = 0;
    let mut min_frame_us: u64 = u64::MAX;
    let mut sum_render_us: u64 = 0;
    let mut sum_flush_us: u64 = 0;
    let mut sum_frame_us: u64 = 0;
    let mut last_expr_name: &str = "Neutral";

    let receiver = DISPLAY_CHANNEL.receiver();
    let btn_receiver = BUTTON_EVENT_CHANNEL.receiver();

    // Frame counter that never resets (unlike loop_frames which resets every second).
    let mut total_frames: u32 = 0;
    // Trace the first N frames step-by-step to diagnose stalls.
    const TRACE_FRAMES: u32 = 10;
    // Starvation state: true while the animation is known to be frozen.
    let mut starved = false;
    // Heartbeat: log every 5s so we know the task is alive even without
    // expression changes or starvation transitions.
    let mut last_heartbeat = Instant::now();

    loop {
        let frame_start = Instant::now();
        let tracing = total_frames < TRACE_FRAMES;

        if tracing {
            log::info!("[display] ── frame {} START (t={}ms) ──",
                total_frames, frame_start.as_millis());
        }

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
                    oled.set_contrast(value).await;
                }
            }
        }

        // 1b. First-iteration heartbeat (proves the loop body executes).
        if loop_frames == 0 && state.frame == 0 {
            log::info!("[display] render loop alive — first frame");
        }

        // 2. Drain button events sent by the async button_task.
        while let Ok(btn_event) = btn_receiver.try_receive() {
            handle_button_event(btn_event, &mut state);
        }

        // 3. Auto-return to face mode after 10 s of inactivity in menu/action.
        if !matches!(state.ui_mode, UiMode::Face) {
            let idle = Instant::now() - state.last_button_time;
            if idle >= Duration::from_secs(10) {
                state.ui_mode = UiMode::Face;
                state.text.clear();
                state.expression = EyeExpression::Neutral;
                state.expression_override = false;
                state.expression_override_since = None;
            }
        }

        // 4. Render and push the frame.
        if tracing { log::info!("[display] frame {} step=render", total_frames); }
        let render_start = Instant::now();
        render_frame(&mut oled, &mut state);
        let render_us = (Instant::now() - render_start).as_micros();
        if tracing { log::info!("[display] frame {} render={}us", total_frames, render_us); }

        // 4b. Flush framebuffer to SSD1306 via async I2C.
        //     The CPU yields to other tasks during each I2C transfer, so
        //     Wi-Fi, the web server and the button task all continue to run.
        if DIAG_SKIP_FLUSH {
            if loop_frames == 0 {
                log::warn!("[display] DIAG_SKIP_FLUSH enabled: OLED transfer disabled");
            }
        } else {
            if tracing { log::info!("[display] frame {} step=flush", total_frames); }
            let flush_start = Instant::now();
            let mut flush_ok = oled.flush().await.is_ok();
            // Immediate retry on transient bus error.
            if !flush_ok {
                flush_ok = oled.flush().await.is_ok();
            }
            last_flush_us = (Instant::now() - flush_start).as_micros() as u64;
            if tracing {
                log::info!("[display] frame {} flush={}us ok={}",
                    total_frames, last_flush_us, flush_ok);
            }
            last_page_errors = if flush_ok { 0 } else { 1 };
            if flush_ok {
                flush_fail_since = None;
                if had_flush_error {
                    log::info!("[display] flush recovered ({}us)", last_flush_us);
                    had_flush_error = false;
                }
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

        // ── Per-frame timing stats ──
        let frame_us = (Instant::now() - frame_start).as_micros() as u64;
        sum_render_us += render_us as u64;
        sum_flush_us += last_flush_us;
        sum_frame_us += frame_us;
        if frame_us > max_frame_us { max_frame_us = frame_us; }
        if frame_us < min_frame_us { min_frame_us = frame_us; }

        loop_frames = loop_frames.wrapping_add(1);
        let now = Instant::now();

        // ── Log expression transitions in real-time ──
        let expr_name = match state.expression {
            EyeExpression::Neutral => "Neutral",
            EyeExpression::Happy => "Happy",
            EyeExpression::Sad => "Sad",
            EyeExpression::Angry => "Angry",
            EyeExpression::Surprised => "Surprised",
            EyeExpression::Thinking => "Thinking",
            EyeExpression::Blink => "Blink",
        };
        if expr_name != last_expr_name {
            let src = if state.expression_override { "abi" } else { "idle" };
            log::info!(
                "[display] expression {} -> {} (src={} idx={})",
                last_expr_name, expr_name, src, state.idle_expr_idx,
            );
            last_expr_name = expr_name;
        }

        if now - last_loop_log >= Duration::from_secs(1) {
            let mode_name = match state.ui_mode {
                UiMode::Face => "face",
                UiMode::Menu => "menu",
                UiMode::MenuAction(_) => "action",
            };
            let now_ms = now.as_millis() as u64;
            let blink_remaining = if state.blink_end_ms > now_ms { state.blink_end_ms - now_ms } else { 0 };
            let next_expr_in = if state.next_expression_ms > now_ms { state.next_expression_ms - now_ms } else { 0 };
            let avg_frame = if loop_frames > 0 { sum_frame_us / loop_frames as u64 } else { 0 };
            let avg_render = if loop_frames > 0 { sum_render_us / loop_frames as u64 } else { 0 };
            let avg_flush = if loop_frames > 0 { sum_flush_us / loop_frames as u64 } else { 0 };
            let is_blinking = now_ms < state.blink_end_ms;
            let in_transition = now_ms < state.transition_end_ms;
            log::info!(
                "[display] fps={} expr={} mode={} override={} blinking={} transition={}",
                loop_frames,
                expr_name,
                mode_name,
                state.expression_override,
                is_blinking,
                in_transition,
            );
            log::info!(
                "[display] timing: avg_frame={}us avg_render={}us avg_flush={}us min_frame={}us max_frame={}us",
                avg_frame,
                avg_render,
                avg_flush,
                min_frame_us,
                max_frame_us,
            );
            log::info!(
                "[display] anim: look_x={} blink_in={}ms next_expr_in={}ms frame={} pgErr={}",
                state.eye_look_x,
                blink_remaining,
                next_expr_in,
                state.frame,
                last_page_errors,
            );
            // Reset per-second accumulators.
            loop_frames = 0;
            last_loop_log = now;
            max_frame_us = 0;
            min_frame_us = u64::MAX;
            sum_render_us = 0;
            sum_flush_us = 0;
            sum_frame_us = 0;
        }

        // 6. Sleep the remainder of the frame period.
        //    Yield at least once per frame so the executor can run other tasks
        //    (Wi-Fi, web server, button task).
        //    If we're already past the deadline (flush took too long), yield
        //    only 100µs so we don't hand off for seconds.
        {
            const POLL_INTERVAL: Duration = Duration::from_millis(10);
            let frame_deadline = frame_start + FRAME_PERIOD;
            if tracing {
                let remaining = if Instant::now() < frame_deadline {
                    (frame_deadline - Instant::now()).as_micros() as i64
                } else {
                    -((Instant::now() - frame_deadline).as_micros() as i64)
                };
                log::info!("[display] frame {} step=sleep remaining={}us",
                    total_frames, remaining);
            }
            if Instant::now() >= frame_deadline {
                // Already over budget — yield once briefly, then continue.
                let yield_start = Instant::now();
                Timer::after(Duration::from_micros(100)).await;
                let yield_us = (Instant::now() - yield_start).as_micros();
                if tracing {
                    log::info!("[display] frame {} yield_overbudget={}us",
                        total_frames, yield_us);
                }
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
                let mut sleep_count: u8 = 0;
                loop {
                    let yield_start = Instant::now();
                    Timer::after(POLL_INTERVAL).await;
                    let yield_us = (Instant::now() - yield_start).as_micros();
                    if tracing && sleep_count == 0 {
                        log::info!("[display] frame {} first_yield={}us (asked 10000us)",
                            total_frames, yield_us);
                    }
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
                    sleep_count = sleep_count.saturating_add(1);
                    if Instant::now() >= frame_deadline {
                        break;
                    }
                }
            }
        }

        total_frames += 1;

        // Heartbeat: prove the task is alive even without visible changes.
        if (Instant::now() - last_heartbeat) >= Duration::from_secs(5) {
            log::info!("[display] heartbeat frame={} starved={}", total_frames, starved);
            last_heartbeat = Instant::now();
        }
    }
}

// ── UI mode ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum UiMode {
    /// Full 128×64 animated face.
    Face,
    /// Split: 4-item menu on left 64 px, mini-face on right 64 px.
    Menu,
    /// Action result displayed on left panel after a long-press selection.
    MenuAction(u8),
}

// ── Button helpers ────────────────────────────────────────────────────────────

// ── UI state machine ──────────────────────────────────────────────────────────

const MENU_ITEMS: [&str; 4] = ["Talk", "Clock", "Web", "Help"];

fn handle_button_event(ev: ButtonEvent, state: &mut FaceState) {
    match ev {
        ButtonEvent::ShortPress => {
            log::info!("[display] BOOT short press");
            state.last_button_time = Instant::now();
            match state.ui_mode {
                UiMode::Face => {
                    state.ui_mode = UiMode::Menu;
                    state.menu_selected = 0;
                    log::info!("[display] mode -> Menu (selected: {})", MENU_ITEMS[0]);
                }
                UiMode::Menu => {
                    state.menu_selected = (state.menu_selected + 1) % 4;
                    log::info!(
                        "[display] menu next -> {}",
                        MENU_ITEMS[state.menu_selected as usize]
                    );
                }
                UiMode::MenuAction(_) => {
                    state.ui_mode = UiMode::Menu;
                    state.text.clear();
                    log::info!("[display] action dismissed -> Menu");
                }
            }
        }
        ButtonEvent::LongPress => {
            log::info!("[display] BOOT long press");
            state.last_button_time = Instant::now();
            match state.ui_mode {
                UiMode::Face => {
                    state.ui_mode = UiMode::Menu;
                    state.menu_selected = 0;
                    log::info!("[display] mode -> Menu (selected: {})", MENU_ITEMS[0]);
                }
                UiMode::Menu => {
                    apply_menu_action(state.menu_selected, state);
                    state.ui_mode = UiMode::MenuAction(state.menu_selected);
                    log::info!(
                        "[display] selected -> {}",
                        MENU_ITEMS[state.menu_selected as usize]
                    );
                }
                UiMode::MenuAction(_) => {
                    state.ui_mode = UiMode::Menu;
                    state.text.clear();
                    log::info!("[display] action dismissed -> Menu");
                }
            }
        }
    }
}

fn apply_menu_action(item: u8, state: &mut FaceState) {
    state.text.clear();
    match item {
        0 => {
            // Talk
            state.expression = EyeExpression::Thinking;
            let _ = state.text.push_str("...");
        }
        1 => {
            // Clock — show uptime
            state.expression = EyeExpression::Happy;
            let secs = Instant::now().as_millis() / 1000;
            format_uptime(secs, &mut state.text);
        }
        2 => {
            // Web server toggle
            if crate::web_server::is_web_server_enabled() {
                crate::web_server::disable_web_server();
                state.expression = EyeExpression::Neutral;
                let _ = state.text.push_str("Web: OFF");
            } else {
                crate::web_server::enable_web_server();
                state.expression = EyeExpression::Happy;
                let _ = state.text.push_str("Web: ON");
            }
        }
        3 => {
            // Help
            state.expression = EyeExpression::Neutral;
            let _ = state.text.push_str("Pr:nxt Hld:sel");
        }
        _ => {}
    }
}

/// Format `secs` as "Up: NNNs" into `out` without heap allocation.
fn format_uptime(secs: u64, out: &mut heapless::String<64>) {
    let _ = out.push_str("Up: ");
    let mut buf = [0u8; 10];
    let mut n = secs;
    let mut len = 0usize;
    if n == 0 {
        buf[0] = b'0';
        len = 1;
    } else {
        while n > 0 {
            buf[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
        buf[..len].reverse();
    }
    for &b in &buf[..len] {
        let _ = out.push(b as char);
    }
    let _ = out.push('s');
}

// ── Face state ────────────────────────────────────────────────────────────────

struct FaceState {
    expression: EyeExpression,
    text: heapless::String<64>,
    brightness: u8,
    frame: u32,
    /// Set to `true` when expression was set via `DisplayCommand::SetExpression`.
    expression_override: bool,
    /// Timestamp of last ABI expression override.
    expression_override_since: Option<Instant>,
    ui_mode: UiMode,
    menu_selected: u8,
    /// Updated on every button event; used for the 10-second inactivity timeout.
    last_button_time: Instant,
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
            text: heapless::String::new(),
            brightness: 255,
            frame: 0,
            expression_override: false,
            expression_override_since: None,
            ui_mode: UiMode::Face,
            menu_selected: 0,
            last_button_time: Instant::from_ticks(0),
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

// ── Render dispatcher ─────────────────────────────────────────────────────────

fn render_frame(oled: &mut OledDisplay, state: &mut FaceState) {
    match state.ui_mode {
        UiMode::Face => render_full_face(oled, state),
        _ => render_menu_mode(oled, state),
    }
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
        log::info!("[display] blink START (300ms, next in {}ms)", next_gap);
    }

    // --- Idle expression cycling (random 3–6 s, brief transition) ---
    if !state.expression_override && now_ms >= state.next_expression_ms {
        state.transition_end_ms = now_ms + 120; // eyes close 120 ms
        let prev_idx = state.idle_expr_idx;
        state.idle_expr_idx = (state.idle_expr_idx + 1) % IDLE_EXPRESSIONS.len() as u8;
        state.expression = IDLE_EXPRESSIONS[state.idle_expr_idx as usize];
        let next_gap = state.rng.range(3000, 6500) as u64;
        state.next_expression_ms = now_ms + next_gap;
        log::info!(
            "[display] idle cycle idx {} -> {} (next in {}ms)",
            prev_idx, state.idle_expr_idx, next_gap,
        );
    }

    // --- Eye drift (gentle horizontal pupil shift every 1–3 s) ---
    if now_ms >= state.next_look_ms {
        let prev_x = state.eye_look_x;
        state.eye_look_x = (state.rng.next() % 5) as i32 - 2; // -2..+2
        state.next_look_ms = now_ms + state.rng.range(1000, 3000) as u64;
        if prev_x != state.eye_look_x {
            log::info!("[display] eye drift {} -> {}", prev_x, state.eye_look_x);
        }
    }
}

/// Smooth sine-ish breathing bob (triangle wave approximation, ±2 px).
fn breathing_offset(now_ms: u64) -> i32 {
    // 2.4 s cycle ; triangle wave 0→2→0→-2→0
    let phase = (now_ms % 2400) as i32; // 0..2399
    if phase < 600 {
        (phase * 2) / 600           // 0 → 1 (we use 0→2 below)
    } else if phase < 1200 {
        2 - ((phase - 600) * 2) / 600 // 2 → 0
    } else if phase < 1800 {
        -(((phase - 1200) * 2) / 600)  // 0 → -2
    } else {
        -2 + ((phase - 1800) * 2) / 600 // -2 → 0
    }
}

/// Render the full 128×64 kawaii robot face.
fn render_full_face(oled: &mut OledDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    let bob_y = breathing_offset(now_ms);
    let in_transition = now_ms < state.transition_end_ms;
    let blinking = now_ms < state.blink_end_ms || in_transition;

    oled.clear(BinaryColor::Off);

    let fill_on = PrimitiveStyle::with_fill(BinaryColor::On);
    let fill_off = PrimitiveStyle::with_fill(BinaryColor::Off);
    let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let stroke2 = PrimitiveStyle::with_stroke(BinaryColor::On, 2);

    // ── Face-plate (rounded rectangle visor) ──
    let plate_y = 8 + bob_y;
    let _ = RoundedRectangle::new(
        Rectangle::new(Point::new(18, plate_y), Size::new(92, 38)),
        CornerRadii::new(Size::new(11, 11)),
    )
    .into_styled(stroke2)
    .draw(oled);

    // ── Antenna nubs ──
    let _ = Line::new(Point::new(46, plate_y), Point::new(46, plate_y - 3))
        .into_styled(stroke)
        .draw(oled);
    let _ = Circle::new(Point::new(44, plate_y - 5), 5)
        .into_styled(fill_on)
        .draw(oled);
    let _ = Line::new(Point::new(82, plate_y), Point::new(82, plate_y - 3))
        .into_styled(stroke)
        .draw(oled);
    let _ = Circle::new(Point::new(80, plate_y - 5), 5)
        .into_styled(fill_on)
        .draw(oled);

    // ── Eyes ──
    let eye_cy = 23 + bob_y;
    let le_cx = 44 + state.eye_look_x; // left-eye centre X
    let re_cx = 84 + state.eye_look_x; // right-eye centre X

    if blinking {
        // Blink: horizontal lines (kawaii closed eyes: ─ ─)
        let _ = Line::new(Point::new(le_cx - 7, eye_cy), Point::new(le_cx + 7, eye_cy))
            .into_styled(stroke2)
            .draw(oled);
        let _ = Line::new(Point::new(re_cx - 7, eye_cy), Point::new(re_cx + 7, eye_cy))
            .into_styled(stroke2)
            .draw(oled);
    } else {
        match state.expression {
            EyeExpression::Happy => {
                // Happy: upward arcs (＾ ＾)
                let _ = Line::new(Point::new(le_cx - 7, eye_cy + 2), Point::new(le_cx, eye_cy - 4))
                    .into_styled(stroke2).draw(oled);
                let _ = Line::new(Point::new(le_cx, eye_cy - 4), Point::new(le_cx + 7, eye_cy + 2))
                    .into_styled(stroke2).draw(oled);
                let _ = Line::new(Point::new(re_cx - 7, eye_cy + 2), Point::new(re_cx, eye_cy - 4))
                    .into_styled(stroke2).draw(oled);
                let _ = Line::new(Point::new(re_cx, eye_cy - 4), Point::new(re_cx + 7, eye_cy + 2))
                    .into_styled(stroke2).draw(oled);
            }
            EyeExpression::Sad => {
                // Sad: half-closed droopy eyes
                let r = 7i32;
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                // Cut upper half as "droopy eyelid"
                let _ = Rectangle::new(Point::new(le_cx - r, eye_cy - r - 1), Size::new((r * 2) as u32, (r + 2) as u32))
                    .into_styled(fill_off).draw(oled);
                let _ = Rectangle::new(Point::new(re_cx - r, eye_cy - r - 1), Size::new((r * 2) as u32, (r + 2) as u32))
                    .into_styled(fill_off).draw(oled);
                // Angled brow line
                let _ = Line::new(Point::new(le_cx - 7, eye_cy - 3), Point::new(le_cx + 5, eye_cy - 1))
                    .into_styled(stroke).draw(oled);
                let _ = Line::new(Point::new(re_cx - 5, eye_cy - 1), Point::new(re_cx + 7, eye_cy - 3))
                    .into_styled(stroke).draw(oled);
            }
            EyeExpression::Angry => {
                // Angry: V-shaped brows + narrowed eyes
                let r = 6i32;
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                // Angry brows
                let _ = Line::new(Point::new(le_cx - 8, eye_cy - 8), Point::new(le_cx + 5, eye_cy - 4))
                    .into_styled(stroke2).draw(oled);
                let _ = Line::new(Point::new(re_cx - 5, eye_cy - 4), Point::new(re_cx + 8, eye_cy - 8))
                    .into_styled(stroke2).draw(oled);
            }
            EyeExpression::Surprised => {
                // Surprised: open circle eyes
                let r = 9i32;
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(stroke2).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(stroke2).draw(oled);
                // Pupils
                let _ = Circle::new(Point::new(le_cx - 2, eye_cy - 2), 4)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - 2, eye_cy - 2), 4)
                    .into_styled(fill_on).draw(oled);
                // Highlight
                let _ = Circle::new(Point::new(le_cx + 2, eye_cy - 5), 3)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx + 2, eye_cy - 5), 3)
                    .into_styled(fill_on).draw(oled);
            }
            EyeExpression::Thinking => {
                // Thinking: one eye normal, one half closed + look-up pupils
                let r = 7i32;
                // Left eye normal
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                // Highlight dot
                let _ = Circle::new(Point::new(le_cx + 2, eye_cy - 4), 3)
                    .into_styled(fill_off).draw(oled);
                // Right eye half-closed (line)
                let _ = Line::new(Point::new(re_cx - 7, eye_cy), Point::new(re_cx + 7, eye_cy))
                    .into_styled(stroke2).draw(oled);
            }
            _ => {
                // Neutral: filled circle eyes with highlight dot (kawaii default)
                let r = 7i32;
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                // Cute highlight (small off-circle in upper-right of each eye)
                let _ = Circle::new(Point::new(le_cx + 2, eye_cy - 5), 3)
                    .into_styled(fill_off).draw(oled);
                let _ = Circle::new(Point::new(re_cx + 2, eye_cy - 5), 3)
                    .into_styled(fill_off).draw(oled);
            }
        }
    }

    // ── Mouth ──
    let mouth_cy = 37 + bob_y;
    draw_mouth(oled, state.expression, 64, mouth_cy, 1, now_ms);

    // ── Status text overlay ──
    if !state.text.is_empty() {
        let text_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        let _ = Rectangle::new(Point::new(0, 54), Size::new(DISPLAY_WIDTH, 10))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
            .draw(oled);
        let _ = Text::new(&state.text, Point::new(0, 63), text_style).draw(oled);
    }
}

// ── Shared mouth drawing ──────────────────────────────────────────────────────

/// Draw the mouth centred at (`cx`, `cy`).  `scale` 1 = full size, 0 = mini.
fn draw_mouth(
    oled: &mut OledDisplay,
    expr: EyeExpression,
    cx: i32,
    cy: i32,
    scale: i32,
    now_ms: u64,
) {
    let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let stroke2 = PrimitiveStyle::with_stroke(BinaryColor::On, if scale > 0 { 2 } else { 1 });
    let fill_on = PrimitiveStyle::with_fill(BinaryColor::On);
    let hw = 11 + scale * 4; // half-width
    let hh = 3 + scale * 2;  // half-height

    match expr {
        EyeExpression::Happy => {
            // Wide smile: V shape, thicker
            let _ = Line::new(Point::new(cx - hw, cy - 1), Point::new(cx, cy + hh))
                .into_styled(stroke2).draw(oled);
            let _ = Line::new(Point::new(cx, cy + hh), Point::new(cx + hw, cy - 1))
                .into_styled(stroke2).draw(oled);
        }
        EyeExpression::Sad => {
            // Frown: inverted V
            let _ = Line::new(Point::new(cx - hw + 4, cy + 2), Point::new(cx, cy - hh + 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx, cy - hh + 2), Point::new(cx + hw - 4, cy + 2))
                .into_styled(stroke).draw(oled);
        }
        EyeExpression::Angry => {
            // Gritted teeth: small rectangle with vertical lines
            let w = (hw * 2 - 4) as u32;
            let _ = Rectangle::new(Point::new(cx - hw + 2, cy - 2), Size::new(w, 5))
                .into_styled(fill_on).draw(oled);
            // Teeth lines (black vertical bars inside the white rect)
            let fill_off = PrimitiveStyle::with_fill(BinaryColor::Off);
            for i in 0..4 {
                let tx = cx - hw + 6 + i * ((hw * 2 - 8) / 4);
                let _ = Rectangle::new(Point::new(tx, cy - 1), Size::new(1, 3))
                    .into_styled(fill_off).draw(oled);
            }
        }
        EyeExpression::Surprised => {
            // Small open "O" mouth
            let r = (3 + scale * 2) as u32;
            let _ = Circle::new(Point::new(cx - r as i32, cy - r as i32), r * 2)
                .into_styled(stroke2).draw(oled);
        }
        EyeExpression::Thinking => {
            // Squiggly/wavy line
            let _ = Line::new(Point::new(cx - 8, cy), Point::new(cx - 3, cy - 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx - 3, cy - 2), Point::new(cx + 3, cy + 2))
                .into_styled(stroke).draw(oled);
            let _ = Line::new(Point::new(cx + 3, cy + 2), Point::new(cx + 8, cy))
                .into_styled(stroke).draw(oled);
        }
        _ => {
            // Neutral: gentle wobbling line
            let wobble = if (now_ms % 1000) < 500 { 0 } else { 1 };
            let _ = Line::new(
                Point::new(cx - hw + 4, cy + wobble),
                Point::new(cx + hw - 4, cy + wobble),
            )
            .into_styled(stroke)
            .draw(oled);
        }
    }
}

// ── Menu mode rendering ───────────────────────────────────────────────────────

/// Render the split-screen menu mode (menu left, mini-face right).
fn render_menu_mode(oled: &mut OledDisplay, state: &mut FaceState) {
    state.frame = state.frame.wrapping_add(1);
    let now_ms = Instant::now().as_millis() as u64;
    tick_animation(state, now_ms);

    oled.clear(BinaryColor::Off);

    // Vertical divider at x=63.
    let divider_style = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let _ = Line::new(Point::new(63, 0), Point::new(63, 63))
        .into_styled(divider_style)
        .draw(oled);

    render_menu_panel(oled, state);
    render_mini_face(oled, state);
}

/// Draw the left 63-px menu panel.
fn render_menu_panel(oled: &mut OledDisplay, state: &FaceState) {
    let normal_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let inverted_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::Off);

    if let UiMode::MenuAction(_) = state.ui_mode {
        // Show the action result text and a dismiss hint.
        render_action_text(oled, &state.text);
        return;
    }

    for (i, &label) in MENU_ITEMS.iter().enumerate() {
        let y_top = (i as i32) * 16;
        if i == state.menu_selected as usize {
            // Filled highlight: white rectangle, black text.
            let _ = Rectangle::new(Point::new(0, y_top), Size::new(63, 16))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(oled);
            let _ = Text::new(label, Point::new(4, y_top + 12), inverted_style).draw(oled);
        } else {
            let _ = Text::new(label, Point::new(4, y_top + 12), normal_style).draw(oled);
        }
    }
}

/// Render action result text in the left panel; chunk into 10-char lines.
fn render_action_text(oled: &mut OledDisplay, text: &heapless::String<64>) {
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let bytes = text.as_bytes();
    let mut y = 11i32;
    for chunk in bytes.chunks(10) {
        if y > 42 {
            break;
        }
        if let Ok(line) = core::str::from_utf8(chunk) {
            let _ = Text::new(line, Point::new(2, y), style).draw(oled);
        }
        y += 14;
    }
    // Dismiss hint at bottom.
    let hint = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::new(">press", Point::new(2, 60), hint).draw(oled);
}

/// Draw the mini animated face on the right 64-px panel (x=64..127).
fn render_mini_face(oled: &mut OledDisplay, state: &FaceState) {
    let now_ms = Instant::now().as_millis() as u64;
    let blinking = now_ms < state.blink_end_ms || now_ms < state.transition_end_ms;

    let bob_y = breathing_offset(now_ms) / 2; // subtler in mini
    let fill_on = PrimitiveStyle::with_fill(BinaryColor::On);
    let fill_off = PrimitiveStyle::with_fill(BinaryColor::Off);
    let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    // Mini visor
    let plate_y = 6 + bob_y;
    let _ = RoundedRectangle::new(
        Rectangle::new(Point::new(67, plate_y), Size::new(56, 36)),
        CornerRadii::new(Size::new(8, 8)),
    )
    .into_styled(stroke)
    .draw(oled);

    let le_cx = 82 + state.eye_look_x / 2;
    let re_cx = 108 + state.eye_look_x / 2;
    let eye_cy = 20 + bob_y;
    let r = 5i32;

    if blinking {
        let _ = Line::new(Point::new(le_cx - r, eye_cy), Point::new(le_cx + r, eye_cy))
            .into_styled(stroke).draw(oled);
        let _ = Line::new(Point::new(re_cx - r, eye_cy), Point::new(re_cx + r, eye_cy))
            .into_styled(stroke).draw(oled);
    } else {
        match state.expression {
            EyeExpression::Happy => {
                // Mini happy arcs
                let _ = Line::new(Point::new(le_cx - 5, eye_cy + 1), Point::new(le_cx, eye_cy - 3))
                    .into_styled(stroke).draw(oled);
                let _ = Line::new(Point::new(le_cx, eye_cy - 3), Point::new(le_cx + 5, eye_cy + 1))
                    .into_styled(stroke).draw(oled);
                let _ = Line::new(Point::new(re_cx - 5, eye_cy + 1), Point::new(re_cx, eye_cy - 3))
                    .into_styled(stroke).draw(oled);
                let _ = Line::new(Point::new(re_cx, eye_cy - 3), Point::new(re_cx + 5, eye_cy + 1))
                    .into_styled(stroke).draw(oled);
            }
            EyeExpression::Angry => {
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Line::new(Point::new(le_cx - 6, eye_cy - 7), Point::new(le_cx + 4, eye_cy - 3))
                    .into_styled(stroke).draw(oled);
                let _ = Line::new(Point::new(re_cx - 4, eye_cy - 3), Point::new(re_cx + 6, eye_cy - 7))
                    .into_styled(stroke).draw(oled);
            }
            EyeExpression::Surprised => {
                let _ = Circle::new(Point::new(le_cx - r - 1, eye_cy - r - 1), ((r + 1) * 2) as u32)
                    .into_styled(stroke).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r - 1, eye_cy - r - 1), ((r + 1) * 2) as u32)
                    .into_styled(stroke).draw(oled);
                let _ = Circle::new(Point::new(le_cx - 2, eye_cy - 2), 4)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - 2, eye_cy - 2), 4)
                    .into_styled(fill_on).draw(oled);
            }
            EyeExpression::Thinking => {
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(le_cx + 1, eye_cy - 4), 3)
                    .into_styled(fill_off).draw(oled);
                let _ = Line::new(Point::new(re_cx - 5, eye_cy), Point::new(re_cx + 5, eye_cy))
                    .into_styled(stroke).draw(oled);
            }
            _ => {
                // Neutral mini eyes with highlight
                let _ = Circle::new(Point::new(le_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(re_cx - r, eye_cy - r), (r * 2) as u32)
                    .into_styled(fill_on).draw(oled);
                let _ = Circle::new(Point::new(le_cx + 1, eye_cy - 4), 3)
                    .into_styled(fill_off).draw(oled);
                let _ = Circle::new(Point::new(re_cx + 1, eye_cy - 4), 3)
                    .into_styled(fill_off).draw(oled);
            }
        }
    }

    // Mini mouth (reuse shared helper)
    draw_mouth(oled, state.expression, 95, 34 + bob_y, 0, now_ms);
}

// ── SSD1306 low-level driver ──────────────────────────────────────────────────

struct OledDisplay {
    i2c: I2c<'static, Async>,
    buffer: [u8; DISPLAY_BUF_SIZE],
}

impl OledDisplay {
    fn new(i2c: I2c<'static, Async>) -> Self {
        Self {
            i2c,
            buffer: [0; DISPLAY_BUF_SIZE],
        }
    }

    /// SSD1306 init + GDDRAM clear.
    ///
    /// Works correctly even after a soft-reset (Ctrl+R) where the SSD1306
    /// retains stale GDDRAM content and an unpredictable internal pointer.
    async fn init(&mut self) {
        // Single transaction: display-off, full config, address window reset.
        // The SSD1306 processes chained commands via control byte 0x00.
        // Uses page addressing mode (the default) which works on both
        // SSD1306 and SH1106 controllers.  Column/page are set per-page
        // in flush_page() instead of relying on horizontal auto-increment.
        let init_cmds: [u8; 25] = [
            0x00, // Control byte: Co=0, D/C#=0 → command stream
            0xAE, // Display OFF
            0xD5, 0x80, // Clock divide ratio / oscillator freq
            0xA8, 0x3F, // Multiplex ratio = 64
            0xD3, 0x00, // Display offset = 0
            0x40, // Start line = 0
            0x8D, 0x14, // Charge pump ON
            0x20, 0x02, // Page addressing mode (compatible with SH1106)
            0xA1, // Segment re-map (col 127 = SEG0)
            0xC8, // COM scan direction remapped
            0xDA, 0x12, // COM pins hardware config
            0x81, 0xCF, // Contrast
            0xD9, 0xF1, // Pre-charge period
            0xDB, 0x40, // VCOMH deselect level
            0xA4, // Resume from RAM content
            0xA6, // Normal display (not inverted)
        ];
        match self.i2c.write(SSD1306_ADDR, &init_cmds).await {
            Ok(()) => log::info!("[display] SSD1306 init cmds OK"),
            Err(e) => log::error!("[display] SSD1306 init cmds FAILED: {:?}", e),
        }

        // Brief async delay (~1 ms) for the SSD1306 to process the init batch.
        Timer::after(Duration::from_millis(1)).await;

        // Clear GDDRAM (writes 1024 zero bytes).  The address window was
        // just reset above so the pointer starts at page 0, column 0.
        self.buffer.fill(0x00);
        match self.flush().await {
            Ok(()) => log::info!("[display] GDDRAM clear OK"),
            Err(()) => log::error!("[display] GDDRAM clear FAILED"),
        }

        // Display ON.
        match self.i2c.write(SSD1306_ADDR, &[0x00, 0xAF]).await {
            Ok(()) => log::info!("[display] display ON cmd OK"),
            Err(e) => log::error!("[display] display ON cmd FAILED: {:?}", e),
        }
    }

    async fn set_contrast(&mut self, value: u8) {
        let _ = self.i2c.write(SSD1306_ADDR, &[0x00, 0x81, value]).await;
    }

    #[allow(dead_code)]
    async fn cmd(&mut self, cmd: u8) {
        let _ = self.i2c.write(SSD1306_ADDR, &[0x00, cmd]).await;
    }

    /// Push the entire 1024-byte framebuffer to the display.
    ///
    /// The CPU yields to other Embassy tasks during each I2C transfer so
    /// Wi-Fi, the web server and the button task all continue to run.
    async fn flush(&mut self) -> Result<(), ()> {
        let mut all_ok = true;
        for page in 0..8 {
            if !self.flush_page(page).await {
                all_ok = false;
            }
        }
        if all_ok { Ok(()) } else { Err(()) }
    }

    /// Write one page (128 bytes, i.e. 8 rows) to the display.
    ///
    /// Sets the page address and column explicitly before each write,
    /// which works on both SSD1306 (any addressing mode) and SH1106.
    /// Retries once on failure to handle transient bus glitches.
    ///
    /// `page` must be in `0..8`.  Returns `true` on success.
    async fn flush_page(&mut self, page: usize) -> bool {
        for _attempt in 0..2 {
            if self.flush_page_inner(page).await {
                return true;
            }
        }
        false
    }

    async fn flush_page_inner(&mut self, page: usize) -> bool {
        // Set page address + column 0.
        // 0xB0|page = page address; 0x00 = lower column nibble;
        // 0x10 = upper column nibble → column 0.
        let page_cmd = 0xB0 | (page as u8);
        if self
            .i2c
            .write(SSD1306_ADDR, &[0x00, page_cmd, 0x00, 0x10])
            .await
            .is_err()
        {
            return false;
        }

        // Write 128 data bytes for this page.
        let mut packet = [0u8; 129]; // 1 control byte + 128 data bytes
        packet[0] = 0x40;
        let start = page * 128;
        let end = (start + 128).min(self.buffer.len());
        let chunk = &self.buffer[start..end];
        packet[1..1 + chunk.len()].copy_from_slice(chunk);
        self.i2c
            .write(SSD1306_ADDR, &packet[..1 + chunk.len()])
            .await
            .is_ok()
    }
}

impl OriginDimensions for OledDisplay {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for OledDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let fill_byte = match color {
            BinaryColor::On => 0xFF,
            BinaryColor::Off => 0x00,
        };
        self.buffer.fill(fill_byte);
        Ok(())
    }

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if coord.x < 0 || coord.y < 0 {
                continue;
            }
            let x = coord.x as usize;
            let y = coord.y as usize;

            if x >= DISPLAY_WIDTH as usize || y >= DISPLAY_HEIGHT as usize {
                continue;
            }

            let idx = x + (y / 8) * DISPLAY_WIDTH as usize;
            let bit = 1u8 << (y % 8);
            match color {
                BinaryColor::On => self.buffer[idx] |= bit,
                BinaryColor::Off => self.buffer[idx] &= !bit,
            }
        }
        Ok(())
    }
}
