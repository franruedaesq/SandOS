# ESP32-S3 OLED Display Configuration Guide

This document describes the recommended configuration values for using an OLED display (e.g., SSD1306/SH1106) with the ESP32-S3 N16R8 microcontroller, based on validated experience with the SandOS firmware.

## Recommended Configuration Values

```
const FRAME_PERIOD: Duration = Duration::from_millis(50);
const FLUSH_TIMEOUT: Duration = Duration::from_millis(400);
const OLED_PAGE_WRITE_CHUNK_SIZE: usize = 16;

let mut cfg = I2cConfig::default();
cfg.frequency = 200.kHz();
cfg.timeout = BusTimeout::BusCycles(100_000);
```

### Parameter Explanations

- **FRAME_PERIOD (50 ms):**
  - Controls how often the display is refreshed. Lower values provide smoother animations but increase I2C bus usage. 50 ms is a good balance for fast, smooth updates on the ESP32-S3.

- **FLUSH_TIMEOUT (400 ms):**
  - Maximum time to wait before forcing a flush of pending display data. Should be higher than `FRAME_PERIOD` to ensure all updates are sent in time.

- **OLED_PAGE_WRITE_CHUNK_SIZE (16 bytes):**
  - Number of bytes sent per I2C transaction. 16 bytes is safe and reliable for most OLED controllers, minimizing glitches.

- **I2C Frequency (200 kHz):**
  - Sets the I2C bus speed. 200 kHz is stable for most OLED displays. Higher speeds (e.g., 400 kHz) may work if both the ESP32-S3 and the display support it, but always test for stability.

- **I2C Timeout (BusCycles(100_000)):**
  - Timeout for I2C operations, set high to avoid premature timeouts during heavy display updates.

## Additional Notes

- These values have been validated on the ESP32-S3 N16R8 with standard 128x64 OLED displays (SSD1306/SH1106).
- If you observe glitches or incomplete updates, try lowering the chunk size or increasing the flush timeout.
- For smoother animations, you can experiment with a lower `FRAME_PERIOD`, but ensure the display and I2C bus can handle the increased update rate.
- Always verify the maximum supported I2C frequency in your OLED display's datasheet before increasing it.
- If you change the display resolution or use a different controller, you may need to adjust these values.

## Troubleshooting

- **Glitches or Artifacts:**
  - Lower the chunk size or increase the flush timeout.
  - Ensure I2C pull-up resistors are present and correctly valued (typically 4.7kΩ).
- **Display Freezes or Resets:**
  - Lower the I2C frequency or increase the timeout.
- **Slow Updates:**
  - Lower the frame period, but only if the display and bus are stable.

## References

- [ESP32-S3 Technical Reference Manual](https://www.espressif.com/sites/default/files/documentation/esp32-s3_technical_reference_manual_en.pdf)
- [SSD1306 Datasheet](https://cdn-shop.adafruit.com/datasheets/SSD1306.pdf)
- [SH1106 Datasheet](https://www.displayfuture.com/Display/datasheet/controller/SH1106.pdf)

---

_Last updated: March 2026_
