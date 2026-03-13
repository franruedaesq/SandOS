use abi::EyeExpression;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
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

    // Animations
    pub button_pop: [i32; 4],

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
            button_pop: [0; 4],
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

        // Remove slow idle bounce to prevent full face flicker
        self.idle_bounce = 0;

        // Fast random blink
        // Let's make it blink for 5 frames every 300 frames.
        if self.frame_count % 300 < 5 {
            self.is_blinking = true;
        } else {
            self.is_blinking = false;
        }



        // State machine
        if self.state == UiState::Menu {
            // Slide R-Kun right
            if self.r_kun_x < 180 {
                self.r_kun_x += 4;
            }
            // Slide menu right
            if self.menu_offset < 10 {
                self.menu_offset += 6;
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
            // Slide R-Kun back to center
            if self.r_kun_x > 120 {
                self.r_kun_x -= 4;
            }
            // Slide menu back left
            if self.menu_offset > -120 {
                self.menu_offset -= 6;
            }
        }
    }

    pub fn handle_touch(&mut self, x: i32, y: i32) {
        self.last_interaction_frame = self.frame_count;

        if self.state == UiState::Idle {
            // Boop on R-Kun
            let dx = x - self.r_kun_x;
            let dy = y - self.r_kun_y;
            if dx * dx + dy * dy < 60 * 60 { // Inside the marshmallow radius
                self.state = UiState::Menu;
            }
        } else if self.state == UiState::Menu {
            // Check button taps on the left menu
            // Menu rects: 10, y, 100, 40
            let button_height = 40;
            let spacing = 15;
            let start_y = 60;

            for i in 0..4 {
                let bx = self.menu_offset;
                let by = start_y + (button_height + spacing) * i as i32;

                if x >= bx && x <= bx + 100 && y >= by && y <= by + button_height {
                    // Tap button
                    self.button_pop[i] = 5; // Start pop animation

                    // Specific button action would be logged or dispatched here
                }
            }

            // Check tapping on R-Kun to return to Idle
            if x > 140 {
                self.state = UiState::Idle;
            }
        }
    }

    pub fn render<D>(&mut self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let needs_redraw = self.force_redraw
            || self.expression != self.prev_expression
            || self.is_blinking != self.prev_is_blinking
            || self.text != self.prev_text
            || self.state == UiState::Menu; // if menu is open, it might be animating

        if needs_redraw {
            // 1. Clear background to Minimalist White
            display.clear(Rgb565::WHITE)?;

            // 3. Draw R-Kun (now just kawaii face)
            self.draw_r_kun(display)?;

            // 4. Draw Menu
            if self.menu_offset > -100 {
                self.draw_menu(display)?;
            }

            // Text status/OpenAI thoughts
            if !self.text.is_empty() {
                 let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                 Text::new(self.text.as_str(), Point::new(15, 300), style).draw(display)?;
            }

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
        let center_y = self.r_kun_y - 10; // slightly up

        // Minimalist Eyes
        let eye_style = PrimitiveStyle::with_fill(Rgb565::BLACK);
        let blush_style = PrimitiveStyle::with_fill(Rgb565::new(63, 40, 40)); // softer pinkish red for white bg

        let mut left_eye_char = "";
        let mut right_eye_char = "";
        let mut mouth_char = "w";
        let mut draw_ellipse_eyes = false;
        let mut eye_w = 14;
        let mut eye_h = 16;

        if self.is_blinking {
            left_eye_char = "-";
            right_eye_char = "-";
            mouth_char = "w";
        } else {
            match self.expression {
                EyeExpression::Neutral => {
                    draw_ellipse_eyes = true;
                    mouth_char = "w";
                },
                EyeExpression::Happy => {
                    left_eye_char = ">";
                    right_eye_char = "<";
                    mouth_char = "w";
                },
                EyeExpression::Sad => {
                    left_eye_char = "T";
                    right_eye_char = "T";
                    mouth_char = "m";
                },
                EyeExpression::Angry => {
                    left_eye_char = "\\";
                    right_eye_char = "/";
                    mouth_char = "m";
                },
                EyeExpression::Surprised => {
                    draw_ellipse_eyes = true;
                    eye_w = 12;
                    eye_h = 18;
                    mouth_char = "o";
                },
                EyeExpression::Thinking => {
                    draw_ellipse_eyes = true;
                    eye_h = 10;
                    mouth_char = "-";
                },
                EyeExpression::Blink => {
                    left_eye_char = "-";
                    right_eye_char = "-";
                    mouth_char = "w";
                },
                EyeExpression::Heart => {
                    // simple fallback for heart if we can't draw hearts easily, or use text "v"
                    left_eye_char = "v";
                    right_eye_char = "v";
                    mouth_char = "w";
                },
                EyeExpression::Sleepy => {
                    left_eye_char = "-";
                    right_eye_char = "-";
                    mouth_char = ".";
                },
            }
        }

        let eye_offset_x = 35;
        let font_style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);

        if draw_ellipse_eyes {
            // Left eye
            Ellipse::new(Point::new(center_x - eye_offset_x - (eye_w/2), center_y - (eye_h/2)), Size::new(eye_w as u32, eye_h as u32))
                .into_styled(eye_style)
                .draw(display)?;
            // Right eye
            Ellipse::new(Point::new(center_x + eye_offset_x - (eye_w/2), center_y - (eye_h/2)), Size::new(eye_w as u32, eye_h as u32))
                .into_styled(eye_style)
                .draw(display)?;
        } else {
            // Draw text based eyes
            Text::new(left_eye_char, Point::new(center_x - eye_offset_x - 5, center_y + 5), font_style).draw(display)?;
            Text::new(right_eye_char, Point::new(center_x + eye_offset_x - 5, center_y + 5), font_style).draw(display)?;
        }

        // Blush
        Ellipse::new(Point::new(center_x - eye_offset_x - 12, center_y + 12), Size::new(14, 6))
            .into_styled(blush_style)
            .draw(display)?;
        Ellipse::new(Point::new(center_x + eye_offset_x - 2, center_y + 12), Size::new(14, 6))
            .into_styled(blush_style)
            .draw(display)?;

        // Mouth
        Text::new(mouth_char, Point::new(center_x - 5, center_y + 15), font_style).draw(display)?;

        Ok(())
    }

    fn draw_menu<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let button_labels = ["Talk", "Play", "Memory", "Settings"];
        let button_height = 40;
        let spacing = 15;
        let start_y = 60;
        let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);

        for i in 0..4 {
            let pop = self.button_pop[i];

            // Pop effect scales up the button (inversely, here we decrease the size to make it pop? Wait. Pop = scale up 110%)
            // If pop > 0, we grow the button slightly
            let w = 100 + pop * 2;
            let h = button_height + pop * 2;

            let bx = self.menu_offset - pop;
            let by = start_y + (button_height + spacing) * i as i32 - pop;

            let btn_style = PrimitiveStyleBuilder::new()
                .fill_color(Rgb565::WHITE)
                .stroke_color(Rgb565::new(20, 20, 20))
                .stroke_width(2)
                .build();

            // Rounded buttons
            // Embedded graphics rounded rect isn't standard, we'll draw a rectangle for now.
            // Oh, RoundedRectangle is imported from `embedded_graphics::primitives::RoundedRectangle`.
            // Let's use it. It requires an embedded_graphics::geometry::CornerRadii.
            let rect = Rectangle::new(Point::new(bx, by), Size::new(w as u32, h as u32));
            // Let's simplify and use Rectangle for speed and compatibility, or simple circles on ends.
            // Using standard rectangle for simplicity and less chance of build errors with missing traits.
            rect.into_styled(btn_style).draw(display)?;

            Text::new(button_labels[i], Point::new(bx + 20, by + 25), style).draw(display)?;
        }

        Ok(())
    }
}
