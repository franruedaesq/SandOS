use abi::EyeExpression;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, RoundedRectangle, Rectangle, CornerRadii, Arc},
    text::Text,
    mono_font::{ascii::FONT_10X20, MonoTextStyle},
};
use heapless::String;

#[derive(Clone, Copy, PartialEq)]
pub enum UiState {
    Idle,
    Menu,
}

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
    pub last_interaction_frame: u32,

    // R-Kun properties
    pub r_kun_x: i32,
    pub r_kun_y: i32,
    pub prev_r_kun_x: i32,
    pub prev_r_kun_y: i32,
    pub prev_idle_bounce: i32,

    // Animations
    pub button_pop: [i32; 4],
    pub prev_menu_offset: i32,

    // Tactile Feedback
    pub ripple_x: i32,
    pub ripple_y: i32,
    pub ripple_radius: i32,
    pub ripple_active: bool,

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
            menu_offset: -120, // offscreen
            slide_target: 0,
            idle_bounce: 0,
            is_blinking: false,
            prev_is_blinking: false,
            last_interaction_frame: 0,
            r_kun_x: 120,
            r_kun_y: 160,
            prev_r_kun_x: 120,
            prev_r_kun_y: 160,
            prev_idle_bounce: 0,
            button_pop: [0; 4],
            prev_menu_offset: -120,
            ripple_x: 0,
            ripple_y: 0,
            ripple_radius: 0,
            ripple_active: false,
            text: String::new(),
            prev_text: String::new(),
            force_redraw: true,
        }
    }
}

// We need an ellipse implementation, but embedded_graphics has it.
use embedded_graphics::primitives::Ellipse;
use embedded_graphics::geometry::{Point, Size};

impl UiManager {
    pub fn update(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);

        if self.state == UiState::Idle {
            // Smooth idle bounce using a simple triangle wave to approximate a breathing effect
            let bounce_period = 120; // frames
            let cycle_pos = self.frame_count % bounce_period;
            let half_period = bounce_period / 2;
            let magnitude = 3; // Max pixel shift

            let normalized_bounce = if cycle_pos < half_period {
                cycle_pos
            } else {
                bounce_period - cycle_pos
            };
            self.idle_bounce = (normalized_bounce * magnitude / half_period) as i32;

            // Random eye movements
            if self.frame_count % 400 == 0 {
                self.r_kun_x = 120 + (self.frame_count % 3) as i32 * 2 - 2; // -2, 0, or 2
            }
        } else {
            self.idle_bounce = 0;
        }

        // Fast random blink
        // Let's make it blink for 5 frames every 300 frames.
        if self.frame_count % 300 < 5 {
            self.is_blinking = true;
        } else {
            self.is_blinking = false;
        }



        // Update Ripple
        if self.ripple_active {
            self.ripple_radius += 4;
            if self.ripple_radius > 40 {
                self.ripple_active = false;
            }
        }

        // State machine
        if self.state == UiState::Menu {
            // Slide R-Kun right (smooth easing)
            let r_kun_target = 180;
            if self.r_kun_x < r_kun_target {
                let step = ((r_kun_target - self.r_kun_x) / 4).max(1);
                self.r_kun_x += step;
            }

            // Slide menu right (smooth easing)
            let menu_target = 10;
            if self.menu_offset < menu_target {
                let step = ((menu_target - self.menu_offset) / 3).max(1);
                self.menu_offset += step;
            }

            // Timeout return to idle (10 seconds without interaction)
            if self.frame_count > self.last_interaction_frame + 1000 {
                self.state = UiState::Idle;
            }

            // Pop animations recovery
            for pop in &mut self.button_pop {
                if *pop > 0 {
                    *pop -= 1;
                }
            }

        } else {
            // Slide R-Kun back to center (smooth easing)
            let r_kun_target = 120;
            if self.r_kun_x > r_kun_target {
                let step = ((self.r_kun_x - r_kun_target) / 4).max(1);
                self.r_kun_x -= step;
            }

            // Slide menu back left (smooth easing)
            let menu_target = -120;
            if self.menu_offset > menu_target {
                let step = ((self.menu_offset - menu_target) / 3).max(1);
                self.menu_offset -= step;
            }
        }
    }

    pub fn handle_touch(&mut self, x: i32, y: i32) {
        self.last_interaction_frame = self.frame_count;

        // Trigger Ripple
        self.ripple_x = x;
        self.ripple_y = y;
        self.ripple_radius = 0;
        self.ripple_active = true;

        if self.state == UiState::Idle {
            // Boop on R-Kun
            let dx = x - self.r_kun_x;
            let dy = y - self.r_kun_y;
            if dx * dx + dy * dy < 60 * 60 { // Inside the marshmallow radius
                self.state = UiState::Menu;
                crate::audio::play_blip();
            }
        } else if self.state == UiState::Menu {
            // Check button taps on the left menu (2x2 Grid)
            let button_width = 80;
            let button_height = 80;
            let spacing = 10;
            let start_y = 60;

            for i in 0..4 {
                let col = i % 2;
                let row = i / 2;

                let bx = self.menu_offset + (button_width + spacing) * col as i32;
                let by = start_y + (button_height + spacing) * row as i32;

                if x >= bx && x <= bx + button_width && y >= by && y <= by + button_height {
                    // Tap button
                    self.button_pop[i] = 5; // Start pop animation
                    crate::audio::play_blip();

                    // Specific button action would be logged or dispatched here
                }
            }

            // Check tapping on R-Kun to return to Idle
            if x > 180 {
                self.state = UiState::Idle;
                crate::audio::play_blip();
            }
        }
    }

    pub fn render<D>(&mut self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let bg_color = Rgb565::new(31, 62, 29);
        let mut full_redraw = self.force_redraw
            || self.expression != self.prev_expression
            || self.is_blinking != self.prev_is_blinking
            || self.text != self.prev_text;

        let r_kun_moved = self.r_kun_x != self.prev_r_kun_x || self.r_kun_y != self.prev_r_kun_y || self.idle_bounce != self.prev_idle_bounce;
        let menu_moved = self.menu_offset != self.prev_menu_offset;
        let buttons_animating = self.button_pop.iter().any(|&p| p > 0);

        if full_redraw {
            display.clear(bg_color)?;
        } else {
            // Partial erasures
            if r_kun_moved && !full_redraw {
                // Erase previous R-Kun bounds (approximate 100x100 region)
                Rectangle::new(Point::new(self.prev_r_kun_x - 50, self.prev_r_kun_y - 30 + self.prev_idle_bounce), Size::new(100, 60))
                    .into_styled(PrimitiveStyle::with_fill(bg_color))
                    .draw(display)?;
            }
            if menu_moved && !full_redraw {
                // Erase previous menu bounds (left edge region)
                Rectangle::new(Point::new(0, 50), Size::new(220, 220)) // large enough to cover the grid
                    .into_styled(PrimitiveStyle::with_fill(bg_color))
                    .draw(display)?;
            }
            if self.ripple_active && !full_redraw {
                // Erase the whole screen if rippling since it expands widely. For a small screen this is acceptable.
                // Alternatively erase the exact previous ripple outline. We'll force redraw for simplicity to clear previous ripple ring.
                full_redraw = true;
                display.clear(bg_color)?;
            }
        }

        if full_redraw || r_kun_moved || menu_moved || buttons_animating || self.ripple_active {

            // Draw R-Kun
            self.draw_r_kun(display)?;

            // Draw Menu
            if self.menu_offset > -200 {
                self.draw_menu(display)?;
            }

            // Text status
            if !self.text.is_empty() {
                 let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                 Text::new(self.text.as_str(), Point::new(15, 300), style).draw(display)?;
            }

            // Draw Ripple
            if self.ripple_active {
                let ripple_style = PrimitiveStyleBuilder::new()
                    .stroke_color(Rgb565::new(20, 40, 20))
                    .stroke_width(2)
                    .build();
                Ellipse::new(
                    Point::new(self.ripple_x - self.ripple_radius, self.ripple_y - self.ripple_radius),
                    Size::new((self.ripple_radius * 2) as u32, (self.ripple_radius * 2) as u32)
                )
                .into_styled(ripple_style)
                .draw(display)?;
            }

            // Update tracked state
            self.prev_r_kun_x = self.r_kun_x;
            self.prev_r_kun_y = self.r_kun_y;
            self.prev_idle_bounce = self.idle_bounce;
            self.prev_menu_offset = self.menu_offset;
            self.prev_expression = self.expression;
            self.prev_is_blinking = self.is_blinking;
            self.prev_text = self.text.clone();
            self.force_redraw = false;
        }

        Ok(())
    }

    fn draw_r_kun<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Face center
        let center_x = self.r_kun_x;
        let center_y = self.r_kun_y - 10 + self.idle_bounce; // apply breathing effect

        // Minimalist Eyes
        let eye_style = PrimitiveStyle::with_fill(Rgb565::BLACK);
        let blush_style = PrimitiveStyle::with_fill(Rgb565::new(31, 40, 24)); // vibrant pastel pink
        let stroke_style = PrimitiveStyleBuilder::new()
            .stroke_color(Rgb565::BLACK)
            .stroke_width(3)
            .build();

        let mut draw_ellipse_eyes = false;
        let mut draw_arc_eyes = false;
        let mut arc_angle_start = 0.0.deg();
        let mut arc_angle_sweep = 180.0.deg();
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
                    arc_angle_start = 180.0.deg(); // upside down arc for happy eyes
                },
                EyeExpression::Sad => {
                    draw_arc_eyes = true; // downward arc
                    draw_mouth_arc = true;
                    mouth_angle_start = 180.0.deg(); // sad mouth
                },
                EyeExpression::Angry => {
                    draw_line_eyes = true; // simplifying angry to slanted lines if needed, or straight for now
                    draw_mouth_arc = true;
                    mouth_angle_start = 180.0.deg();
                },
                EyeExpression::Surprised => {
                    draw_ellipse_eyes = true;
                    eye_w = 12;
                    eye_h = 18;
                    draw_mouth_arc = false; // "o" shape
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
                    arc_angle_start = 180.0.deg(); // fallback to happy for now
                },
                EyeExpression::Sleepy => {
                    draw_line_eyes = true;
                },
            }
        }

        let eye_offset_x = 35;

        // Draw Left and Right Eyes
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
            // Arc bounds must encompass the whole ellipse
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
            // Using 16 as diameter for both width and height to satisfy Arc::new(center, diameter, start, sweep)
            Arc::new(Point::new(center_x - 8, center_y + 8), 16, mouth_angle_start, 180.0.deg())
                .into_styled(stroke_style)
                .draw(display)?;
        } else {
             // small circle mouth
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
        let button_width = 80;
        let button_height = 80;
        let spacing = 10;
        let start_y = 60;

        for i in 0..4 {
            let pop = self.button_pop[i];

            let col = i % 2;
            let row = i / 2;

            let w = button_width + pop * 2;
            let h = button_height + pop * 2;

            let bx = self.menu_offset + (button_width + spacing) * col as i32 - pop;
            let by = start_y + (button_height + spacing) * row as i32 - pop;

            let fill_color = if pop > 0 { Rgb565::new(25, 50, 25) } else { Rgb565::WHITE };
            let text_color = if pop > 0 { Rgb565::WHITE } else { Rgb565::BLACK };

            let btn_style = PrimitiveStyleBuilder::new()
                .fill_color(fill_color)
                .stroke_color(Rgb565::new(25, 50, 25)) // COLOR_SOFT_GRAY
                .stroke_width(2)
                .build();

            // Rounded buttons
            let radii = CornerRadii::new(Size::new(12, 12));
            let rect = RoundedRectangle::new(Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32)), radii);

            rect.into_styled(btn_style).draw(display)?;

            let text_style = MonoTextStyle::new(&FONT_10X20, text_color);
            // Center text roughly within the 80x80 box
            let text_x = bx + 10;
            let text_y = by + 45;
            Text::new(button_labels[i], Point::new(text_x, text_y), text_style).draw(display)?;
        }

        Ok(())
    }
}
