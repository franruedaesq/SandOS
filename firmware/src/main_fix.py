import re

with open("firmware/src/main.rs", "r") as f:
    code = f.read()

# Fix the lstimer0 lifetime issue by using a `StaticCell` or dynamic allocation if not possible.
# Actually, the esp-hal LEDC requires the timer reference to live as long as the channel.
# But Embassy spawns tasks taking ownership of channel. We can't spawn a task that takes `channel` if it borrows `lstimer0` which is a local variable in main.
# We need to make `lstimer0` have a `'static` lifetime.
# For esp-hal timer configuration, we can use `Box::leak` or `StaticCell`.

static_cell_str = """
    static TIMER0: StaticCell<esp_hal::ledc::timer::Timer<'static, LowSpeed>> = StaticCell::new();
    let lstimer0 = TIMER0.init(ledc.timer::<LowSpeed>(timer::Number::Timer0));
"""

code = code.replace("let mut lstimer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);", static_cell_str)
code = code.replace("timer: &lstimer0,", "timer: lstimer0,")

with open("firmware/src/main.rs", "w") as f:
    f.write(code)
