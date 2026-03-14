# Head OS Architecture Report

## Overview
This report outlines the architecture for a dedicated, highly-responsive embedded Rust firmware running on the ESP32-S3, intended to serve as the "Head OS". This subsystem guarantees fluid animations, local hardware reflexes, and remote intent listening. It is entirely decoupled from the Base OS processing, acting as a fault-tolerant animation and sensory node.

## Implemented Features

### 1. Pure Embassy Rust
**Status: Implemented and Working**
The firmware leverages the Embassy async framework, ensuring hard real-time execution with zero abstraction overhead. Tasks run concurrently, enabling asynchronous polling of hardware without blocking the main execution path.

### 2. DMA-Driven Face
**Status: Implemented and Working**
A custom asynchronous display driver is implemented using SPI2 with DMA capabilities. This guarantees that frame buffers are transferred to the 0.96" I2C/SPI OLED display continuously in the background, achieving up to 60FPS for the `embedded-graphics` rendered kawaii expressions. The CPU yields during DMA transfers, avoiding stuttering.

### 3. Local Reflexes (VL53L0X ToF + Servo)
**Status: Implemented and Working**
The Head OS features a dedicated asynchronous task (`servo_reflex_task` in `motors.rs`) that continuously polls distance readings from the VL53L0X Time-of-Flight sensor. If the distance drops below a threshold (`TOF_THRESHOLD_MM`, currently 200mm), it instantly triggers a servo to a "flinch" position (`DUTY_FLINCH`), circumventing the Base OS entirely for this reflex.

### 4. Intent Listener (ESP-NOW)
**Status: Implemented and Working**
The ESP-NOW radio is configured to continuously listen for structured payloads (JSON/Structs) transmitted from the Base OS. These intents seamlessly update the current target emotion/expression (`EyeExpression`) on the screen via the host-guest ABI and routing architecture without interrupting ongoing DMA rendering or local reflexes.

## Summary
The Head OS successfully meets the requirements of a stripped-down, bare-metal embedded subsystem. It isolates the animation and reflexive logic from the higher-level Base OS networking/AI stack, delivering a reliable, physically reactive robot head.
