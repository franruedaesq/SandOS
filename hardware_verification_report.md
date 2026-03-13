# SandOS Hardware Verification Report

This report outlines the verification status of the requested hardware components for the SandOS project on the ESP32-S3. The verification is based on analyzing the current state of the firmware source code.

## 1. ESP32-S3 Core
*   **Description:** Dual-core processor up to 240MHz featuring 384KB ROM, 512KB SRAM, and 16MB External QSPI Flash.
*   **Status:** **Connected & Functional.**
*   **Details:** The project is correctly configured for the ESP32-S3. Core 0 runs the `main` entrypoint, Embassy framework, and WebAssembly VM. Core 1 is explicitly initialized and started (`cpu_control.start_app_core(...)` in `firmware/src/main.rs`) to handle independent real-time loops. The heap allocator manages both internal SRAM (`esp_alloc::heap_allocator!`) and external PSRAM (`esp_alloc::psram_allocator!`).

## 2. 2.8" LCD Screen
*   **Description:** 240x320 TFT display using the ILI9341V driver connected via SPI pins IO10 (CS), IO46 (DC), IO12 (SCK), IO11 (MOSI), IO13 (MISO), and IO45 (Backlight).
*   **Status:** **Connected & Functional.**
*   **Details:** The SPI bus (`SPI2`) is initialized with `GPIO12` (SCK), `GPIO11` (MOSI), and `GPIO13` (MISO). The control pins `GPIO10` (CS) and `GPIO46` (DC) are correctly configured as outputs. The backlight on `GPIO45` is also initialized as an output and explicitly set high. The `display_task` is actively spawned to manage rendering.

## 3. Capacitive Touchscreen
*   **Description:** D-FT6336G driver operating over I2C on pins IO16 (SDA) and IO15 (SCL), with IO18 (Reset) and IO17 (Interrupt).
*   **Status:** **Connected & Partially Functional (Initialization verified).**
*   **Details:** The I2C bus (`I2C0`) is successfully initialized at 400kHz using `GPIO16` (SDA) and `GPIO15` (SCL). The Reset pin on `GPIO18` is configured as an output (level High), and the Interrupt pin on `GPIO17` is configured as an input with a pull-up resistor. While the pins and I2C bus are initialized, extensive driver logic for the D-FT6336G within the main control loops appears minimal or handled outside the immediate main init block.

## 4. Battery Support Socket
*   **Description:** 1.25mm 2P socket for 3.7V lithium batteries with integrated charging management and voltage sensing on pin IO9 (ADC).
*   **Status:** **Connected (Initialization verified).**
*   **Details:** The ADC pin for battery sensing (`GPIO9`) is successfully acquired and documented (`let _bat_adc = peripherals.GPIO9;`).

## 5. Status RGB LED
*   **Description:** Onboard three-color LED controlled via a single signal line on pin IO42.
*   **Status:** **Connected & Functional.**
*   **Details:** The RGB LED is controlled using the RMT peripheral on `GPIO42`. A TX channel is configured with the correct clock divider, attached to the WS2812 driver, and turned off initially to establish a known state.

## 6. MicroSD Storage Slot
*   **Description:** Supports memory expansion using SDIO bus pins IO38 (CLK), IO40 (CMD), and IO39, IO41, IO48, IO47 (Data 0-3).
*   **Status:** **Connected (Pins acquired).**
*   **Details:** The specific SDIO pins (`GPIO38`, `GPIO40`, `GPIO39`, `GPIO41`, `GPIO48`, `GPIO47`) are successfully acquired in `main.rs`, ensuring no other peripheral conflicts with their assignment.

## 7. Audio Speaker Interface
*   **Description:** 1.25mm 2P socket for speakers (up to 2W) using I2S pins IO1 (Enable), IO4 (MCLK), IO5 (BCLK), IO6 (Data Out), and IO7 (LRCK).
*   **Status:** **Connected (Pins acquired).**
*   **Details:** The Audio Enable pin (`GPIO1`) is initialized as an output and set low (enabled). The I2S output pins (`GPIO4`, `GPIO5`, `GPIO6`, `GPIO7`) are properly acquired to prevent conflicts.

## 8. Built-in Microphone
*   **Description:** Integrated voice capture connected via I2S input on pin IO8.
*   **Status:** **Connected (Pins acquired).**
*   **Details:** The I2S data input pin (`GPIO8`) is successfully acquired alongside the speaker pins.

## 9. Physical Buttons
*   **Description:** Dedicated Reset key (EN pin) and Boot/User key (IO0).
*   **Status:** **Connected & Functional.**
*   **Details:** The Boot button (`GPIO0`) is initialized as an input with a pull-up resistor. It is actively passed to the display task to handle user interactions like toggling the web server. The EN pin is handled purely in hardware.

## 10. USB-C Interface
*   **Description:** Standard Type-C port for power supply and automatic program downloading.
*   **Status:** **Functional.**
*   **Details:** The standard UART-over-USB interface is used for logging (`esp_println::logger::init_logger`) and program flashing.

## 11. Expansion & Serial Pins
*   **Description:** Dedicated 1.25mm 4P UART socket (IO43 RX / IO44 TX) and an Expansion socket breaking out GPIOs IO2, IO3, IO14, and IO21.
*   **Status:** **Not explicitly initialized in main OS flow.**
*   **Details:** A review of `main.rs` and the peripheral initialization code does not show explicit acquisition or configuration for `GPIO43`, `GPIO44`, `GPIO2`, `GPIO3`, `GPIO14`, or `GPIO21`. These pins remain free for user expansion but are not actively driven or protected by the core OS initialization sequence. However, standard UART logging is functional (likely mapped to the default USB-Serial JTAG or UART0 pins rather than explicitly routing `43/44`).
