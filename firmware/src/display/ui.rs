use abi::EyeExpression;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, RoundedRectangle, Rectangle, CornerRadii, Arc},
    text::Text,
    mono_font::{ascii::{FONT_6X10, FONT_10X20}, MonoTextStyle},
};
use embassy_time::Instant;
use heapless::String;

#[derive(Clone, Copy, PartialEq)]
pub enum UiState {
    Idle,
    Menu,
    SettingsMenu,
    Metrics,
    ToolsMenu,
    InfoMenu,
    Clock,
    Pomodoro,
}

// Landscape display: 320 wide × 240 tall
const SCREEN_W: i32 = 320;
const SCREEN_H: i32 = 240;

// Menu layout constants
const MENU_ITEM_WIDTH: i32 = 120;
const MENU_ITEM_HEIGHT: i32 = 36;
const MENU_ITEM_SPACING: i32 = 6;
const MENU_START_Y: i32 = 20;
const MENU_HIDE_OFFSET: i32 = -160;
const MENU_SHOW_OFFSET: i32 = 8;
const MENU_CORNER_RADIUS: u32 = 12;

// R-Kun positions (landscape)
const RKUN_CENTER_X: i32 = 160;  // center of 320
const RKUN_CENTER_Y: i32 = 120;  // center of 240
const RKUN_MENU_X: i32 = 240;    // shifted right when menu is open

// Sine-approximation LUT for smooth breathing
const SINE_LUT: [i32; 8] = [0, 1, 2, 3, 3, 2, 1, 0];

pub struct UiManager {
    pub state: UiState,
    pub expression: EyeExpression,
    pub prev_expression: EyeExpression,
    pub frame_count: u32,
    pub menu_offset: i32,
    pub slide_target: i32,
    pub idle_bounce: i32,
    pub is_blinking: bool,
    pub prev_is_blinking: bool,

    // Delta-time fields
    pub dt_ms: u32,         // milliseconds since last frame
    pub elapsed_ms: u64,    // total elapsed time (for periodic animations)
    pub last_interaction_time: Option<Instant>,

    // R-Kun properties
    pub r_kun_x: i32,
    pub r_kun_y: i32,
    pub prev_r_kun_x: i32,
    pub prev_r_kun_y: i32,
    pub prev_idle_bounce: i32,

    // Menu
    pub selected_menu_item: usize,
    pub menu_scroll_index: usize,
    pub prev_menu_scroll_index: usize,
    pub tools_button_pop: [i32; 4],
    pub info_button_pop: [i32; 3],
    pub pomodoro_button_pop: [i32; 4],
    pub flashlight_on: bool,
    pub party_mode_on: bool,
    pub pomodoro_ms: u32,
    pub pomodoro_running: bool,
    pub button_pop: [i32; 4],
    pub prev_menu_offset: i32,

    // Tactile Feedback — simple: draw one ring, erase it next frame
    pub ripple_x: i32,
    pub ripple_y: i32,
    pub ripple_radius: i32,
    pub ripple_active: bool,
    pub ripple_dirty: bool, // true = there are ripple pixels on screen that need erasing

    // FPS tracking
    pub last_frame_time: Option<Instant>,
    pub fps: u16,
    pub fps_accum: u32,
    pub fps_frame_count: u16,

    // Status
    pub text: String<64>,
    pub prev_text: String<64>,
    pub force_redraw: bool,

    // Settings sub-menu
    pub selected_settings_item: usize,
    pub settings_button_pop: [i32; 1],

    // Metrics data (refreshed each frame from atomics)
    pub metrics_scroll_y: i32,
    pub metrics: MetricsData,
}

/// Snapshot of system metrics displayed on the Metrics screen.
#[derive(Clone, Copy)]
pub struct MetricsData {
    pub uptime_secs: u64,
    pub heap_free: usize,
    pub heap_used: usize,
    pub psram_total: usize,
    pub psram_free: usize,
    pub sram_total: usize,
    pub sram_free: usize,
    pub cpu0_pct: u32,
    pub cpu1_pct: u32,
    pub battery_mv: u32,
    pub wifi_status: u8,
    pub wifi_ip: Option<[u8; 4]>,
    pub temp_tenths: u32,
    pub fps: u16,
}

impl MetricsData {
    pub const fn new() -> Self {
        Self {
            uptime_secs: 0,
            heap_free: 0,
            heap_used: 0,
            psram_total: 0,
            psram_free: 0,
            sram_total: 0,
            sram_free: 0,
            cpu0_pct: 0,
            cpu1_pct: 0,
            battery_mv: 0,
            wifi_status: 0,
            wifi_ip: None,
            temp_tenths: 0,
            fps: 0,
        }
    }

    /// Refresh metrics from global atomics / system calls.
    pub fn refresh(&mut self, fps: u16) {
        self.uptime_secs = Instant::now().as_millis() / 1000;

        // General heap info (combined SRAM/PSRAM by esp_alloc)
        self.heap_free = esp_alloc::HEAP.free();
        self.heap_used = esp_alloc::HEAP.used();

        // Approximate split based on known heap allocation in main.rs:
        // SRAM is roughly 72KB, rest is PSRAM
        self.sram_total = 72 * 1024;
        self.psram_total = (self.heap_free + self.heap_used).saturating_sub(self.sram_total);

        // Best effort: assume SRAM is filled first or we show proportions. Since we can't get
        // exact split from esp_alloc easily, we provide total/used as global and estimated free for each.
        // Actually, for display purposes we will just split proportionally or display total,
        // but it's better to show exact if possible. Since we can't, we show Total.
        self.sram_free = (self.heap_free.min(self.sram_total)); // simple estimation
        self.psram_free = self.heap_free.saturating_sub(self.sram_free);

        // CPU Usage
        self.cpu0_pct = crate::cpu_usage::CORE0_USAGE_PCT.load(portable_atomic::Ordering::Relaxed);
        self.cpu1_pct = crate::cpu_usage::CORE1_USAGE_PCT.load(portable_atomic::Ordering::Relaxed);

        self.battery_mv = crate::battery::BATTERY_VOLTAGE_MV.load(portable_atomic::Ordering::Relaxed);
        self.wifi_status = crate::wifi::wifi_status();
        self.wifi_ip = crate::wifi::wifi_ipv4();
        self.temp_tenths = crate::ulp::last_temp_tenths();
        self.fps = fps;
    }
}

impl UiManager {
    pub fn new() -> Self {
        Self {
            state: UiState::Idle,
            expression: EyeExpression::Neutral,
            prev_expression: EyeExpression::Neutral,
            frame_count: 0,
            menu_offset: MENU_HIDE_OFFSET,
            slide_target: 0,
            idle_bounce: 0,
            is_blinking: false,
            prev_is_blinking: false,
            dt_ms: 10,
            elapsed_ms: 0,
            last_interaction_time: None,
            r_kun_x: RKUN_CENTER_X,
            r_kun_y: RKUN_CENTER_Y,
            prev_r_kun_x: RKUN_CENTER_X,
            prev_r_kun_y: RKUN_CENTER_Y,
            prev_idle_bounce: 0,
            selected_menu_item: 0,
            menu_scroll_index: 0,
            prev_menu_scroll_index: 0,
            tools_button_pop: [0; 4],
            info_button_pop: [0; 3],
            pomodoro_button_pop: [0; 4],
            flashlight_on: false,
            party_mode_on: false,
            pomodoro_ms: 25 * 60 * 1000,
            pomodoro_running: false,
            button_pop: [0; 4],
            prev_menu_offset: MENU_HIDE_OFFSET,
            ripple_x: 0,
            ripple_y: 0,
            ripple_radius: 0,
            ripple_active: false,
            ripple_dirty: false,
            last_frame_time: None,
            fps: 0,
            fps_accum: 0,
            fps_frame_count: 0,
            text: String::new(),
            prev_text: String::new(),
            force_redraw: true,
            selected_settings_item: 0,
            settings_button_pop: [0; 1],
            metrics_scroll_y: 0,
            metrics: MetricsData::new(),
        }
    }
}

use embedded_graphics::primitives::Ellipse;
use embedded_graphics::geometry::{Point, Size};

impl UiManager {
    pub fn update(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);

        // ── Delta time ─────────────────────────────────────────────────────
        let now = Instant::now();
        if let Some(prev) = self.last_frame_time {
            let elapsed_us = now.duration_since(prev).as_micros() as u32;
            self.dt_ms = (elapsed_us / 1000).max(1); // at least 1 ms
            self.elapsed_ms += self.dt_ms as u64;

            // FPS measurement: average over 10 frames
            self.fps_accum += elapsed_us;
            self.fps_frame_count += 1;
            if self.fps_frame_count >= 10 {
                let avg_us = self.fps_accum / self.fps_frame_count as u32;
                self.fps = if avg_us > 0 { (1_000_000 / avg_us) as u16 } else { 0 };
                self.fps_accum = 0;
                self.fps_frame_count = 0;
            }
        }
        self.last_frame_time = Some(now);

        let dt = self.dt_ms;
        if self.party_mode_on {
            let r = ((self.elapsed_ms / 10) % 255) as u8;
            let g = ((self.elapsed_ms / 15) % 255) as u8;
            let b = ((self.elapsed_ms / 20) % 255) as u8;
            unsafe {
                if let Some(led) = crate::rgb_led::RGB_LED.as_mut() {
                    led.set_color(r, g, b);
                }
            }
        }
        if self.pomodoro_running {
            self.pomodoro_ms = self.pomodoro_ms.saturating_sub(dt);
            if self.pomodoro_ms == 0 {
                self.pomodoro_running = false;
                crate::audio::play_blip(); // Alert when done
            }
            if self.state == UiState::Pomodoro {
                self.force_redraw = true;
            }
        }


        // ── Periodic animations (time-based) ───────────────────────────────
        if self.state == UiState::Idle {
            // Smooth breathing: 2-second cycle via sine-approximation LUT
            let cycle_ms = 2000u64;
            let cycle_pos = (self.elapsed_ms % cycle_ms) as usize;
            let lut_index = cycle_pos * 8 / cycle_ms as usize;
            self.idle_bounce = SINE_LUT[lut_index.min(7)];

            // Subtle eye drift every ~4 seconds
            let prev_bucket = self.elapsed_ms.saturating_sub(dt as u64) / 4000;
            let curr_bucket = self.elapsed_ms / 4000;
            if curr_bucket != prev_bucket {
                let variation = (curr_bucket % 3) as i32 * 2 - 2;
                self.r_kun_x = RKUN_CENTER_X + variation;
            }
        } else {
            self.idle_bounce = 0;
        }

        // Blink: 166 ms blink every 10 seconds
        let blink_cycle = 10_000u64;
        self.is_blinking = (self.elapsed_ms % blink_cycle) < 166;

        // ── Ripple: 300 pixels/sec ─────────────────────────────────────────
        if self.ripple_active {
            self.ripple_radius += (300 * dt as i32) / 1000;
            if self.ripple_radius > 40 {
                self.ripple_active = false;
            }
        }

        // ── Global Pop Animation Decay ─────────────────────────────────────
        let pop_decay = (dt as i32 * 100 / 1000).max(1);
        for pop in &mut self.button_pop { if *pop > 0 { *pop = (*pop - pop_decay).max(0); } }
        for pop in &mut self.settings_button_pop { if *pop > 0 { *pop = (*pop - pop_decay).max(0); } }
        for pop in &mut self.tools_button_pop { if *pop > 0 { *pop = (*pop - pop_decay).max(0); } }
        for pop in &mut self.info_button_pop { if *pop > 0 { *pop = (*pop - pop_decay).max(0); } }
        for pop in &mut self.pomodoro_button_pop { if *pop > 0 { *pop = (*pop - pop_decay).max(0); } }

        // ── State machine (dt-scaled easing) ───────────────────────────────
        match self.state {
            UiState::Menu => {
                // Slide R-Kun right — exponential ease: diff * dt / 30
                if self.r_kun_x < RKUN_MENU_X {
                    let diff = RKUN_MENU_X - self.r_kun_x;
                    let step = (diff * dt as i32 / 30).max(1);
                    self.r_kun_x = (self.r_kun_x + step).min(RKUN_MENU_X);
                }

                // Slide menu in
                if self.menu_offset < MENU_SHOW_OFFSET {
                    let diff = MENU_SHOW_OFFSET - self.menu_offset;
                    let step = (diff * dt as i32 / 20).max(1);
                    self.menu_offset = (self.menu_offset + step).min(MENU_SHOW_OFFSET);
                }

                // Timeout return to idle (10 seconds wall-clock)
                if let Some(last) = self.last_interaction_time {
                    if now.duration_since(last).as_millis() >= 10_000 {
                        self.state = UiState::Idle;
                    }
                }

            }
            UiState::SettingsMenu | UiState::ToolsMenu | UiState::InfoMenu | UiState::Clock | UiState::Pomodoro => {
                // Push R-Kun off screen to the right
                if self.r_kun_x < SCREEN_W + 50 {
                    let diff = (SCREEN_W + 50) - self.r_kun_x;
                    let step = (diff * dt as i32 / 20).max(2);
                    self.r_kun_x = (self.r_kun_x + step).min(SCREEN_W + 50);
                }

                // Slide menu panel in
                if self.menu_offset < MENU_SHOW_OFFSET {
                    let diff = MENU_SHOW_OFFSET - self.menu_offset;
                    let step = (diff * dt as i32 / 20).max(1);
                    self.menu_offset = (self.menu_offset + step).min(MENU_SHOW_OFFSET);
                }

                // Timeout
                if let Some(last) = self.last_interaction_time {
                    if now.duration_since(last).as_millis() >= 15_000 {
                        self.state = UiState::Idle;
                    }
                }

            }
            UiState::Metrics => {
                // Push R-Kun off screen
                if self.r_kun_x < SCREEN_W + 50 {
                    let diff = (SCREEN_W + 50) - self.r_kun_x;
                    let step = (diff * dt as i32 / 20).max(2);
                    self.r_kun_x = (self.r_kun_x + step).min(SCREEN_W + 50);
                }

                // Hide menu panel
                if self.menu_offset > MENU_HIDE_OFFSET {
                    let diff = self.menu_offset - MENU_HIDE_OFFSET;
                    let step = (diff * dt as i32 / 30).max(1);
                    self.menu_offset = (self.menu_offset - step).max(MENU_HIDE_OFFSET);
                }

                // Refresh metrics data every frame
                self.metrics.refresh(self.fps);

                // Timeout
                if let Some(last) = self.last_interaction_time {
                    if now.duration_since(last).as_millis() >= 30_000 {
                        self.state = UiState::Idle;
                    }
                }

                // Force redraw every frame for live data
                self.force_redraw = true;
            }
            UiState::Idle => {
                // Slide R-Kun back to center
                if self.r_kun_x > RKUN_CENTER_X {
                    let diff = self.r_kun_x - RKUN_CENTER_X;
                    let step = (diff * dt as i32 / 40).max(1);
                    self.r_kun_x = (self.r_kun_x - step).max(RKUN_CENTER_X);
                }

                // Slide menu out
                if self.menu_offset > MENU_HIDE_OFFSET {
                    let diff = self.menu_offset - MENU_HIDE_OFFSET;
                    let step = (diff * dt as i32 / 30).max(1);
                    self.menu_offset = (self.menu_offset - step).max(MENU_HIDE_OFFSET);
                }
            }
        }
    }

    pub fn handle_touch_action(&mut self, action: crate::touch::TouchAction) {
        self.last_interaction_time = Some(Instant::now());

        match action {
            crate::touch::TouchAction::SwipeRight => {
                if self.state == UiState::Idle {
                    self.state = UiState::Menu;
                    crate::audio::play_blip();
                }
            }
            crate::touch::TouchAction::SwipeLeft => {
                match self.state {
                    UiState::Menu => {
                        self.state = UiState::Idle;
                        crate::audio::play_blip();
                    }
                    UiState::SettingsMenu | UiState::ToolsMenu | UiState::InfoMenu | UiState::Clock | UiState::Pomodoro => {
                        self.state = UiState::Menu;
                        crate::audio::play_blip();
                    }
                    UiState::Metrics => {
                        self.state = UiState::InfoMenu;
                        self.force_redraw = true;
                        crate::audio::play_blip();
                    }
                    UiState::ToolsMenu => {
                        self.state = UiState::Menu;
                        self.menu_scroll_index = 0;
                        crate::audio::play_blip();
                    }
                    UiState::InfoMenu => {
                        self.state = UiState::Menu;
                        self.menu_scroll_index = 0;
                        crate::audio::play_blip();
                    }
                    UiState::Clock => {
                        self.state = UiState::InfoMenu;
                        self.force_redraw = true;
                        crate::audio::play_blip();
                    }
                    UiState::Pomodoro => {
                        self.state = UiState::ToolsMenu;
                        self.force_redraw = true;
                        crate::audio::play_blip();
                    }
                    _ => {}
                }
            }
            crate::touch::TouchAction::SwipeUp => {
                let max_items = match self.state {
                    UiState::Menu => 2,
                    UiState::ToolsMenu => 4,
                    UiState::InfoMenu => 3,
                    _ => 0,
                };

                if max_items > 4 && self.menu_scroll_index < max_items - 4 {
                    self.menu_scroll_index += 1;
                    self.force_redraw = true;
                }

                if self.state == UiState::Metrics {
                    // Scrolling down (content goes up, so metrics_scroll_y decreases)
                    self.metrics_scroll_y = (self.metrics_scroll_y - 40).max(-160);
                    self.force_redraw = true;
                }
            }
            crate::touch::TouchAction::SwipeDown => {
                if matches!(self.state, UiState::Menu | UiState::ToolsMenu | UiState::InfoMenu) {
                    if self.menu_scroll_index > 0 {
                        self.menu_scroll_index -= 1;
                        self.force_redraw = true;
                    }
                }

                if self.state == UiState::Metrics {
                    // Scrolling up (content goes down, so metrics_scroll_y increases)
                    self.metrics_scroll_y = (self.metrics_scroll_y + 40).min(0);
                    self.force_redraw = true;
                }
            }
            crate::touch::TouchAction::Tap(x, y) => {
                // Ripple at tap location
                self.ripple_x = x;
                self.ripple_y = y;
                self.ripple_radius = 0;
                self.ripple_active = true;

                match self.state {
                    UiState::Menu => {
                        let button_labels_len = 2;
                        let visible_count = core::cmp::min(4, button_labels_len - self.menu_scroll_index);
                        for i in 0..visible_count {
                            let bx = self.menu_offset;
                            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32;
                            if x >= bx && x <= bx + MENU_ITEM_WIDTH && y >= by && y <= by + MENU_ITEM_HEIGHT {
                                let actual_idx = i + self.menu_scroll_index;
                                self.selected_menu_item = actual_idx;
                                self.button_pop[actual_idx] = 5;
                                crate::audio::play_blip();
                                if actual_idx == 0 {
                                    self.state = UiState::ToolsMenu;
                                    self.menu_scroll_index = 0;
                                    self.force_redraw = true;
                                } else if actual_idx == 1 {
                                    self.state = UiState::InfoMenu;
                                    self.menu_scroll_index = 0;
                                    self.force_redraw = true;
                                }
                            }
                        }
                    }
                    UiState::ToolsMenu => {
                        let button_labels_len = 4;
                        let visible_count = core::cmp::min(4, button_labels_len - self.menu_scroll_index);
                        for i in 0..visible_count {
                            let bx = self.menu_offset;
                            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32;
                            if x >= bx && x <= bx + MENU_ITEM_WIDTH && y >= by && y <= by + MENU_ITEM_HEIGHT {
                                let actual_idx = i + self.menu_scroll_index;
                                self.tools_button_pop[actual_idx] = 5;
                                crate::audio::play_blip();
                                if actual_idx == 0 {
                                    self.flashlight_on = !self.flashlight_on;
                                    unsafe {
                                        if let Some(led) = crate::rgb_led::RGB_LED.as_mut() {
                                            if self.flashlight_on { led.set_color(255, 255, 255); } else { led.set_color(0, 0, 0); }
                                        }
                                    }
                                } else if actual_idx == 1 {
                                    self.state = UiState::Pomodoro;
                                } else if actual_idx == 2 {
                                    self.party_mode_on = !self.party_mode_on;
                                    if !self.party_mode_on {
                                        unsafe {
                                            if let Some(led) = crate::rgb_led::RGB_LED.as_mut() { led.set_color(0, 0, 0); }
                                        }
                                    }
                                } else if actual_idx == 3 {
                                    self.state = UiState::Menu;
                                    self.menu_scroll_index = 0;
                                }
                                self.force_redraw = true;
                            }
                        }
                    }
                    UiState::InfoMenu => {
                        let button_labels_len = 3;
                        let visible_count = core::cmp::min(4, button_labels_len - self.menu_scroll_index);
                        for i in 0..visible_count {
                            let bx = self.menu_offset;
                            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32;
                            if x >= bx && x <= bx + MENU_ITEM_WIDTH && y >= by && y <= by + MENU_ITEM_HEIGHT {
                                let actual_idx = i + self.menu_scroll_index;
                                self.info_button_pop[actual_idx] = 5;
                                crate::audio::play_blip();
                                if actual_idx == 0 {
                                    self.state = UiState::Metrics;
                                    self.metrics_scroll_y = 0;
                                } else if actual_idx == 1 {
                                    self.state = UiState::Clock;
                                } else if actual_idx == 2 {
                                    self.state = UiState::Menu;
                                    self.menu_scroll_index = 0;
                                }
                                self.force_redraw = true;
                            }
                        }
                    }
                    UiState::Pomodoro => {
                        for i in 0..4 {
                            let bx = self.menu_offset;
                            let by = MENU_START_Y + 60 + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32;
                            if x >= bx && x <= bx + MENU_ITEM_WIDTH && y >= by && y <= by + MENU_ITEM_HEIGHT {
                                self.pomodoro_button_pop[i] = 5;
                                crate::audio::play_blip();
                                if i == 0 {
                                    self.pomodoro_ms += 5 * 60 * 1000;
                                } else if i == 1 {
                                    self.pomodoro_ms = self.pomodoro_ms.saturating_sub(5 * 60 * 1000);
                                } else if i == 2 {
                                    self.pomodoro_running = !self.pomodoro_running;
                                } else if i == 3 {
                                    self.state = UiState::ToolsMenu;
                                }
                                self.force_redraw = true;
                            }
                        }
                    }
                    UiState::SettingsMenu | UiState::ToolsMenu | UiState::InfoMenu | UiState::Clock | UiState::Pomodoro => {
                        // Check tap on settings sub-menu items
                        let bx = self.menu_offset;
                        let by = MENU_START_Y;
                        if x >= bx && x <= bx + MENU_ITEM_WIDTH
                            && y >= by && y <= by + MENU_ITEM_HEIGHT
                        {
                            self.settings_button_pop[0] = 5;
                            crate::audio::play_blip();
                            self.state = UiState::Metrics;
                            self.metrics_scroll_y = 0;
                            self.metrics.refresh(self.fps);
                            self.force_redraw = true;
                        }
                    }
                    UiState::Metrics => {
                        // Tap anywhere on metrics screen scrolls down a bit or could be ignored
                        // For now just refresh the interaction timer
                    }
                    _ => {}
                }
            }
        }
    }

    /// Bounding box for R-Kun face, accounting for the -10 Y offset used in draw_r_kun
    fn r_kun_bbox(cx: i32, cy: i32, bounce: i32) -> (Point, Size) {
        let face_cy = cy - 10 + bounce;
        // eyes at ±35 x, ±9 y; blush at ±47 x, +18 y; mouth at +26 y; stroke margins +2
        (Point::new(cx - 50, face_cy - 12), Size::new(100, 42))
    }

    /// Check whether the display needs to be redrawn this frame.
    pub fn needs_draw(&self) -> bool {
        let full_redraw = self.force_redraw || self.text != self.prev_text;
        let r_kun_moved = self.r_kun_x != self.prev_r_kun_x
            || self.r_kun_y != self.prev_r_kun_y
            || self.idle_bounce != self.prev_idle_bounce
            || self.expression != self.prev_expression
            || self.is_blinking != self.prev_is_blinking;
        let menu_moved = self.menu_offset != self.prev_menu_offset;
        let buttons_animating = self.button_pop.iter().any(|&p| p > 0)
            || self.settings_button_pop.iter().any(|&p| p > 0)
            || self.tools_button_pop.iter().any(|&p| p > 0)
            || self.info_button_pop.iter().any(|&p| p > 0)
            || self.pomodoro_button_pop.iter().any(|&p| p > 0);

        full_redraw || r_kun_moved || menu_moved || buttons_animating || self.ripple_active || self.ripple_dirty || self.pomodoro_running
    }

    /// Save current state as "previous" so the next frame can detect changes.
    pub fn save_state(&mut self) {
        self.prev_r_kun_x = self.r_kun_x;
        self.prev_r_kun_y = self.r_kun_y;
        self.prev_idle_bounce = self.idle_bounce;
        self.prev_menu_offset = self.menu_offset;
        self.prev_expression = self.expression;
        self.prev_is_blinking = self.is_blinking;
        self.prev_text = self.text.clone();
        self.force_redraw = false;
        if !self.ripple_active {
            self.ripple_dirty = false;
        }
    }

    /// Draw all UI elements into the given draw target.
    ///
    /// With strip rendering, the target is pre-cleared to the background color
    /// before each call, so no explicit erase phase is needed.
    pub fn draw<D>(&mut self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        match self.state {
            UiState::Metrics => {
                self.draw_metrics(display)?;
            }
            UiState::Clock => {
                self.draw_clock(display)?;
            }
            UiState::Pomodoro => {
                self.draw_pomodoro(display)?;
            }
            _ => {
                // R-Kun face
                self.draw_r_kun(display)?;

                // Menu
                if self.menu_offset > MENU_HIDE_OFFSET {
                    match self.state {
                        UiState::SettingsMenu => self.draw_settings_menu(display)?,
                        UiState::ToolsMenu => self.draw_tools_menu(display)?,
                        UiState::InfoMenu => self.draw_info_menu(display)?,
                        _ => self.draw_menu(display)?,
                    }
                }

                // Status text
                if !self.text.is_empty() {
                    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                    Text::new(self.text.as_str(), Point::new(10, SCREEN_H - 10), style).draw(display)?;
                }
            }
        }

        // Ripple ring
        if self.ripple_active && self.ripple_radius > 0 {
            let ripple_style = PrimitiveStyleBuilder::new()
                .stroke_color(Rgb565::new(20, 40, 20))
                .stroke_width(2)
                .build();
            Ellipse::new(
                Point::new(self.ripple_x - self.ripple_radius, self.ripple_y - self.ripple_radius),
                Size::new((self.ripple_radius * 2) as u32, (self.ripple_radius * 2) as u32),
            ).into_styled(ripple_style).draw(display)?;
            self.ripple_dirty = true;
        }

        // FPS overlay (top-right, small font) — skip on metrics screen (already shown)
        if self.state != UiState::Metrics {
            let mut fps_buf: String<12> = String::new();
            let _ = core::fmt::Write::write_fmt(&mut fps_buf, format_args!("{}fps", self.fps));
            let small_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(20, 40, 20));
            Text::new(fps_buf.as_str(), Point::new(SCREEN_W - 48, 11), small_style).draw(display)?;
        }

        Ok(())
    }

    fn draw_r_kun<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Face center
        let center_x = self.r_kun_x;
        let center_y = self.r_kun_y - 10 + self.idle_bounce;

        // Styles
        let eye_style = PrimitiveStyle::with_fill(Rgb565::BLACK);
        let blush_style = PrimitiveStyle::with_fill(Rgb565::new(31, 40, 24));
        let stroke_style = PrimitiveStyleBuilder::new()
            .stroke_color(Rgb565::BLACK)
            .stroke_width(3)
            .build();

        let mut draw_ellipse_eyes = false;
        let mut draw_arc_eyes = false;
        let mut arc_angle_start = 0.0.deg();
        let arc_angle_sweep = 180.0.deg();
        let mut draw_line_eyes = false;
        let mut eye_w = 14;
        let mut eye_h = 16;
        let mut draw_mouth_arc = true;
        let mut mouth_angle_start = 0.0.deg();

        if self.is_blinking {
            draw_line_eyes = true;
        } else {
            match self.expression {
                EyeExpression::Neutral => {
                    draw_ellipse_eyes = true;
                },
                EyeExpression::Happy => {
                    draw_arc_eyes = true;
                    arc_angle_start = 180.0.deg();
                },
                EyeExpression::Sad => {
                    draw_arc_eyes = true;
                    draw_mouth_arc = true;
                    mouth_angle_start = 180.0.deg();
                },
                EyeExpression::Angry => {
                    draw_line_eyes = true;
                    draw_mouth_arc = true;
                    mouth_angle_start = 180.0.deg();
                },
                EyeExpression::Surprised => {
                    draw_ellipse_eyes = true;
                    eye_w = 12;
                    eye_h = 18;
                    draw_mouth_arc = false;
                },
                EyeExpression::Thinking => {
                    draw_ellipse_eyes = true;
                    eye_h = 10;
                    draw_mouth_arc = false;
                },
                EyeExpression::Blink => {
                    draw_line_eyes = true;
                },
                EyeExpression::Heart => {
                    draw_arc_eyes = true;
                    arc_angle_start = 180.0.deg();
                },
                EyeExpression::Sleepy => {
                    draw_line_eyes = true;
                },
            }
        }

        let eye_offset_x = 35;
        let left_eye_center = Point::new(center_x - eye_offset_x, center_y);
        let right_eye_center = Point::new(center_x + eye_offset_x, center_y);

        if draw_ellipse_eyes {
            Ellipse::new(Point::new(left_eye_center.x - (eye_w/2), left_eye_center.y - (eye_h/2)), Size::new(eye_w as u32, eye_h as u32))
                .into_styled(eye_style)
                .draw(display)?;
            Ellipse::new(Point::new(right_eye_center.x - (eye_w/2), right_eye_center.y - (eye_h/2)), Size::new(eye_w as u32, eye_h as u32))
                .into_styled(eye_style)
                .draw(display)?;
        } else if draw_arc_eyes {
            Arc::new(Point::new(left_eye_center.x - 10, left_eye_center.y - 10), 20, arc_angle_start, arc_angle_sweep)
                .into_styled(stroke_style)
                .draw(display)?;
            Arc::new(Point::new(right_eye_center.x - 10, right_eye_center.y - 10), 20, arc_angle_start, arc_angle_sweep)
                .into_styled(stroke_style)
                .draw(display)?;
        } else if draw_line_eyes {
            embedded_graphics::primitives::Line::new(Point::new(left_eye_center.x - 8, left_eye_center.y), Point::new(left_eye_center.x + 8, left_eye_center.y))
                .into_styled(stroke_style)
                .draw(display)?;
            embedded_graphics::primitives::Line::new(Point::new(right_eye_center.x - 8, right_eye_center.y), Point::new(right_eye_center.x + 8, right_eye_center.y))
                .into_styled(stroke_style)
                .draw(display)?;
        }

        // Blush
        Ellipse::new(Point::new(center_x - eye_offset_x - 12, center_y + 12), Size::new(14, 6))
            .into_styled(blush_style)
            .draw(display)?;
        Ellipse::new(Point::new(center_x + eye_offset_x - 2, center_y + 12), Size::new(14, 6))
            .into_styled(blush_style)
            .draw(display)?;

        // Mouth
        if draw_mouth_arc {
            Arc::new(Point::new(center_x - 8, center_y + 8), 16, mouth_angle_start, 180.0.deg())
                .into_styled(stroke_style)
                .draw(display)?;
        } else {
            Ellipse::new(Point::new(center_x - 4, center_y + 10), Size::new(8, 8))
                .into_styled(stroke_style)
                .draw(display)?;
        }

        Ok(())
    }

    fn draw_menu<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let button_labels = ["Tools", "Info"];
        let visible_count = core::cmp::min(4, button_labels.len().saturating_sub(self.menu_scroll_index));

        for i in 0..visible_count {
            let actual_idx = i + self.menu_scroll_index;
            let pop = self.button_pop[actual_idx];

            let w = MENU_ITEM_WIDTH + pop * 2;
            let h = MENU_ITEM_HEIGHT + pop * 2;

            let bx = self.menu_offset - pop;
            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32 - pop;

            let is_pressed = pop > 0;

            if is_pressed {
                let btn_style = PrimitiveStyleBuilder::new()
                    .fill_color(Rgb565::WHITE)
                    .build();
                let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
                RoundedRectangle::new(
                    Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
                    radii,
                )
                .into_styled(btn_style)
                .draw(display)?;

                let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                let label_len = button_labels[actual_idx].len() as i32;
                let label_width = label_len * 10;
                let text_x = bx + (w - label_width) / 2;
                let text_y = by + (h + 14) / 2;
                Text::new(button_labels[actual_idx], Point::new(text_x, text_y), text_style).draw(display)?;
            } else {
                let bg_color = Rgb565::new(31, 62, 29);
                let btn_style = PrimitiveStyleBuilder::new()
                    .fill_color(bg_color)
                    .stroke_color(Rgb565::new(25, 50, 25))
                    .stroke_width(1)
                    .build();
                let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
                RoundedRectangle::new(
                    Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
                    radii,
                )
                .into_styled(btn_style)
                .draw(display)?;

                let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                let label_len = button_labels[actual_idx].len() as i32;
                let label_width = label_len * 10;
                let text_x = bx + (w - label_width) / 2;
                let text_y = by + (h + 14) / 2;
                Text::new(button_labels[actual_idx], Point::new(text_x, text_y), text_style).draw(display)?;
            }
        }

        if button_labels.len() > 4 {
            let arrow_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
            if self.menu_scroll_index > 0 {
                Text::new("^", Point::new(self.menu_offset + MENU_ITEM_WIDTH / 2 - 5, MENU_START_Y - 5), arrow_style).draw(display)?;
            }
            if self.menu_scroll_index + 4 < button_labels.len() {
                Text::new("v", Point::new(self.menu_offset + MENU_ITEM_WIDTH / 2 - 5, MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * 4 + 10), arrow_style).draw(display)?;
            }
        }

        Ok(())
    }

    fn draw_tools_menu<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let header_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
        Text::new("< Tools", Point::new(self.menu_offset, MENU_START_Y - 4), header_style).draw(display)?;

        let button_labels = ["Flashlight", "Pomodoro", "Party Mode", "< Back"];
        let visible_count = core::cmp::min(4, button_labels.len().saturating_sub(self.menu_scroll_index));

        for i in 0..visible_count {
            let actual_idx = i + self.menu_scroll_index;
            let pop = self.tools_button_pop[actual_idx];

            let w = MENU_ITEM_WIDTH + pop * 2;
            let h = MENU_ITEM_HEIGHT + pop * 2;
            let bx = self.menu_offset - pop;
            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32 - pop;
            let is_pressed = pop > 0;

            let (fill, text_color) = if is_pressed {
                (Rgb565::WHITE, Rgb565::BLACK)
            } else {
                if actual_idx == 0 && self.flashlight_on {
                    (Rgb565::new(20, 40, 60), Rgb565::WHITE)
                } else if actual_idx == 2 && self.party_mode_on {
                    (Rgb565::new(60, 20, 40), Rgb565::WHITE)
                } else {
                    (Rgb565::new(31, 62, 29), Rgb565::BLACK)
                }
            };

            let btn_style = if is_pressed {
                PrimitiveStyleBuilder::new().fill_color(fill).build()
            } else {
                PrimitiveStyleBuilder::new()
                    .fill_color(fill)
                    .stroke_color(Rgb565::new(25, 50, 25))
                    .stroke_width(1)
                    .build()
            };

            let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
            RoundedRectangle::new(
                Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
                radii,
            )
            .into_styled(btn_style)
            .draw(display)?;

            let text_style = MonoTextStyle::new(&FONT_10X20, text_color);
            let label_len = button_labels[actual_idx].len() as i32;
            let label_width = label_len * 10;
            let text_x = bx + (w - label_width) / 2;
            let text_y = by + (h + 14) / 2;
            Text::new(button_labels[actual_idx], Point::new(text_x, text_y), text_style).draw(display)?;
        }
        Ok(())
    }

    fn draw_info_menu<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let header_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
        Text::new("< Info", Point::new(self.menu_offset, MENU_START_Y - 4), header_style).draw(display)?;

        let button_labels = ["Metrics", "Clock", "< Back"];
        let visible_count = core::cmp::min(4, button_labels.len().saturating_sub(self.menu_scroll_index));

        for i in 0..visible_count {
            let actual_idx = i + self.menu_scroll_index;
            let pop = self.info_button_pop[actual_idx];

            let w = MENU_ITEM_WIDTH + pop * 2;
            let h = MENU_ITEM_HEIGHT + pop * 2;
            let bx = self.menu_offset - pop;
            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32 - pop;
            let is_pressed = pop > 0;

            let (fill, text_color) = if is_pressed {
                (Rgb565::WHITE, Rgb565::BLACK)
            } else {
                (Rgb565::new(31, 62, 29), Rgb565::BLACK)
            };

            let btn_style = if is_pressed {
                PrimitiveStyleBuilder::new().fill_color(fill).build()
            } else {
                PrimitiveStyleBuilder::new()
                    .fill_color(fill)
                    .stroke_color(Rgb565::new(25, 50, 25))
                    .stroke_width(1)
                    .build()
            };

            let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
            RoundedRectangle::new(
                Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
                radii,
            )
            .into_styled(btn_style)
            .draw(display)?;

            let text_style = MonoTextStyle::new(&FONT_10X20, text_color);
            let label_len = button_labels[actual_idx].len() as i32;
            let label_width = label_len * 10;
            let text_x = bx + (w - label_width) / 2;
            let text_y = by + (h + 14) / 2;
            Text::new(button_labels[actual_idx], Point::new(text_x, text_y), text_style).draw(display)?;
        }
        Ok(())
    }

    fn draw_pomodoro<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let title_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
        Text::new("Pomodoro", Point::new(self.menu_offset, 20), title_style).draw(display)?;

        let mut time_str: String<16> = String::new();
        let secs = self.pomodoro_ms / 1000;
        let m = secs / 60;
        let s = secs % 60;
        let _ = core::fmt::Write::write_fmt(&mut time_str, format_args!("{:02}:{:02}", m, s));
        Text::new(time_str.as_str(), Point::new(self.menu_offset + 25, 50), title_style).draw(display)?;

        let button_labels = ["+5 Min", "-5 Min", if self.pomodoro_running { "Stop" } else { "Start" }, "< Back"];

        for i in 0..4 {
            let pop = self.pomodoro_button_pop[i];
            let w = MENU_ITEM_WIDTH + pop * 2;
            let h = MENU_ITEM_HEIGHT + pop * 2;
            let bx = self.menu_offset - pop;
            let by = MENU_START_Y + 60 + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32 - pop;
            let is_pressed = pop > 0;

            let (fill, text_color) = if is_pressed {
                (Rgb565::WHITE, Rgb565::BLACK)
            } else {
                (Rgb565::new(31, 62, 29), Rgb565::BLACK)
            };

            let btn_style = if is_pressed {
                PrimitiveStyleBuilder::new().fill_color(fill).build()
            } else {
                PrimitiveStyleBuilder::new()
                    .fill_color(fill)
                    .stroke_color(Rgb565::new(25, 50, 25))
                    .stroke_width(1)
                    .build()
            };

            let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
            RoundedRectangle::new(
                Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
                radii,
            )
            .into_styled(btn_style)
            .draw(display)?;

            let text_style = MonoTextStyle::new(&FONT_10X20, text_color);
            let label_len = button_labels[i].len() as i32;
            let label_width = label_len * 10;
            let text_x = bx + (w - label_width) / 2;
            let text_y = by + (h + 14) / 2;
            Text::new(button_labels[i], Point::new(text_x, text_y), text_style).draw(display)?;
        }
        Ok(())
    }

    fn draw_clock<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let title_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
        Text::new("Clock", Point::new(8, 20), title_style).draw(display)?;

        let mut time_str: String<32> = String::new();
        if let Some(secs) = crate::ntp::wall_clock_secs() {
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            let _ = core::fmt::Write::write_fmt(&mut time_str, format_args!("{:02}:{:02}:{:02}", h, m, s));
        } else {
            let _ = core::fmt::Write::write_fmt(&mut time_str, format_args!("Uptime: {}s", self.metrics.uptime_secs));
        }

        let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::new(20, 60, 20));
        Text::new(time_str.as_str(), Point::new(100, 120), text_style).draw(display)?;

        let hint_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(20, 40, 20));
        Text::new("< swipe left to go back", Point::new(8, SCREEN_H - 6), hint_style).draw(display)?;

        Ok(())
    }

    fn draw_settings_menu<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Header: "Settings" in small font
        let header_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);
        Text::new("< Settings", Point::new(self.menu_offset, MENU_START_Y - 4), header_style)
            .draw(display)?;

        // Single item: "Metrics"
        let settings_labels = ["Metrics"];
        let pop = self.settings_button_pop[0];
        let w = MENU_ITEM_WIDTH + pop * 2;
        let h = MENU_ITEM_HEIGHT + pop * 2;
        let bx = self.menu_offset - pop;
        let by = MENU_START_Y + 6 - pop;
        let is_pressed = pop > 0;

        let (fill, text_color) = if is_pressed {
            (Rgb565::WHITE, Rgb565::BLACK)
        } else {
            (Rgb565::new(31, 62, 29), Rgb565::BLACK)
        };

        let btn_style = if is_pressed {
            PrimitiveStyleBuilder::new().fill_color(fill).build()
        } else {
            PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(Rgb565::new(25, 50, 25))
                .stroke_width(1)
                .build()
        };

        let radii = CornerRadii::new(Size::new(MENU_CORNER_RADIUS, MENU_CORNER_RADIUS));
        RoundedRectangle::new(
            Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)),
            radii,
        )
        .into_styled(btn_style)
        .draw(display)?;

        let text_style = MonoTextStyle::new(&FONT_10X20, text_color);
        let label_len = settings_labels[0].len() as i32;
        let label_width = label_len * 10;
        let text_x = bx + (w - label_width) / 2;
        let text_y = by + (h + 14) / 2;
        Text::new(settings_labels[0], Point::new(text_x, text_y), text_style).draw(display)?;

        Ok(())
    }

    fn draw_metrics<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let title_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
        let label_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(10, 20, 10));
        let value_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);

        let m = &self.metrics;
        let mut y = 8 + self.metrics_scroll_y;

        // Title
        Text::new("System Metrics", Point::new(8, y + 14), title_style).draw(display)?;
        y += 22;

        // Divider line
        Rectangle::new(Point::new(8, y), Size::new(304, 1))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(20, 40, 20)))
            .draw(display)?;
        y += 6;

        // -- Uptime --
        Text::new("UPTIME", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            let secs = m.uptime_secs;
            let days = secs / 86400;
            let hours = (secs % 86400) / 3600;
            let mins = (secs % 3600) / 60;
            let s = secs % 60;
            let mut buf: String<32> = String::new();
            if days > 0 {
                let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("{}d {}h {}m {}s", days, hours, mins, s));
            } else {
                let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("{}h {}m {}s", hours, mins, s));
            }
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- CPU / Clock --
        Text::new("CPU (240MHz)", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            let mut buf: String<48> = String::new();
            let _ = core::fmt::Write::write_fmt(&mut buf,
                format_args!("Core0: {}%  Core1: {}%  {}fps", m.cpu0_pct, m.cpu1_pct, m.fps));
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- Memory --
        Text::new("MEMORY", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            // SRAM stats
            let s_total_kb = m.sram_total / 1024;
            let s_free_kb = m.sram_free / 1024;
            let s_used_kb = s_total_kb - s_free_kb;

            // PSRAM stats
            let p_total_kb = m.psram_total / 1024;
            let p_free_kb = m.psram_free / 1024;
            let p_used_kb = p_total_kb.saturating_sub(p_free_kb);

            let mut buf: String<48> = String::new();
            let _ = core::fmt::Write::write_fmt(&mut buf,
                format_args!("SRAM: {}KB/{}KB ({}KB free)", s_used_kb, s_total_kb, s_free_kb));
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;

            y += 12;
            buf.clear();
            let _ = core::fmt::Write::write_fmt(&mut buf,
                format_args!("PSRAM: {}KB/{}KB ({}KB free)", p_used_kb, p_total_kb, p_free_kb));
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- Battery --
        Text::new("BATTERY", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            let mv = m.battery_mv;
            let mut buf: String<32> = String::new();
            if mv > 0 {
                // Simple percentage: 3.0V=0%, 4.2V=100%
                let pct = if mv >= 4200 { 100u32 }
                    else if mv <= 3000 { 0 }
                    else { ((mv - 3000) * 100) / 1200 };
                let _ = core::fmt::Write::write_fmt(&mut buf,
                    format_args!("{}.{}V  ~{}%", mv / 1000, (mv % 1000) / 100, pct));
            } else {
                let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("No reading"));
            }
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- WiFi --
        Text::new("NETWORK", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            let mut buf: String<48> = String::new();
            match m.wifi_status {
                0 => { let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("Disconnected")); }
                1 => { let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("Connecting...")); }
                2 => {
                    if let Some(ip) = m.wifi_ip {
                        let _ = core::fmt::Write::write_fmt(&mut buf,
                            format_args!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
                    } else {
                        let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("Connected"));
                    }
                }
                _ => { let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("Error")); }
            }
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- Temperature --
        Text::new("TEMPERATURE", Point::new(8, y + 8), label_style).draw(display)?;
        y += 12;
        {
            let mut buf: String<32> = String::new();
            if m.temp_tenths > 0 {
                let _ = core::fmt::Write::write_fmt(&mut buf,
                    format_args!("{}.{}C", m.temp_tenths / 10, m.temp_tenths % 10));
            } else {
                let _ = core::fmt::Write::write_fmt(&mut buf, format_args!("ULP not active"));
            }
            Text::new(buf.as_str(), Point::new(14, y + 8), value_style).draw(display)?;
        }
        y += 14;

        // -- Scroll/Back hint --
        let hint_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(20, 40, 20));
        let _ = y;
        // The hint is drawn at the absolute bottom, so we ignore metrics_scroll_y here
        Text::new("< swipe left to go back | drag up/down", Point::new(8, SCREEN_H - 6), hint_style).draw(display)?;

        Ok(())
    }
}
