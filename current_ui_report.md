
# Current UI Report

## Overview
The current UI is built for a 2.8-inch 240x320 ILI9341 SPI display, managed by an `embedded-graphics` based `UiManager` in `firmware/src/display/ui.rs`. It implements a minimalist 'Kawaii' aesthetic featuring a desktop companion ('R-Kun').

## State Machine
The UI operates on a simple state machine with two primary states:
1.  **Idle**: The default state showing the companion's face.
2.  **Menu**: Triggered by tapping the companion, displaying a slide-out menu with interactive buttons.

## Visual Design & Aesthetics
*   **Background**: Pure minimalist white (`Rgb565::WHITE`).
*   **Companion (R-Kun)**:
    *   Positioned in the center/slightly offset depending on the state.
    *   Facial features are drawn using a combination of ellipses and text characters to achieve a kaomoji style.
    *   Color palette uses stark black for features and a soft pinkish-red (`Rgb565::new(63, 40, 40)`) for blush.
*   **Typography**: Uses a monospaced font (`FONT_10X20`) in black for both the companion's text-based facial expressions and status/menu text.

## Animations & Transitions
*   **Micro-animations**:
    *   **Blinking**: A random fast blink occurs for 5 frames every 300 frames.
    *   **Breathing/Idle Bounce**: The code mentions an `idle_bounce`, but it is currently disabled (`self.idle_bounce = 0`) to prevent full face flicker.
*   **Transitions**:
    *   **Menu Slide**: When transitioning to the `Menu` state, R-Kun slides to the right (x: 120 -> 180), and the menu slides in from the left (x: -120 -> 10). When returning to `Idle`, they slide back.
*   **Interactions**:
    *   **Button Pop**: Tapping a menu button triggers a simple scale animation (`button_pop`), increasing the button's size slightly before shrinking back.

## UI Elements
1.  **Companion Face**: Changes based on `EyeExpression` (Neutral, Happy, Sad, Angry, Surprised, Thinking, Blink, Heart, Sleepy). Different expressions use either drawn ellipses or specific text characters (e.g., '> <', 'T T', 'v v').
2.  **Slide-out Menu**: Contains four buttons: 'Talk', 'Play', 'Memory', 'Settings'. Buttons are currently drawn as standard rectangles with a white fill and dark gray stroke, despite the intention to use rounded rectangles.
3.  **Status Text**: Displayed at the bottom of the screen (y: 300) to show current status or thoughts.

## Rendering Strategy
*   The UI employs a dirty-flag based rendering approach. The screen is only cleared and redrawn if there's a change in expression, blinking state, text, or if the menu is open (which implies animation).
*   This approach helps reduce display flickering, a common issue with ILI9341 SPI displays when clearing the entire screen per frame.
