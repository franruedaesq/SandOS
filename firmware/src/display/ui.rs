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

        // ── State machine (dt-scaled easing) ───────────────────────────────
        if self.state == UiState::Menu {
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

            // Pop animations decay: 100 units/sec (pop 5 → 0 in ~50 ms)
            let pop_decay = (dt as i32 * 100 / 1000).max(1);
            for pop in &mut self.button_pop {
                if *pop > 0 {
                    *pop = (*pop - pop_decay).max(0);
                }
            }
        } else {
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

    pub fn handle_touch(&mut self, x: i32, y: i32) {
        self.last_interaction_time = Some(Instant::now());

        // Start new ripple at touch location
        self.ripple_x = x;
        self.ripple_y = y;
        self.ripple_radius = 0;
        self.ripple_active = true;

        if self.state == UiState::Idle {
            // Boop on R-Kun
            let dx = x - self.r_kun_x;
            let dy = y - self.r_kun_y;
            if dx * dx + dy * dy < 60 * 60 {
                self.state = UiState::Menu;
                crate::audio::play_blip();
            }
        } else if self.state == UiState::Menu {
            // Check taps on vertical menu items
            for i in 0..4 {
                let bx = self.menu_offset;
                let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32;

                if x >= bx && x <= bx + MENU_ITEM_WIDTH
                    && y >= by && y <= by + MENU_ITEM_HEIGHT
                {
                    self.selected_menu_item = i;
                    self.button_pop[i] = 5;
                    crate::audio::play_blip();
                }
            }

            // Tap right side to dismiss
            if x > RKUN_MENU_X - 20 {
                self.state = UiState::Idle;
                crate::audio::play_blip();
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
        let buttons_animating = self.button_pop.iter().any(|&p| p > 0);

        full_redraw || r_kun_moved || menu_moved || buttons_animating
            || self.ripple_active || self.ripple_dirty
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
        // R-Kun face
        self.draw_r_kun(display)?;

        // Menu
        if self.menu_offset > MENU_HIDE_OFFSET {
            self.draw_menu(display)?;
        }

        // Status text
        if !self.text.is_empty() {
            let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
            Text::new(self.text.as_str(), Point::new(10, SCREEN_H - 10), style).draw(display)?;
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

        // FPS overlay (top-right, small font)
        {
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
        let button_labels = ["Talk", "Play", "Memory", "Settings"];

        for i in 0..4 {
            let pop = self.button_pop[i];

            let w = MENU_ITEM_WIDTH + pop * 2;
            let h = MENU_ITEM_HEIGHT + pop * 2;

            let bx = self.menu_offset - pop;
            let by = MENU_START_Y + (MENU_ITEM_HEIGHT + MENU_ITEM_SPACING) * i as i32 - pop;

            let is_pressed = pop > 0;

            if is_pressed {
                // Pressed: white pill with black text
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
                let label_len = button_labels[i].len() as i32;
                let label_width = label_len * 10;
                let text_x = bx + (w - label_width) / 2;
                let text_y = by + (h + 14) / 2;
                Text::new(button_labels[i], Point::new(text_x, text_y), text_style).draw(display)?;
            } else {
                // Default: filled bg pill with subtle outline
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
                let label_len = button_labels[i].len() as i32;
                let label_width = label_len * 10;
                let text_x = bx + (w - label_width) / 2;
                let text_y = by + (h + 14) / 2;
                Text::new(button_labels[i], Point::new(text_x, text_y), text_style).draw(display)?;
            }
        }

        Ok(())
    }
}
