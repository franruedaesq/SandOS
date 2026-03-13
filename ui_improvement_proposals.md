
# UI Improvement Proposals

## Objective
To refine the current UI into a polished, production-ready desktop companion with a stronger focus on minimalist kawaii aesthetics, fluid micro-animations, and a refined menu system.

## 1. Aesthetic Refinement
*   **Rounded Geometry**: Implement actual `RoundedRectangle` primitives for all UI elements, especially the menu buttons, to soften the visual language. The current implementation falls back to sharp rectangles.
*   **Softer Palette**: While pure white is good, consider using a very soft pastel background (e.g., a warm off-white or very light cream) to reduce eye strain and feel more 'physical'. Keep the blush, but perhaps use a slightly more vibrant pastel pink. Use `COLOR_SOFT_GRAY` for non-active elements as mentioned in the memory.
*   **Typography**: If possible within the constraints of `embedded-graphics`, use a slightly thicker or more stylized font for the menu items to make them look less 'terminal-like' and more integrated.

## 2. Companion (R-Kun) Enhancements
*   **Fluid Facial Features**: Replace text-based characters (e.g., '>', '<', 'T') with actual drawn primitives (arcs, rounded lines) for expressions. This will make the face look much more cohesive and professional than a mix of shapes and ASCII characters.
*   **Dynamic Idle Animation (Breathing)**: Re-introduce the `idle_bounce` (breathing effect) but implement it smoothly. Instead of redrawing the whole face and causing flicker, selectively update only the Y-coordinates of the facial features by 1-2 pixels using a sine wave function based on `frame_count`.
*   **Eye Tracking/Gaze**: Introduce subtle eye movements. Even when idle, the eyes could occasionally shift left or right slightly, making the companion feel more alive.

## 3. Menu System Redesign
*   **Modern Layout**: Instead of a simple vertical list, consider a grid layout or a semi-circular radial menu that fans out from the companion when tapped. This feels more modern and interactive.
*   **Iconography**: Add simple, stylized icons next to or above the text labels for 'Talk', 'Play', 'Memory', and 'Settings'.
*   **Refined Button Pop**: The current 'pop' animation scales the rectangle. Enhance this by adding an 'inverted' state on touch (e.g., filling the button with a soft color) before scaling back, providing better tactile feedback.
*   **Smooth Easing**: The current sliding animation uses linear increments (`+= 4`, `+= 6`). Implement basic easing functions (like ease-out) so the slide slows down as it reaches its target, giving a more natural, physics-based feel.

## 4. Tactile & Visual Feedback
*   **Touch Ripples**: Implement a subtle, expanding circle animation at the exact `x, y` coordinate of a touch event, fading out over a few frames. This provides immediate, satisfying feedback confirming the touch was registered.
*   **Audio-Visual Synergy**: Ensure every significant UI action (opening menu, tapping a button) is tied to a specific programmatic waveform (e.g., `play_blip`) sent to the `AUDIO_TX_CHANNEL`, as outlined in the system memory.

## 5. Rendering Optimization
*   **Partial Redraws**: To fully eliminate flicker and support continuous animations (like breathing or touch ripples) without clearing the whole screen, enhance the `FaceState` to track previous positions of all elements. Only erase the *previous* bounding boxes of moving elements by drawing over them with the background color, then draw the new positions.
