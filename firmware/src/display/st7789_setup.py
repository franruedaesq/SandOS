import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

# Fix duplicate Rgb565 import again
code = re.sub(r"pixelcolor::Rgb565,\n\s+pixelcolor::Rgb565,", "pixelcolor::Rgb565,", code)
code = re.sub(r"pixelcolor::Rgb565,\n\s+pixelcolor::Rgb565,", "pixelcolor::Rgb565,", code)
code = re.sub(r"pixelcolor::Rgb565,\n\s+pixelcolor::RgbColor,\n\s+pixelcolor::Rgb565,", "pixelcolor::Rgb565,\n    pixelcolor::RgbColor,", code)

# Clean up I2c imports properly
code = re.sub(r"use esp_hal::\{\n\s+gpio::\{GpioPin, Input, Level, Output\},\n\s+i2c::master::\{BusTimeout, Config as I2cConfig, I2c\},\n\s+peripherals::I2C0,\n\s+time::RateExtU32,\n\s+Async,\n\};", "use esp_hal::{\n    gpio::{GpioPin, Input, Level, Output},\n    time::RateExtU32,\n};", code)

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
