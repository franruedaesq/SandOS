# SandOS UI Report

This document outlines the visual look and behavior of the current UI on the 0.96" 128x64 OLED display.

## Overview and Layout

The interface is designed around distinct modes, with the primary experience being the **Face Mode** and secondary interactions managed through a **Menu Mode**.

* **Face Mode:** By default, the entire 128x64 screen is occupied by an animated face. The face acts as an idle companion, exhibiting random blinking and a breathing animation.
* **Menu Mode:** When accessed, the screen splits. The face shrinks and shifts to the right half of the screen (64x64 pixels), acting as a "mini-face", while a structured, scrollable menu appears on the left half (64x64 pixels).
* **Full-Screen Modes:** Certain features and tools take over the entire 128x64 display to show detailed information.

## Face Mode

The central feature of the UI is a dynamic, animated companion face. The face conveys state and emotion through various distinct expressions:

* **Neutral:** The default, calm state.
* **Happy:** Wide, curved crescent eyes.
* **Sad:** Downturned, inverted crescent eyes.
* **Angry:** Focused expression with gritted teeth.
* **Surprised:** Eyes wide open with an "O"-shaped mouth.
* **Thinking:** Eyes accompanied by a wavy, squiggly mouth.
* **Blink:** A transitional state showing closed eyes.
* **Heart:** Pixel-art heart-shaped eyes.
* **Sleepy:** Downward U-shaped closed eyes with a tiny "w"-shaped mouth.

## Menu Mode

The Menu Mode provides access to various tools, information screens, and system settings. The interface highlights the currently selected item, allowing navigation through the following top-level categories and their specific features:

### 1. Tools
A submenu for utility applications.
* **Flashlight:** Activates a bright mode, utilizing the RGB LED in full white while displaying status on the screen.
* **Pomodoro:** Launches a full-screen, 25-minute countdown timer designed for focus sessions.
* **Party Mode:** Engages a dynamic mode where the RGB LED cycles through colors, with status reflected on the OLED.
* **< Back:** Returns to the top menu.

### 2. Info
A submenu for system and time details.
* **System:** Opens a full-screen System Monitor displaying vital statistics like WiFi status, IP address, uptime, and memory usage.
* **Clock:** Displays a digital clock interface, showing NTP-synchronized time (or uptime as a fallback).
* **< Back:** Returns to the top menu.

### 3. Transport
A submenu dedicated to public transit.
* **Wiener L (Vienna Lines):** Opens a full-screen view listing Vienna public transport departures (showing station and direction). Selecting a stop opens a detail view with comprehensive departure times.
* **< Back:** Returns to the top menu.

### 4. Web
A dedicated inline toggle menu for managing the internal web server.
* **ON:** Enables the web dashboard.
* **OFF:** Disables the web dashboard.
(Displays the current connection status below the options).

### 5. Settings
A submenu for device configuration.
* **Brightness:** Cycles through available display brightness levels.
* **< Back:** Returns to the top menu.

## Navigation Behavior

Interaction with the UI relies on a single hardware button (BOOT button), utilizing distinct press behaviors to navigate the system naturally:
* **Short Press:** Transitions from the Face Mode to the Menu Mode. Inside a menu, it moves the highlight to the next available item.
* **Long Press:** Functions as a "select" action. If on the Face Mode, it opens the menu. Inside a menu, it activates the highlighted item (opening a submenu or triggering a feature).
* **Auto-Return:** If the menu is left inactive for a duration, the UI automatically reverts to the default Face Mode.
