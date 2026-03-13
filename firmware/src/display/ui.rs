use abi::EyeExpression;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
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
    pub frame_count: u32,
    pub menu_offset: i32,
    pub slide_target: i32,
    pub idle_bounce: i32,
    pub is_blinking: bool,
    pub last_interaction_frame: u32,

    // R-Kun properties
    pub r_kun_x: i32,
    pub r_kun_y: i32,

    // Animations
    pub button_pop: [i32; 4],

    // Particles
    pub drifters: [(i32, i32); 5],

    // Status
    pub text: String<64>,
}

impl UiManager {
    pub fn new() -> Self {
        Self {
            state: UiState::Idle,
            expression: EyeExpression::Neutral,
            frame_count: 0,
            menu_offset: -120, // offscreen
            slide_target: 0,
            idle_bounce: 0,
            is_blinking: false,
            last_interaction_frame: 0,
            r_kun_x: 120,
            r_kun_y: 160,
            button_pop: [0; 4],
            drifters: [
                (20, 300),
                (80, 250),
                (150, 310),
                (200, 200),
                (100, 350),
            ],
            text: String::new(),
        }
    }
}

// We need an ellipse implementation, but embedded_graphics has it.
use embedded_graphics::primitives::Ellipse;
use embedded_graphics::geometry::{Point, Size};

impl UiManager {
    pub fn update(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);

        // Idle Bounce: 5% vertical squish, slow 3s loop.
        // Assume 100fps -> 300 frames per loop
        let phase = self.frame_count % 300;

        // Sine wave approximation for bounce
        // We'll use a simple triangle wave to approximate it here
        let half_phase = if phase > 150 { 300 - phase } else { phase };
        // half_phase goes from 0 to 150 and back to 0. Let's scale it to an offset of 0..8 pixels.
        self.idle_bounce = (half_phase as i32) * 8 / 150;

        // Random blink (every 4-8s)
        // Assume 100fps -> 400 to 800 frames. Use pseudo-random.
        // Let's make it blink for 10 frames every 500 frames.
        if self.frame_count % 500 < 10 {
            self.is_blinking = true;
        } else {
            self.is_blinking = false;
        }

        // Drifter particles
        for i in 0..self.drifters.len() {
            self.drifters[i].1 -= 1;
            if self.drifters[i].1 < -20 {
                self.drifters[i].1 = 340;
                // Pseudo random X
                self.drifters[i].0 = (self.frame_count.wrapping_add(i as u32 * 37) % 240) as i32;
            }
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
        // 1. Draw Background (Sakura Pink to Cloud White gradient-ish)
        // A simple flat or two-tone background to keep performance up.
        let bg_color = Rgb565::new(31, 50, 31); // Pastel pinkish approx.
        display.clear(bg_color)?;

        // 2. Draw Drifters
        let drifter_style = PrimitiveStyleBuilder::new()
            .fill_color(Rgb565::new(31, 55, 31)) // Slightly brighter pink
            .build();

        for d in &self.drifters {
            Circle::new(Point::new(d.0, d.1), 10)
                .into_styled(drifter_style)
                .draw(display)?;
        }

        // 3. Draw R-Kun
        self.draw_r_kun(display)?;

        // 4. Draw Menu
        if self.menu_offset > -100 {
            self.draw_menu(display)?;
        }

        // Text status/OpenAI thoughts
        if !self.text.is_empty() {
             let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
             // Draw text box
             Rectangle::new(Point::new(10, 280), Size::new(220, 30))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                .draw(display)?;
             Text::new(self.text.as_str(), Point::new(15, 300), style).draw(display)?;
        }

        Ok(())
    }

    fn draw_r_kun<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Body: Rounded rectangle or Ellipse
        let body_w = 120;
        let body_h = 100 - self.idle_bounce; // Squash
        let body_x = self.r_kun_x - body_w / 2;
        let body_y = self.r_kun_y - body_h / 2 + self.idle_bounce; // Stay on the floor

        let body_style = PrimitiveStyleBuilder::new()
            .fill_color(Rgb565::WHITE)
            .stroke_color(Rgb565::new(28, 56, 28)) // soft border
            .stroke_width(2)
            .build();

        // Use Ellipse for a squishy marshmallow look
        Ellipse::new(Point::new(body_x, body_y), Size::new(body_w as u32, body_h as u32))
            .into_styled(body_style)
            .draw(display)?;

        // Eyes
        let eye_color = PrimitiveStyle::with_fill(Rgb565::BLACK);
        let eye_w = 10;
        let eye_h = if self.is_blinking { 2 } else { 16 };
        let eye_y = body_y + body_h / 2 - 10;

        // Left eye
        Ellipse::new(Point::new(body_x + 30, eye_y), Size::new(eye_w, eye_h))
            .into_styled(eye_color)
            .draw(display)?;

        // Right eye
        Ellipse::new(Point::new(body_x + body_w - 40, eye_y), Size::new(eye_w, eye_h))
            .into_styled(eye_color)
            .draw(display)?;

        // Blush
        let blush_style = PrimitiveStyle::with_fill(Rgb565::new(31, 40, 31)); // Reddish
        Ellipse::new(Point::new(body_x + 20, eye_y + 15), Size::new(16, 8))
            .into_styled(blush_style)
            .draw(display)?;
        Ellipse::new(Point::new(body_x + body_w - 36, eye_y + 15), Size::new(16, 8))
            .into_styled(blush_style)
            .draw(display)?;

        // Expression specific drawing
        match self.expression {
            EyeExpression::Thinking => {
                // Draw sparkles or a question mark
                 let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                 Text::new("?", Point::new(body_x + 90, body_y - 10), style).draw(display)?;
            },
            // Note: Since `Listening` is not in `EyeExpression`, we fall back or use another expression
            EyeExpression::Surprised => {
                // R-Kun surprised
                 let style = MonoTextStyle::new(&FONT_10X20, Rgb565::BLACK);
                 Text::new("!", Point::new(body_x + 90, body_y - 10), style).draw(display)?;
            },
            _ => {}
        }

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
